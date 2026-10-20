// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// M2 acceptance: a subclass declared in Rust registers with the GObject type
// system of the *system* GLib, and you can instantiate it. All of M3/M4 needs
// this. Thus the test calls a real `g_object_new`, and does not only check
// that registration returned a non-zero GType.

use jmap_backend_core::subclass::{
    InterfaceDecl, InterfaceImpl, ObjectSubclass, register_dynamic, register_static,
};
use std::ffi::CStr;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use glib_sys::{GFALSE, GTRUE, gboolean};
use gobject_sys::{
    GObject, GObjectClass, GTypeInstance, GTypeModule, GTypeModuleClass, GTypePlugin,
    GTypePluginClass, g_object_new, g_object_unref, g_type_class_ref, g_type_class_unref,
    g_type_interface_peek, g_type_module_get_type, g_type_module_unuse, g_type_module_use,
    g_type_plugin_get_type, g_type_plugin_use,
};

/// Counts the runs of each trampoline. Thus the test can see the difference
/// between "the vfunc is connected" and "nothing ran and the defaults worked".
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
// instance struct and class struct, as the trait requires.
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

    // The type system has the requested name and parent.
    let name = unsafe { CStr::from_ptr(gobject_sys::g_type_name(gtype)) };
    assert_eq!(name, Test::NAME);
    assert_eq!(
        unsafe { gobject_sys::g_type_parent(gtype) },
        Test::parent_type()
    );

    let obj = unsafe { g_object_new(gtype, std::ptr::null()) };
    assert!(!obj.is_null(), "g_object_new returned NULL");

    // It is really an instance of the test type, and instance_init ran on it.
    assert_ne!(
        unsafe { gobject_sys::g_type_check_instance_is_a(obj as *mut GTypeInstance, gtype) },
        0
    );
    assert_eq!(unsafe { (*(obj as *mut TestInstance)).marker }, 0xF00D);
    assert_eq!(INSTANCE_INITS.load(Ordering::SeqCst), 1);
    assert_eq!(CLASS_INITS.load(Ordering::SeqCst), 1);

    unsafe { g_object_unref(obj) };
}

/// Registration occurs from `e_module_load`, which a process can call more
/// than once (EDS re-loads modules). A second `g_type_register_static` with
/// the same name is a fatal GLib error, so the helper must be idempotent.
#[test]
fn registering_twice_returns_the_same_type_instead_of_aborting() {
    let first = register_static::<Test>();
    let second = register_static::<Test>();
    assert_eq!(first, second);

    // GObject runs class_init lazily, at the first reference to the class.
    // The test above makes that reference through g_object_new. Without a
    // reference here, the assertion depends on which test ran first, and
    // cargo runs them concurrently. Thus reference the class here. With only
    // one registration in the process, class_init can run only once.
    let class = unsafe { gobject_sys::g_type_class_ref(first) };
    assert_eq!(CLASS_INITS.load(Ordering::SeqCst), 1);
    unsafe { gobject_sys::g_type_class_unref(class) };
}

// ---------------------------------------------------------------------------
// the dynamic half

/// Registered on the module below, not statically, so that the module has a
/// type to return.
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

/// A `GTypeModule` that registers [`Dynamic`] each time GLib loads it. An EDS
/// backend module does the same. It is the only way to test the dynamic path
/// without a shared object to dlopen.
#[repr(C)]
struct TestModule {
    parent: GTypeModule,
}

#[repr(C)]
struct TestModuleClass {
    parent_class: GTypeModuleClass,
}

// SAFETY: both structs are #[repr(C)] and start with the GTypeModule instance
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
        // SAFETY: `class` starts with GTypeModuleClass, which has both slots.
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

