// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the module entry point and the `ECollectionBackendFactory` that it
//! registers. These connect `evolution-source-registry` to the backend.
//!
//! The registry server is an `EDataFactory`. Its `backend_factory_type` is
//! `E_TYPE_COLLECTION_BACKEND_FACTORY` and its `module_directory` is the
//! libebackend `registry-modules`. For each shared object there, it loads an
//! `EModule`, which dlopens the file and calls `e_module_load`. Then it collects
//! the new extensions of itself. An account uses the factory whose name matches
//! the `BackendName` in its `[Collection]` group.
//!
//! The tests use that path. A `GTypeModule` subclass, whose `load` calls the
//! entry point, replaces the `EModule` that dlopens the `.so`. The
//! `jmap-backend-book` `tests/factory.rs` does the same. Each field of this
//! factory has a working default (see `EDS_DEFAULTS`). Thus a factory that
//! installs nothing gives an account of a different backend, or a plain
//! `ECollectionBackend` that does nothing.

use std::ffi::CStr;
use std::ptr;
use std::sync::OnceLock;

use eds_sys::{
    ECollectionBackendFactoryClass, e_backend_factory_get_hash_key,
    e_collection_backend_factory_get_type, e_collection_backend_get_type,
    e_source_registry_server_get_type,
};
use glib_sys::{GFALSE, GTRUE, GType, gboolean};
use gobject_sys::{
    GTypeModule, GTypeModuleClass, g_object_new, g_object_unref, g_type_class_peek,
    g_type_class_ref, g_type_class_unref, g_type_create_instance, g_type_from_name,
    g_type_get_plugin, g_type_is_a, g_type_module_get_type, g_type_module_unuse, g_type_module_use,
};
use jmap_backend_collection::backend::JmapCollectionBackend;
use jmap_backend_collection::factory::JmapCollectionFactory;
use jmap_backend_collection::module::{load, unload};
use jmap_backend_core::subclass::{ObjectSubclass, register_static};

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The values that `e_collection_backend_factory_class_init` (EDS 3.52) puts in
/// the two fields that this subclass sets.
///
/// `factory_name` defaults to `"none"` and `backend_type` to
/// `E_TYPE_COLLECTION_BACKEND`. Neither default causes an error. With the first,
/// the registry files the factory under `none:Collection` and does not find it
/// for a JMAP account. With the second, the factory builds a plain
/// `ECollectionBackend`. That type passes the `new_backend` check
/// `g_type_is_a (backend_type, E_TYPE_COLLECTION_BACKEND)`, but has none of the
/// vfuncs of this crate. The account connects, but fans out to nothing, and
/// shows no error.
const EDS_DEFAULTS: (&CStr, &str) = (c"none", "ECollectionBackend");

/// A `GTypeModule` that replaces the `EModule` that the registry loads.
///
/// The `EModule` `load` dlopens a shared object, finds `e_module_load` in it and
/// calls it. This module calls the entry point in the same way, without a file.
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
    const NAME: &'static CStr = c"JmapCollectionFactoryTestModule";
    type Instance = TestModule;
    type Class = TestModuleClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { g_type_module_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points to a new class struct of this type. It starts
        // with GTypeModuleClass, which holds both slots.
        let vfuncs = unsafe { &mut (*class).parent_class };
        vfuncs.load = Some(module_load);
        vfuncs.unload = Some(module_unload);
    }
}

unsafe extern "C" fn module_load(module: *mut GTypeModule) -> gboolean {
    // SAFETY: GLib passes the module that it loads, which is the argument
    // that the entry point expects.
    unsafe { load(module) };
    GTRUE
}

unsafe extern "C" fn module_unload(module: *mut GTypeModule) {
    // SAFETY: as `module_load`.
    unsafe { unload(module) };
}

/// The only module in the process, used, unused, and used again.
///
/// Two `GTypeModule`s cannot register the same type name: the second causes a
/// GLib warning and a zero `GType`. The registry also unuses and uses a module
/// again. When the last user goes, `g_type_module_unuse` marks each type of the
/// module as unloaded. GLib does not return the module until a second
/// `e_module_load` registers the types again.
struct Loaded {
    module: *mut GTypeModule,
    first_use: gboolean,
    use_after_unload: gboolean,
}

