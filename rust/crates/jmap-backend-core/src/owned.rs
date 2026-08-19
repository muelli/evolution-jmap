// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A GObject reference this side owns, released when it goes out of scope.
//!
//! GLib's transfer annotations are a contract stated in prose: a
//! `(transfer full)` return is a reference the caller must release, and a
//! `(transfer none)` one is a reference it must not. Written out by hand, that
//! contract is a `g_object_unref` on every path out of the function — which is
//! correct exactly as long as nobody adds a path. `docs/UNSAFE-AUDIT.md`
//! Pattern C found ~15 such sites in `jmap-backend-cal/src/marshal.rs` alone,
//! every one of them correct at the time of the audit and none of them
//! checkable by the compiler.
//!
//! [`Owned`] moves that contract into the type system: a transfer-full pointer
//! becomes a value with a [`Drop`], so a new `return`, `?` or `continue` in the
//! middle of a function releases it without anyone remembering to. Transfer-none
//! pointers stay raw, which is the other half of the point — the two kinds no
//! longer look alike at the call site.
//!
//! This is the same idiom the tree already applies to non-GObject state
//! (`jmap_backend_core::instance::Slot`, `jmap-mail`'s `MessageCache`,
//! `jmap-backend-collection`'s `populate::Frozen`), just applied to the
//! reference-counted C pointers that were left out of it.
//!
//! Scope: **GObject instances**, released with `g_object_unref`. That covers
//! the libical-glib types (`ICalComponent`, `ICalProperty`, `ICalParameter`,
//! `ICalTimezone`) as well as EDS's and Camel's own objects, because
//! libical-glib is a GObject binding. It deliberately does not cover
//! GLib allocations with their own free function — a `gchar *` is
//! `marshal::read_string`/`take_string`'s business, and a boxed type would need
//! its own wrapper rather than a wrong `unref` here.

use std::ptr::NonNull;

use gobject_sys::g_object_unref;

/// A strong reference to a GObject instance, released on drop.
///
/// Construct it from a `(transfer full)` pointer with [`Owned::from_raw`], read
/// the pointer back with [`Owned::as_ptr`] for a borrowing call, and hand the
/// reference on with [`Owned::into_raw`] when a C function takes ownership of it
/// (`i_cal_component_take_component`, a `GSList` node, an out-parameter).
///
/// Not `Send` or `Sync`: the refcount is atomic but the object behind it is
/// generally not thread-safe, and every use in this tree is within one vfunc
/// call anyway. `NonNull` gives that for free.
///
/// `T` is the concrete C struct — `Owned<ICalComponent>` rather than
/// `Owned<GObject>` — so the pointer that comes back out needs no cast and
/// cannot be passed to the wrong function.
pub struct Owned<T> {
    ptr: NonNull<T>,
}

impl<T> Owned<T> {
    /// Takes ownership of a `(transfer full)` pointer. `None` for NULL, which
    /// every libical and EDS getter uses for "there is none" and which is
    /// therefore not an error to report — the caller's `let Some(…) else` or
    /// `is_some()` *is* the NULL check, and one that cannot be forgotten
    /// separately from the unref.
    ///
    /// # Safety
    ///
    /// `ptr` must be NULL, or a pointer to a live GObject instance this caller
    /// owns a strong reference to. That reference moves into the returned value:
    /// the caller must not release it, and must not use `ptr` after the value is
    /// dropped.
    #[must_use]
    pub unsafe fn from_raw(ptr: *mut T) -> Option<Self> {
        NonNull::new(ptr).map(|ptr| Self { ptr })
    }

    /// The pointer, still owned here. Valid for as long as `self` is, which is
    /// what makes it the right thing to pass to a borrowing call.
    #[must_use]
    pub fn as_ptr(&self) -> *mut T {
        self.ptr.as_ptr()
    }

    /// Gives the reference away: the pointer comes back out and **no unref
    /// happens**, so this is the call for a C function that takes ownership, and
    /// the wrong call for one that borrows.
    #[must_use]
    pub fn into_raw(self) -> *mut T {
        let ptr = self.ptr.as_ptr();
        // The reference now belongs to the caller; running `Drop` would release
        // a reference it is being handed.
        std::mem::forget(self);
        ptr
    }
}

impl<T> Drop for Owned<T> {
    fn drop(&mut self) {
        // SAFETY: `from_raw`'s contract is that this is a live GObject instance
        // and that the reference is ours; `into_raw` is the only way to leave
        // here without it, and it forgets `self` rather than reaching this.
        unsafe { g_object_unref(self.ptr.as_ptr().cast()) }
    }
}
