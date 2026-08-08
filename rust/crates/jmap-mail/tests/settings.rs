// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// `CamelJmapSettings` is where a JMAP account's server lives on the Camel
// side: the host to reach, the port, the user name, and whether the connection
// has to be encrypted. All four are properties of the `CamelNetworkSettings`
// interface, and none of Camel's stock settings classes implements it — which
// is the whole reason this type exists rather than the provider reusing
// `CamelOfflineSettings`.
//
// Implementing an interface is two halves, and only the second one has a
// visible failure mode. Claiming it (`g_type_add_interface_static`) is what
// makes `CAMEL_IS_NETWORK_SETTINGS` true; overriding its properties
// (`g_object_class_override_property` plus a `set_property`/`get_property`
// pair) is what makes them reachable through `g_object_set`, which is how EDS
// configures a service — `e_source_camel_configure_service` binds the
// `ESource` extension's properties to the settings object's by name. A type
// that does the first and not the second passes `CAMEL_IS_NETWORK_SETTINGS`,
// logs five criticals at class-init time that nothing fails on, and then
// quietly ignores every host EDS tries to give it. So the tests below drive
// the properties, not the interface.

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

/// The five properties `CamelNetworkSettings` declares. Spelled the way GObject
/// spells them — a typo here is a property that is never found, which is
/// exactly the failure this file is about.
const NETWORK_PROPERTIES: [&CStr; 5] = [
    c"auth-mechanism",
    c"host",
    c"port",
    c"security-method",
    c"user",
];

fn new_settings() -> *mut CamelSettings {
    // SAFETY: the type is registered by `settings_type` and has no construct
    // properties of its own; the caller unrefs the result.
    let settings = unsafe { g_object_new(settings_type(), ptr::null()) };
    assert!(!settings.is_null(), "g_object_new returned NULL");
    settings.cast::<CamelSettings>()
}

/// The claim half: the type derives from the settings class a
/// `CamelOfflineStore` expects and implements the interface Camel's network
/// accessors demand.
#[test]
fn the_settings_type_is_an_offline_settings_that_talks_to_a_network() {
    let gtype = settings_type();
    assert_ne!(gtype, 0, "registration returned the invalid GType");

    // SAFETY: plain type-system reads on a registered type.
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

/// The override half. `g_object_class_find_property` is what `g_object_set`
/// and `g_object_bind_property` use, so a property it cannot find is one EDS
/// cannot set — and an interface property that has not been overridden is not
/// found on the implementing class.
#[test]
fn every_network_property_is_reachable_on_the_class() {
    // SAFETY: the type is registered; the ref is released below.
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

/// The two halves joined up: a value set the way EDS sets it — by property —
/// comes back out of the accessor the client will read it with. Setting a
/// property that is overridden but whose `set_property` drops it on the floor
/// is silent, so the read is deliberately through a different door than the
/// write.
#[test]
fn a_property_set_by_name_reaches_camels_network_accessors() {
    let settings = new_settings();
    // SAFETY: `settings` is an instance of the type, the names are NUL
    // terminated and the varargs are the types the properties declare.
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

/// ...and the other direction, which is the one Evolution's account editor
/// reads the current configuration back through.
#[test]
fn a_value_set_through_camels_accessors_reads_back_as_a_property() {
    let settings = new_settings();
    // SAFETY: as above; the two `g_object_get` out-parameters are a
    // g_malloc'd string this call frees and a plain integer.
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
        // Never set, and the interface's default is 0 — which is what tells
        // the client to use the scheme's default port rather than port 0.
        assert_eq!(port, 0);
        glib_sys::g_free(host.cast::<gpointer>() as gpointer);

        g_object_unref(settings.cast());
    }
}

/// The end-to-end consequence of the overrides, and the one that fails loudest
/// without them: `camel_settings_clone` copies a settings object by walking
/// its properties. Camel clones a service's settings whenever it compares the
/// configured account against the connected one, so a host that is not a
/// property is a host that disappears from the copy — and two accounts that
/// differ only in their server compare equal.
#[test]
fn cloning_carries_the_server_along() {
    let settings = new_settings();
    // SAFETY: `camel_settings_clone` returns a new reference this test owns.
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

        // ...and a difference in the server is a difference Camel can see.
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

/// A settings object nothing has configured already asks for TLS — and that is
/// a consequence of the overrides, not a separate decision.
///
/// The interface's properties are `G_PARAM_CONSTRUCT`, so `g_object_new`
/// pushes each declared default through the class's `set_property` on the way
/// out. A class that overrides them therefore starts at the interface's
/// default, which for `security-method` is TLS; a class that does not override
/// them is never told, and starts at the enum's zero value, which is
/// plaintext. So the overrides are a security property of this type and not
/// only a configuration one, and this is the test that would notice them
/// disappearing.
///
/// `STARTTLS_ON_STANDARD_PORT` is a name about a protocol JMAP does not have.
/// JMAP is HTTP: what the mapping to an origin will read out of this is the
/// one bit that is really there, `NONE` or not `NONE`.
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
        // Nothing invents a server, though — and "no server" here is the
        // *empty string*, which the construct default pushed in, not the NULL
        // an unset `ESource` field reads back as. Whatever turns these
        // settings into an origin has to treat the two the same way, or an
        // account nobody configured becomes a request to `https://`.
        let host = camel_network_settings_get_host(network);
        assert!(!host.is_null(), "the default host is NULL after all");
        assert_eq!(CStr::from_ptr(host), c"");
        g_object_unref(settings.cast());
    }
}

/// What connects the type to the store: `CamelServiceClass.settings_type` is
/// the type `camel_service_ref_settings` instantiates when nothing has handed
/// the service a settings object. Left at the inherited `CamelOfflineSettings`,
/// every store would come up with settings that carry no server at all — the
/// failure this whole file is about, in the one place where nothing would set
/// the properties to notice it.
#[test]
fn the_store_instantiates_these_settings_and_not_camels_default() {
    // SAFETY: the store type is registered by `store_type`, and the class ref
    // is released below. Referencing the class is also what runs the store's
    // class_init, which is where the field is set.
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