// SAFETY: the OnceLock publishes the fields, which are only read after that.
// The module that they describe is never finalized.
unsafe impl Send for Loaded {}
unsafe impl Sync for Loaded {}

fn loaded() -> &'static Loaded {
    static LOADED: OnceLock<Loaded> = OnceLock::new();
    LOADED.get_or_init(|| {
        let gtype = register_static::<TestModule>();
        assert_ne!(gtype, 0, "the stand-in module type did not register");

        // SAFETY: the type is registered and GTypeModule has no construct
        // properties of its own.
        let module = unsafe { g_object_new(gtype, ptr::null()) }.cast::<GTypeModule>();
        assert!(!module.is_null(), "g_object_new returned NULL");

        // SAFETY: `module` is a GTypeModule. The second use keeps its
        // reference, so the types stay usable for all the tests.
        unsafe {
            let first_use = g_type_module_use(module);
            g_type_module_unuse(module);
            let use_after_unload = g_type_module_use(module);
            Loaded {
                module,
                first_use,
                use_after_unload,
            }
        }
    })
}

fn backend_type() -> GType {
    loaded();
    // SAFETY: NAME is a 'static NUL-terminated string.
    unsafe { g_type_from_name(<JmapCollectionBackend as ObjectSubclass>::NAME.as_ptr()) }
}

fn factory_type() -> GType {
    loaded();
    // SAFETY: as `backend_type`.
    unsafe { g_type_from_name(<JmapCollectionFactory as ObjectSubclass>::NAME.as_ptr()) }
}

/// The factory class, referenced while the test runs.
struct FactoryClass(*mut ECollectionBackendFactoryClass);

impl FactoryClass {
    fn get() -> Self {
        let gtype = factory_type();
        assert_ne!(gtype, 0, "the factory type is not registered");
        // SAFETY: the type is registered, so a reference to its class runs
        // class_init. The class starts with ECollectionBackendFactoryClass.
        Self(unsafe { g_type_class_ref(gtype) }.cast())
    }

    fn get_ref(&self) -> &ECollectionBackendFactoryClass {
        // SAFETY: the class is referenced for as long as `self` lives.
        unsafe { &*self.0 }
    }
}

impl Drop for FactoryClass {
    fn drop(&mut self) {
        // SAFETY: the reference taken in `get` is given back exactly once.
        unsafe { g_type_class_unref(self.0.cast()) };
    }
}

/// The parent class, which holds "the default" in each assertion below.
///
/// `peek`, not `ref`: the reference to the factory class initialises the
/// parent class, and an EDS parent class lives as long as the process.
fn parent_class() -> &'static ECollectionBackendFactoryClass {
    let _ = FactoryClass::get();
    // SAFETY: the type is registered. The line above initialised its class
    // and the parent class.
    let class = unsafe { g_type_class_peek(e_collection_backend_factory_get_type()) }
        .cast::<ECollectionBackendFactoryClass>();
    assert!(!class.is_null(), "the parent class is not initialised");
    // SAFETY: non-NULL, checked, and alive for the life of the process.
    unsafe { &*class }
}

#[test]
fn the_entry_point_registers_the_backend_type() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let gtype = backend_type();
        assert_ne!(gtype, 0, "e_module_load did not register the backend type");
        assert_ne!(
            // SAFETY: both are registered types.
            unsafe { g_type_is_a(gtype, e_collection_backend_get_type()) },
            0,
            "the registered backend is not an ECollectionBackend"
        );
    });
}

/// The `backend_factory_type` of the registry server is
/// `E_TYPE_COLLECTION_BACKEND_FACTORY`. The server does not collect a type from
/// a module unless the type derives from it.
#[test]
fn the_entry_point_registers_a_collection_backend_factory() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let gtype = factory_type();
        assert_ne!(gtype, 0, "e_module_load did not register the factory type");
        assert_ne!(
            // SAFETY: both are registered types.
            unsafe { g_type_is_a(gtype, e_collection_backend_factory_get_type()) },
            0,
            "evolution-source-registry collects the collection factories a module \
             provides as extensions of itself, keyed by ECollectionBackendFactory; \
             a type that is not one of them is never looked at"
        );
    });
}

