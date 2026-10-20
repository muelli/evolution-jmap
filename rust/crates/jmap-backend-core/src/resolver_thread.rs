// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A small pool of resolver worker threads, each owning its own GMainContext.
//!
//! GLib synchronous resolver lookups attach completion sources to the
//! thread-default context. Philip Withnall (glib!5341): "The only reliable
//! advice for making synchronous calls is to do them in a thread with its
//! own GMainContext." This module keeps a process-lifetime pool of worker
//! threads. Each runs a GMainLoop on its own private GMainContext.
//!
//! Why a pool and not one dedicated thread: with a single shared
//! GMainContext, every account's SRV lookup queued behind the running one.
//! Thus one slow or hung domain delayed the lookups of all other accounts,
//! for up to the resolver's own internal timeout. Round-robin dispatch
//! across some independent contexts changes the limit from "every account
//! queues behind one" to "this many accounts can be mid-setup at once
//! before a new one queues".

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
/// It is larger than GLib's internal resolver timeout (60 s).
pub const RESOLVER_TIMEOUT: Duration = Duration::from_secs(75);

/// Worker count. Small and fixed: SRV lookup only runs during account
/// setup/reconnect, not in the steady-state sync path. Thus it only must
/// cover the number of accounts that are possibly mid-setup at the same
/// time, not one thread per core.
const POOL_SIZE: usize = 4;

#[derive(Clone, Copy)]
struct SendContext(*mut glib_sys::GMainContext);

// SAFETY: the GMainContext reference is valid for the life of the process.
// The documentation of g_main_context_invoke_full says it is thread-safe.
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
        // Relaxed: this only spreads load round-robin. It does not need an
        // ordering with the work that the chosen worker does next.
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
            // SAFETY: a new GMainContext, pushed as thread-default for the
            // full lifetime of this thread, and a GMainLoop that iterates it.
            let (context, main_loop) = unsafe {
                let context = glib_sys::g_main_context_new();
                glib_sys::g_main_context_push_thread_default(context);
                let main_loop = glib_sys::g_main_loop_new(context, glib_sys::GFALSE);
                (context, main_loop)
            };

            if ready_tx.send(SendContext(context)).is_err() {
                // SAFETY: releases resources if the caller disconnected early.
                unsafe {
                    glib_sys::g_main_loop_unref(main_loop);
                    glib_sys::g_main_context_pop_thread_default(context);
                    glib_sys::g_main_context_unref(context);
                }
                return;
            }

            // SAFETY: main_loop runs on context and iterates until quit.
            unsafe { glib_sys::g_main_loop_run(main_loop) };

            // SAFETY: releases resources if the loop exits.
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

/// A job dispatched to a worker's context. It is boxed once, and
/// [`invoke_cb`] runs it once. It is `None` after [`invoke_cb`] takes it. A
/// cancelled invocation (its context is destroyed before it runs) leaves it
/// `Some`. [`drop_payload`] must then drop it without running it.
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
    // SAFETY: reclaims the leaked Job box after dispatch or cancellation.
    drop(unsafe { Box::from_raw(data.cast::<Job>()) });
}

/// Runs `f` on the GMainContext of `worker`. This is the same dispatch
/// mechanism that every lookup used before the pool: a boxed job that
/// [`invoke_cb`] takes and runs once, and that [`drop_payload`] always frees.
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
    // SAFETY: resolver is a live GResolver. The strings are valid and
    // NUL-terminated. error is initialized to NULL. The returned list is
    // transfer-full.
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
        // SAFETY: targets is the transfer-full list from lookup_service.
        unsafe { g_resolver_free_targets(targets) };
    }
    if !error.is_null() {
        // SAFETY: the failed call above wrote error, and this code owns it.
        unsafe { g_error_free(error) };
    }
    // SAFETY: balances the g_resolver_get_default reference.
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
    // SAFETY: a valid NUL-terminated string that the target owns.
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

    #[test]
    fn the_pool_has_more_than_one_worker() {
        let pool = get_resolver_pool().expect("resolver pool starts");
        assert!(
            pool.workers.len() > 1,
            "a pool of one thread cannot fix the head-of-line blocking F31 found"
        );
    }

    /// Regression test: before this module had a pool, every lookup shared
    /// one GMainContext. Thus a slow lookup delayed all lookups behind it.
    /// This test uses plain closures and no real DNS. It proves by order,
    /// not by a wall-clock margin. The slow job blocks on a gate that this
    /// test holds. Thus the fast job on the other worker can only answer
    /// before the gate opens if it ran independently. The large timeout only
    /// prevents a hang. It does not limit the duration of the fast job, so
    /// CPU load cannot make this test flake.
    #[test]
    fn a_slow_job_on_one_worker_does_not_delay_a_job_on_another() {
        let pool = get_resolver_pool().expect("resolver pool starts");
        assert!(
            pool.workers.len() >= 2,
            "need at least two workers to prove parallelism"
        );
        let slow_worker = &pool.workers[0];
        let fast_worker = &pool.workers[1];

        const HANG_TIMEOUT: Duration = Duration::from_secs(10);

        // The gate: the slow job waits here until the test releases it.
        // The test releases it only after the fast job has answered.
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel::<()>(0);
        let (slow_tx, slow_rx) = std::sync::mpsc::sync_channel::<()>(1);
        invoke_on_worker(slow_worker, move || {
            release_rx.recv().expect("test releases the slow job");
            let _ = slow_tx.send(());
        });

        let (fast_tx, fast_rx) = std::sync::mpsc::sync_channel::<()>(1);
        invoke_on_worker(fast_worker, move || {
            let _ = fast_tx.send(());
        });

        fast_rx
            .recv_timeout(HANG_TIMEOUT)
            .expect("the fast job must answer without waiting for the slow one's worker");

        // The fast job has answered, and the slow job still waits on the
        // gate (release_tx has not sent yet). Open the gate and make sure
        // the slow job also completes. This proves the fast answer is valid.
        release_tx
            .send(())
            .expect("slow job still waiting on the gate");
        slow_rx
            .recv_timeout(HANG_TIMEOUT)
            .expect("the slow job eventually completes on its own worker");
    }

    /// Serializes tests that read `ResolverPool::next` directly. It is a
    /// process-global counter. Thus two such tests that run concurrently in
    /// the same test binary can see the increments of the other test.
    static POOL_NEXT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Round-robin dispatch spreads jobs across all workers, and does not,
    /// for example, always send them to worker 0.
    #[test]
    fn round_robin_dispatch_reaches_every_worker() {
        let _guard = POOL_NEXT_TEST_LOCK.lock().expect("lock not poisoned");
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

    /// `resolve_srv` must dispatch to a worker, and not only report "no
    /// record" itself. A failed lookup and a short-circuited one return the
    /// same `None`. Thus this test checks the round-robin counter, not the
    /// result. The counter only changes if `next_worker` ran.
    #[test]
    fn resolve_srv_dispatches_through_the_pool_even_when_the_lookup_fails() {
        let _guard = POOL_NEXT_TEST_LOCK.lock().expect("lock not poisoned");
        let pool = get_resolver_pool().expect("resolver pool starts");
        let before = pool.next.load(Ordering::Relaxed);

        let domain = CString::new("no-jmap-here.evolution-jmap-test.invalid")
            .expect("domain has no interior NUL");
        assert_eq!(resolve_srv(c"jmap", c"tcp", domain), None);

        let after = pool.next.load(Ordering::Relaxed);
        assert_ne!(
            before, after,
            "resolve_srv must advance the round-robin counter, not short-circuit before dispatch"
        );
    }
}
