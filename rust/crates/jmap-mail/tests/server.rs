// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Converts a `CamelJmapSettings` object into the server a JMAP client talks to.
//
// This is the Camel-side sibling of `jmap-backend-core`'s `SourceConfig`. The
// two sides store the same account in different shapes: EDS keeps host, port,
// user, and the security flag in `ESource` extensions. Camel keeps them on the
// `CamelNetworkSettings` interface. The answer must not differ between sides:
// the same host validation applies, and both refuse to send a password over
// plain HTTP to any host except loopback. The rules live in one place.
// These tests check that this side reaches them.
//
// Two differences between the sides are real. Both are tested below.
// Here, "nothing configured" is the empty string. In `ESource`, an unset
// field is NULL. The security method here is an enum with three values.
// In `ESource`, it is a boolean.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::Once;

use eds_sys::{
    CAMEL_NETWORK_SECURITY_METHOD_NONE, CAMEL_NETWORK_SECURITY_METHOD_SSL_ON_ALTERNATE_PORT,
    CamelNetworkSecurityMethod, CamelNetworkSettings, CamelSettings,
    camel_network_settings_set_host, camel_network_settings_set_port,
    camel_network_settings_set_security_method, camel_network_settings_set_user,
    camel_offline_settings_get_type,
};
use glib_sys::{G_LOG_LEVEL_CRITICAL, G_LOG_LEVEL_WARNING, g_log_set_always_fatal};
use gobject_sys::{g_object_new, g_object_unref};
use jmap_backend_core::source::{ConnectTarget, SourceError};
use jmap_mail::server::ServerConfig;
use jmap_mail::settings::settings_type;

/// A settings object as Evolution's account editor would leave it: a host,
/// plus whatever the caller sets for the rest.
fn settings(
    host: &CStr,
    port: u16,
    method: Option<CamelNetworkSecurityMethod>,
) -> *mut CamelSettings {
    // SAFETY: `settings_type` registers the type. It has no construct
    // properties of its own. Every accessor below takes an instance of it.
    unsafe {
        let object = g_object_new(settings_type(), ptr::null());
        assert!(!object.is_null(), "g_object_new returned NULL");
        let network = object.cast::<CamelNetworkSettings>();
        camel_network_settings_set_host(network, host.as_ptr());
        camel_network_settings_set_port(network, port);
        if let Some(method) = method {
            camel_network_settings_set_security_method(network, method);
        }
        object.cast::<CamelSettings>()
    }
}

/// Configure GLib critical and warning logs to abort the test process.
fn glib_complaints_are_failures() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: no preconditions; the call returns the previous mask.
        unsafe { g_log_set_always_fatal(G_LOG_LEVEL_CRITICAL | G_LOG_LEVEL_WARNING) };
    });
}

fn config_of(settings: *mut CamelSettings) -> Result<ServerConfig, SourceError> {
    // SAFETY: the caller built `settings` with `g_object_new`. The call
    // only reads from it.
    let config = unsafe { ServerConfig::from_settings(settings) };
    // SAFETY: this test owns the reference `g_object_new` returned.
    // `from_settings` kept nothing that points into the object.
    unsafe { g_object_unref(settings.cast()) };
    config
}

/// The ordinary case: an account on a real server, over TLS, on the scheme's
/// default port. The security method stays at the construct default. That is
/// the actual state of an account whose user never opened the encryption menu.
#[test]
fn a_configured_account_becomes_an_https_origin() {
    let settings = settings(c"jmap.example.com", 0, None);
    // SAFETY: the object is alive until `config_of` drops it.
    unsafe {
        camel_network_settings_set_user(
            settings.cast::<CamelNetworkSettings>(),
            c"vera@example.com".as_ptr(),
        );
    }
    let config = config_of(settings).expect("a configured account has a server");
    // No port named: RFC 8620 §2.2 SRV autodiscovery applies.
    assert_eq!(
        config.target,
        ConnectTarget::Domain("jmap.example.com".into())
    );
    assert_eq!(config.user.as_deref(), Some("vera@example.com"));
}

/// A port the account names becomes part of the origin. A port it does not
/// name is left out, so the scheme's default port applies instead of port 0.
#[test]
fn the_port_the_account_names_is_part_of_the_origin() {
    let config = config_of(settings(c"jmap.example.com", 8443, None)).expect("a server");
    assert_eq!(
        config.target,
        ConnectTarget::Origin("https://jmap.example.com:8443".into())
    );
}

/// Both non-`NONE` security methods mean the same thing here. They name
/// concepts from a protocol JMAP does not use: JMAP is HTTP, so there is no
/// STARTTLS and no alternate port. The only real bit in this field is
/// whether the connection must be encrypted.
#[test]
fn every_security_method_but_none_is_just_tls() {
    let config = config_of(settings(
        c"jmap.example.com",
        0,
        Some(CAMEL_NETWORK_SECURITY_METHOD_SSL_ON_ALTERNATE_PORT),
    ))
    .expect("a server");
    assert_eq!(
        config.target,
        ConnectTarget::Domain("jmap.example.com".into())
    );
}

