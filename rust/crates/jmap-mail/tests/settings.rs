// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// `CamelJmapSettings` stores a JMAP account's server settings on the Camel
// side: host, port, user name, and whether the connection must be encrypted.
// These four properties belong to the `CamelNetworkSettings` interface. None
// of Camel's stock settings classes implements this interface. This is why
// this type exists instead of the provider reusing `CamelOfflineSettings`.
//
// Implementing an interface has two halves. Only the second half has a
// visible failure mode. Claiming the interface (`g_type_add_interface_static`)
// makes `CAMEL_IS_NETWORK_SETTINGS` true. Overriding its properties
// (`g_object_class_override_property` plus a `set_property`/`get_property`
// pair) makes them reachable through `g_object_set`. This is how EDS
// configures a service: `e_source_camel_configure_service` binds the
// `ESource` extension's properties to the settings object's properties by
// name. A type that claims the interface but does not override its
// properties still passes `CAMEL_IS_NETWORK_SETTINGS`. It logs five
// criticals at class-init time, none of which fail the build, and then
// silently ignores every host EDS tries to set. So the tests below check the
// properties, not the interface.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    CAMEL_NETWORK_SECURITY_METHOD_NONE, CAMEL_NETWORK_SECURITY_METHOD_STARTTLS_ON_STANDARD_PORT,
    CamelNetworkSettings, CamelServiceClass, CamelSettings, camel_network_settings_get_host,
    camel_network_settings_get_port, camel_network_settings_get_security_method,
    camel_network_settings_get_type, camel_network_settings_get_user,
    camel_network_settings_set_host, camel_network_settings_set_user,
    camel_offline_settings_get_type, camel_settings_clone, camel_settings_equal,
};
use glib_sys::{GFALSE, gpointer};
use gobject_sys::{
    GObject, g_object_class_find_property, g_object_get, g_object_new, g_object_set,
    g_object_unref, g_type_class_ref, g_type_class_unref, g_type_is_a,
};
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_mail::settings::{JmapSettings, settings_type};
use jmap_mail::store::store_type;

/// The five properties that `CamelNetworkSettings` declares, spelled exactly
/// as GObject spells them. A typo here creates a property that is never
/// found, which is the failure this file tests for.
const NETWORK_PROPERTIES: [&CStr; 5] = [
    c"auth-mechanism",
    c"host",
    c"port",
    c"security-method",
    c"user",
];

fn new_settings() -> *mut CamelSettings {
    // SAFETY: `settings_type` registers the type, and the type has no
    // construct properties of its own. The caller unrefs the result.
    let settings = unsafe { g_object_new(settings_type(), ptr::null()) };
    assert!(!settings.is_null(), "g_object_new returned NULL");
    settings.cast::<CamelSettings>()
}

/// The claim half of the interface. The type derives from the settings class
/// that a `CamelOfflineStore` expects, and implements the interface that
/// Camel's network accessors require.
#[test]
fn the_settings_type_is_an_offline_settings_that_talks_to_a_network() {
    let gtype = settings_type();
    assert_ne!(gtype, 0, "registration returned the invalid GType");

    // SAFETY: these are read-only type-system queries on a registered type.
    unsafe {
        assert_eq!(
            CStr::from_ptr(gobject_sys::g_type_name(gtype)),
            JmapSettings::NAME
        );
        assert_ne!(
            g_type_is_a(gtype, camel_offline_settings_get_type()),
            GFALSE,
            "a CamelOfflineStore's settings must derive from CamelOfflineSettings"
        );
        assert_ne!(
            g_type_is_a(gtype, camel_network_settings_get_type()),
            GFALSE,
            "the settings type does not implement CamelNetworkSettings"
        );
    }
}

