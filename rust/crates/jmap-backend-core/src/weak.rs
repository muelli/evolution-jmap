// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Access to a backend from a thread that does not own it.
//!
//! [`push`](crate::push) runs a JMAP Push reader on its own thread. When the
//! server reports a change, the reader must call an EDS function *on the
//! backend instance*. That instance is a GObject that EDS owns, and EDS can
//! drop it at any time. This includes the time between the push thread's
//! decision to call and the call. A raw `*mut` is a possible use-after-free.
//! A strong reference for the full life of the subscription is a cycle (the
//! instance owns the subscription), so the backend is never finalized.
//!
//! `GWeakRef` is GLib's solution to this problem, and the only one that is
//! safe from another thread. [`WeakBackend`] is a thin owning wrapper around
//! one:
//!
//! - [`WeakBackend::with_strong`] gives the closure a pointer that is alive
//!   *for the full call*, because it holds a strong reference during the
//!   call. If it cannot, it does not run the closure;
//! - it returns `None` from the time the object's last strong reference goes
//!   away. In GLib 2.80 (`gobject.c`'s `g_object_unref`), this is *before*
//!   `dispose` runs, not after `finalize`. GLib clears the weak locations
//!   under the same atomic that sees the refcount reach one. If a
//!   `g_weak_ref_get` wins that race, the unref starts again and does not
//!   dispose a resurrected object. Thus this type never gives out a pointer
//!   to a half-destroyed backend.
//!
//! [`WeakBackend::with_strong`] documents the one obligation that this type
//! cannot satisfy itself. If the last reference is dropped *inside* the
//! closure, finalization runs on the calling thread. Thus nothing that the
//! closure causes can wait on that thread.

use std::cell::UnsafeCell;
use std::ptr;

use gobject_sys::{
    GObject, GWeakRef, g_object_unref, g_weak_ref_clear, g_weak_ref_get, g_weak_ref_init,
};

/// A weak reference to a GObject, usable from any thread.
pub struct WeakBackend {
    /// Boxed because a `GWeakRef` registers itself with the object *by
    /// address*, so it must not move after
    /// [`g_weak_ref_init`](gobject_sys::g_weak_ref_init). The box keeps the
    /// address fixed, and the `WeakBackend` itself can move.
    /// `UnsafeCell` because every `g_weak_ref_*` call needs `*mut`, and this
    /// type gives out only `&self`. GLib does its own locking.
    weak: Box<UnsafeCell<GWeakRef>>,
}

// SAFETY: the documentation says that `GWeakRef` is safe to use concurrently
// from many threads. This is its purpose (`g_object_add_weak_pointer` is not).
// Only `with_strong` gives access to the object, and it does so under a
// strong reference.
unsafe impl Send for WeakBackend {}
unsafe impl Sync for WeakBackend {}

impl WeakBackend {
    /// Take a weak reference to `object`.
    ///
    /// # Safety
    ///
    /// `object` must be a valid `GObject`. The caller must hold at least one
    /// strong reference to it for the full call.
    pub unsafe fn new(object: *mut GObject) -> Self {
        let weak = Box::new(UnsafeCell::new(GWeakRef {
            priv_: gobject_sys::GWeakRef_priv { p: ptr::null_mut() },
        }));
        // SAFETY: the box is new and its contents never move, and this
        // function's contract makes `object` valid.
        unsafe { g_weak_ref_init(weak.get(), object) };
        Self { weak }
    }

