// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The address book backend has to hold a live `BookSync` between
// `connect_sync` and `disconnect_sync`, which means owning a Rust value inside
// a GObject instance struct. GObject hands `instance_init` memory it has
// zeroed and frees that memory once `finalize` returns, so the value has to be
// created and destroyed at exactly those two points — and a mistake at either
// end is a leak or a use-after-free in `evolution-addressbook-factory`, not a
// failing assertion. These tests pin both ends down.

use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use glib_sys::GType;
use gobject_sys::{GObject, GObjectClass, g_object_new, g_object_unref};
use jmap_backend_core::instance::Slot;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};

/// Reports its own destruction, so a test can tell "dropped" from "leaked".
struct Spy(&'static AtomicU32);

impl Drop for Spy {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// The state GObject leaves an instance struct in before `instance_init` runs.
/// A `Slot` has to read as empty there, because that is the only initialiser
/// it will ever get.
#[test]
fn a_zeroed_slot_is_empty() {
    // SAFETY: exactly the claim under test — all-zero bytes are a valid,
    // empty `Slot`, which is why one may live in memory GObject zeroed.
    let slot: Slot<String> = unsafe { MaybeUninit::zeroed().assume_init() };
    assert!(slot.get().is_none());
}

#[test]
fn a_value_put_in_a_slot_reads_back() {
    let slot = Slot::new();
    assert!(slot.init(String::from("Ab1")));
    assert_eq!(slot.get().map(String::as_str), Some("Ab1"));
    // SAFETY: no borrow taken above is still alive.
    unsafe { slot.clear() };
}

#[test]
fn clearing_a_slot_drops_the_value_and_leaves_it_empty() {
    static DROPS: AtomicU32 = AtomicU32::new(0);

    let slot = Slot::new();
    assert!(slot.init(Spy(&DROPS)));
    assert_eq!(DROPS.load(Ordering::SeqCst), 0);

    // SAFETY: nothing borrows the value.
    unsafe { slot.clear() };
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);
    assert!(slot.get().is_none());
}

/// Two things reach an empty slot: an instance whose `instance_init` stored
/// nothing, and a `clear` that already ran. Both end up freeing a NULL, which
/// has to be a no-op rather than the double free it would otherwise be.
#[test]
fn clearing_an_empty_slot_is_a_no_op() {
    static DROPS: AtomicU32 = AtomicU32::new(0);

    let never_filled: Slot<Spy> = Slot::new();
    // SAFETY: nothing borrows anything; the slot is empty.
    unsafe { never_filled.clear() };

    let slot = Slot::new();
    slot.init(Spy(&DROPS));
    // SAFETY: as above.
    unsafe {
        slot.clear();
        slot.clear();
    }
    assert_eq!(
        DROPS.load(Ordering::SeqCst),
        1,
        "the value was dropped twice"
    );
}

/// Initialising twice is a bug in the caller, and the recovery has to be the
/// conservative one: the value already in the slot may be borrowed by another
/// thread, so it stays and the newcomer is dropped rather than leaked.
#[test]
fn a_second_init_keeps_the_first_value_and_drops_the_second() {
    static FIRST: AtomicU32 = AtomicU32::new(0);
    static SECOND: AtomicU32 = AtomicU32::new(0);

    let slot = Slot::new();
    assert!(slot.init(Spy(&FIRST)));
    assert!(!slot.init(Spy(&SECOND)), "the second value took the slot");

    assert_eq!(
        SECOND.load(Ordering::SeqCst),
        1,
        "the rejected value leaked"
    );
    assert_eq!(
        FIRST.load(Ordering::SeqCst),
        0,
        "the first value was dropped"
    );

    // SAFETY: nothing borrows the value.
    unsafe { slot.clear() };
    assert_eq!(FIRST.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// The same thing again, through the GObject type system rather than by hand.
// ---------------------------------------------------------------------------

static HELD_DROPS: AtomicU32 = AtomicU32::new(0);

#[repr(C)]
struct HolderInstance {
    parent: GObject,
    data: Slot<Spy>,
}

#[repr(C)]
struct HolderClass {
    parent: GObjectClass,
}

struct Holder;

// SAFETY: both structs are #[repr(C)] and lead with the GObject instance and
// class structs respectively.
unsafe impl ObjectSubclass for Holder {
    const NAME: &'static CStr = c"JmapBackendCoreSlotHolder";
    type Instance = HolderInstance;
    type Class = HolderClass;

    fn parent_type() -> GType {
        gobject_sys::G_TYPE_OBJECT
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        unsafe { (*instance).data.init(Spy(&HELD_DROPS)) };
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: finalize runs once, after the last reference is gone, so no
        // borrow of the value can still be alive.
        unsafe { (*instance).data.clear() };
    }
}

#[test]
fn instance_data_lives_and_dies_with_the_object() {
    let obj = unsafe { g_object_new(register_static::<Holder>(), ptr::null()) };
    assert!(!obj.is_null());

    let instance = obj.cast::<HolderInstance>();
    assert!(
        unsafe { (*instance).data.get() }.is_some(),
        "instance_init left the slot empty"
    );
    assert_eq!(HELD_DROPS.load(Ordering::SeqCst), 0);

    unsafe { g_object_unref(obj) };
    assert_eq!(
        HELD_DROPS.load(Ordering::SeqCst),
        1,
        "finalize did not drop the instance data"
    );
}

// ---------------------------------------------------------------------------
// Chaining up. Skipping the parent's finalize leaks whatever it owns, which
// for an EBookMetaBackend is the ESource, the cache and the connection state.
// ---------------------------------------------------------------------------

static ORDER: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn record(what: &'static str) {
    ORDER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(what);
}

#[repr(C)]
struct BaseInstance {
    parent: GObject,
}

#[repr(C)]
struct BaseClass {
    parent: GObjectClass,
}

struct Base;

// SAFETY: #[repr(C)], leading with the GObject structs.
unsafe impl ObjectSubclass for Base {
    const NAME: &'static CStr = c"JmapBackendCoreFinalizeBase";
    type Instance = BaseInstance;
    type Class = BaseClass;

    fn parent_type() -> GType {
        gobject_sys::G_TYPE_OBJECT
    }

    unsafe fn finalize(_instance: *mut Self::Instance) {
        record("base");
    }
}

#[repr(C)]
struct QuietInstance {
    parent: BaseInstance,
}

#[repr(C)]
struct QuietClass {
    parent: BaseClass,
}

struct Quiet;

// SAFETY: #[repr(C)], leading with Base's instance and class structs, which is
// the type `parent_type` returns.
unsafe impl ObjectSubclass for Quiet {
    const NAME: &'static CStr = c"JmapBackendCoreFinalizeQuiet";
    type Instance = QuietInstance;
    type Class = QuietClass;

    /// Registering the parent from inside `parent_type` is what an EDS module
    /// does not need but a test hierarchy does — and it is also the shape that
    /// deadlocks if registration resolves the parent while holding its lock.
    fn parent_type() -> GType {
        register_static::<Base>()
    }

    unsafe fn finalize(_instance: *mut Self::Instance) {
        record("quiet");
    }
}

#[repr(C)]
struct BoomerInstance {
    parent: BaseInstance,
}

#[repr(C)]
struct BoomerClass {
    parent: BaseClass,
}

struct Boomer;

// SAFETY: as Quiet.
unsafe impl ObjectSubclass for Boomer {
    const NAME: &'static CStr = c"JmapBackendCoreFinalizeBoomer";
    type Instance = BoomerInstance;
    type Class = BoomerClass;

    fn parent_type() -> GType {
        register_static::<Base>()
    }

    unsafe fn finalize(_instance: *mut Self::Instance) {
        panic!("deliberate");
    }
}

/// Both halves in one test on purpose: they share `Base`, so running them
/// concurrently would make the recorded order depend on the scheduler.
#[test]
fn finalize_runs_the_subclass_first_and_always_chains_up() {
    let quiet = unsafe { g_object_new(register_static::<Quiet>(), ptr::null()) };
    assert!(!quiet.is_null());
    unsafe { g_object_unref(quiet) };
    assert_eq!(
        *ORDER.lock().unwrap(),
        ["quiet", "base"],
        "the parent's finalize did not run after the subclass's"
    );

    // A panic is caught, as everywhere else at the C boundary — but catching
    // it must not cost the parent its finalize, or a backend that panics once
    // leaks every instance from then on.
    let boomer = unsafe { g_object_new(register_static::<Boomer>(), ptr::null()) };
    assert!(!boomer.is_null());
    unsafe { g_object_unref(boomer) };
    assert_eq!(
        *ORDER.lock().unwrap(),
        ["quiet", "base", "base"],
        "a panicking finalize skipped the parent"
    );
}