/// The override half of the interface. `g_object_set` and
/// `g_object_bind_property` both use `g_object_class_find_property`. A
/// property that this function cannot find is a property that EDS cannot
/// set. An interface property that the class has not overridden is not
/// found on that class.
#[test]
fn every_network_property_is_reachable_on_the_class() {
    // SAFETY: the type is registered, and the ref is released below.
    unsafe {
        let class = g_type_class_ref(settings_type()).cast::<gobject_sys::GObjectClass>();
        for name in NETWORK_PROPERTIES {
            let pspec = g_object_class_find_property(class, name.as_ptr());
            assert!(
                !pspec.is_null(),
                "the class does not implement the {name:?} property; EDS cannot configure it"
            );
        }
        g_type_class_unref(class.cast());
    }
}

/// The two halves joined: a value set by property, the way EDS sets it,
/// comes back out through the accessor the client reads it with. A
/// `set_property` override that silently drops the value would not fail
/// loudly, so this test reads the value back through a different path than
/// it wrote it.
#[test]
fn a_property_set_by_name_reaches_camels_network_accessors() {
    let settings = new_settings();
    // SAFETY: `settings` is an instance of the type. The names are NUL
    // terminated, and the varargs match the types the properties declare.
    unsafe {
        g_object_set(
            settings.cast::<GObject>(),
            c"host".as_ptr(),
            c"jmap.example.com".as_ptr(),
            c"port".as_ptr(),
            8080u32,
            c"user".as_ptr(),
            c"vera@example.com".as_ptr(),
            c"security-method".as_ptr(),
            CAMEL_NETWORK_SECURITY_METHOD_STARTTLS_ON_STANDARD_PORT,
            ptr::null::<i8>(),
        );

        let network = settings.cast::<CamelNetworkSettings>();
        assert_eq!(
            CStr::from_ptr(camel_network_settings_get_host(network)),
            c"jmap.example.com"
        );
        assert_eq!(camel_network_settings_get_port(network), 8080);
        assert_eq!(
            CStr::from_ptr(camel_network_settings_get_user(network)),
            c"vera@example.com"
        );
        assert_eq!(
            camel_network_settings_get_security_method(network),
            CAMEL_NETWORK_SECURITY_METHOD_STARTTLS_ON_STANDARD_PORT
        );

        g_object_unref(settings.cast());
    }
}

/// The other direction: Evolution's account editor reads the current
/// configuration back through this same path.
#[test]
fn a_value_set_through_camels_accessors_reads_back_as_a_property() {
    let settings = new_settings();
    // SAFETY: as above. The two `g_object_get` out-parameters are a
    // g_malloc'd string, freed by this call, and a plain integer.
    unsafe {
        let network = settings.cast::<CamelNetworkSettings>();
        camel_network_settings_set_host(network, c"127.0.0.1".as_ptr());
        camel_network_settings_set_user(network, c"vera".as_ptr());

        let mut host: *mut i8 = ptr::null_mut();
        let mut port: u32 = 0;
        g_object_get(
            settings.cast::<GObject>(),
            c"host".as_ptr(),
            &mut host,
            c"port".as_ptr(),
            &mut port,
            ptr::null::<i8>(),
        );
        assert!(!host.is_null(), "the host property read back as NULL");
        assert_eq!(CStr::from_ptr(host), c"127.0.0.1");
        // The port is never set, and the interface's default value is 0. A
        // port of 0 tells the client to use the scheme's default port, not
        // port 0.
        assert_eq!(port, 0);
        glib_sys::g_free(host.cast::<gpointer>() as gpointer);

        g_object_unref(settings.cast());
    }
}

