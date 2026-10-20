// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Two modules, two entry points, one process: each call must reach the module
// that it was made on.
//
// Each EDS/Evolution module that this repository installs exports the same two
// C symbols, `e_module_load` and `e_module_unload`, because `EModule` resolves
// that pair from a shared object. In production this is safe: each `.so` is
// dlopened separately and the symbols do not meet. In a test binary, they
// meet. `jmap-config` dev-depends on `jmap-backend-collection` (the account
// that this crate writes is correct only if the collection backend reads it
// back as the same account). Thus both rlibs are in one link, with two
// definitions of `e_module_load`.
//
// A `#[unsafe(no_mangle)]` function *is* its C symbol. Thus, while the two
// rlibs had those definitions, there were no separate
// `jmap_config::module::` and `jmap_backend_collection::module::` entry points.
// The two Rust paths compiled to a call to the same symbol, and the definition
// that the linker kept answered both. With `CARGO_INCREMENTAL=0` (the CMake
// setting for the `rust-test-eds` ctest), this is a `duplicate symbol` link
// error. With the incremental default, it linked and the failure was silent:
// `jmap_config::module::e_module_load` registered the types of the
// *collection* backend, and nothing reported it.
//
// The rlibs do not define these symbols now. Each `.so` is built from a
// `*-module` cdylib crate that holds only the two `no_mangle` functions. The
// bodies are ordinary Rust functions with ordinary mangled names. This test
// keeps it so. It calls the entry point of each crate through a separate
// `GTypeModule` and asks the type system which types appeared. The answer
// identifies the two definitions, and a link that merged them cannot give that
// answer. Thus, if a `no_mangle` is added again to an rlib, this test fails:
// with a wrong answer here, or with a link error under CMake.

use std::ffi::CStr;
use std::ptr;

use glib_sys::{GTRUE, GType, gboolean};
use gobject_sys::{
    GTypeModule, GTypeModuleClass, g_object_new, g_type_from_name, g_type_get_plugin,
    g_type_module_get_type, g_type_module_use,
};
use jmap_backend_core::subclass::{ObjectSubclass, register_static};

/// The names that the two crates register, written here and not imported.
///
/// Imported constants are equal to the names that the types under test use,
/// and those names are the subject of the test. A test that gets the answer
/// from the code agrees with a module that registers wrong types with wrong
/// names.
const CONFIG_BACKEND: &CStr = c"EMailConfigServiceBackendJmap";
const COLLECTION_BACKEND: &CStr = c"ECollectionBackendJmap";
const COLLECTION_FACTORY: &CStr = c"ECollectionBackendJmapFactory";

/// A `GTypeModule` in place of the `EModule` that a host loads this code as.
/// The concrete types below select the entry point.
#[repr(C)]
struct TestModule {
    parent: GTypeModule,
}

#[repr(C)]
struct TestModuleClass {
    parent_class: GTypeModuleClass,
}

/// The substitute for the setup module: its `load` calls the `jmap-config`
/// entry point.
#[repr(C)]
struct ConfigModule(TestModule);

#[repr(C)]
struct ConfigModuleClass(TestModuleClass);

// SAFETY: both structs are #[repr(C)] and lead with the GTypeModule instance
// and class structs, and GTypeModule derives from GObject.
unsafe impl ObjectSubclass for ConfigModule {
    const NAME: &'static CStr = c"JmapConfigEntryPointTestModule";
    type Instance = ConfigModule;
    type Class = ConfigModuleClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { g_type_module_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points to a newly allocated class struct of this type,
        // which starts with GTypeModuleClass, where the slot is.
        unsafe { (*class).0.parent_class.load = Some(load_config) }
    }
}

unsafe extern "C" fn load_config(module: *mut GTypeModule) -> gboolean {
    // SAFETY: GLib gives the module that loads, which is what the entry
    // point needs.
    unsafe { jmap_config::module::load(module) };
    GTRUE
}

/// The substitute for the collection module: its `load` calls the entry point
/// of the *other* crate, which was the same C symbol before.
#[repr(C)]
struct CollectionModule(TestModule);

#[repr(C)]
struct CollectionModuleClass(TestModuleClass);

// SAFETY: as `ConfigModule`.
unsafe impl ObjectSubclass for CollectionModule {
    const NAME: &'static CStr = c"JmapCollectionEntryPointTestModule";
    type Instance = CollectionModule;
    type Class = CollectionModuleClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { g_type_module_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: as `ConfigModule`'s.
        unsafe { (*class).0.parent_class.load = Some(load_collection) }
    }
}

unsafe extern "C" fn load_collection(module: *mut GTypeModule) -> gboolean {
    // SAFETY: as `load_config`.
    unsafe { jmap_backend_collection::module::load(module) };
    GTRUE
}

/// Builds a substitute module of type `T` and uses it, which runs its `load`.
///
/// The reference from the use is never released: the types must stay
/// registered for the subsequent assertions.
fn use_module<T: ObjectSubclass>() -> *mut GTypeModule {
    let gtype = register_static::<T>();
    assert_ne!(gtype, 0, "the stand-in module type did not register");

    // SAFETY: the type is registered and GTypeModule has no construct
    // properties of its own.
    let module = unsafe { g_object_new(gtype, ptr::null()) }.cast::<GTypeModule>();
    assert!(!module.is_null(), "g_object_new returned NULL");

    // SAFETY: `module` is a GTypeModule.
    assert_ne!(
        unsafe { g_type_module_use(module) },
        0,
        "the stand-in module would not load"
    );
    module
}

/// The module that the type system has for a type, or null.
fn plugin_of(name: &CStr) -> *mut GTypeModule {
    // SAFETY: `name` is NUL-terminated. g_type_get_plugin accepts all GTypes,
    // also 0 for a name that is not registered.
    unsafe { g_type_get_plugin(g_type_from_name(name.as_ptr())) }.cast()
}

/// One test, not three, for two reasons. The sequence of the steps is
/// important: only before the second `load` runs can the test check what the
/// first `load` did *not* register. Also, the GLib type system is
/// process-global, so separate `#[test]`s would race.
#[test]
fn each_module_registers_its_own_types_and_only_its_own() {
    let config = use_module::<ConfigModule>();

    assert_eq!(
        plugin_of(CONFIG_BACKEND),
        config,
        "the setup module's entry point did not register {CONFIG_BACKEND:?} \
         against it"
    );
    assert!(
        plugin_of(COLLECTION_BACKEND).is_null(),
        "the setup module's entry point registered the collection backend: the \
         two crates' `e_module_load` definitions have collapsed into one symbol"
    );

    let collection = use_module::<CollectionModule>();

    assert_eq!(
        plugin_of(COLLECTION_BACKEND),
        collection,
        "the collection module's entry point did not register \
         {COLLECTION_BACKEND:?} against it"
    );
    assert_eq!(
        plugin_of(COLLECTION_FACTORY),
        collection,
        "the collection module's entry point did not register \
         {COLLECTION_FACTORY:?} against it"
    );
    assert_eq!(
        plugin_of(CONFIG_BACKEND),
        config,
        "the collection module's entry point moved the setup backend type"
    );
}
