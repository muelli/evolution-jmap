// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Two modules, two entry points, one process — and each call has to reach the
// module it was made on.
//
// Every EDS/Evolution module this repository installs exports the same two C
// symbols, `e_module_load` and `e_module_unload`, because that is the pair
// `EModule` resolves out of a shared object. In the field that is harmless:
// each `.so` is dlopened separately and the symbols never meet. In a test
// binary they do. `jmap-config` dev-depends on `jmap-backend-collection` — the
// account this crate writes is only right if the collection backend reads it
// back as the account that was written — so both rlibs land in one link, and
// two definitions of `e_module_load` land with them.
//
// A `#[unsafe(no_mangle)]` function *is* its C symbol, so while the two rlibs
// still carried those definitions there was no longer a
// `jmap_config::module::` entry point and a `jmap_backend_collection::module::`
// one to call: the two Rust paths compiled to a call to the same symbol, and
// whichever definition the linker kept answered both. Under `CARGO_INCREMENTAL=0`
// — CMake's setting for the `rust-test-eds` ctest — that is a hard `duplicate
// symbol` link error; under the incremental default it linked, and the failure
// was silent, with `jmap_config::module::e_module_load` registering the
// *collection* backend's types and nothing anywhere saying so.
//
// The rlibs no longer define them: each `.so` is now built from a `*-module`
// cdylib crate that holds the two `no_mangle` functions and nothing else, and
// the bodies below it are ordinary Rust functions with ordinary mangled names.
// This test is what keeps it that way. It calls each crate's entry point
// through a `GTypeModule` of its own and asks the type system which types
// appeared; the answer distinguishes the two definitions, which a link that has
// collapsed them cannot do — so re-adding a `no_mangle` to either rlib turns
// this red, either as a wrong answer here or as a link error under CMake.

use std::ffi::CStr;
use std::ptr;

use glib_sys::{GTRUE, GType, gboolean};
use gobject_sys::{
    GTypeModule, GTypeModuleClass, g_object_new, g_type_from_name, g_type_get_plugin,
    g_type_module_get_type, g_type_module_use,
};
use jmap_backend_core::subclass::{ObjectSubclass, register_static};

/// The names the two crates register, spelled out rather than imported.
///
/// Imported constants would be equal to whatever the types under test call
/// themselves, which is exactly the question — a test that asked the code for
/// the answer would agree with a module that registered the wrong types under
/// the wrong names.
const CONFIG_BACKEND: &CStr = c"EMailConfigServiceBackendJmap";
const COLLECTION_BACKEND: &CStr = c"ECollectionBackendJmap";
const COLLECTION_FACTORY: &CStr = c"ECollectionBackendJmapFactory";

/// A `GTypeModule` standing in for the `EModule` a host would load us as, with
/// the entry point to call left to the concrete type below.
#[repr(C)]
struct TestModule {
    parent: GTypeModule,
}

#[repr(C)]
struct TestModuleClass {
    parent_class: GTypeModuleClass,
}

/// The setup module's stand-in: its `load` calls `jmap-config`'s entry point.
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
        // SAFETY: `class` points at a freshly allocated class struct of ours,
        // which leads with GTypeModuleClass, where the slot lives.
        unsafe { (*class).0.parent_class.load = Some(load_config) }
    }
}

unsafe extern "C" fn load_config(module: *mut GTypeModule) -> gboolean {
    // SAFETY: GLib passes the module being loaded, which is what the entry
    // point wants.
    unsafe { jmap_config::module::load(module) };
    GTRUE
}

/// The collection module's stand-in: its `load` calls the *other* crate's entry
/// point, which is the one that used to be the same C symbol.
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

/// Builds a stand-in module of type `T` and uses it, which runs its `load`.
///
/// The reference the use takes is never given back: the types have to stay
/// registered for the assertions that follow.
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

/// What the type system says a type was registered against, or null.
fn plugin_of(name: &CStr) -> *mut GTypeModule {
    // SAFETY: `name` is NUL-terminated; g_type_get_plugin takes any GType,
    // including 0 for a name nothing registered.
    unsafe { g_type_get_plugin(g_type_from_name(name.as_ptr())) }.cast()
}

/// One test, not three: the steps are ordered — what the first `load` must
/// *not* have registered is only a question before the second one runs — and
/// GLib's type system is process-global, so separate `#[test]`s would race.
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