    /// Run `f` on the object, under a strong reference that keeps it alive
    /// for the full call. After the object's last strong reference is gone,
    /// returns `None` and does not run `f`.
    ///
    /// # Panics and the last reference
    ///
    /// If the owner drops its own reference while `f` runs, the reference
    /// released here is the last one. Then the object is disposed and
    /// finalized *on this thread*, in this call. If a caller's finalize path
    /// can wait on this thread, the caller must handle this.
    /// [`push`](crate::push) does this explicitly.
    ///
    /// A panic in `f` leaks the reference and does not release it. This is
    /// intentional: it does not unwind through the borrow. For an object that
    /// EDS still owns, one reference too many is the safe failure, not one
    /// too few.
    pub fn with_strong<R>(&self, f: impl FnOnce(*mut GObject) -> R) -> Option<R> {
        // SAFETY: `new` initialised the `GWeakRef`, and this box still owns
        // it. `g_weak_ref_get` is thread-safe. It returns NULL or a reference
        // that this call now owns.
        let object = unsafe { g_weak_ref_get(self.weak.get()) };
        if object.is_null() {
            return None;
        }
        let result = f(object);
        // SAFETY: this releases the reference from `g_weak_ref_get`.
        unsafe { g_object_unref(object) };
        Some(result)
    }
}

impl Drop for WeakBackend {
    fn drop(&mut self) {
        // SAFETY: `new` initialised it, this clears it only once, and
        // `&mut self` means no `with_strong` can be running.
        unsafe { g_weak_ref_clear(self.weak.get()) };
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use glib_sys::gpointer;
    use gobject_sys::{G_TYPE_OBJECT, g_object_new_with_properties, g_object_weak_ref};

    use super::*;

    /// A bare `GObject`, which needs no EDS, no D-Bus and no display.
    /// `WeakBackend` does not depend on *which* GObject it gets, so the real
    /// backend adds no behaviour to check.
    fn plain_object() -> *mut GObject {
        // SAFETY: this sets no properties, so the count is zero and both
        // arrays can be NULL.
        unsafe { g_object_new_with_properties(G_TYPE_OBJECT, 0, ptr::null_mut(), ptr::null()) }
    }

    /// Counts finalizations. Thus a test can assert *when* one occurred, and
    /// does not read memory that is possibly not readable.
    static FINALIZED: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn count_finalize(_data: gpointer, _object: *mut GObject) {
        FINALIZED.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn a_weak_reference_reaches_a_live_object() {
        let object = plain_object();
        // SAFETY: the object is new, and this test holds one reference.
        let weak = unsafe { WeakBackend::new(object) };

        let seen = weak.with_strong(|strong| strong);
        assert_eq!(seen, Some(object));

        // SAFETY: this releases the test's own reference, the last one.
        unsafe { g_object_unref(object) };
    }

    #[test]
    fn a_weak_reference_stops_reaching_a_dropped_object() {
        let object = plain_object();
        // SAFETY: the object is new, and this test holds one reference.
        let weak = unsafe { WeakBackend::new(object) };
        // SAFETY: this releases the test's own reference, the last one. Thus
        // the object is gone before `with_strong` looks for it.
        unsafe { g_object_unref(object) };

        assert!(
            weak.with_strong(|_| unreachable!("the closure must not run"))
                .is_none(),
            "a weak reference to a released object must reach nothing"
        );
    }

    #[test]
    fn the_object_survives_until_the_closure_returns() {
        let before = FINALIZED.load(Ordering::SeqCst);
        let object = plain_object();
        // SAFETY: the object is new and alive. The notify runs at
        // finalization and only changes a static counter.
        unsafe { g_object_weak_ref(object, Some(count_finalize), ptr::null_mut()) };
        // SAFETY: the object is new, and this test holds one reference.
        let weak = unsafe { WeakBackend::new(object) };

        let finalized_inside = weak
            .with_strong(|_| {
                // The owner drops its reference during the call. This type
                // exists to survive this race. Now the strong reference from
                // `with_strong` is the only one.
                // SAFETY: this releases the test's own reference.
                unsafe { g_object_unref(object) };
                FINALIZED.load(Ordering::SeqCst)
            })
            .expect("the object was alive when the call started");

        assert_eq!(
            finalized_inside, before,
            "the object must not be finalized while the closure holds it"
        );
        assert_eq!(
            FINALIZED.load(Ordering::SeqCst),
            before + 1,
            "releasing the last reference on the way out must finalize it"
        );
        assert!(
            weak.with_strong(|_| ()).is_none(),
            "and the weak reference must then be empty"
        );
    }
}