/// The registry gets the factory as an extension. Thus no code in this crate
/// gives the factory to the registry.
///
/// `e_collection_backend_factory_class_init` sets
/// `EExtensionClass.extensible_type` to `E_TYPE_SOURCE_REGISTRY_SERVER`. When
/// the server is constructed, its `e_extensible_load_extensions` makes one
/// instance of each registered subclass. The factory inherits this value. A
/// factory that derives from `EBackendFactory` directly passes all the other
/// tests in this file, but nothing makes an instance of it.
#[test]
fn the_factory_is_an_extension_of_the_registry_server() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let class = FactoryClass::get();
        assert_eq!(
            class.get_ref().parent_class.parent_class.extensible_type,
            // SAFETY: no arguments, and the EDS type system initialises itself.
            unsafe { e_source_registry_server_get_type() },
            "the factory does not extend ESourceRegistryServer, so nothing would \
             ever construct one"
        );
    });
}

/// The registry finds the factory by the `BackendName` in the `[Collection]`
/// group of the account `.source` file.
///
/// The default name is `"none"` (see [`EDS_DEFAULTS`]), not an empty name. Thus
/// a factory with the default is silently the wrong factory.
#[test]
fn the_factory_answers_to_the_backend_name_an_account_source_asks_for() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let class = FactoryClass::get();
        let name = class.get_ref().factory_name;
        assert!(!name.is_null(), "the factory installed no name");
        // SAFETY: a NUL-terminated string constant, checked non-NULL above.
        let name = unsafe { CStr::from_ptr(name) };
        assert_ne!(name, EDS_DEFAULTS.0, "the factory kept EDS's default name");
        assert_eq!(name, c"jmap");
    });
}

/// The key that the registry files the factory under, read through the parent
/// `get_hash_key`, not from the field.
///
/// `collection_backend_factory_get_hash_key` builds `"<factory_name>:Collection"`.
/// `e_data_factory_ref_backend_factory` looks up this string for a `jmap`
/// account. The parent reads `factory_name` at the offset that it expects. The
/// previous test cannot check that offset.
///
/// The test uses `g_type_create_instance`, not `g_object_new`.
/// `EExtension:extensible` is `G_PARAM_CONSTRUCT_ONLY`, and GObject sets each
/// construct property during construction. Thus `g_object_new` with no
/// properties gives `extension_set_extensible` a NULL. Its `E_IS_EXTENSIBLE`
/// assertion then logs a critical. The critical is harmless (the field stays
/// NULL), but `G_DEBUG=fatal-criticals` makes it abort. The direct call skips
/// property defaults and `constructed`, as `g_object_new_internal` does before
/// it sets properties. `EExtension` and `EBackendFactory` do not override
/// `constructed`. `g_object_unref` runs the usual dispose and finalize chain,
/// which ends in the `g_type_free_instance` that matches this allocation.
#[test]
fn the_registry_would_file_the_factory_under_the_account_name() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let gtype = factory_type();
        assert_ne!(gtype, 0, "the factory type is not registered");
        // SAFETY: a registered type that can have instances. The GObject
        // instance_init gives the result a reference count of one.
        let factory = unsafe { g_type_create_instance(gtype) };
        assert!(!factory.is_null(), "g_type_create_instance returned NULL");

        // SAFETY: a live factory of a type derived from EBackendFactory. The key
        // is `(transfer none)` and interned for the life of the process.
        let key = unsafe { e_backend_factory_get_hash_key(factory.cast()) };
        assert!(!key.is_null(), "the factory produced no hash key");
        // SAFETY: an interned NUL-terminated string, checked non-NULL above.
        assert_eq!(unsafe { CStr::from_ptr(key) }, c"jmap:Collection");

        // SAFETY: releases the reference from instance creation once. The
        // finalize that follows frees the instance.
        unsafe { g_object_unref(factory.cast()) };
    });
}

