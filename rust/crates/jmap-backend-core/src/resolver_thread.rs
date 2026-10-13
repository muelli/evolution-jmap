// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A small pool of resolver worker threads, each owning its own GMainContext.
//!
//! GLib synchronous resolver lookups park completion sources on the
//! thread-default context. Philip Withnall (glib!5341): "The only reliable
//! advice for making synchronous calls is to do them in a thread with its
//! own GMainContext." This module maintains a process-lifetime pool of
//! worker threads, each running a GMainLoop on its own private GMainContext.
//!
//! A pool rather than one dedicated thread: a single shared GMainContext
//! made every account's SRV lookup queue behind whichever one is currently
//! running, so one slow or hung domain delayed every other account's lookup
//! too, for up to the resolver's own internal timeout. Round-robin dispatch
//! across a handful of independent contexts bounds that to "this many
//! accounts can be mid-setup at once before a new one queues", rather than
//! "every account queues behind one".

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use gio_sys::{
    GSrvTarget, g_resolver_free_targets, g_resolver_get_default, g_resolver_lookup_service,
    g_srv_target_get_hostname, g_srv_target_get_port,
};
use glib_sys::{GError, g_error_free};
use gobject_sys::g_object_unref;
use jmap_client::resolver::SrvTarget;

/// Bounded timeout for SRV resolution on a resolver worker thread.
/// Comfortably above GLib's internal resolver timeout (60 s).
pub const RESOLVER_TIMEOUT: Duration = Duration::from_secs(75);

/// Worker count. Small and fixed: SRV lookup only runs during account
/// setup/reconnect, not the steady-state sync path, so this only needs to
/// cover how many accounts are plausibly mid-setup at the same moment, not
/// one thread per core.
const POOL_SIZE: usize = 4;

#[derive(Clone, Copy)]
struct SendContext(*mut glib_sys::GMainContext);

// SAFETY: GMainContext reference is valid for the life of the process and
// g_main_context_invoke_full is documented as thread-safe across threads.
unsafe impl Send for SendContext {}
unsafe impl Sync for SendContext {}

struct ResolverWorker {
    _thread: std::thread::JoinHandle<()>,
    context: SendContext,
}

struct ResolverPool {
    workers: Vec<ResolverWorker>,
    next: AtomicUsize,
}

impl ResolverPool {
    fn next_worker(&self) -> &ResolverWorker {
        // Relaxed: this only needs to spread load round-robin, not establish
        // any ordering with the work the chosen worker goes on to do.
        let index = self.next.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        &self.workers[index]
    }
}

static RESOLVER_POOL: OnceLock<Option<ResolverPool>> = OnceLock::new();

fn get_resolver_pool() -> Option<&'static ResolverPool> {
    RESOLVER_POOL.get_or_init(spawn_resolver_pool).as_ref()
}

fn spawn_resolver_pool() -> Option<ResolverPool> {
    let mut workers = Vec::with_capacity(POOL_SIZE);
    for _ in 0..POOL_SIZE {
        match spawn_resolver_worker() {
            Ok(worker) => workers.push(worker),
            Err(e) => {
                tracing::warn!(error = %e, "failed to start a JMAP resolver worker thread");
            }
        }
    }
    if workers.is_empty() {
        return None;
    }
    Some(ResolverPool {
        workers,
        next: AtomicUsize::new(0),
    })
}

fn spawn_resolver_worker() -> std::io::Result<ResolverWorker> {
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("jmap-resolver".to_owned())
        .spawn(move || {
            // SAFETY: a fresh GMainContext, pushed as thread-default for this
            // thread's entire lifetime, and a GMainLoop iterating it.
            let (context, main_loop) = unsafe {
                let context = glib_sys::g_main_context_new();
                glib_sys::g_main_context_push_thread_default(context);
                let main_loop = glib_sys::g_main_loop_new(context, glib_sys::GFALSE);
                (context, main_loop)
            };

            if ready_tx.send(SendContext(context)).is_err() {
                // SAFETY: releasing resources if caller disconnected early.
                unsafe {
                    glib_sys::g_main_loop_unref(main_loop);
                    glib_sys::g_main_context_pop_thread_default(context);
                    glib_sys::g_main_context_unref(context);
                }
                return;
            }

            // SAFETY: main_loop is running on context; iterates until quit.
            unsafe { glib_sys::g_main_loop_run(main_loop) };

            // SAFETY: releasing resources if the loop ever exits.
            unsafe {
                glib_sys::g_main_loop_unref(main_loop);
                glib_sys::g_main_context_pop_thread_default(context);
                glib_sys::g_main_context_unref(context);
            }
        })?;

    let context = ready_rx.recv().map_err(std::io::Error::other)?;

    Ok(ResolverWorker {
        _thread: thread,
        context,
    })
}

