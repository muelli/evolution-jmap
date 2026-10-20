// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Ownership of a Rust value inside a GObject instance struct.
//!
//! The address book backend keeps a live connection between `connect_sync`
//! and `disconnect_sync`. Thus its instance struct must hold a Rust value
//! with a destructor. GObject's allocator gives that value a special life.
//! The struct is zeroed when it gets to `instance_init`, and the allocator
//! gets the memory back immediately after `finalize` returns. There is no
//! `Drop`: the bytes are simply no longer ours.
//!
//! [`Slot`] gives the necessary support. It stores an owning pointer, so
//! *all-zero bytes are its empty state*. This is the state that GObject gives
//! the field before our code runs. Because of this property, the type is
//! usable: at no time does the field hold a value that is not initialised
//! and not safe to read.
//!
//! Thus every operation is defined on every state of a real instance. This
//! includes the states that occur only after a failure:
//!
//! - a read before `instance_init` (or after a failed one) gives `None`, not
//!   a dangling reference. Thus a vfunc on a half-built instance can report a
//!   clean error;
//! - a clear on an empty slot does nothing, because GObject also finalizes
//!   an instance whose `instance_init` stored nothing;
//! - a second [`init`](Slot::init) is refused and does not overwrite,
//!   because another thread can borrow the value that is already there.

use std::any::type_name;
use std::marker::PhantomData;
#[cfg(feature = "testing")]
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use crate::trampoline::log_critical;

/// Storage for one `T` inside a GObject instance struct.
///
/// Put one in a `#[repr(C)]` instance struct. Fill it in
/// [`ObjectSubclass::instance_init`]. Empty it in
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
/// A `T` that threads can share but cannot *move* between them cannot go in
/// a slot. The bound below enforces this:
///
/// ```compile_fail
/// use std::sync::MutexGuard;
/// use jmap_backend_core::instance::Slot;
///
/// fn assert_sync<T: Sync>() {}
/// // `MutexGuard` is `Sync` and deliberately not `Send`, so a `Slot` holding
/// // one must not be `Sync`: see the `Sync` impl below for why.
/// assert_sync::<Slot<MutexGuard<'static, ()>>>();
/// ```
///
/// [`ObjectSubclass::instance_init`]: crate::subclass::ObjectSubclass::instance_init
/// [`ObjectSubclass::finalize`]: crate::subclass::ObjectSubclass::finalize
#[repr(transparent)]
pub struct Slot<T> {
    /// NULL means empty. Otherwise it is a `Box<T>` that this slot owns.
    /// Atomic because EDS calls a backend's vfuncs from more than one thread.
    value: AtomicPtr<T>,
    /// Tells that this slot *owns* a `T`. A bare `AtomicPtr<T>` does not tell
    /// this. Drop-check needs it, because [`clear`](Slot::clear) runs `T`'s
    /// destructor. Without it, the compiler accepts a `T` that borrows from a
    /// value that outlives the slot only by inference.
    ///
    /// This field does *not* control `Send` and `Sync`. See the two impls
    /// below.
    _owns: PhantomData<T>,
}

// SAFETY: a `Slot<T>` owns only a heap `T`. Thus a move between threads moves
// that `T`, the same as for `Box<T>`.
unsafe impl<T: Send> Send for Slot<T> {}

// SAFETY: the bound is `Send` and `Sync`. The `Send` bound is the only reason
// that this impl is explicit and not derived from the `PhantomData<T>` above.
//
// `Sync` lets a `&Slot<T>` cross a thread boundary, and every method here
// takes `&self`. [`get`](Slot::get) gives out a `&T`, so it needs `T: Sync`.
// [`clear`](Slot::clear) *drops* the `T`. [`init`](Slot::init) drops the `T`
// it gets when the slot is already full. `Send` controls whether a destructor
// can run on a thread that the value never belonged to. Thus a `T` that is
// `Sync` but not `Send` (for example, a `MutexGuard`) must not make a
// `Slot<T>` `Sync`. The derived bound was only `T: Sync` and allowed it.
//
// Every `Slot` in this project holds a `Mutex`/`RwLock` of owned data, which
// is both. Thus this stricter bound prevents no use.
unsafe impl<T: Send + Sync> Sync for Slot<T> {}

