// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The module entry point. It is the last part between Evolution and the
// `EMailConfigServiceBackend` subclass, and the first part of M7 that is a
// shared object, not a library.
//
// At startup, the Evolution shell calls `e_module_load_all_in_directory` on its
// module directory. It puts each `.so` in an `EModule` and calls
// `g_type_module_use` on it. The `load` of `EModule` dlopens the file, resolves
// `e_module_load`, and calls it with the `EModule` as the `GTypeModule`. The
// types registered against that module when the call returns are the
// contribution of the module.
//
// After that, Evolution never finds this class by name. `EMailConfigServicePage`
// is an `EExtensible`. Its `constructed` calls `e_extensible_load_extensions`,
// which examines the *children of `EExtension`* that exist at that time. It
// makes an instance of each child whose class `extensible_type` is the type of
// the page. Thus the module has two tasks: put the type in the type system, and
// register it against the module that GLib unloads. The tests below examine
// these two tasks. They also make sure that the class still has the name and
// the vfuncs that `tests/backend.rs` pins.
//
// As in `tests/factory.rs` of `jmap-backend-cal`, a `GTypeModule` subclass
// replaces the `EModule`. Its `load` calls the entry point directly. To load
// the real `.so`, it must be built first: the CMake test `install-config-module`
// does that, one layer up. This test examines what the entry point does when
// it is called.

use std::ptr;
use std::sync::OnceLock;

use eds_sys::EExtensionClass;
use evo_sys::{
    EMailConfigServiceBackendClass, e_config_lookup_get_type, e_config_lookup_worker_get_type,
    e_mail_config_service_backend_get_type, e_source_config_backend_get_type,
};
use glib_sys::{GFALSE, GType, gboolean};
use gobject_sys::{
    GTypeModule, g_object_new, g_type_class_ref, g_type_class_unref, g_type_from_name,
    g_type_get_plugin, g_type_is_a, g_type_module_unuse, g_type_module_use, g_type_name,
};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_backend_core::test_module;
use jmap_config::backend::JmapConfigServiceBackend;
use jmap_config::config_lookup::JmapConfigLookup;
use jmap_config::mail::MAIL_BACKEND_NAME;
use jmap_config::module::{load, unload};
use jmap_config::oauth2_service;
use jmap_config::source_config::{JmapBookConfig, JmapCalConfig};

// Initialise the parent classes of the Evolution types before the tests start threads.
jmap_backend_core::initialise_parents_before_main!(
    jmap_config::backend::JmapConfigServiceBackend,
    jmap_config::config_lookup::JmapConfigLookup,
    jmap_config::source_config::JmapBookConfig,
    jmap_config::source_config::JmapCalConfig,
);

// A `GTypeModule` that replaces the `EModule` that Evolution loads this crate as.
test_module!(
    TestModule,
    c"JmapConfigTestModule",
    load = load,
    unload = unload
);

/// The one module in the process: used, unused, and used again.
///
/// One, because two `GTypeModule`s cannot register the same type name. Used
/// two times around an unuse, because that occurs in real use (refer to
/// [`a_module_that_is_unloaded_and_loaded_again_hands_its_types_back`]).
struct Loaded {
    module: *mut GTypeModule,
    first_use: gboolean,
    use_after_unload: gboolean,
}

// SAFETY: the OnceLock publishes the fields, and code only reads them after
// that. The module that they describe is deliberately never finalized.
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

        // SAFETY: `module` is a GTypeModule. The reference from the second use
        // is never released, so the types stay usable until the process stops.
        // All tests below need this.
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

/// The type of the setup backend, as the module registered it.
fn backend_type() -> GType {
    loaded();
    // SAFETY: NAME is a 'static NUL-terminated string.
    unsafe { g_type_from_name(<JmapConfigServiceBackend as ObjectSubclass>::NAME.as_ptr()) }
}

/// The registered class. The test keeps a reference to it.
struct Class(*mut EMailConfigServiceBackendClass);

impl Class {
    fn get() -> Self {
        let gtype = backend_type();
        assert_ne!(gtype, 0, "the setup backend type is not registered");
        // SAFETY: the type is registered, so a reference to its class runs
        // class_init. The class starts with EMailConfigServiceBackendClass.
        Self(unsafe { g_type_class_ref(gtype) }.cast())
    }

    fn vfuncs(&self) -> &EMailConfigServiceBackendClass {
        // SAFETY: the class stays referenced while `self` is alive.
        unsafe { &*self.0 }
    }
}

impl Drop for Class {
    fn drop(&mut self) {
        // SAFETY: this releases the reference from `get` exactly one time.
        unsafe { g_type_class_unref(self.0.cast()) };
    }
}

