// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A GObject reference that this side owns. The reference is released when
//! it goes out of scope.
//!
//! GLib's transfer annotations are a contract in prose. The caller must
//! release a `(transfer full)` return, and must not release a
//! `(transfer none)` return. By hand, that contract is a `g_object_unref` on
//! every path out of the function. This is correct only until somebody adds
//! a path. `jmap-backend-cal/src/marshal.rs` alone once had ~15 such sites.
//! All were correct, but the compiler could check none of them.
//!
//! [`Owned`] moves that contract into the type system. A transfer-full
//! pointer becomes a value with a [`Drop`]. Thus a new `return`, `?` or
//! `continue` in a function releases it automatically. Transfer-none pointers
//! stay raw, so the two kinds look different at the call site.
//!
//! The tree already uses this idiom for non-GObject state
//! (`jmap_backend_core::instance::Slot`, `jmap-mail`'s `MessageCache`,
//! `jmap-backend-collection`'s `populate::Frozen`). This type applies it to
//! reference-counted C pointers.
//!
//! Scope: **GObject instances**, released with `g_object_unref`. This
//! includes the libical-glib types (`ICalComponent`, `ICalProperty`,
//! `ICalParameter`, `ICalTimezone`), because libical-glib is a GObject
//! binding. It also includes EDS's and Camel's own objects. It does not
//! include GLib allocations that have their own free function. A `gchar *`
//! is for `marshal::read_string`/`take_string`. A boxed type needs its own
//! wrapper, not an incorrect `unref` here.

use std::ptr::NonNull;

use gobject_sys::g_object_unref;

/// A strong reference to a GObject instance, released on drop.
///
/// Construct it from a `(transfer full)` pointer with [`Owned::from_raw`].
/// Use [`Owned::as_ptr`] to get the pointer for a borrowing call.
/// Use [`Owned::into_raw`] when a C function takes ownership of the reference
/// (`i_cal_component_take_component`, a `GSList` node, an out-parameter).
///
/// Not `Send` or `Sync`: the refcount is atomic, but the object is usually
/// not thread-safe. Also, every use in this tree is in one vfunc call.
/// `NonNull` gives this property automatically.
///
/// `T` is the concrete C struct (`Owned<ICalComponent>`, not
/// `Owned<GObject>`). Thus the returned pointer needs no cast, and you cannot
/// give it to the incorrect function.
pub struct Owned<T> {
    ptr: NonNull<T>,
}

impl<T> Owned<T> {
    /// Takes ownership of a `(transfer full)` pointer. Returns `None` for NULL.
    /// Every libical and EDS getter uses NULL for "there is none", so it is not
    /// an error. The caller's `let Some(…) else` or `is_some()` *is* the NULL
    /// check, and the caller cannot forget it separately from the unref.
    ///
    /// # Safety
    ///
    /// `ptr` must be NULL, or a pointer to a live GObject instance. The caller
    /// must own a strong reference to it. That reference moves into the
    /// returned value. The caller must not release it, and must not use `ptr`
    /// after the value is dropped.
    #[must_use]
    pub unsafe fn from_raw(ptr: *mut T) -> Option<Self> {
        NonNull::new(ptr).map(|ptr| Self { ptr })
    }

    /// The pointer, still owned here. It is valid while `self` is valid, so it
    /// is correct for a borrowing call.
    #[must_use]
    pub fn as_ptr(&self) -> *mut T {
        self.ptr.as_ptr()
    }

    /// Gives the reference away. It returns the pointer and does **no unref**.
    /// Use it for a C function that takes ownership, not for one that borrows.
    #[must_use]
    pub fn into_raw(self) -> *mut T {
        let ptr = self.ptr.as_ptr();
        // The reference now belongs to the caller. `Drop` must not release it.
        std::mem::forget(self);
        ptr
    }
}

impl<T> Drop for Owned<T> {
    fn drop(&mut self) {
        // SAFETY: the `from_raw` contract makes this a live GObject instance,
        // and the reference is ours. Only `into_raw` removes the reference
        // without this unref. It forgets `self`, so it does not get here.
        unsafe { g_object_unref(self.ptr.as_ptr().cast()) }
    }
}
