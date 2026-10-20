// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests that the module registers `jmap_config::oauth2_service::Service` in
//! the real `evolution-source-registry`. That process refreshes the OAuth2
//! token of a JMAP account.
//!
//! evolution-ews registers its `EOAuth2ServiceOffice365` in the same way. It
//! also registers it in the shell process and in its book and calendar backend
//! factories (`module-ews-configuration.c`, `e-book-backend-ews-factory.c`,
//! `e-cal-backend-ews-factory.c`). That is four modules, as in this project.
//!
//! One call is sufficient for this reason. `EOAuth2ServiceBase`
//! (`e-oauth2-service-base.c`) is an `EExtension` whose `extensible_type` is
//! `E_TYPE_OAUTH2_SERVICES`. The constructor of `EOAuth2Services`
//! (`e-oauth2-services.c`) calls `e_extensible_load_extensions()` on itself
//! before it returns. Thus a type must only be *registered* before this
//! process constructs the `EOAuth2Services` singleton. The `jmap-config`
//! module docs describe the same idiom for `EMailConfigServiceBackend`.
//!
//! This test runs that full path without a manual `g_object_new` of the
//! service. It registers the module and constructs the registry as an EDS
//! process does. Then it requests a service for a source that names ours.

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
use jmap_backend_core::oauth2::OAUTH2_AUTH_METHOD;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A `GTypeModule` in place of the `EModule` that the registry loads. Also
/// used in `tests/factory.rs` and `tests/textdomain.rs`.
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
        // SAFETY: the function takes no arguments, and the type initialises
        // itself.
        unsafe { g_type_module_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points to our new class struct. It starts with
        // GTypeModuleClass, which contains both slots.
        let vfuncs = unsafe { &mut (*class).parent_class };
        vfuncs.load = Some(module_load);
        vfuncs.unload = Some(module_unload);
    }
}

unsafe extern "C" fn module_load(module: *mut GTypeModule) -> gboolean {
    // SAFETY: GLib passes the module that it loads, as the entry point
    // requires.
    unsafe { load(module) };
    GTRUE
}

unsafe extern "C" fn module_unload(module: *mut GTypeModule) {
    // SAFETY: as `module_load`.
    unsafe { unload(module) };
}

/// A temporary `ESource` whose authentication method is the name of this
/// service. The setup UI writes this `[Authentication] method` for a JMAP
/// OAuth2 account.
fn source_naming_this_service() -> *mut ESource {
    let uid = CString::new("jmap-collection-oauth2-service-test").expect("no NUL in a literal");
    let mut error = ptr::null_mut();
    // SAFETY: a NUL-terminated uid, a NULL main context (the default context)
    // and a GError out-parameter are the documented arguments.
    let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
    assert!(!source.is_null(), "e_source_new_with_uid failed");

    // SAFETY: a live source. `E_SOURCE_EXTENSION_AUTHENTICATION` is an EDS
    // extension name, and EDS can always create it.
    unsafe {
        let auth =
            e_source_get_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
        e_source_authentication_set_method(auth, OAUTH2_AUTH_METHOD.as_ptr());
    }
    source
}

/// Registers the module, then requests a service for a matching source from
/// the `EOAuth2Services` singleton. A real `evolution-source-registry` uses this
/// path. A test of only the `GType` registration is not sufficient: a service
/// that nothing instantiates also passes it.
#[test]
fn the_registry_finds_the_jmap_service_once_the_module_has_loaded() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let gtype = register_static::<TestModule>();
        assert_ne!(gtype, 0, "the stand-in module type did not register");

        // SAFETY: the type is registered, and GTypeModule has no construct
        // properties.
        let module = unsafe { g_object_new(gtype, ptr::null()) }.cast::<GTypeModule>();
        assert!(!module.is_null(), "g_object_new returned NULL");

        // SAFETY: `module` is a GTypeModule. The test does not release this
        // use, as the registry does while it needs the backend.
        assert_ne!(
            unsafe { g_type_module_use(module) },
            GFALSE,
            "the module would not load at all"
        );

        // SAFETY: the function takes no arguments. `EOAuth2Services` is a
        // process-wide singleton (`e-oauth2-services.c`). Thus all OAuth2 code in
        // this process gets this registry.
        let registry = unsafe { e_oauth2_services_new() };
        let source = source_naming_this_service();

        // SAFETY: a live registry and a live source. A non-NULL result is a new
        // reference that this test releases.
        unsafe {
            let found = e_oauth2_services_find(registry, source);
            assert!(
                !found.is_null(),
                "the module load did not register a service that answers to \
                 this service's own authentication method — the JMAP OAuth2 \
                 flow would silently never run"
            );

            // The service that answers depends on the EDS version. If EDS has
            // no dynamic service, only `load` above supplies a service. If EDS
            // has one, this project must not register its own service.
            // Otherwise, two services claim one account, and
            // `e_oauth2_services_find` selects one by list order.
            let found_type = (*(*found.cast::<gobject_sys::GTypeInstance>()).g_class).g_type;
            let found_name = std::ffi::CStr::from_ptr(gobject_sys::g_type_name(found_type));
            #[cfg(not(eds_oauth2_dynamic))]
            assert_eq!(found_name, c"JmapOAuth2Service");
            #[cfg(eds_oauth2_dynamic)]
            {
                assert_eq!(found_name, c"EOAuth2ServiceDynamic");
                assert_eq!(
                    gobject_sys::g_type_from_name(c"JmapOAuth2Service".as_ptr()),
                    0,
                    "the module registered this project's own EOAuth2Service on \
                     an EDS that brings its own, so two services now claim the \
                     same account"
                );
            }
            g_object_unref(found.cast());
            g_object_unref(source.cast());
        }
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_oauth2_service_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}
