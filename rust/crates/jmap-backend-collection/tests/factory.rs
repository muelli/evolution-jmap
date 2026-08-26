// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The module entry point and the `ECollectionBackendFactory` it registers —
//! the last two pieces between `evolution-source-registry` and the backend.
//!
//! The registry server never instantiates a collection backend itself, and
//! never looks at this crate directly either. It is an `EDataFactory` whose
//! `backend_factory_type` is `E_TYPE_COLLECTION_BACKEND_FACTORY` and whose
//! `module_directory` is libebackend's `registry-modules`; it loads an `EModule`
//! per shared object it finds there — which dlopens the file and calls
//! `e_module_load` — and then collects the extensions of itself that appeared.
//! An account resolves to whichever of those factories answers to the
//! `BackendName` in its source's `[Collection]` group.
//!
//! So the test drives that path rather than the functions underneath it: a
//! `GTypeModule` subclass whose `load` calls our entry point, standing in for
//! the `EModule` that would dlopen the built `.so`, exactly as
//! `jmap-backend-book`'s `tests/factory.rs` does. What is different here is the
//! reason each assertion exists, because *every* field this factory installs has
//! a working default underneath it (see `EDS_DEFAULTS`): a collection factory
//! that registers and installs nothing is not a broken account, it is an account
//! that belongs to a different backend, or one whose backend is EDS's own
//! do-nothing `ECollectionBackend`.

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

/// What `e_collection_backend_factory_class_init` leaves in the two fields this
/// subclass exists to fill in — read off EDS 3.52's own source, because they are
/// the reason the two tests below are worth writing.
///
/// `factory_name` defaults to `"none"` and `backend_type` to
/// `E_TYPE_COLLECTION_BACKEND`. Neither default fails: a factory that keeps the
/// first is one the registry files under `none:Collection` and never finds for a
/// JMAP account, and a factory that keeps the second builds a *plain*
/// `ECollectionBackend` — which passes `new_backend`'s own
/// `g_type_is_a (backend_type, E_TYPE_COLLECTION_BACKEND)` check, has none of
/// this crate's vfuncs, and so gives the user an account that connects, fans out
/// to nothing, and reports no error anywhere.
const EDS_DEFAULTS: (&CStr, &str) = (c"none", "ECollectionBackend");

/// A `GTypeModule` standing in for the `EModule` the registry would load us as.
///
/// `EModule`'s own `load` dlopens a shared object, resolves `e_module_load` in
/// it and calls it; this does the same without a file to open, which is what
/// lets the test reach the entry point the way the registry will.
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
        // SAFETY: `class` points at a freshly allocated class struct of ours,
        // which leads with GTypeModuleClass, where both slots live.
        let vfuncs = unsafe { &mut (*class).parent_class };
        vfuncs.load = Some(module_load);
        vfuncs.unload = Some(module_unload);
    }
}

unsafe extern "C" fn module_load(module: *mut GTypeModule) -> gboolean {
    // SAFETY: GLib passes the module being loaded, which is what the entry
    // point wants.
    unsafe { load(module) };
    GTRUE
}

unsafe extern "C" fn module_unload(module: *mut GTypeModule) {
    // SAFETY: as `module_load`.
    unsafe { unload(module) };
}

/// The one module in the process, used, unused, and used again.
///
/// One, because two `GTypeModule`s cannot register the same type name: the
/// second is a GLib warning and a zero `GType`. Used twice around an unuse
/// because that is the interesting case and it happens in the field —
/// `g_type_module_unuse` marks every type the module registered as unloaded
/// when the last user of it goes away, and GLib will not hand the module back
/// until a second `e_module_load` has registered them again.
struct Loaded {
    module: *mut GTypeModule,
    first_use: gboolean,
    use_after_unload: gboolean,
}

// SAFETY: the fields are published by the OnceLock and only read afterwards,
// and the module they describe is deliberately never finalized.
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

        // SAFETY: `module` is a GTypeModule, and the reference taken by the
        // second use is never given back — the types stay usable for the rest
        // of the process, which is what every test below relies on.
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

