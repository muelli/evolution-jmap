// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A private thread-default `GMainContext`, held for the span of a value.
//!
//! GLib's synchronous calls built over its async machinery park their
//! completion bookkeeping — a `GTask` and its completion-idle `GSource`,
//! ~519 bytes a call — on the calling thread's thread-default main context,
//! reclaimed only once that context is iterated. glib#4041 answered that this
//! is by design: "the guarantee that nothing leaks applies only if you
//! eventually iterate the main context". A worker thread that never iterates
//! one therefore accumulates per call, forever. Holding a [`PrivateContext`]
//! around such a call gives the bookkeeping a context of its own, drained and
//! finalized on drop.
//!
//! Private rather than iterating the inherited context on purpose: iterating
//! a context someone else owns dispatches someone else's sources (the
//! ownership hazard `jmap-mail/tests/common/signals.rs` documents), and
//! finalizing our own destroys any straggler source even if a drain raced the
//! attach.

use std::ptr::NonNull;

use glib_sys::{
    GFALSE, GMainContext, g_main_context_iteration, g_main_context_new,
    g_main_context_pop_thread_default, g_main_context_push_thread_default, g_main_context_unref,
};

/// A fresh `GMainContext`, pushed as this thread's default until dropped.
///
/// `NonNull` keeps it `!Send`/`!Sync`: the pop must happen on the pushing
/// thread, per `g_main_context_pop_thread_default`'s stack discipline.
pub struct PrivateContext(NonNull<GMainContext>);

impl PrivateContext {
    pub fn push() -> Self {
        // SAFETY: a fresh context, pushed on this thread and popped in `drop`.
        let context = unsafe {
            let context = g_main_context_new();
            g_main_context_push_thread_default(context);
            context
        };
        Self(NonNull::new(context).expect("g_main_context_new aborts rather than returning NULL"))
    }
}

impl Drop for PrivateContext {
    fn drop(&mut self) {
        // The drain reliably collects a sync GIO call's completion idle: it is
        // attached inside g_task_return(), before the condvar wake that lets
        // the caller reach this drop.
        // SAFETY: the context pushed in `push`, popped on the same thread; the
        // unref finalizes it, destroying any source a racing attach left.
        unsafe {
            while g_main_context_iteration(self.0.as_ptr(), GFALSE) != GFALSE {}
            g_main_context_pop_thread_default(self.0.as_ptr());
            g_main_context_unref(self.0.as_ptr());
        }
    }
}
