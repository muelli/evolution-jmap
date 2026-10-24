// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Each test owns a private context on the test thread and invokes from another
// thread. GLib then puts the closure in the queue of the context.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use glib_sys::{
    GFALSE, GMainContext, g_main_context_acquire, g_main_context_iteration, g_main_context_new,
    g_main_context_release, g_main_context_unref,
};
use jmap_backend_core::invoke::invoke_in;

struct Context(*mut GMainContext);

// SAFETY: GLib documents `g_main_context_invoke_full` as thread-safe.
unsafe impl Send for Context {}

impl Context {
    /// A new context that the calling thread owns.
    fn owned() -> Self {
        // SAFETY: a new context, acquired by its only user.
        let context = unsafe { g_main_context_new() };
        assert_ne!(unsafe { g_main_context_acquire(context) }, GFALSE);
        Self(context)
    }

    /// Invoke `f` from another thread, so that `f` goes into the queue.
    fn invoke_from_thread(&self, f: impl FnOnce() + Send + 'static) {
        let context = Context(self.0);
        thread::spawn(move || {
            let context = context;
            // SAFETY: the test keeps the context alive until it joins this thread.
            unsafe { invoke_in(context.0, "invoke test", f) };
        })
        .join()
        .unwrap();
    }

    /// Iterate without blocking until `done`. Fail after five seconds.
    fn iterate_until(&self, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "the context never dispatched");
            // SAFETY: this thread owns the context.
            unsafe { g_main_context_iteration(self.0, GFALSE) };
        }
    }

    /// Release and free the context. GLib destroys the sources still in its queue.
    fn destroy(self) {
        // SAFETY: paired with `owned`; this is the last reference.
        unsafe {
            g_main_context_release(self.0);
            g_main_context_unref(self.0);
        }
    }
}

/// A value that reports its drop.
struct Signal(Arc<AtomicBool>);

impl Drop for Signal {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
fn the_closure_runs_one_time_on_the_thread_that_iterates() {
    let context = Context::owned();
    let runs = Arc::new(AtomicUsize::new(0));
    let ran_on = Arc::new(std::sync::Mutex::new(None));

    let (count, thread_id) = (runs.clone(), ran_on.clone());
    context.invoke_from_thread(move || {
        count.fetch_add(1, Ordering::SeqCst);
        *thread_id.lock().unwrap() = Some(thread::current().id());
    });
    assert_eq!(
        runs.load(Ordering::SeqCst),
        0,
        "the closure must wait in the queue"
    );

    context.iterate_until(|| runs.load(Ordering::SeqCst) > 0);
    for _ in 0..10 {
        // SAFETY: this thread owns the context.
        unsafe { g_main_context_iteration(context.0, GFALSE) };
    }
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(*ran_on.lock().unwrap(), Some(thread::current().id()));
    context.destroy();
}

#[test]
fn the_closure_is_dropped_after_it_runs() {
    let context = Context::owned();
    let dropped = Arc::new(AtomicBool::new(false));

    let signal = Signal(dropped.clone());
    context.invoke_from_thread(move || {
        let _signal = &signal;
    });
    context.iterate_until(|| dropped.load(Ordering::SeqCst));
    context.destroy();
}

#[test]
fn a_panic_stays_in_the_guard_and_the_context_continues() {
    let context = Context::owned();
    let after = Arc::new(AtomicBool::new(false));

    context.invoke_from_thread(|| panic!("a panic in an invoked closure"));
    let seen = after.clone();
    context.invoke_from_thread(move || seen.store(true, Ordering::SeqCst));

    context.iterate_until(|| after.load(Ordering::SeqCst));
    context.destroy();
}

#[test]
fn a_destroyed_context_drops_the_closure_without_running_it() {
    let context = Context::owned();
    let ran = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));

    let (seen, signal) = (ran.clone(), Signal(dropped.clone()));
    context.invoke_from_thread(move || {
        let _signal = &signal;
        seen.store(true, Ordering::SeqCst);
    });
    context.destroy();

    assert!(dropped.load(Ordering::SeqCst), "the closure must be freed");
    assert!(!ran.load(Ordering::SeqCst), "the closure must not run");
}
