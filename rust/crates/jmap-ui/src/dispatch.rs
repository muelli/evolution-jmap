// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A worker thread's answer, delivered back on GLib's main loop.
//!
//! Every feature in this crate has the same shape: a callback fires on the
//! main loop, the JMAP round trip must not block it, and the answer has to
//! land back on the main loop — where widgets may be touched — but only if
//! the widget still exists. jmap-backend-core has both halves
//! ([`WeakBackend`], [`guard`]); what it never needed is the bridge between
//! them, because EDS backends are driven *by* worker threads while a UI is
//! driven by the main loop. This is that bridge.
//!
//! "Main loop" here is the global default `GMainContext`, which in Evolution
//! is owned by GTK's main loop. In a process where nothing owns it — a test —
//! GLib runs an invoked function on whichever thread called, so the tests
//! below acquire the context first to get the queueing the shell provides for
//! free.

use std::ptr;

use glib_sys::{G_PRIORITY_DEFAULT, GFALSE, g_main_context_invoke_full, gboolean, gpointer};
use gobject_sys::GObject;
use jmap_backend_core::trampoline::guard;
use jmap_backend_core::weak::WeakBackend;

/// What crosses into GLib: the closure, boxed so it has one address, inside
/// an `Option` so the dispatch can take it while the destroy notify keeps
/// sole ownership of the box.
type Payload = Option<Box<dyn FnOnce() + Send>>;

/// Run `f` once on the global main context, panic-guarded.
pub fn on_main<F>(f: F)
where
    F: FnOnce() + Send + 'static,
{
    unsafe extern "C" fn call(data: gpointer) -> gboolean {
        // SAFETY: `data` is the `Payload` box `on_main` leaked. Borrowed, not
        // reclaimed: GLib still owns it and frees it through `drop_payload`.
        let payload = unsafe { &mut *data.cast::<Payload>() };
        if let Some(f) = payload.take() {
            guard("jmap-ui main-loop dispatch", (), f);
        }
        GFALSE // one shot — G_SOURCE_REMOVE
    }
    unsafe extern "C" fn drop_payload(data: gpointer) {
        // SAFETY: reclaiming the box `on_main` leaked, exactly once: GLib
        // calls the destroy notify after the source dispatched (or when the
        // context is freed with the source still queued).
        drop(unsafe { Box::from_raw(data.cast::<Payload>()) });
    }

    let payload: Box<Payload> = Box::new(Some(Box::new(f)));
    // SAFETY: NULL names the global main context, and the payload stays alive
    // until `drop_payload` because GLib owns it from here.
    unsafe {
        g_main_context_invoke_full(
            ptr::null_mut(),
            G_PRIORITY_DEFAULT,
            Some(call),
            Box::into_raw(payload).cast(),
            Some(drop_payload),
        );
    }
}

/// Run `work` on its own thread, then `finish(object, answer)` on the global
/// main context — or not at all, when `object` died in the meantime. The weak
/// reference is what makes a closed composer or settings page unreachable
/// rather than dangling.
///
/// `work` runs panic-guarded: a panic is logged and delivers nothing.
///
/// # Safety
///
/// `object` must be a valid `GObject` with a strong reference held by the
/// caller for the length of this call.
pub unsafe fn spawn_for<T, W, F>(object: *mut GObject, work: W, finish: F)
where
    T: Send + 'static,
    W: FnOnce() -> T + Send + 'static,
    F: FnOnce(*mut GObject, T) + Send + 'static,
{
    // SAFETY: valid with a strong reference, per this function's contract.
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

    use glib_sys::{g_main_context_acquire, g_main_context_iteration, g_main_context_release};
    use gobject_sys::{G_TYPE_OBJECT, g_object_new_with_properties, g_object_unref};

    use super::*;

    /// The tests below all iterate the one global main context, so they must
    /// not run concurrently.
    static MAIN_CONTEXT: Mutex<()> = Mutex::new(());

    /// Own the global main context for one test, the way Evolution's main
    /// loop owns it for real: without an owner, `g_main_context_invoke` runs
    /// closures on the calling thread instead of queueing them.
    struct Owned {
        _lock: MutexGuard<'static, ()>,
    }

    impl Owned {
        fn acquire() -> Self {
            let lock = MAIN_CONTEXT.lock().unwrap();
            // SAFETY: NULL is the global context; serialized by the lock.
            assert_ne!(unsafe { g_main_context_acquire(ptr::null_mut()) }, GFALSE);
            Self { _lock: lock }
        }

        /// Iterate without blocking until `done`, or fail after five seconds.
        fn iterate_until(&self, done: &AtomicBool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !done.load(Ordering::SeqCst) {
                assert!(Instant::now() < deadline, "main loop never delivered");
                // SAFETY: the context is owned by this thread.
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

    /// An answer that reports its own drop: how the dead-object test can tell
    /// the dispatch really ran (and discarded it) rather than never arriving.
    struct Signal(Arc<AtomicBool>);

    impl Drop for Signal {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    fn plain_object() -> *mut GObject {
        // SAFETY: no properties, so the count is zero and both arrays NULL.
        unsafe { g_object_new_with_properties(G_TYPE_OBJECT, 0, ptr::null_mut(), ptr::null()) }
    }

    #[test]
    fn the_answer_reaches_a_live_object_on_the_main_loop() {
        let owned = Owned::acquire();
        let object = plain_object();
        let finished = Arc::new(AtomicBool::new(false));

        let seen = finished.clone();
        // SAFETY: freshly constructed, one reference held here.
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

        // SAFETY: releasing this test's own reference.
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
        // SAFETY: freshly constructed, one reference held here.
        unsafe {
            spawn_for(
                object,
                move || {
                    // Wait for the object to die first; that ordering is the
                    // whole test.
                    held.recv().unwrap();
                    signal
                },
                move |_, _| seen.store(true, Ordering::SeqCst),
            );
        }

        // SAFETY: releasing this test's own reference, the last one.
        unsafe { g_object_unref(object) };
        release.send(()).unwrap();

        // The answer's own drop marks the dispatch as delivered-and-discarded.
        owned.iterate_until(&dispatched);
        assert!(
            !finished.load(Ordering::SeqCst),
            "finish must not run for an object that is gone"
        );
    }
}
