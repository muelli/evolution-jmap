// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Run a closure one time on a `GMainContext`, panic-guarded.

use glib_sys::{
    G_PRIORITY_DEFAULT, GFALSE, GMainContext, g_main_context_invoke_full, gboolean, gpointer,
};

use crate::trampoline::guard;

/// The closure and the guard label, boxed so that they have one address. The
/// dispatch takes the closure. A context that is destroyed before the dispatch
/// leaves it `Some`, and the destroy notify then drops it without running it.
struct Job {
    label: &'static str,
    f: Option<Box<dyn FnOnce() + Send>>,
}

unsafe extern "C" fn call(data: gpointer) -> gboolean {
    // SAFETY: `data` is the `Job` box that `invoke_in` leaked. Borrowed, not
    // reclaimed: GLib still owns it and frees it through `drop_job`.
    let job = unsafe { &mut *data.cast::<Job>() };
    if let Some(f) = job.f.take() {
        guard(job.label, (), f);
    }
    GFALSE // G_SOURCE_REMOVE: run one time
}

unsafe extern "C" fn drop_job(data: gpointer) {
    // SAFETY: this reclaims the box that `invoke_in` leaked, one time only.
    // GLib calls the destroy notify after the dispatch, or when it frees the
    // context with the source still in the queue.
    drop(unsafe { Box::from_raw(data.cast::<Job>()) });
}

/// Run `f` one time on `context`, inside a panic guard that logs `label`.
///
/// GLib runs `f` at once on the calling thread if that thread owns `context`
/// or can acquire it. Otherwise `f` waits in the queue of `context`.
///
/// # Safety
///
/// `context` must be NULL (the global default context) or a valid
/// `GMainContext`.
pub unsafe fn invoke_in<F>(context: *mut GMainContext, label: &'static str, f: F)
where
    F: FnOnce() + Send + 'static,
{
    let job = Box::new(Job {
        label,
        f: Some(Box::new(f)),
    });
    // SAFETY: `context` is NULL or valid, by the contract of this function.
    // GLib owns the job from here, so the job stays alive until `drop_job`.
    unsafe {
        g_main_context_invoke_full(
            context,
            G_PRIORITY_DEFAULT,
            Some(call),
            Box::into_raw(job).cast(),
            Some(drop_job),
        );
    }
}