/// The type that the factory gives to `g_object_new`. This type decides if the
/// code of this crate runs.
///
/// The EDS default is `E_TYPE_COLLECTION_BACKEND`, which works. The
/// `new_backend` `g_type_is_a` check passes and the account shows in the
/// sidebar. But it has none of the vfuncs of this crate: no fan-out, no
/// credential request, no error. Thus the test asserts the JMAP type, not only
/// a collection backend.
#[test]
fn the_factory_builds_the_jmap_collection_backend() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let class = FactoryClass::get();
        let built = class.get_ref().backend_type;
        assert_eq!(
            built,
            backend_type(),
            "the factory would instantiate some other type than the backend the \
             same module registered"
        );
        // SAFETY: a registered type.
        let built_name = unsafe { CStr::from_ptr(gobject_sys::g_type_name(built)) };
        assert_ne!(
            built_name.to_str(),
            Ok(EDS_DEFAULTS.1),
            "the factory kept EDS's default backend type"
        );
    });
}

/// The two fields above are in the parent part of the class struct. This crate
/// does not control that layout. `factory_name` and `backend_type` are between
/// `EBackendFactoryClass` and `prepare_mail`. A wrong offset compiles, but it
/// replaces an adjacent slot. EDS then calls a bad pointer.
///
/// Before the fields, `get_hash_key` and `new_backend` must keep the values
/// that the parent installed. After them, the crate overrides `prepare_mail`
/// (see `tests/prepare_mail.rs`). Thus `prepare_mail` must change, and
/// `reserved` must not. A write one slot too far keeps the parent
/// `prepare_mail` and corrupts the padding that EDS keeps for future vfuncs.
#[test]
fn writing_the_fields_left_the_parent_vfuncs_alone() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let class = FactoryClass::get();
        let ours = class.get_ref();
        let parent = parent_class();

        assert_eq!(
            ours.parent_class.get_hash_key.map(|f| f as usize),
            parent.parent_class.get_hash_key.map(|f| f as usize),
            "get_hash_key was overwritten"
        );
        assert_eq!(
            ours.parent_class.new_backend.map(|f| f as usize),
            parent.parent_class.new_backend.map(|f| f as usize),
            "new_backend was overwritten"
        );
        assert!(
            parent.prepare_mail.is_some(),
            "EDS installs a default prepare_mail; if it stopped doing so, the \
             comparison below stopped saying anything"
        );
        assert_ne!(
            ours.prepare_mail.map(|f| f as usize),
            parent.prepare_mail.map(|f| f as usize),
            "prepare_mail is still the parent's, so the mail sources of a JMAP \
             account would name no provider"
        );
        assert!(
            ours.reserved.iter().all(|slot| slot.is_null()),
            "something was written past prepare_mail, into the slots EDS keeps for \
             vfuncs it has not added yet"
        );
        assert!(
            parent.reserved.iter().all(|slot| slot.is_null()),
            "EDS itself put something in `reserved`, so the check above no longer \
             distinguishes our write from its own"
        );
    });
}

/// The types are registered with the module, not statically. A static type
/// keeps its class (with pointers into this shared object) after the registry
/// unloads the module.
#[test]
fn the_types_belong_to_the_module_that_registered_them() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let loaded = loaded();
        for gtype in [backend_type(), factory_type()] {
            assert_eq!(
                // SAFETY: a registered type. The plugin of a dynamic type is the
                // GTypeModule that registered it.
                unsafe { g_type_get_plugin(gtype) }.cast::<GTypeModule>(),
                loaded.module,
                "{:?} was not registered against the module",
                // SAFETY: a registered type.
                unsafe { CStr::from_ptr(gobject_sys::g_type_name(gtype)) }
            );
        }
    });
}

/// The registry unuses a module when its last backend goes, and uses it again
/// for the next account. The second use calls `e_module_load` again. If the
/// entry point does nothing for registered types, they stay unloaded. GLib then
/// refuses the module, and no JMAP account can be constructed.
#[test]
fn a_module_that_is_unloaded_and_loaded_again_hands_its_types_back() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let loaded = loaded();
        assert_ne!(loaded.first_use, GFALSE, "the module would not load at all");
        assert_ne!(
            loaded.use_after_unload, GFALSE,
            "the second e_module_load did not re-register the module's types"
        );
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_factory_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}
