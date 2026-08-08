// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// M2 acceptance: a subclass declared in Rust registers with the GObject type
// system of the *system* GLib and can be instantiated. Everything in M3/M4
// hangs off this, so the test drives a real `g_object_new` rather than only
// checking that registration returned a non-zero GType.

use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use std::ffi::CStr;
use std::sync::atomic::{AtomicU32, Ordering};

use gobject_sys::{GObject, GObjectClass, GTypeInstance, g_object_new, g_object_unref};

/// Counts how often each trampoline ran, so the test can tell "the vfunc was
/// wired up" from "nothing was called and the defaults happened to work".
static CLASS_INITS: AtomicU32 = AtomicU32::new(0);
static INSTANCE_INITS: AtomicU32 = AtomicU32::new(0);

#[repr(C)]
struct TestInstance {
    parent: GObject,
    marker: u32,
}

#[repr(C)]
struct TestClass {
    parent: GObjectClass,
}

struct Test;

// SAFETY: TestInstance/TestClass are #[repr(C)] and start with the GObject
// instance and class structs respectively, as the trait requires.
unsafe impl ObjectSubclass for Test {
    const NAME: &'static CStr = c"JmapBackendCoreTestObject";
    type Instance = TestInstance;
    type Class = TestClass;

    fn parent_type() -> glib_sys::GType {
        gobject_sys::G_TYPE_OBJECT
    }

    unsafe fn class_init(class: *mut Self::Class) {
        assert!(!class.is_null());
        CLASS_INITS.fetch_add(1, Ordering::SeqCst);
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        unsafe { (*instance).marker = 0xF00D };
        INSTANCE_INITS.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn a_rust_declared_subclass_registers_and_instantiates() {
    let gtype = register_static::<Test>();
    assert_ne!(gtype, 0, "registration returned the invalid GType");

    // The type system agrees on the name and the parent we asked for.
    let name = unsafe { CStr::from_ptr(gobject_sys::g_type_name(gtype)) };
    assert_eq!(name, Test::NAME);
    assert_eq!(
        unsafe { gobject_sys::g_type_parent(gtype) },
        Test::parent_type()
    );

    let obj = unsafe { g_object_new(gtype, std::ptr::null()) };
    assert!(!obj.is_null(), "g_object_new returned NULL");

    // It really is an instance of our type, and instance_init ran on it.
    assert_ne!(
        unsafe { gobject_sys::g_type_check_instance_is_a(obj as *mut GTypeInstance, gtype) },
        0
    );
    assert_eq!(unsafe { (*(obj as *mut TestInstance)).marker }, 0xF00D);
    assert_eq!(INSTANCE_INITS.load(Ordering::SeqCst), 1);
    assert_eq!(CLASS_INITS.load(Ordering::SeqCst), 1);

    unsafe { g_object_unref(obj) };
}

/// Registration happens from `e_module_load`, which a process can reach more
/// than once (EDS re-loads modules). A second `g_type_register_static` with
/// the same name is a fatal GLib error, so the helper has to be idempotent.
#[test]
fn registering_twice_returns_the_same_type_instead_of_aborting() {
    let first = register_static::<Test>();
    let second = register_static::<Test>();
    assert_eq!(first, second);

    // GObject runs class_init lazily, when the class is first referenced —
    // which the test above happens to do via g_object_new. Asserting the
    // counter without forcing it makes this test depend on which of the two
    // ran first, and cargo runs them concurrently. Reference the class here
    // instead; with only one registration in the process it can only ever
    // have run once.
    let class = unsafe { gobject_sys::g_type_class_ref(first) };
    assert_eq!(CLASS_INITS.load(Ordering::SeqCst), 1);
    unsafe { gobject_sys::g_type_class_unref(class) };
}