/// The factory's class, kept referenced for the test's duration.
struct FactoryClass(*mut ECollectionBackendFactoryClass);

impl FactoryClass {
    fn get() -> Self {
        let gtype = factory_type();
        assert_ne!(gtype, 0, "the factory type is not registered");
        // SAFETY: the type is registered, so referencing its class runs
        // class_init; the class leads with ECollectionBackendFactoryClass.
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

/// The parent's class, which is what "the default" means in every assertion
/// below.
///
/// `peek` rather than `ref`: referencing our own class has already initialised
/// the parent's, so it is there, and the parent class of an EDS type lives as
/// long as the process.
fn parent_class() -> &'static ECollectionBackendFactoryClass {
    let _ = FactoryClass::get();
    // SAFETY: the type is registered and its class initialised by the line
    // above, which also initialised its parent's.
    let class = unsafe { g_type_class_peek(e_collection_backend_factory_get_type()) }
        .cast::<ECollectionBackendFactoryClass>();
    assert!(!class.is_null(), "the parent class is not initialised");
    // SAFETY: non-NULL, checked, and alive for the life of the process.
    unsafe { &*class }
}

#[test]
fn the_entry_point_registers_the_backend_type() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

/// The registry server is an `EDataFactory` whose `backend_factory_type` is
/// `E_TYPE_COLLECTION_BACKEND_FACTORY`; a type that is not one of its children
/// is never collected out of a loaded module, however correct the rest of it is.
#[test]
fn the_entry_point_registers_a_collection_backend_factory() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

/// And it is reached *as an extension* — which is the whole registration story,
/// and why no call anywhere in this crate hands the factory to the registry.
///
/// `e_collection_backend_factory_class_init` sets
/// `EExtensionClass.extensible_type` to `E_TYPE_SOURCE_REGISTRY_SERVER`, so the
/// server's own `e_extensible_load_extensions` instantiates one of every
/// registered subclass when it is constructed. Inherited rather than written
/// here — this asserts the inheritance holds, because a factory that lost it
/// (by, say, deriving from `EBackendFactory` directly) would register cleanly,
/// pass every other test in this file, and never be instantiated.
#[test]
fn the_factory_is_an_extension_of_the_registry_server() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

/// The name is the contract with the account's `.source` file: the registry
/// looks a collection up by the `BackendName` in its `[Collection]` group.
///
/// The default is not "no name" but `"none"` — see [`EDS_DEFAULTS`] — so this is
/// a factory that is silently the wrong one rather than one that is obviously
/// unfinished.
#[test]
fn the_factory_answers_to_the_backend_name_an_account_source_asks_for() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let class = FactoryClass::get();
        let name = class.get_ref().factory_name;
        assert!(!name.is_null(), "the factory installed no name");
        // SAFETY: a NUL-terminated string constant, checked non-NULL above.
        let name = unsafe { CStr::from_ptr(name) };
        assert_ne!(name, EDS_DEFAULTS.0, "the factory kept EDS's default name");
        assert_eq!(name, c"jmap");
    });
}