/// The end-to-end consequence of the overrides, and the one that fails most
/// visibly without them. `camel_settings_clone` copies a settings object by
/// walking its properties. Camel clones a service's settings whenever it
/// compares the configured account against the connected one. A host that is
/// not a property disappears from the copy, so two accounts that differ only
/// in their server would compare equal.
#[test]
fn cloning_carries_the_server_along() {
    let settings = new_settings();
    // SAFETY: `camel_settings_clone` returns a new reference that this test
    // owns.
    unsafe {
        let network = settings.cast::<CamelNetworkSettings>();
        camel_network_settings_set_host(network, c"jmap.example.com".as_ptr());

        let clone = camel_settings_clone(settings);
        assert!(!clone.is_null(), "camel_settings_clone returned NULL");
        assert_eq!(
            CStr::from_ptr(camel_network_settings_get_host(
                clone.cast::<CamelNetworkSettings>()
            )),
            c"jmap.example.com",
            "the clone lost the host, so it is not a property"
        );
        assert_ne!(
            camel_settings_equal(settings, clone),
            GFALSE,
            "a fresh clone does not compare equal to its original"
        );

        // A difference in the server is a difference that Camel can detect.
        camel_network_settings_set_host(
            clone.cast::<CamelNetworkSettings>(),
            c"other.example.com".as_ptr(),
        );
        assert_eq!(
            camel_settings_equal(settings, clone),
            GFALSE,
            "two accounts on different servers compare equal"
        );

        g_object_unref(clone.cast());
        g_object_unref(settings.cast());
    }
}

/// An unconfigured settings object already asks for TLS. This is a
/// consequence of the overrides, not a separate decision.
///
/// The interface's properties are `G_PARAM_CONSTRUCT`, so `g_object_new`
/// pushes each declared default through the class's `set_property`. A class
/// that overrides the properties starts at the interface's default, which is
/// TLS for `security-method`. A class that does not override the properties
/// is never told the default, and starts at the enum's zero value, which is
/// plaintext. So the overrides are a security property of this type, not
/// only a configuration one. This test would notice if the overrides
/// disappeared.
///
/// `STARTTLS_ON_STANDARD_PORT` names a protocol that JMAP does not have.
/// JMAP runs over HTTP: the mapping to an origin reads only one real bit
/// from this value, whether it is `NONE` or not `NONE`.
#[test]
fn an_unconfigured_settings_object_already_asks_for_tls() {
    let settings = new_settings();
    // SAFETY: `settings` is an instance of the type.
    unsafe {
        let network = settings.cast::<CamelNetworkSettings>();
        assert_ne!(
            camel_network_settings_get_security_method(network),
            CAMEL_NETWORK_SECURITY_METHOD_NONE,
            "a settings object that was never configured defaults to plaintext"
        );
        assert_eq!(
            camel_network_settings_get_security_method(network),
            CAMEL_NETWORK_SECURITY_METHOD_STARTTLS_ON_STANDARD_PORT
        );
        // Nothing invents a server, though. Here "no server" is the empty
        // string, which the construct default pushes in, not the NULL that
        // an unset `ESource` field reads back as. Whatever converts these
        // settings into an origin must treat both values the same way, or an
        // unconfigured account becomes a request to `https://`.
        let host = camel_network_settings_get_host(network);
        assert!(!host.is_null(), "the default host is NULL after all");
        assert_eq!(CStr::from_ptr(host), c"");
        g_object_unref(settings.cast());
    }
}

/// What connects the type to the store. `CamelServiceClass.settings_type` is
/// the type that `camel_service_ref_settings` instantiates when nothing has
/// given the service a settings object. Left at the inherited
/// `CamelOfflineSettings`, every store would get settings with no server at
/// all. This is the failure this file tests for, in the one place where
/// nothing sets the properties to reveal it.
#[test]
fn the_store_instantiates_these_settings_and_not_camels_default() {
    // SAFETY: `store_type` registers the store type, and the class ref is
    // released below. Referencing the class also runs the store's
    // class_init, which sets the field.
    unsafe {
        let class = g_type_class_ref(store_type()).cast::<CamelServiceClass>();
        assert_eq!(
            (*class).settings_type,
            settings_type(),
            "the store does not instantiate CamelJmapSettings"
        );
        g_type_class_unref(class.cast());
    }
}