/// The credentials prompter runs in the Evolution SHELL process. It sends an
/// `[Authentication] Method=JMAP` source to the OAuth 2.0 sign-in window only
/// if an `EOAuth2Service` with that name is registered in the same process. The
/// collection module registers the service in the process of the source
/// registry. This module must do the same in the shell process. If not, the
/// prompter shows the password dialog. Observed live (2026-08-23, Fastmail): a
/// newly discovered OAuth account asked for a password that nothing could satisfy.
#[cfg(not(eds_oauth2_dynamic))]
#[test]
fn the_entry_point_registers_the_oauth2_service_type() {
    loaded();
    assert_ne!(
        // SAFETY: NAME is a 'static NUL-terminated string.
        unsafe { g_type_from_name(<oauth2_service::Service as ObjectSubclass>::NAME.as_ptr()) },
        0,
        "e_module_load did not register the OAuth2 service type, so the shell's \
         credentials prompter can only offer the password dialog"
    );
}

/// The opposite of the test above. Where EDS has its own
/// `EOAuth2ServiceDynamic`, the prompter already has a service for the method
/// of these accounts. A second service for the same account makes
/// `e_oauth2_services_find` select by list sequence.
#[cfg(eds_oauth2_dynamic)]
#[test]
fn the_entry_point_leaves_the_oauth2_service_to_eds() {
    loaded();
    assert_eq!(
        // SAFETY: NAME is a 'static NUL-terminated string.
        unsafe { g_type_from_name(<oauth2_service::Service as ObjectSubclass>::NAME.as_ptr()) },
        0,
        "e_module_load registered this project's own EOAuth2Service on an EDS \
         that brings its own, so two services now claim the same account"
    );
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

/// The page does not find this class by name. It makes instances of the
/// children of `EExtension` whose `extensible_type` is the type of the page.
/// That field is in the parent class. Evolution's own `class_init` sets it one
/// time, and the subclass inherits it. If a subclass changed it, no code makes
/// an instance of the type, and no error tells this.
#[test]
fn the_registered_type_is_an_extension_of_the_page_that_will_load_it() {
    let class = Class::get();
    // SAFETY: EMailConfigServiceBackendClass starts with EExtensionClass.
    let extension = unsafe { &*ptr::from_ref(class.vfuncs()).cast::<EExtensionClass>() };
    assert_ne!(
        extension.extensible_type, 0,
        "the class names no extensible type, so no page would ever build one"
    );
    // SAFETY: a registered type. GLib owns the name, which is NUL-terminated.
    let extensible = unsafe { read_string(g_type_name(extension.extensible_type)) };
    assert_eq!(
        extensible.as_deref(),
        Some("EMailConfigServicePage"),
        "the extensible type is not the page Evolution loads mail config \
         backends from"
    );
}

/// Registered *against the module*, not statically. GLib unregisters the types
/// of a module when it unloads the module. A static type lives longer than the
/// code of its class_init and vfunc pointers. When Evolution unloads this
/// module, a call through them is a dangling call.
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

/// Evolution unuses a module when the last object that it supplied is removed.
/// It uses the module again when it needs the next object. The second use calls
/// `e_module_load` again. Assume an entry point that does nothing for "already
/// registered". Then all types stay marked unloaded. GLib then refuses the
/// module, and the account type never shows again.
#[test]
fn a_module_that_is_unloaded_and_loaded_again_hands_its_types_back() {
    let loaded = loaded();
    assert_ne!(loaded.first_use, GFALSE, "the module would not load at all");
    assert_ne!(
        loaded.use_after_unload, GFALSE,
        "the second e_module_load did not re-register the module's types"
    );
}

/// The registered type is useful only if its class is the class that
/// `tests/backend.rs` pins. The page reads `backend_name` to find the Camel
/// provider that this backend is *for*, and dispatches the three vfuncs.
/// Registration through a `GTypeModule` runs the same `class_init`. This test
/// makes sure of it, because the dynamic path initialises the class later and
/// initialises it again after each reload.
#[test]
fn the_class_the_module_registers_carries_the_name_and_the_vfuncs() {
    let class = Class::get();
    let vfuncs = class.vfuncs();

    // SAFETY: the 'static NUL-terminated string from class initialisation.
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

/// The type of `JmapConfigLookup`, as the module registered it.
fn lookup_type() -> GType {
    loaded();
    // SAFETY: NAME is a 'static NUL-terminated string.
    unsafe { g_type_from_name(<JmapConfigLookup as ObjectSubclass>::NAME.as_ptr()) }
}

/// The other type that the module registers. Unlike for the backend, no code
/// here constructs an instance. `JmapConfigLookup::constructed` calls
/// `e_config_lookup_register_worker` on the return value of
/// `e_extension_get_extensible`. A bare `g_object_new` with no real
/// `EConfigLookup` in its construct-only `extensible` property makes that call
/// with a NULL. The `g_return_if_fail` of `e_config_lookup_register_worker` then
/// logs a `CRITICAL`, and this project does not accept that as a pass. (The
/// `TestSource` comment in `jmap-config/tests/oauth2_service.rs` gives the same
/// reason.) Thus these tests stay at the class level, which needs no instance:
/// `g_type_class_ref` alone runs `class_init`.
#[test]
fn the_lookup_worker_type_is_registered_against_the_module() {
    let loaded = loaded();
    let gtype = lookup_type();
    assert_ne!(
        gtype, 0,
        "e_module_load did not register the lookup worker type"
    );
    assert_eq!(
        // SAFETY: a registered type.
        unsafe { g_type_get_plugin(gtype) }.cast::<GTypeModule>(),
        loaded.module,
        "the lookup worker type was not registered against the module"
    );
}

/// The two `ESourceConfigBackend`s, with the same two checks as the backend and
/// the lookup worker: they are in the type system, and they are registered
/// against the module, not statically.
///
/// The operator found that this part was missing. `ESourceConfig` builds one
/// candidate for each registered subclass of `ESourceConfigBackend`. Before
/// these two, the New Address Book and New Calendar dialogs had no JMAP entry,
/// an edit of an existing JMAP source found a NULL backend, and OK asserted and
/// did not save. `tests/source_config.rs` pins the *values* of the classes. This
/// test pins that the entry point registers them where Evolution looks.
///
/// Registered against the module, not statically, for the reason that
/// [`the_registered_type_belongs_to_the_module`] gives: a static type lives
/// longer than the code of its `class_init` and vfunc pointers. When Evolution
/// unloads this module, a call through them is a dangling call.
#[test]
fn the_source_config_backends_are_registered_against_the_module() {
    let loaded = loaded();
    for (label, name) in [
        (
            "address book",
            <JmapBookConfig as ObjectSubclass>::NAME.as_ptr(),
        ),
        ("calendar", <JmapCalConfig as ObjectSubclass>::NAME.as_ptr()),
    ] {
        // SAFETY: NAME is a 'static NUL-terminated string.
        let gtype = unsafe { g_type_from_name(name) };
        assert_ne!(
            gtype, 0,
            "e_module_load did not register the {label} config backend — its \
             dialog offers no JMAP entry without it"
        );
        assert_ne!(
            // SAFETY: both are registered types.
            unsafe { g_type_is_a(gtype, e_source_config_backend_get_type()) },
            0,
            "the registered {label} type is not an ESourceConfigBackend, so \
             ESourceConfig would not build a candidate from it"
        );
        assert_eq!(
            // SAFETY: a registered type.
            unsafe { g_type_get_plugin(gtype) }.cast::<GTypeModule>(),
            loaded.module,
            "the {label} config backend was not registered against the module"
        );
    }
}

#[test]
fn the_lookup_worker_type_implements_econfiglookupworker() {
    let gtype = lookup_type();
    assert_ne!(gtype, 0, "the lookup worker type is not registered");
    assert_ne!(
        // SAFETY: both are registered types.
        unsafe { g_type_is_a(gtype, e_config_lookup_worker_get_type()) },
        0,
        "the registered type does not implement EConfigLookupWorker"
    );
}

/// The one failure that stops the Evolution account assistant from finding this
/// worker: an `extensible_type` that is not the type of `EConfigLookup`.
/// `the_registered_type_is_an_extension_of_the_page_that_will_load_it` above
/// prevents the same failure for the backend.
#[test]
fn the_lookup_worker_extends_econfiglookup() {
    let gtype = lookup_type();
    assert_ne!(gtype, 0, "the lookup worker type is not registered");
    // SAFETY: the type is registered. JmapConfigLookupClass starts with
    // EExtensionClass, where `class_init` writes `extensible_type`.
    let class = unsafe { g_type_class_ref(gtype) }.cast::<EExtensionClass>();
    let extensible_type = unsafe { (*class).extensible_type };
    // SAFETY: this releases the reference from above exactly one time.
    unsafe { g_type_class_unref(class.cast()) };

    assert_eq!(
        extensible_type,
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_config_lookup_get_type() },
        "the lookup worker does not extend EConfigLookup, so no account \
         assistant would ever instantiate it"
    );
}
