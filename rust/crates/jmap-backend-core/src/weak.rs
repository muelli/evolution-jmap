// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reaching a backend from a thread that does not own it.
//!
//! [`push`](crate::push) runs a JMAP Push reader on its own thread and, when
//! the server says something changed, has to call an EDS function *on the
//! backend instance*. That instance is a GObject owned by EDS, which may
//! decide to drop it at any moment — including while the push thread is
//! between deciding to call and calling. A raw `*mut` would be a
//! use-after-free waiting for the right interleaving; a strong reference held
//! for the subscription's whole life would be a cycle (the instance owns the
//! subscription), so the backend would never be finalized at all.
//!
//! `GWeakRef` is GLib's own answer to exactly this, and the only one that is
//! safe from another thread. [`WeakBackend`] is a thin owning wrapper around
//! one:
//!
//! - [`WeakBackend::with_strong`] either hands the closure a pointer that is
//!   guaranteed alive *for the length of the call* — it holds a strong
//!   reference across it — or does not run the closure at all;
//! - it starts returning `None` from the moment the object's last strong
//!   reference goes away, which in GLib 2.80 (`gobject.c`'s `g_object_unref`)
//!   is *before* `dispose` runs, not after `finalize`: the weak locations are
//!   cleared under the same atomic that observes the refcount reaching one,
//!   and a `g_weak_ref_get` that wins that race makes the unref restart
//!   rather than dispose a resurrected object. So there is no window in which
//!   this hands out a pointer to a half-destroyed backend.
//!
//! The one obligation it cannot discharge itself is noted on
//! [`WeakBackend::with_strong`]: dropping the last reference *inside* the
//! closure means finalization runs on the calling thread, so whatever the
//! closure ultimately triggers must not wait on that thread.

use std::cell::UnsafeCell;
use std::ptr;

use gobject_sys::{
    GObject, GWeakRef, g_object_unref, g_weak_ref_clear, g_weak_ref_get, g_weak_ref_init,
};

/// A weak reference to a GObject, usable from any thread.
pub struct WeakBackend {
    /// Boxed because a `GWeakRef` registers itself with the object *by
    /// address* and must therefore not be moved after
    /// [`g_weak_ref_init`](gobject_sys::g_weak_ref_init); the box keeps the
    /// address fixed while the `WeakBackend` itself is free to move.
    /// `UnsafeCell` because every `g_weak_ref_*` call wants `*mut`, and this
    /// type hands out only `&self` — GLib does its own locking.
    weak: Box<UnsafeCell<GWeakRef>>,
}

// SAFETY: `GWeakRef` is documented as safe to use concurrently from several
// threads — that is the entire point of it over `g_object_add_weak_pointer` —
// and `with_strong` is the only way to get at the object, which it does under
// a strong reference.
unsafe impl Send for WeakBackend {}
unsafe impl Sync for WeakBackend {}

impl WeakBackend {
    /// Take a weak reference to `object`.
    ///
    /// # Safety
    ///
    /// `object` must be a valid `GObject` with at least one strong reference
    /// held by the caller for the length of this call.
    pub unsafe fn new(object: *mut GObject) -> Self {
        let weak = Box::new(UnsafeCell::new(GWeakRef {
            priv_: gobject_sys::GWeakRef_priv { p: ptr::null_mut() },
        }));
        // SAFETY: the box is freshly allocated and never moved from, and
        // `object` is valid by this function's contract.
        unsafe { g_weak_ref_init(weak.get(), object) };
        Self { weak }
    }

    /// Run `f` against the object, under a strong reference that keeps it
    /// alive for the whole call. `None` — and `f` never runs — once the
    /// object's last strong reference is gone.
    ///
    /// # Panics and the last reference
    ///
    /// If the owner drops its own reference while `f` runs, the reference
    /// released here is the last one, so the object is disposed and finalized
    /// *on this thread*, inside this call. Callers whose finalize path can
    /// wait on this thread must cope with that; [`push`](crate::push)'s does
    /// explicitly.
    ///
    /// A panic in `f` leaks the reference rather than releasing it: this
    /// deliberately does not unwind through the borrow, since the safe
    /// failure for an object EDS still owns is one too many references, not
    /// one too few.
    pub fn with_strong<R>(&self, f: impl FnOnce(*mut GObject) -> R) -> Option<R> {
        // SAFETY: the `GWeakRef` was initialised by `new` and is still
        // owned by this box; `g_weak_ref_get` is thread-safe and returns
        // either NULL or a reference this call now owns.
        let object = unsafe { g_weak_ref_get(self.weak.get()) };
        if object.is_null() {
            return None;
        }
        let result = f(object);
        // SAFETY: releasing the reference `g_weak_ref_get` handed us.
        unsafe { g_object_unref(object) };
        Some(result)
    }
}

impl Drop for WeakBackend {
    fn drop(&mut self) {
        // SAFETY: initialised by `new`, cleared exactly once, and no
        // `with_strong` can be running — `&mut self`.
        unsafe { g_weak_ref_clear(self.weak.get()) };
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use glib_sys::gpointer;
    use gobject_sys::{G_TYPE_OBJECT, g_object_new_with_properties, g_object_weak_ref};

    use super::*;

    /// A bare `GObject`, which needs no EDS, no D-Bus and no display — the
    /// point of testing against one is that `WeakBackend` says nothing about
    /// *which* GObject, so the real backend adds no behaviour to check.
    fn plain_object() -> *mut GObject {
        // SAFETY: no properties are being set, so the count is zero and both
        // arrays may be NULL.
        unsafe { g_object_new_with_properties(G_TYPE_OBJECT, 0, ptr::null_mut(), ptr::null()) }
    }

    /// Counts finalizations, so a test can assert *when* one happened rather
    /// than poking at memory that may or may not still be readable.
    static FINALIZED: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn count_finalize(_data: gpointer, _object: *mut GObject) {
        FINALIZED.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn a_weak_reference_reaches_a_live_object() {
        let object = plain_object();
        // SAFETY: freshly constructed, one reference held here.
        let weak = unsafe { WeakBackend::new(object) };

        let seen = weak.with_strong(|strong| strong);
        assert_eq!(seen, Some(object));

        // SAFETY: releasing this test's own reference, the last one.
        unsafe { g_object_unref(object) };
    }

    #[test]
    fn a_weak_reference_stops_reaching_a_dropped_object() {
        let object = plain_object();
        // SAFETY: freshly constructed, one reference held here.
        let weak = unsafe { WeakBackend::new(object) };
        // SAFETY: releasing this test's own reference, the last one, so the
        // object is gone by the time `with_strong` looks for it.
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
        // SAFETY: freshly constructed and alive; the notify runs at
        // finalization and only touches a static counter.
        unsafe { g_object_weak_ref(object, Some(count_finalize), ptr::null_mut()) };
        // SAFETY: freshly constructed, one reference held here.
        let weak = unsafe { WeakBackend::new(object) };

        let finalized_inside = weak
            .with_strong(|_| {
                // The owner drops its reference mid-call, exactly the race
                // this type exists to survive: the strong reference taken by
                // `with_strong` is now the only one left.
                // SAFETY: releasing this test's own reference.
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