/// What the registry actually files the factory under, through the parent's own
/// `get_hash_key` rather than through our reading of the field it comes from.
///
/// `collection_backend_factory_get_hash_key` builds `"<factory_name>:Collection"`
/// — so this is the string `e_data_factory_ref_backend_factory` is asked for
/// when an account named `jmap` is added, and it is read out of the offset the
/// *parent* believes `factory_name` is at, which is the half of the previous
/// test that reading our own struct cannot check.
///
/// `g_type_create_instance` and not `g_object_new`, which is the one awkward
/// part. `EExtension:extensible` is `G_PARAM_CONSTRUCT_ONLY`, and GObject sets
/// *every* construct property during construction — supplied or not — so a
/// `g_object_new` with no properties hands `extension_set_extensible` a NULL and
/// earns a critical from its `E_IS_EXTENSIBLE` assertion. Harmless (the
/// assertion returns early and the field stays NULL, which is where it would
/// have been anyway) but not something to leave in a green test run, where it
/// would sit next to real criticals and under a `G_DEBUG=fatal-criticals` would
/// abort. Creating the instance directly skips property defaults and
/// `constructed` — which is what GObject's own `g_object_new_internal` does
/// before it sets any — and neither `EExtension` nor `EBackendFactory` overrides
/// `constructed` or needs anything from it. `g_object_unref` still runs the
/// normal dispose/finalize chain, whose end is the `g_type_free_instance` this
/// allocation pairs with.
#[test]
fn the_registry_would_file_the_factory_under_the_account_name() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let gtype = factory_type();
        assert_ne!(gtype, 0, "the factory type is not registered");
        // SAFETY: a registered, instantiatable type; GObject's own instance_init
        // gives the result a reference count of one.
        let factory = unsafe { g_type_create_instance(gtype) };
        assert!(!factory.is_null(), "g_type_create_instance returned NULL");

        // SAFETY: a live factory of a type derived from EBackendFactory; the key
        // comes back `(transfer none)`, interned for the life of the process.
        let key = unsafe { e_backend_factory_get_hash_key(factory.cast()) };
        assert!(!key.is_null(), "the factory produced no hash key");
        // SAFETY: an interned NUL-terminated string, checked non-NULL above.
        assert_eq!(unsafe { CStr::from_ptr(key) }, c"jmap:Collection");

        // SAFETY: the reference instance creation left behind, given back once; the
        // finalize it reaches is what frees the instance.
        unsafe { g_object_unref(factory.cast()) };
    });
}

/// The type the factory would `g_object_new`, which is the one thing between a
/// found factory and this crate's code running.
///
/// EDS's default is `E_TYPE_COLLECTION_BACKEND` itself, and it is a *working*
/// type: `new_backend`'s `g_type_is_a` check passes, an account appears in the
/// sidebar, and it has none of this crate's three vfuncs — no fan-out, no
/// credentials asked for, no error. So "is a collection backend" is not enough
/// to assert; it has to be *ours*.
#[test]
fn the_factory_builds_the_jmap_collection_backend() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

/// The two fields above are written into the *parent's* half of the class
/// struct, whose layout this crate does not own — `factory_name` and
/// `backend_type` sit between `EBackendFactoryClass` and `prepare_mail`. A wrong
/// offset there does not fail to compile; it overwrites a neighbouring slot,
/// which is a call through a bad pointer the first time EDS uses it.
///
/// So: the two neighbours EDS fills in and this crate does not touch —
/// `get_hash_key` and `new_backend`, on the near side — must still be exactly
/// what the parent class installed. On the far side sits `prepare_mail`, which
/// *is* overridden (see `tests/prepare_mail.rs`), so what is asserted there is
/// that it changed and that `reserved` past it did not: a write that landed one
/// slot too far would leave the parent's `prepare_mail` in place and scribble
/// into the padding EDS keeps for future vfuncs.
#[test]
fn writing_the_fields_left_the_parent_vfuncs_alone() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

/// Registered against the module, not statically: a statically registered type
/// keeps its class — and so pointers into this shared object — alive after the
/// registry has unloaded the module underneath it.
#[test]
fn the_types_belong_to_the_module_that_registered_them() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let loaded = loaded();
        for gtype in [backend_type(), factory_type()] {
            assert_eq!(
                // SAFETY: a registered type; the plugin of a dynamic type is the
                // GTypeModule it was registered against.
                unsafe { g_type_get_plugin(gtype) }.cast::<GTypeModule>(),
                loaded.module,
                "{:?} was not registered against the module",
                // SAFETY: a registered type.
                unsafe { CStr::from_ptr(gobject_sys::g_type_name(gtype)) }
            );
        }
    });
}

/// The registry unuses a module when the last backend it provided goes away,
/// and uses it again when the next account wants one. The second use calls
/// `e_module_load` a second time, and an entry point that treats "already
/// registered" as "nothing to do" leaves every type marked unloaded — GLib then
/// refuses the module and no JMAP account can be constructed again.
#[test]
fn a_module_that_is_unloaded_and_loaded_again_hands_its_types_back() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