/// A job dispatched to a worker's context: boxed once, run once, by
/// [`invoke_cb`]. `None` after [`invoke_cb`] has taken it; also the state a
/// cancelled invocation (one whose context is torn down before it runs)
/// leaves it in, which [`drop_payload`] must handle without running it.
type Job = Option<Box<dyn FnOnce() + Send>>;

unsafe extern "C" fn invoke_cb(data: glib_sys::gpointer) -> glib_sys::gboolean {
    // SAFETY: data points to the heap-allocated Job passed to invoke_full.
    let job = unsafe { &mut *data.cast::<Job>() };
    if let Some(f) = job.take() {
        crate::trampoline::guard("jmap-resolver worker", (), f);
    }
    glib_sys::GFALSE
}

unsafe extern "C" fn drop_payload(data: glib_sys::gpointer) {
    // SAFETY: reclaiming the leaked Job box after dispatch or cancellation.
    drop(unsafe { Box::from_raw(data.cast::<Job>()) });
}

/// Runs `f` on `worker`'s own GMainContext, by the same dispatch mechanism
/// every lookup already used before the pool: a boxed job, taken and run
/// once by [`invoke_cb`], freed by [`drop_payload`] either way.
fn invoke_on_worker(worker: &ResolverWorker, f: impl FnOnce() + Send + 'static) {
    let job: Job = Some(Box::new(f));
    let payload: Box<Job> = Box::new(job);

    // SAFETY: worker.context.0 is a live GMainContext for the process lifetime.
    unsafe {
        glib_sys::g_main_context_invoke_full(
            worker.context.0,
            glib_sys::G_PRIORITY_DEFAULT,
            Some(invoke_cb),
            Box::into_raw(payload).cast(),
            Some(drop_payload),
        );
    }
}

/// Dispatches an SRV lookup request to the next resolver worker in the pool.
pub(crate) fn resolve_srv(
    service: &'static CStr,
    protocol: &'static CStr,
    domain: CString,
) -> Option<SrvTarget> {
    let domain_display = domain.to_string_lossy().into_owned();
    let Some(pool) = get_resolver_pool() else {
        tracing::warn!(
            domain = %domain_display,
            "SRV lookup skipped because no resolver worker thread could start"
        );
        return None;
    };

    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let worker = pool.next_worker();
    invoke_on_worker(worker, move || {
        let target = lookup_first_service_target(service, protocol, &domain);
        let _ = tx.send(target);
    });

    match rx.recv_timeout(RESOLVER_TIMEOUT) {
        Ok(target) => target,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            tracing::warn!(
                domain = %domain_display,
                timeout_secs = RESOLVER_TIMEOUT.as_secs(),
                "SRV lookup timed out waiting for a resolver worker thread"
            );
            None
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            tracing::warn!(
                domain = %domain_display,
                "resolver worker thread disconnected without providing an SRV answer"
            );
            None
        }
    }
}

fn lookup_first_service_target(
    service: &CStr,
    protocol: &CStr,
    domain: &CStr,
) -> Option<SrvTarget> {
    // SAFETY: returns a strong reference or NULL; unrefed below.
    let resolver = unsafe { g_resolver_get_default() };
    if resolver.is_null() {
        return None;
    }

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: resolver is a live GResolver; strings are valid NUL-terminated;
    // error is initialized to NULL. Returned list is transfer-full.
    let targets = unsafe {
        g_resolver_lookup_service(
            resolver,
            service.as_ptr(),
            protocol.as_ptr(),
            domain.as_ptr(),
            ptr::null_mut(),
            &mut error,
        )
    };

    let first = if targets.is_null() {
        None
    } else {
        // SAFETY: targets is a live GList with GSrvTarget data elements.
        unsafe { read_target((*targets).data.cast::<GSrvTarget>()) }
    };

    if !targets.is_null() {
        // SAFETY: targets is transfer-full list from lookup_service.
        unsafe { g_resolver_free_targets(targets) };
    }
    if !error.is_null() {
        // SAFETY: error was written by the failing call above and is owned by us.
        unsafe { g_error_free(error) };
    }
    // SAFETY: balances g_resolver_get_default reference.
    unsafe { g_object_unref(resolver.cast()) };

    first
}

