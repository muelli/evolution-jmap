// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! That the module registers `jmap_config::oauth2_service::Service` — the
//! real-server-readiness half of the current roadmap priority, and last
//! session's own "next session" note.
//!
//! `evolution-source-registry` is where a JMAP account's OAuth2 token
//! actually gets refreshed, so this is where the service belongs — checked
//! against evolution-ews's own working implementation
//! (`gitlab.gnome.org/GNOME/evolution-ews`,
//! `src/EWS/registry/module-ews-backend.c`) rather than guessed, since a
//! wrong placement is invisible on this headless VM either way. That module
//! calls `e_oauth2_service_office365_type_register(type_module)` beside its
//! backend/factory registrations and nothing more — no `g_object_new`, no
//! `e_oauth2_services_add` anywhere in that codebase.
//!
//! The reason one call is enough: `EOAuth2ServiceBase` (`e-oauth2-service-base.c`,
//! read rather than assumed) is itself an `EExtension` whose
//! `extensible_type` is `E_TYPE_OAUTH2_SERVICES`, and `EOAuth2Services`'s own
//! constructor (`e-oauth2-services.c`) calls `e_extensible_load_extensions()`
//! on itself before returning. So a type only has to be *registered* by the
//! time anything in this process constructs the `EOAuth2Services` singleton
//! — the same "registration is instantiation" idiom this project's own
//! `jmap-config` module docs already describe for `EMailConfigServiceBackend`.
//!
//! So this test drives exactly that path end to end, with no manual
//! `g_object_new` of the service anywhere: register the module, then
//! construct the registry the way any real EDS process eventually will, and
//! ask it to find a service for a source that names ours.

use std::ffi::CString;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, ESource, e_oauth2_services_find, e_oauth2_services_new,
    e_source_authentication_set_method, e_source_get_extension, e_source_new_with_uid,
};
use glib_sys::{GFALSE, GTRUE, GType, gboolean};
use gobject_sys::{
    GTypeModule, GTypeModuleClass, g_object_new, g_object_unref, g_type_module_get_type,
    g_type_module_use,
};
use jmap_backend_collection::module::{load, unload};
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_config::oauth2_service::NAME;

/// A `GTypeModule` standing in for the `EModule` the registry would load us
/// as — the same stand-in `tests/factory.rs` and `tests/textdomain.rs` use.
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
    const NAME: &'static std::ffi::CStr = c"JmapCollectionOAuth2TestModule";
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

/// A throwaway `ESource` authenticating by this service's own name — what a
/// real JMAP account's `[Authentication] method` is set to when M7's setup UI
/// writes an OAuth2 account.
fn source_naming_this_service() -> *mut ESource {
    let uid = CString::new("jmap-collection-oauth2-service-test").expect("no NUL in a literal");
    let mut error = ptr::null_mut();
    // SAFETY: a NUL-terminated uid, no D-Bus object and a GError
    // out-parameter are the documented arguments.
    let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
    assert!(!source.is_null(), "e_source_new_with_uid failed");

    // SAFETY: a live source; `E_SOURCE_EXTENSION_AUTHENTICATION` is EDS's own
    // extension name and always creatable.
    unsafe {
        let auth =
            e_source_get_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
        e_source_authentication_set_method(auth, NAME.as_ptr());
    }
    source
}

/// Registering the module, then asking the `EOAuth2Services` singleton to
/// find a service for a matching source — the exact path a real
/// `evolution-source-registry` process takes, and the one that would stay
/// green even if this file mistakenly asserted only that the `GType`
/// registered, which a service nothing ever instantiates would also pass.
#[test]
fn the_registry_finds_the_jmap_service_once_the_module_has_loaded() {
    let gtype = register_static::<TestModule>();
    assert_ne!(gtype, 0, "the stand-in module type did not register");

    // SAFETY: the type is registered and GTypeModule has no construct
    // properties of its own.
    let module = unsafe { g_object_new(gtype, ptr::null()) }.cast::<GTypeModule>();
    assert!(!module.is_null(), "g_object_new returned NULL");

    // SAFETY: `module` is a GTypeModule; the reference this takes is never
    // given back, which is what the registry does for as long as the backend
    // is wanted.
    assert_ne!(
        unsafe { g_type_module_use(module) },
        GFALSE,
        "the module would not load at all"
    );

    // SAFETY: no arguments; `EOAuth2Services` is a process-wide singleton
    // (`e-oauth2-services.c`), so this is the same registry any other OAuth2
    // code in this process would get.
    let registry = unsafe { e_oauth2_services_new() };
    let source = source_naming_this_service();

    // SAFETY: a live registry and a live source; a non-NULL result is a new
    // ref this test drops.
    unsafe {
        let found = e_oauth2_services_find(registry, source);
        assert!(
            !found.is_null(),
            "the module load did not register a service that answers to \
             this service's own authentication method — the JMAP OAuth2 \
             flow would silently never run"
        );
        g_object_unref(found.cast());
        g_object_unref(source.cast());
    }
}