/// The security decision, and why this mapping is not a plain `format!`.
/// A `.source` keyfile or an account editor can switch encryption off.
/// Doing so for a server not on this machine would send the password
/// over the network unencrypted.
#[test]
fn plaintext_is_refused_for_a_remote_server() {
    assert_eq!(
        config_of(settings(
            c"jmap.example.com",
            0,
            Some(CAMEL_NETWORK_SECURITY_METHOD_NONE)
        )),
        Err(SourceError::InsecureTransport("jmap.example.com".into()))
    );
}

/// The exception that makes development possible: `jmap-mockd` speaks
/// plain HTTP on loopback, where no network exists to leak onto.
#[test]
fn the_mock_server_is_reachable_without_tls() {
    let config = config_of(settings(
        c"127.0.0.1",
        8080,
        Some(CAMEL_NETWORK_SECURITY_METHOD_NONE),
    ))
    .expect("a server");
    assert_eq!(
        config.target,
        ConnectTarget::Origin("http://127.0.0.1:8080".into())
    );

    // An IPv6 literal must be bracketed. Otherwise its colons read as the
    // port separator.
    let config = config_of(settings(
        c"::1",
        8080,
        Some(CAMEL_NETWORK_SECURITY_METHOD_NONE),
    ))
    .expect("a server");
    assert_eq!(
        config.target,
        ConnectTarget::Origin("http://[::1]:8080".into())
    );
}

/// The origin is built by concatenation, so the host is not just data.
/// A value other than a bare host name or an IP literal could aim the
/// client at another server, or slip a plaintext endpoint past the check above.
#[test]
fn a_host_that_is_not_a_bare_host_name_is_rejected() {
    for host in [
        c"jmap.example.com/../evil.example.com",
        c"https://evil.example.com",
        c"jmap.example.com:80",
        c"jmap.example.com#",
        c"user@jmap.example.com",
    ] {
        let host_str = host.to_str().expect("ASCII");
        assert_eq!(
            config_of(settings(host, 0, None)),
            Err(SourceError::InvalidHost(host_str.into())),
            "{host_str} was accepted as a host name"
        );
    }
}

/// Validation converts an internationalised host name to its ASCII form
/// first, because that is the form sent on the wire. Camel offers this
/// conversion for that reason. Rejecting the UTF-8 spelling instead would
/// make a valid account unusable.
#[test]
fn an_internationalised_host_is_punycoded_before_it_is_checked() {
    let config = config_of(settings(c"m\u{fc}nchen.example", 0, None)).expect("a server");
    assert_eq!(
        config.target,
        ConnectTarget::Domain("xn--mnchen-3ya.example".into())
    );
}

/// A settings object nobody configured names no server. Its host is the
/// empty string: the construct default that property overrides push in.
/// An unset `ESource` field, by contrast, is NULL. Read without care, this
/// account becomes a request to `https://`.
#[test]
fn an_unconfigured_account_names_no_server() {
    assert_eq!(
        config_of(settings(c"", 0, None)),
        Err(SourceError::MissingHost)
    );
}

/// The same rule applies to the user field: a user name nobody filled in is
/// absent, not present and empty. `Some("")` would be sent as a login name.
#[test]
fn a_user_nobody_filled_in_is_absent_rather_than_empty() {
    let config = config_of(settings(c"jmap.example.com", 0, None)).expect("a server");
    assert_eq!(config.user, None);
}

/// Defence in depth: a service can in principle receive a settings object of
/// another class. Camel's network accessors assert on the type. Answering
/// "no server" matches what those accessors would effectively answer anyway,
/// without the critical warnings. It must not crash a process full of mail.
#[test]
fn settings_that_carry_no_network_name_no_server() {
    // The accessors give the same answer with a critical. Only an abort shows the difference.
    glib_complaints_are_failures();
    // SAFETY: `CamelOfflineSettings` is instantiable and has no construct
    // properties. It does not implement `CamelNetworkSettings`.
    let settings = unsafe { g_object_new(camel_offline_settings_get_type(), ptr::null()) };
    assert!(!settings.is_null(), "g_object_new returned NULL");
    assert_eq!(
        config_of(settings.cast::<CamelSettings>()),
        Err(SourceError::MissingHost)
    );
}

/// The other half of the conversion, and why the code reads only the ASCII
/// spelling: a host Camel cannot convert comes back from
/// `dup_host_ensure_ascii` unchanged, not as NULL. So an unconvertible host
/// is not silently reported as a missing server. It reaches the validator
/// still holding what the account said, and is rejected there.
///
/// A settings property is a byte string. Nothing guarantees the account
/// that wrote it produced UTF-8. This test shows that case arriving.
#[test]
fn a_host_camel_cannot_convert_is_rejected_rather_than_lost() {
    let host = CString::new(b"\xff\xfe.example".to_vec()).expect("no interior NUL");
    let config = config_of(settings(&host, 0, None));
    assert!(
        matches!(config, Err(SourceError::InvalidHost(_))),
        "an unconvertible host became {config:?}"
    );
}
