// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The module entry point — the last piece between Evolution and the
// `EMailConfigServiceBackend` subclass, and the first part of M7 that is a
// shared object rather than a library.
//
// Evolution's shell calls `e_module_load_all_in_directory` over its own module
// directory at startup, wrapping each `.so` it finds in an `EModule` and
// `g_type_module_use`-ing it; `EModule`'s `load` dlopens the file, resolves
// `e_module_load`, and calls it with itself as the `GTypeModule`. Whatever
// types are registered against that module by the time the call returns are
// the module's contribution.
//
// From there Evolution never looks this class up by name. `EMailConfigServicePage`
// is an `EExtensible`, and its `constructed` calls `e_extensible_load_extensions`,
// which walks the *children of `EExtension`* that exist by then and instantiates
// every one whose class `extensible_type` is the page's own type. So the module
// has exactly two jobs — put the type in the type system, and put it there
// against the module GLib will unload — and the tests below are those two plus
// what they are worth only together with: the class that comes out still
// carrying the name and the vfuncs `tests/backend.rs` pins down.
//
// As in `jmap-backend-cal`'s `tests/factory.rs`, the `EModule` is stood in for
// by a `GTypeModule` subclass whose `load` calls our entry point directly.
// Loading the real built `.so` would need it built first, which is CMake's
// `install-config-module` test one layer up; this one is about what the entry
// point does once it is called.

use std::ffi::CStr;
use std::ptr;
use std::sync::OnceLock;

use eds_sys::EExtensionClass;
use evo_sys::{EMailConfigServiceBackendClass, e_mail_config_service_backend_get_type};
use glib_sys::{GFALSE, GTRUE, GType, gboolean};
use gobject_sys::{
    GTypeModule, GTypeModuleClass, g_object_new, g_type_class_ref, g_type_class_unref,
    g_type_from_name, g_type_get_plugin, g_type_is_a, g_type_module_get_type, g_type_module_unuse,
    g_type_module_use, g_type_name,
};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_config::backend::JmapConfigServiceBackend;
use jmap_config::mail::MAIL_BACKEND_NAME;
use jmap_config::module::{load, unload};

/// A `GTypeModule` standing in for the `EModule` Evolution would load us as.
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
    const NAME: &'static CStr = c"JmapConfigTestModule";
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
/// One, because two `GTypeModule`s cannot register the same type name. Used
/// twice around an unuse because that is what happens in the field — see
/// [`a_module_that_is_unloaded_and_loaded_again_hands_its_types_back`].
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

/// The setup backend's type, as the module left it in the type system.
fn backend_type() -> GType {
    loaded();
    // SAFETY: NAME is a 'static NUL-terminated string.
    unsafe { g_type_from_name(<JmapConfigServiceBackend as ObjectSubclass>::NAME.as_ptr()) }
}

/// The registered class, kept referenced for the test's duration.
struct Class(*mut EMailConfigServiceBackendClass);

impl Class {
    fn get() -> Self {
        let gtype = backend_type();
        assert_ne!(gtype, 0, "the setup backend type is not registered");
        // SAFETY: the type is registered, so referencing its class runs
        // class_init; our class leads with EMailConfigServiceBackendClass.
        Self(unsafe { g_type_class_ref(gtype) }.cast())
    }

    fn vfuncs(&self) -> &EMailConfigServiceBackendClass {
        // SAFETY: the class is referenced for as long as `self` lives.
        unsafe { &*self.0 }
    }
}

impl Drop for Class {
    fn drop(&mut self) {
        // SAFETY: the reference taken in `get` is given back exactly once.
        unsafe { g_type_class_unref(self.0.cast()) };
    }
}

#[test]
fn the_entry_point_registers_the_setup_backend_type() {
    let gtype = backend_type();
    assert_ne!(
        gtype, 0,
        "e_module_load did not register the setup backend type"
    );
    assert_ne!(
        // SAFETY: both are registered types.
        unsafe { g_type_is_a(gtype, e_mail_config_service_backend_get_type()) },
        0,
        "the registered type is not an EMailConfigServiceBackend"
    );
}

/// The page does not look this class up by name — it instantiates the children
/// of `EExtension` whose `extensible_type` is its own. That field is the
/// parent's, set once in Evolution's own `class_init` and inherited; a subclass
/// that overwrote it would register a type nothing ever instantiates, with no
/// error anywhere to say so.
#[test]
fn the_registered_type_is_an_extension_of_the_page_that_will_load_it() {
    let class = Class::get();
    // SAFETY: EMailConfigServiceBackendClass leads with EExtensionClass.
    let extension = unsafe { &*ptr::from_ref(class.vfuncs()).cast::<EExtensionClass>() };
    assert_ne!(
        extension.extensible_type, 0,
        "the class names no extensible type, so no page would ever build one"
    );
    // SAFETY: a registered type; the name is GLib's and NUL-terminated.
    let extensible = unsafe { read_string(g_type_name(extension.extensible_type)) };
    assert_eq!(
        extensible.as_deref(),
        Some("EMailConfigServicePage"),
        "the extensible type is not the page Evolution loads mail config \
         backends from"
    );
}

/// Registered *against the module*, not statically. GLib unregisters a
/// module's types when the module is unloaded; a type the module registered
/// statically would outlive the code its class_init and vfunc pointers live in,
/// which is a dangling call the moment Evolution unloads us.
#[test]
fn the_registered_type_belongs_to_the_module() {
    let loaded = loaded();
    let gtype = backend_type();
    assert_ne!(gtype, 0, "the setup backend type is not registered");
    assert_eq!(
        // SAFETY: a registered type.
        unsafe { g_type_get_plugin(gtype) }.cast::<GTypeModule>(),
        loaded.module,
        "the setup backend type was not registered against the module"
    );
}

/// Evolution unuses a module when the last thing it provided goes away, and
/// uses it again when the next one is wanted. The second use calls
/// `e_module_load` a second time, and an entry point that treated "already
/// registered" as "nothing to do" would leave every type marked unloaded — GLib
/// then refuses the module and the account type never appears again.
#[test]
fn a_module_that_is_unloaded_and_loaded_again_hands_its_types_back() {
    let loaded = loaded();
    assert_ne!(loaded.first_use, GFALSE, "the module would not load at all");
    assert_ne!(
        loaded.use_after_unload, GFALSE,
        "the second e_module_load did not re-register the module's types"
    );
}

/// The type being registered is only worth anything if the class that comes out
/// of it is the one `tests/backend.rs` pins down: the page reads `backend_name`
/// to decide which Camel provider this backend is *for*, and dispatches the
/// three vfuncs. Registration through a `GTypeModule` runs the same
/// `class_init`, and this is the assertion that says so rather than assuming it
/// — the dynamic path initialises the class later and re-initialises it after
/// every reload.
#[test]
fn the_class_the_module_registers_carries_the_name_and_the_vfuncs() {
    let class = Class::get();
    let vfuncs = class.vfuncs();

    // SAFETY: a 'static NUL-terminated string the class was initialised from.
    let name = unsafe { read_string(vfuncs.backend_name) };
    assert_eq!(
        name.as_deref(),
        MAIL_BACKEND_NAME.to_str().ok(),
        "the class the module registered names no provider, or the wrong one"
    );

    assert!(
        vfuncs.new_collection.is_some(),
        "no new_collection: a JMAP account would be committed as a bare mail \
         source"
    );
    assert!(
        vfuncs.check_complete.is_some(),
        "no check_complete: the assistant would accept an account with no \
         address and no server"
    );
    assert!(
        vfuncs.commit_changes.is_some(),
        "no commit_changes: the mail source would be written with no host"
    );
}