unsafe fn read_target(target: *mut GSrvTarget) -> Option<SrvTarget> {
    if target.is_null() {
        return None;
    }
    // SAFETY: caller guarantees a live target; hostname is transfer-none.
    let host = unsafe { g_srv_target_get_hostname(target) };
    if host.is_null() {
        return None;
    }
    // SAFETY: valid NUL-terminated string owned by the target.
    let host = crate::resolver::srv_host(&unsafe { CStr::from_ptr(host) }.to_string_lossy())?;

    // SAFETY: target is valid.
    let port = unsafe { g_srv_target_get_port(target) };
    if port == 0 {
        return None;
    }

    Some(SrvTarget { host, port })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn the_pool_has_more_than_one_worker() {
        let pool = get_resolver_pool().expect("resolver pool starts");
        assert!(
            pool.workers.len() > 1,
            "a pool of one thread cannot fix the head-of-line blocking F31 found"
        );
    }

    /// The regression test for F31: before this module had a pool, every
    /// lookup shared one GMainContext, so a slow one delayed every other
    /// one behind it. Proven here with plain closures and no real DNS, so
    /// it is fast and deterministic: a job that sleeps on one worker must
    /// not delay a job dispatched to a different worker.
    #[test]
    fn a_slow_job_on_one_worker_does_not_delay_a_job_on_another() {
        let pool = get_resolver_pool().expect("resolver pool starts");
        assert!(
            pool.workers.len() >= 2,
            "need at least two workers to prove parallelism"
        );
        let slow_worker = &pool.workers[0];
        let fast_worker = &pool.workers[1];

        const SLOW: Duration = Duration::from_millis(300);
        let start = Instant::now();

        let (slow_tx, slow_rx) = std::sync::mpsc::sync_channel::<()>(1);
        invoke_on_worker(slow_worker, move || {
            std::thread::sleep(SLOW);
            let _ = slow_tx.send(());
        });

        let (fast_tx, fast_rx) = std::sync::mpsc::sync_channel::<Instant>(1);
        invoke_on_worker(fast_worker, move || {
            let _ = fast_tx.send(Instant::now());
        });

        let fast_done = fast_rx
            .recv_timeout(SLOW)
            .expect("the fast job must answer without waiting for the slow one's worker");
        assert!(
            fast_done.duration_since(start) < SLOW / 2,
            "the fast job waited on the slow job's worker instead of running on its own"
        );

        slow_rx
            .recv_timeout(SLOW * 2)
            .expect("the slow job eventually completes on its own worker");
    }

    /// Round-robin dispatch actually spreads jobs across every worker,
    /// rather than, say, always landing on worker 0.
    #[test]
    fn round_robin_dispatch_reaches_every_worker() {
        let pool = get_resolver_pool().expect("resolver pool starts");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));

        let (done_tx, done_rx) = std::sync::mpsc::sync_channel::<()>(0);
        for _ in 0..pool.workers.len() * 3 {
            let seen = std::sync::Arc::clone(&seen);
            let done_tx = done_tx.clone();
            let worker = pool.next_worker();
            let worker_ptr = worker.context.0 as usize;
            invoke_on_worker(worker, move || {
                seen.lock().expect("lock not poisoned").insert(worker_ptr);
                let _ = done_tx.send(());
            });
        }
        for _ in 0..pool.workers.len() * 3 {
            done_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("every dispatched job completes");
        }

        assert_eq!(
            seen.lock().expect("lock not poisoned").len(),
            pool.workers.len(),
            "round-robin dispatch should reach every worker in the pool"
        );
    }
}
