// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The answer of a worker thread, delivered on the GLib main loop.
//!
//! All features in this crate have the same structure:
//!
//! 1. A callback starts on the main loop.
//! 2. The JMAP round trip must not block the main loop.
//! 3. The answer must go back to the main loop, where code can change
//!    widgets, but only if the widget still exists.
//!
//! jmap-backend-core has the two halves ([`WeakBackend`], [`guard`]). It has
//! no connection between them, because worker threads control EDS backends,
//! and the main loop controls a UI. This module is that connection.
//!
//! Here, "main loop" is the global default `GMainContext`. In Evolution, the
//! GTK main loop owns it. In a process where nothing owns it (a test), GLib
//! runs an invoked function on the calling thread. Thus the tests below
//! acquire the context first, to get the queue that the shell supplies.

use std::ptr;

use gobject_sys::GObject;
use jmap_backend_core::invoke::invoke_in;
use jmap_backend_core::trampoline::guard;
use jmap_backend_core::weak::WeakBackend;

/// Run `f` once on the global main context, panic-guarded.
pub fn on_main<F>(f: F)
where
    F: FnOnce() + Send + 'static,
{
    // SAFETY: NULL identifies the global main context.
    unsafe { invoke_in(ptr::null_mut(), "jmap-ui main-loop dispatch", f) };
}

/// Run `work` on its own thread, then run `finish(object, answer)` on the
/// global main context. If `object` is finalized before that, do not run
/// `finish`.
///
/// The weak reference makes a closed composer or settings page unreachable,
/// not a dangling pointer.
///
/// A panic guard wraps `work`: it logs a panic and delivers nothing.
///
/// # Safety
///
/// `object` must be a valid `GObject`. The caller must hold a strong
/// reference to it for the duration of this call.
pub unsafe fn spawn_for<T, W, F>(object: *mut GObject, work: W, finish: F)
where
    T: Send + 'static,
    W: FnOnce() -> T + Send + 'static,
    F: FnOnce(*mut GObject, T) + Send + 'static,
{
    // SAFETY: valid, with a strong reference, by the contract of this function.
    let weak = unsafe { WeakBackend::new(object) };
    std::thread::spawn(move || {
        let Some(answer) = guard("jmap-ui worker", None, || Some(work())) else {
            return;
        };
        on_main(move || {
            weak.with_strong(|object| finish(object, answer));
        });
    });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex, MutexGuard};
    use std::time::{Duration, Instant};

    use glib_sys::{
        GFALSE, g_main_context_acquire, g_main_context_iteration, g_main_context_release,
    };
    use gobject_sys::{G_TYPE_OBJECT, g_object_new_with_properties, g_object_unref};

    use super::*;

    /// All tests below iterate the one global main context. Thus they must
    /// not run at the same time.
    static MAIN_CONTEXT: Mutex<()> = Mutex::new(());

    /// Own the global main context for one test, as the Evolution main loop
    /// owns it. Without an owner, `g_main_context_invoke` runs closures on the
    /// calling thread and does not put them in a queue.
    struct Owned {
        _lock: MutexGuard<'static, ()>,
    }

    impl Owned {
        fn acquire() -> Self {
            let lock = MAIN_CONTEXT.lock().unwrap();
            // SAFETY: NULL is the global context. The lock serializes access.
            assert_ne!(unsafe { g_main_context_acquire(ptr::null_mut()) }, GFALSE);
            Self { _lock: lock }
        }

        /// Iterate without blocking until `done`. Fail after five seconds.
        fn iterate_until(&self, done: &AtomicBool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !done.load(Ordering::SeqCst) {
                assert!(Instant::now() < deadline, "main loop never delivered");
                // SAFETY: this thread owns the context.
                unsafe { g_main_context_iteration(ptr::null_mut(), GFALSE) };
                std::thread::yield_now();
            }
        }
    }

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: paired with the acquire above.
            unsafe { g_main_context_release(ptr::null_mut()) };
        }
    }

    /// An answer that reports its drop. With it, the dead-object test can see
    /// that the dispatch ran and discarded the answer.
    struct Signal(Arc<AtomicBool>);

    impl Drop for Signal {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    fn plain_object() -> *mut GObject {
        // SAFETY: no properties, so the count is zero and both arrays are NULL.
        unsafe { g_object_new_with_properties(G_TYPE_OBJECT, 0, ptr::null_mut(), ptr::null()) }
    }

    #[test]
    fn the_answer_reaches_a_live_object_on_the_main_loop() {
        let owned = Owned::acquire();
        let object = plain_object();
        let finished = Arc::new(AtomicBool::new(false));

        let seen = finished.clone();
        // SAFETY: a new object, with one reference held here.
        unsafe {
            spawn_for(
                object,
                || 6 * 7,
                move |_, answer| {
                    assert_eq!(answer, 42);
                    seen.store(true, Ordering::SeqCst);
                },
            );
        }
        owned.iterate_until(&finished);

        // SAFETY: this releases the reference of this test.
        unsafe { g_object_unref(object) };
    }

    #[test]
    fn a_dead_object_receives_nothing() {
        let owned = Owned::acquire();
        let object = plain_object();
        let dispatched = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));

        let (release, held) = mpsc::channel::<()>();
        let signal = Signal(dispatched.clone());
        let seen = finished.clone();
        // SAFETY: a new object, with one reference held here.
        unsafe {
            spawn_for(
                object,
                move || {
                    // Wait until the object is finalized. The test checks this order.
                    held.recv().unwrap();
                    signal
                },
                move |_, _| seen.store(true, Ordering::SeqCst),
            );
        }

        // SAFETY: this releases the reference of this test, the last one.
        unsafe { g_object_unref(object) };
        release.send(()).unwrap();

        // The drop of the answer shows that the dispatch delivered and discarded it.
        owned.iterate_until(&dispatched);
        assert!(
            !finished.load(Ordering::SeqCst),
            "finish must not run for an object that is gone"
        );
    }
}