impl<T> Slot<T> {
    /// An empty slot. It is identical, byte for byte, to the zeroed field that
    /// GObject gives `instance_init`. Real instances get their slot there.
    pub const fn new() -> Self {
        Self {
            value: AtomicPtr::new(ptr::null_mut()),
            _owns: PhantomData,
        }
    }

    /// Fills an empty slot. Returns whether it took the value.
    ///
    /// A slot that is already full keeps its value and drops `value`. A
    /// replacement would free a value that another thread can read through a
    /// [`get`](Slot::get) borrow. The refusal is logged as a GLib critical,
    /// because it occurs only when the caller has a bug.
    pub fn init(&self, value: T) -> bool {
        let boxed = Box::into_raw(Box::new(value));
        if self
            .value
            .compare_exchange(ptr::null_mut(), boxed, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return true;
        }

        // SAFETY: the exchange failed, so nothing took ownership of `boxed`.
        // This is the only pointer to it.
        drop(unsafe { Box::from_raw(boxed) });
        log_critical(&format!(
            "instance data of type {} initialised twice; keeping the first value",
            type_name::<T>()
        ));
        false
    }

    /// The value, or `None` if the slot is not filled yet or is already
    /// empty. A vfunc that runs after `finalize` sees an empty slot.
    pub fn get(&self) -> Option<&T> {
        // SAFETY: a non-NULL pointer came from `init`, and only `clear` frees
        // it. `clear` is unsafe so that its caller must not run it while a
        // borrow from this function is still alive.
        unsafe { self.value.load(Ordering::Acquire).as_ref() }
    }

    /// Drops the value and empties the slot. A second call, or a call on a
    /// slot that was never filled, does nothing.
    ///
    /// # Safety
    ///
    /// No borrow from [`get`](Slot::get) can still be alive, and no code can
    /// take one concurrently. `finalize` always satisfies this. It runs once,
    /// after the last reference to the object is gone, so nothing can get to
    /// the instance.
    pub unsafe fn clear(&self) {
        let previous = self.value.swap(ptr::null_mut(), Ordering::AcqRel);
        if !previous.is_null() {
            // SAFETY: the swap took the pointer out of the slot, so this is
            // the only owner of the `Box` from `init`.
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
    /// Runs only for a `Slot` in Rust-owned memory. A slot in a GObject
    /// instance struct is never dropped, because GObject frees the struct
    /// without a destructor. [`ObjectSubclass::finalize`] does that work.
    ///
    /// [`ObjectSubclass::finalize`]: crate::subclass::ObjectSubclass::finalize
    fn drop(&mut self) {
        // SAFETY: `&mut self` proves no borrow is outstanding.
        unsafe { self.clear() }
    }
}

/// Builds a `Box<T>` from an all-zero allocation, and does not use any of
/// `T`'s own constructors.
///
/// The `detached()` test doubles in the backend crates all use this function.
/// Each builds an instance outside the GObject type system. A real instance
/// needs infrastructure that the test VM and CI do not have (a running
/// `evolution-source-registry`, a `CamelSession`, a `GtkWidget`'s display).
/// `#[cfg(feature = "testing")]` makes the compiler enforce that only tests
/// use this path.
///
/// # Safety
///
/// Every field of `T` must be valid at all-zero bytes. Each call site must
/// already verify this itself (a pointer or an integer field, or a [`Slot`],
/// whose empty state *is* all-zero). Apart from this property, the result is
/// not a valid `T`. Do not give it to code that needs real construction (for
/// example, an EDS or Camel function). That is undefined behaviour.
#[cfg(feature = "testing")]
pub unsafe fn zeroed_box<T>() -> Box<T> {
    // SAFETY: this function's contract (above) makes this the caller's duty.
    unsafe { Box::new(MaybeUninit::zeroed().assume_init()) }
}