/// The reason why [`register_dynamic`] cannot use the "already registered,
/// nothing to do" shortcut that [`register_static`] must use.
///
/// An unuse of a module marks every type that the module registered as
/// unloaded. EDS does this when the last backend of the module is removed.
/// The next use calls the entry point again. If that call does not register
/// again, GLib does not fail gracefully. It aborts the full process with
/// "Could not reload previously loaded plugin".
#[test]
fn a_dynamic_type_is_registered_again_every_time_its_module_is_loaded() {
    let gtype = register_static::<TestModule>();
    // SAFETY: the type is registered and GTypeModule has no construct
    // properties of its own.
    let module = unsafe { g_object_new(gtype, ptr::null()) }.cast::<GTypeModule>();
    assert!(!module.is_null(), "g_object_new returned NULL");

    // SAFETY: `module` is a GTypeModule, and each use below has a matching
    // unuse.
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
/// `GTypePlugin`, not an EDS interface. It is a GObject interface with no
/// properties and four vfuncs that only GLib's own type loader calls. Thus it
/// is the only interface that a test can implement without satisfying it. The
/// Camel provider's settings type implements `CamelNetworkSettings`, which has
/// five properties that an implementer must override. That test belongs to
/// the implementing type, not here.
struct Implementor;

// SAFETY: as `Test`; the structs are shared with it.
unsafe impl ObjectSubclass for Implementor {
    const NAME: &'static CStr = c"JmapBackendCoreTestImplementor";
    type Instance = TestInstance;
    type Class = TestClass;

    fn parent_type() -> glib_sys::GType {
        gobject_sys::G_TYPE_OBJECT
    }

    fn interfaces() -> Vec<InterfaceDecl> {
        // SAFETY: no arguments, and the type initialises itself.
        vec![InterfaceDecl::defaults(unsafe { g_type_plugin_get_type() })]
    }
}

/// An interface that a Rust-declared type claims must be added to the type
/// before any code reads its class. An implementer satisfies the properties of
/// an interface with `g_object_class_override_property`. That runs in
/// `class_init`, and finds only properties of interfaces that the type already
/// implements. Thus the trait must add the interface, not the caller. If the
/// caller adds it after `register_static` returns, the caller gives out a
/// `GType` that implements nothing for a short time.
#[test]
fn a_declared_interface_is_added_before_the_type_is_handed_back() {
    let gtype = register_static::<Implementor>();
    // SAFETY: plain type-system reads on a type that this test registered.
    unsafe {
        assert_ne!(
            gobject_sys::g_type_is_a(gtype, gobject_sys::g_type_plugin_get_type()),
            GFALSE,
            "the type does not implement the interface it declared"
        );

        // The class initialises (an implementer's property overrides run
        // there), and an instance is really an instance of the interface.
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

/// Registration is idempotent. If you add the same interface to the same type
/// twice, GLib gives an error, not a no-op. Thus the second call must use the
/// same "already there" exit as the type.
#[test]
fn registering_an_implementor_twice_does_not_add_the_interface_again() {
    assert_eq!(
        register_static::<Implementor>(),
        register_static::<Implementor>()
    );
}

// ---------------------------------------------------------------------------
// interfaces whose vfunc slots the implementing type fills
//
// `GTypePlugin` again, for the reason above and one more. GLib itself
// dispatches through its vfuncs (`g_type_plugin_use` calls the slot, with no
// default). Thus a test can see the difference between "the slot is filled"
// and "something plausible occurred".

/// The number of dispatches to the slot that [`FilledPlugin`] installs.
static USES: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn count_use(_plugin: *mut GTypePlugin) {
    USES.fetch_add(1, Ordering::SeqCst);
}

/// A type that fills the interface it declares.
struct Filled;

/// …and the filling, declared next to it, not on it. One type can implement
/// several interfaces. A trait implemented on the type itself can describe
/// only one of them.
struct FilledPlugin;

// SAFETY: `GTypePluginClass` is gobject-sys's binding of the interface struct
// that `g_type_plugin_get_type()` names, and it starts with `GTypeInterface`.
unsafe impl InterfaceImpl for FilledPlugin {
    type Vtable = GTypePluginClass;

    fn gtype() -> glib_sys::GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { g_type_plugin_get_type() }
    }

    unsafe fn interface_init(vtable: *mut Self::Vtable) {
        // SAFETY: GLib passes this type's own copy of the vtable.
        unsafe { (*vtable).use_plugin = Some(count_use) };
    }
}

// SAFETY: as `Test`; the structs are shared with it.
unsafe impl ObjectSubclass for Filled {
    const NAME: &'static CStr = c"JmapBackendCoreTestFilled";
    type Instance = TestInstance;
    type Class = TestClass;

    fn parent_type() -> glib_sys::GType {
        gobject_sys::G_TYPE_OBJECT
    }

    fn interfaces() -> Vec<InterfaceDecl> {
        vec![InterfaceDecl::filled_by::<FilledPlugin>()]
    }
}

/// The main check: GLib dispatches an interface call to the slot that an
/// `interface_init` wrote. The test calls `g_type_plugin_use`, the interface's
/// own public wrapper. Camel's `camel_subscribable_folder_is_subscribed` will
/// get to the store the same way. The test does not read the slot back,
/// because that also passes if GLib never reads that copy of the vtable.
#[test]
fn glib_dispatches_an_interface_call_to_the_slot_the_type_filled() {
    let gtype = register_static::<Filled>();
    // SAFETY: the type is registered and has no construct properties.
    // `g_type_plugin_use` calls the slot that `FilledPlugin` filled. The code
    // below drops the one reference that it takes.
    unsafe {
        let obj = g_object_new(gtype, ptr::null());
        assert!(!obj.is_null(), "g_object_new returned NULL");

        assert_eq!(USES.load(Ordering::SeqCst), 0);
        g_type_plugin_use(obj.cast::<GTypePlugin>());
        assert_eq!(
            USES.load(Ordering::SeqCst),
            1,
            "the interface call did not reach the slot the type filled"
        );

        g_object_unref(obj);
    }
}

/// A type whose `interface_init` panics.
struct Faulty;
struct FaultyPlugin;

// SAFETY: as `FilledPlugin`.
unsafe impl InterfaceImpl for FaultyPlugin {
    type Vtable = GTypePluginClass;

    fn gtype() -> glib_sys::GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { g_type_plugin_get_type() }
    }

    unsafe fn interface_init(_vtable: *mut Self::Vtable) {
        panic!("an interface_init that gets it wrong");
    }
}

// SAFETY: as `Test`; the structs are shared with it.
unsafe impl ObjectSubclass for Faulty {
    const NAME: &'static CStr = c"JmapBackendCoreTestFaulty";
    type Instance = TestInstance;
    type Class = TestClass;

    fn parent_type() -> glib_sys::GType {
        gobject_sys::G_TYPE_OBJECT
    }

    fn interfaces() -> Vec<InterfaceDecl> {
        vec![InterfaceDecl::filled_by::<FaultyPlugin>()]
    }
}

/// An `interface_init` runs inside the type system, while the type system
/// holds its global lock. A panic that unwinds out of it aborts the process:
/// the full mail session, not only the account whose provider registered.
/// A guard protects it, as for every other entry point from C into this crate.
/// The guard logs the panic and does not change the vtable. For a slot that
/// was never written, the value stays NULL from the interface's own default.
#[test]
fn a_panicking_interface_init_does_not_unwind_into_the_type_system() {
    let gtype = register_static::<Faulty>();
    // SAFETY: the type is registered. The code below drops the class ref, and
    // only reads the vtable from it, with no call.
    unsafe {
        let class = g_type_class_ref(gtype);
        assert!(!class.is_null(), "g_type_class_ref returned NULL");

        let vtable =
            g_type_interface_peek(class, g_type_plugin_get_type()).cast::<GTypePluginClass>();
        assert!(!vtable.is_null(), "the type never got the vtable");
        assert!(
            (*vtable).use_plugin.is_none(),
            "the panic left a half-filled vtable behind"
        );

        g_type_class_unref(class);
    }
}

/// A type that declares no interfaces is the common case. It must not pay a
/// cost for the hook, or get an interface list that GObject must walk.
#[test]
fn a_type_that_declares_no_interfaces_implements_none() {
    let gtype = register_static::<Test>();
    let mut n = 0;
    // SAFETY: `g_type_interfaces` fills a count that this test owns and
    // returns a g_malloc'd array that the caller frees.
    unsafe {
        let interfaces = gobject_sys::g_type_interfaces(gtype, &mut n);
        glib_sys::g_free(interfaces.cast());
    }
    assert_eq!(n, 0);
}
