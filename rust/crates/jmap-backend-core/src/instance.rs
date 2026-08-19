// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Owning a Rust value inside a GObject instance struct.
//!
//! The address book backend keeps a live connection between `connect_sync`
//! and `disconnect_sync`, so its instance struct has to hold a Rust value with
//! a destructor. GObject's allocator gives that value a very particular life:
//! the struct arrives at `instance_init` zeroed, and the memory is handed back
//! to the allocator as soon as `finalize` returns. There is no `Drop` — the
//! bytes simply stop being ours.
//!
//! [`Slot`] is the plumbing for that. It stores an owning pointer, so
//! *all-zero bytes are its empty state* — which is exactly the state GObject
//! leaves the field in before anything of ours has run. That one property is
//! what makes the type usable at all: there is no window in which the field
//! holds something that is neither initialised nor safely readable.
//!
//! Every operation is therefore defined on every state a real instance can be
//! in, including the ones that only happen when something has already gone
//! wrong:
//!
//! - reading before `instance_init` (or after a failed one) yields `None`
//!   rather than a dangling reference, so a vfunc reached on a half-built
//!   instance can report a clean error;
//! - clearing an empty slot is a no-op, because an instance whose
//!   `instance_init` stored nothing is still finalized;
//! - a second [`init`](Slot::init) is refused rather than overwriting, since
//!   the value already there may be borrowed by another thread.

use std::any::type_name;
use std::marker::PhantomData;
#[cfg(feature = "testing")]
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use crate::trampoline::log_critical;

/// Storage for one `T` inside a GObject instance struct.
///
/// Put one in a `#[repr(C)]` instance struct, fill it from
/// [`ObjectSubclass::instance_init`], and empty it from
/// [`ObjectSubclass::finalize`]:
///
/// ```ignore
/// #[repr(C)]
/// struct JmapBookBackend {
///     parent: EBookMetaBackend,
///     session: Slot<Mutex<Option<BookSync>>>,
/// }
/// ```
///
/// [`ObjectSubclass::instance_init`]: crate::subclass::ObjectSubclass::instance_init
/// [`ObjectSubclass::finalize`]: crate::subclass::ObjectSubclass::finalize
#[repr(transparent)]
pub struct Slot<T> {
    /// NULL means empty; otherwise a `Box<T>` this slot owns. Atomic because
    /// EDS calls a backend's vfuncs from more than one thread.
    value: AtomicPtr<T>,
    /// Makes `Send`/`Sync` follow `T`, which a bare `AtomicPtr<T>` would not:
    /// it is unconditionally both, and a `&Slot<T>` hands out a `&T`.
    _owns: PhantomData<T>,
}

impl<T> Slot<T> {
    /// An empty slot. Byte-for-byte identical to the zeroed field GObject
    /// hands `instance_init`, which is where real instances get theirs.
    pub const fn new() -> Self {
        Self {
            value: AtomicPtr::new(ptr::null_mut()),
            _owns: PhantomData,
        }
    }

    /// Fills an empty slot, returning whether it took the value.
    ///
    /// A slot that is already filled keeps what it has and drops `value`:
    /// replacing it would free something another thread may be reading
    /// through a [`get`](Slot::get) borrow. The refusal is logged as a GLib
    /// critical, because reaching it at all is a bug in the caller.
    pub fn init(&self, value: T) -> bool {
        let boxed = Box::into_raw(Box::new(value));
        if self
            .value
            .compare_exchange(ptr::null_mut(), boxed, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return true;
        }

        // SAFETY: the exchange failed, so nothing took ownership of `boxed`
        // and this is the only pointer to it.
        drop(unsafe { Box::from_raw(boxed) });
        log_critical(&format!(
            "instance data of type {} initialised twice; keeping the first value",
            type_name::<T>()
        ));
        false
    }

    /// The value, or `None` if the slot has not been filled — or has already
    /// been emptied, which is what a vfunc reached after `finalize` would see.
    pub fn get(&self) -> Option<&T> {
        // SAFETY: a non-NULL pointer came from `init`, and only `clear` frees
        // it. `clear` is unsafe precisely so that it carries the obligation
        // not to run while a borrow handed out here is still alive.
        unsafe { self.value.load(Ordering::Acquire).as_ref() }
    }

    /// Drops the value and empties the slot. Doing so twice, or on a slot that
    /// was never filled, does nothing.
    ///
    /// # Safety
    ///
    /// No borrow returned by [`get`](Slot::get) may still be alive, and none
    /// may be taken concurrently. `finalize` satisfies this by construction:
    /// it runs once, after the last reference to the object is gone, so
    /// nothing can still reach the instance.
    pub unsafe fn clear(&self) {
        let previous = self.value.swap(ptr::null_mut(), Ordering::AcqRel);
        if !previous.is_null() {
            // SAFETY: the swap took the pointer out of the slot, so this is
            // the only owner of the `Box` `init` created.
            drop(unsafe { Box::from_raw(previous) });
        }
    }
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for Slot<T> {
    /// Only reached by a `Slot` that lives in Rust-owned memory — a slot in a
    /// GObject instance struct is never dropped, because GObject frees the
    /// struct without running a destructor over it. That is what
    /// [`ObjectSubclass::finalize`] is for.
    ///
    /// [`ObjectSubclass::finalize`]: crate::subclass::ObjectSubclass::finalize
    fn drop(&mut self) {
        // SAFETY: `&mut self` proves no borrow is outstanding.
        unsafe { self.clear() }
    }
}

/// Builds a `Box<T>` from an all-zero allocation, bypassing `T`'s own
/// constructors entirely.
///
/// This is the shared plumbing behind the `detached()` test doubles across
/// the backend crates (`docs/UNSAFE-AUDIT.md` Pattern A): each builds an
/// instance outside the GObject type system because a real one needs
/// infrastructure — a running `evolution-source-registry`, a `CamelSession`,
/// a `GtkWidget`'s display — that neither the test VM nor CI has.
/// `#[cfg(feature = "testing")]`-gated so that path is compiler-enforced
/// test-only, not just documented.
///
/// # Safety
///
/// Every field of `T` must be valid at all-zero bytes — the same thing each
/// call site already has to verify on its own account (a pointer or an
/// integer field, or a [`Slot`], whose empty state *is* all-zero). The
/// result is not a valid `T` beyond that one property: passing it to code
/// that assumes real construction ran — an EDS or Camel function, for
/// instance — is undefined behaviour.
#[cfg(feature = "testing")]
pub unsafe fn zeroed_box<T>() -> Box<T> {
    // SAFETY: forwarded to the caller via this function's own contract above.
    unsafe { Box::new(MaybeUninit::zeroed().assume_init()) }
}
