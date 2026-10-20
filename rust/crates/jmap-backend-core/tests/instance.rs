// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The address book backend must hold a live `BookSync` between
// `connect_sync` and `disconnect_sync`. Thus a GObject instance struct owns a
// Rust value. GObject gives zeroed memory to `instance_init` and frees that
// memory after `finalize` returns. Thus the value must be created and
// destroyed at exactly those two points. A mistake at either point is a leak
// or a use-after-free in `evolution-addressbook-factory`, not a failed
// assertion. These tests check both points.

use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use glib_sys::GType;
use gobject_sys::{GObject, GObjectClass, g_object_new, g_object_unref};
use jmap_backend_core::instance::Slot;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};

/// Reports its own destruction, so a test can see "dropped" or "leaked".
struct Spy(&'static AtomicU32);

impl Drop for Spy {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// This is the state of an instance struct before `instance_init` runs.
/// A `Slot` must read as empty in this state, because it gets no other
/// initialiser.
#[test]
fn a_zeroed_slot_is_empty() {
    // SAFETY: this is the claim under test. All-zero bytes are a valid,
    // empty `Slot`, so a `Slot` can be in memory that GObject zeroed.
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

/// Two cases give an empty slot: an instance whose `instance_init` stored
/// nothing, and a `clear` that already ran. Both free a NULL, which must be a
/// no-op and not a double free.
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

/// The `Drop` impl of `Slot` runs only outside a GObject instance struct,
/// where Rust owns the memory of the slot. These tests use a bare `Slot` that
/// goes out of scope without a manual `clear`.
#[test]
fn dropping_a_filled_slot_drops_the_value() {
    static DROPS: AtomicU32 = AtomicU32::new(0);
    {
        let slot = Slot::new();
        assert!(slot.init(Spy(&DROPS)));
        assert_eq!(DROPS.load(Ordering::SeqCst), 0);
    }
    assert_eq!(
        DROPS.load(Ordering::SeqCst),
        1,
        "dropping the slot did not drop its value"
    );
}

#[test]
fn dropping_an_empty_slot_does_not_call_drop_on_anything() {
    static DROPS: AtomicU32 = AtomicU32::new(0);
    {
        let _never_filled: Slot<Spy> = Slot::new();
    }
    assert_eq!(DROPS.load(Ordering::SeqCst), 0);
}

/// A second initialisation is a bug in the caller, and the recovery must be
/// conservative. Another thread can borrow the value in the slot, so that
/// value stays. The new value is dropped, not leaked.
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
// zeroed_box

/// A substitute for the value that the `detached()` of each backend builds.
/// For each field, the all-zero bit pattern is a valid value. Each call site
/// must make sure of this property before it calls `zeroed_box`.
#[repr(C)]
struct ZeroableFields {
    flag: AtomicU32,
    pointer: *mut u8,
    count: usize,
}

#[test]
fn zeroed_box_builds_a_value_from_all_zero_bytes_without_running_a_constructor() {
    // SAFETY: each field of `ZeroableFields` is valid at all-zero bytes: an
    // `AtomicU32`, a pointer (NULL) and a `usize`.
    let value: Box<ZeroableFields> = unsafe { jmap_backend_core::instance::zeroed_box() };
    assert_eq!(value.flag.load(Ordering::SeqCst), 0);
    assert!(value.pointer.is_null());
    assert_eq!(value.count, 0);
}

// ---------------------------------------------------------------------------
// The same test again, through the GObject type system, not by hand.
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

// SAFETY: both structs are #[repr(C)] and start with the GObject instance and
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
        // SAFETY: finalize runs once, after the last reference is released,
        // so no borrow of the value can still be alive.
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
// Chain up. Without the finalize of the parent, all that the parent owns
// leaks. For an EBookMetaBackend, this is the ESource, the cache and the
// connection state.
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

// SAFETY: #[repr(C)], starting with the GObject structs.
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

// SAFETY: #[repr(C)], starting with the instance and class structs of Base,
// which is the type that `parent_type` returns.
unsafe impl ObjectSubclass for Quiet {
    const NAME: &'static CStr = c"JmapBackendCoreFinalizeQuiet";
    type Instance = QuietInstance;
    type Class = QuietClass;

    /// An EDS module does not register the parent from inside `parent_type`,
    /// but a test hierarchy must. This shape also deadlocks if registration
    /// resolves the parent while it holds its lock.
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

/// Both halves are in one test intentionally. They share `Base`, so if they
/// ran concurrently, the recorded order would depend on the scheduler.
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

    // A panic is caught, as at all other points of the C boundary. But the
    // parent finalize must still run, or a backend that panics once leaks
    // each later instance.
    let boomer = unsafe { g_object_new(register_static::<Boomer>(), ptr::null()) };
    assert!(!boomer.is_null());
    unsafe { g_object_unref(boomer) };
    assert_eq!(
        *ORDER.lock().unwrap(),
        ["quiet", "base", "base"],
        "a panicking finalize skipped the parent"
    );
}
