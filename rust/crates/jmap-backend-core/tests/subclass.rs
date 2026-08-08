// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// M2 acceptance: a subclass declared in Rust registers with the GObject type
// system of the *system* GLib and can be instantiated. Everything in M3/M4
// hangs off this, so the test drives a real `g_object_new` rather than only
// checking that registration returned a non-zero GType.

use jmap_backend_core::subclass::{ObjectSubclass, register_dynamic, register_static};
use std::ffi::CStr;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use glib_sys::{GFALSE, GTRUE, gboolean};
use gobject_sys::{
    GObject, GObjectClass, GTypeInstance, GTypeModule, GTypeModuleClass, g_object_new,
    g_object_unref, g_type_module_get_type, g_type_module_unuse, g_type_module_use,
};

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

// ---------------------------------------------------------------------------
// the dynamic half

/// Registered against the module below rather than statically, so that the
/// module has a type to hand back.
struct Dynamic;

// SAFETY: as `Test`; the structs are shared with it.
unsafe impl ObjectSubclass for Dynamic {
    const NAME: &'static CStr = c"JmapBackendCoreTestDynamicObject";
    type Instance = TestInstance;
    type Class = TestClass;

    fn parent_type() -> glib_sys::GType {
        gobject_sys::G_TYPE_OBJECT
    }
}

/// A `GTypeModule` that registers [`Dynamic`] whenever GLib loads it — which
/// is what an EDS backend module does, and the only way to exercise the
/// dynamic path without a shared object to dlopen.
#[repr(C)]
struct TestModule {
    parent: GTypeModule,
}

#[repr(C)]
struct TestModuleClass {
    parent_class: GTypeModuleClass,
}

// SAFETY: both structs are #[repr(C)] and lead with the GTypeModule instance
// and class structs, and GTypeModule derives from GObject.
unsafe impl ObjectSubclass for TestModule {
    const NAME: &'static CStr = c"JmapBackendCoreTestModule";
    type Instance = TestModule;
    type Class = TestModuleClass;

    fn parent_type() -> glib_sys::GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { g_type_module_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` leads with GTypeModuleClass, where both slots live.
        let vfuncs = unsafe { &mut (*class).parent_class };
        vfuncs.load = Some(module_load);
        vfuncs.unload = Some(module_unload);
    }
}

unsafe extern "C" fn module_load(module: *mut GTypeModule) -> gboolean {
    // SAFETY: GLib passes the module it is loading.
    unsafe { register_dynamic::<Dynamic>(module) };
    GTRUE
}

unsafe extern "C" fn module_unload(_module: *mut GTypeModule) {}

/// The reason [`register_dynamic`] cannot take the same "already registered,
/// nothing to do" shortcut [`register_static`] has to take.
///
/// Unusing a module — which EDS does as soon as the last backend it provided
/// goes away — marks every type that module registered as unloaded. Using it
/// again calls the entry point a second time, and if that call does not
/// re-register, GLib does not fail gracefully: it aborts the whole process
/// with "Could not reload previously loaded plugin".
#[test]
fn a_dynamic_type_is_registered_again_every_time_its_module_is_loaded() {
    let gtype = register_static::<TestModule>();
    // SAFETY: the type is registered and GTypeModule has no construct
    // properties of its own.
    let module = unsafe { g_object_new(gtype, ptr::null()) }.cast::<GTypeModule>();
    assert!(!module.is_null(), "g_object_new returned NULL");

    // SAFETY: `module` is a GTypeModule, and every use below is balanced by
    // an unuse.
    unsafe {
        assert_ne!(
            g_type_module_use(module),
            GFALSE,
            "the module would not load"
        );
        g_type_module_unuse(module);
        assert_ne!(
            g_type_module_use(module),
            GFALSE,
            "the second load did not re-register the module's types"
        );
        g_type_module_unuse(module);
    }
}

// ---------------------------------------------------------------------------
// interfaces

/// A type that implements an interface as well as deriving from one.
///
/// `GTypePlugin` rather than anything from EDS: it is a GObject interface with
/// no properties and four vfuncs nothing but GLib's own type loader ever calls,
/// which makes it the one interface a test can implement without also having to
/// satisfy it. The interface the Camel provider's settings type actually
/// implements, `CamelNetworkSettings`, carries five properties an implementer
/// must override — that belongs to the type that implements it, not here.
struct Implementor;

// SAFETY: as `Test`; the structs are shared with it.
unsafe impl ObjectSubclass for Implementor {
    const NAME: &'static CStr = c"JmapBackendCoreTestImplementor";
    type Instance = TestInstance;
    type Class = TestClass;

    fn parent_type() -> glib_sys::GType {
        gobject_sys::G_TYPE_OBJECT
    }

    fn interfaces() -> Vec<glib_sys::GType> {
        // SAFETY: no arguments, and the type initialises itself.
        vec![unsafe { gobject_sys::g_type_plugin_get_type() }]
    }
}

/// An interface a Rust-declared type claims has to be added to the type before
/// anything can look at its class — `g_object_class_override_property`, which
/// is how an implementer satisfies an interface's properties, runs in
/// `class_init` and can only find a property of an interface the type is
/// already known to implement. So this is the trait's job and not the caller's:
/// a caller that added the interface after `register_static` returned would
/// have handed out a `GType` that, for one window, implements nothing.
#[test]
fn a_declared_interface_is_added_before_the_type_is_handed_back() {
    let gtype = register_static::<Implementor>();
    // SAFETY: plain type-system reads on a type we just registered.
    unsafe {
        assert_ne!(
            gobject_sys::g_type_is_a(gtype, gobject_sys::g_type_plugin_get_type()),
            GFALSE,
            "the type does not implement the interface it declared"
        );

        // The class initialises — which is where an implementer's property
        // overrides would run — and an instance really is an instance of the
        // interface.
        let obj = g_object_new(gtype, ptr::null());
        assert!(!obj.is_null(), "g_object_new returned NULL");
        assert_ne!(
            gobject_sys::g_type_check_instance_is_a(
                obj.cast::<GTypeInstance>(),
                gobject_sys::g_type_plugin_get_type()
            ),
            GFALSE
        );
        g_object_unref(obj);
    }
}

/// Registration is idempotent, and adding the same interface to the same type
/// twice is a GLib error rather than a no-op. The second call has to take the
/// same "already there" exit the type itself takes.
#[test]
fn registering_an_implementor_twice_does_not_add_the_interface_again() {
    assert_eq!(
        register_static::<Implementor>(),
        register_static::<Implementor>()
    );
}

/// A type with nothing to declare is the common case and must not pay for the
/// hook — nor acquire an interface list GObject would then have to walk.
#[test]
fn a_type_that_declares_no_interfaces_implements_none() {
    let gtype = register_static::<Test>();
    let mut n = 0;
    // SAFETY: `g_type_interfaces` fills a count we own and returns a
    // g_malloc'd array the caller frees.
    unsafe {
        let interfaces = gobject_sys::g_type_interfaces(gtype, &mut n);
        glib_sys::g_free(interfaces.cast());
    }
    assert_eq!(n, 0);
}
