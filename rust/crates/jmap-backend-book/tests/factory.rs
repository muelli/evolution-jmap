// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for the module entry point and `EBookBackendFactory` registration.

use std::ffi::CStr;
use std::ptr;
use std::sync::OnceLock;

use eds_sys::{
    EBookBackendFactoryClass, e_book_backend_factory_get_type, e_book_meta_backend_get_type,
};
use glib_sys::{GFALSE, GType, gboolean};
use gobject_sys::{
    GTypeModule, g_object_new, g_type_class_ref, g_type_class_unref, g_type_from_name,
    g_type_get_plugin, g_type_is_a, g_type_module_unuse, g_type_module_use,
};
use jmap_backend_book::backend::JmapBookBackend;
use jmap_backend_book::factory::JmapBookFactory;
use jmap_backend_book::module::{load, unload};
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_backend_core::test_module;

// A `GTypeModule` standing in for the `EModule` loaded by EDS.
//
// Simulates `EModule` loading without requiring a file.
test_module!(
    TestModule,
    c"JmapBookFactoryTestModule",
    load = load,
    unload = unload
);

/// The single test module in the process, exercised across load, unload, and reload.
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
        // second use is kept so types remain usable for subsequent tests.
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
    unsafe { g_type_from_name(<JmapBookBackend as ObjectSubclass>::NAME.as_ptr()) }
}

fn factory_type() -> GType {
    loaded();
    // SAFETY: as `backend_type`.
    unsafe { g_type_from_name(<JmapBookFactory as ObjectSubclass>::NAME.as_ptr()) }
}

/// The factory's class, kept referenced for the test's duration.
struct FactoryClass(*mut EBookBackendFactoryClass);

impl FactoryClass {
    fn get() -> Self {
        let gtype = factory_type();
        assert_ne!(gtype, 0, "the factory type is not registered");
        // SAFETY: the type is registered, so referencing its class runs
        // class_init; the class leads with EBookBackendFactoryClass.
        Self(unsafe { g_type_class_ref(gtype) }.cast())
    }

    fn get_ref(&self) -> &EBookBackendFactoryClass {
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

#[test]
fn the_entry_point_registers_the_backend_type() {
    let gtype = backend_type();
    assert_ne!(gtype, 0, "e_module_load did not register the backend type");
    assert_ne!(
        // SAFETY: both are registered types.
        unsafe { g_type_is_a(gtype, e_book_meta_backend_get_type()) },
        0,
        "the registered backend is not an EBookMetaBackend"
    );
}

#[test]
fn the_entry_point_registers_a_book_backend_factory() {
    let gtype = factory_type();
    assert_ne!(gtype, 0, "e_module_load did not register the factory type");
    assert_ne!(
        // SAFETY: both are registered types.
        unsafe { g_type_is_a(gtype, e_book_backend_factory_get_type()) },
        0,
        "EDS finds the factories a module provides by walking the children of \
         EBookBackendFactory; a type that is not one of them is never looked at"
    );
}

/// Verifies the factory matches the `BackendName` expected by address book sources.
#[test]
fn the_factory_answers_to_the_backend_name_a_source_file_asks_for() {
    let class = FactoryClass::get();
    let name = class.get_ref().factory_name;
    assert!(!name.is_null(), "the factory installed no name");
    // SAFETY: a NUL-terminated string constant, checked non-NULL above.
    assert_eq!(unsafe { CStr::from_ptr(name) }, c"jmap");
}

#[test]
fn the_factory_builds_the_jmap_backend() {
    let class = FactoryClass::get();
    let built = class.get_ref().backend_type;
    assert_eq!(
        built,
        backend_type(),
        "the factory would instantiate some other type than the backend the \
         same module registered"
    );
    assert_ne!(
        // SAFETY: both are registered types.
        unsafe { g_type_is_a(built, e_book_meta_backend_get_type()) },
        0
    );
}

/// Types are registered dynamically against the module rather than statically.
#[test]
fn the_types_belong_to_the_module_that_registered_them() {
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
}

/// Unloading and reloading the module successfully re-registers its types.
#[test]
fn a_module_that_is_unloaded_and_loaded_again_hands_its_types_back() {
    let loaded = loaded();
    assert_ne!(loaded.first_use, GFALSE, "the module would not load at all");
    assert_ne!(
        loaded.use_after_unload, GFALSE,
        "the second e_module_load did not re-register the module's types"
    );
}
