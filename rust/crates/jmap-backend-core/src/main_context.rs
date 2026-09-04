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
//! around such a call gives the bookkeeping a context of its own, iterated on
//! drop.
//!
//! Iterating is the load-bearing step, not the drop: the pending GTask holds a
//! reference on the context, so releasing our reference leaves a
//! context->source->task->context cycle standing; only dispatching the source
//! breaks it (confirmed on glib!5341). The context is private so that dispatch
//! does not also run someone else's sources (the ownership hazard
//! `jmap-mail/tests/common/signals.rs` documents).

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
        // Iterating is what reclaims: it dispatches the sync call's completion
        // idle (attached inside g_task_return, before the condvar wake that
        // lets the caller reach here), dropping the source's ref on the GTask
        // and the GTask's ref on this context. Pop and unref then release our
        // own reference; without the iteration they would free nothing.
        // SAFETY: the context pushed in `push`, popped on the same thread.
        unsafe {
            while g_main_context_iteration(self.0.as_ptr(), GFALSE) != GFALSE {}
            g_main_context_pop_thread_default(self.0.as_ptr());
            g_main_context_unref(self.0.as_ptr());
        }
    }
}
