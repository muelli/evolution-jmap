// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A dedicated resolver thread that owns and iterates its own GMainContext.
//!
//! GLib synchronous resolver lookups park completion sources on the
//! thread-default context. Philip Withnall (glib!5341): "The only reliable
//! advice for making synchronous calls is to do them in a thread with its
//! own GMainContext." This module maintains a process-lifetime worker thread
//! running a GMainLoop on its own private GMainContext.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::OnceLock;
use std::time::Duration;

use gio_sys::{
    GSrvTarget, g_resolver_free_targets, g_resolver_get_default, g_resolver_lookup_service,
    g_srv_target_get_hostname, g_srv_target_get_port,
};
use glib_sys::{GError, g_error_free};
use gobject_sys::g_object_unref;
use jmap_client::resolver::SrvTarget;

/// Bounded timeout for SRV resolution on the dedicated resolver thread.
/// Comfortably above GLib's internal resolver timeout (60 s).
pub const RESOLVER_TIMEOUT: Duration = Duration::from_secs(75);

#[derive(Clone, Copy)]
struct SendContext(*mut glib_sys::GMainContext);

// SAFETY: GMainContext reference is valid for the life of the process and
// g_main_context_invoke_full is documented as thread-safe across threads.
unsafe impl Send for SendContext {}
unsafe impl Sync for SendContext {}

struct ResolverThread {
    _thread: std::thread::JoinHandle<()>,
    context: SendContext,
}

static RESOLVER_THREAD: OnceLock<Option<ResolverThread>> = OnceLock::new();

fn get_resolver_thread() -> Option<&'static ResolverThread> {
    RESOLVER_THREAD
        .get_or_init(|| match spawn_resolver_thread() {
            Ok(handle) => Some(handle),
            Err(e) => {
                tracing::warn!(error = %e, "failed to start JMAP resolver thread");
                None
            }
        })
        .as_ref()
}

fn spawn_resolver_thread() -> std::io::Result<ResolverThread> {
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

    Ok(ResolverThread {
        _thread: thread,
        context,
    })
}

struct ResolveRequest {
    service: &'static CStr,
    protocol: &'static CStr,
    domain: CString,
    tx: std::sync::mpsc::SyncSender<Option<SrvTarget>>,
}

type Payload = Option<ResolveRequest>;

unsafe extern "C" fn invoke_cb(data: glib_sys::gpointer) -> glib_sys::gboolean {
    // SAFETY: data points to the heap-allocated Payload passed to invoke_full.
    let payload = unsafe { &mut *data.cast::<Payload>() };
    if let Some(req) = payload.take() {
        crate::trampoline::guard("jmap-resolver worker", (), || {
            let target = lookup_first_service_target(req.service, req.protocol, &req.domain);
            let _ = req.tx.send(target);
        });
    }
    glib_sys::GFALSE
}

unsafe extern "C" fn drop_payload(data: glib_sys::gpointer) {
    // SAFETY: reclaiming the leaked Payload box after dispatch or cancellation.
    drop(unsafe { Box::from_raw(data.cast::<Payload>()) });
}

/// Dispatches an SRV lookup request to the dedicated resolver thread.
pub(crate) fn resolve_srv(
    service: &'static CStr,
    protocol: &'static CStr,
    domain: CString,
) -> Option<SrvTarget> {
    let domain_display = domain.to_string_lossy().into_owned();
    let Some(handle) = get_resolver_thread() else {
        tracing::warn!(
            domain = %domain_display,
            "SRV lookup skipped because resolver thread failed to start"
        );
        return None;
    };

    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let req = ResolveRequest {
        service,
        protocol,
        domain,
        tx,
    };
    let payload: Box<Payload> = Box::new(Some(req));

    // SAFETY: handle.context.0 is a live GMainContext for the process lifetime.
    unsafe {
        glib_sys::g_main_context_invoke_full(
            handle.context.0,
            glib_sys::G_PRIORITY_DEFAULT,
            Some(invoke_cb),
            Box::into_raw(payload).cast(),
            Some(drop_payload),
        );
    }

    match rx.recv_timeout(RESOLVER_TIMEOUT) {
        Ok(target) => target,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            tracing::warn!(
                domain = %domain_display,
                timeout_secs = RESOLVER_TIMEOUT.as_secs(),
                "SRV lookup timed out waiting for resolver thread"
            );
            None
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            tracing::warn!(
                domain = %domain_display,
                "resolver thread disconnected without providing an SRV answer"
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
