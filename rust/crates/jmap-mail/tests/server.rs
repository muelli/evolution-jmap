// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Turning a `CamelJmapSettings` into the server a JMAP client talks to.
//
// This is the Camel-side sibling of `jmap-backend-core`'s `SourceConfig`, and
// it exists because the two sides carry the same account in different shapes:
// EDS keeps host, port, user and "is it secure" in `ESource` extensions, Camel
// keeps them on the `CamelNetworkSettings` interface. What must *not* differ is
// the answer — the same host validation, and the same refusal to send a
// password over plain HTTP to anything but loopback — so the rules live in one
// place and these tests check that this side reaches them.
//
// Two differences between the sides are real, and both are tested below:
// "nothing configured" is the empty string here where an unset `ESource` field
// is NULL, and the security method is an enum with three values where the
// source's is a boolean.

use std::ffi::{CStr, CString};
use std::ptr;

use eds_sys::{
    CAMEL_NETWORK_SECURITY_METHOD_NONE, CAMEL_NETWORK_SECURITY_METHOD_SSL_ON_ALTERNATE_PORT,
    CamelNetworkSecurityMethod, CamelNetworkSettings, CamelSettings,
    camel_network_settings_set_host, camel_network_settings_set_port,
    camel_network_settings_set_security_method, camel_network_settings_set_user,
    camel_offline_settings_get_type,
};
use gobject_sys::{g_object_new, g_object_unref};
use jmap_backend_core::source::SourceError;
use jmap_mail::server::ServerConfig;
use jmap_mail::settings::settings_type;

/// A settings object configured the way Evolution's account editor would leave
/// one: a host, and whatever the caller wants to say about the rest.
fn settings(
    host: &CStr,
    port: u16,
    method: Option<CamelNetworkSecurityMethod>,
) -> *mut CamelSettings {
    // SAFETY: the type is registered by `settings_type` and has no construct
    // properties of its own; every accessor below takes an instance of it.
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

fn config_of(settings: *mut CamelSettings) -> Result<ServerConfig, SourceError> {
    // SAFETY: the caller built `settings` with `g_object_new`, and the call
    // only reads from it.
    let config = unsafe { ServerConfig::from_settings(settings) };
    // SAFETY: this test owns the reference `g_object_new` returned, and
    // `from_settings` kept nothing that points into the object.
    unsafe { g_object_unref(settings.cast()) };
    config
}

/// The ordinary case: an account on a real server, over TLS, on the scheme's
/// default port. The security method is left at the construct default, because
/// that is the state an account the user never touched the "encryption" menu on
/// is actually in.
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
    assert_eq!(config.origin, "https://jmap.example.com");
    assert_eq!(config.user.as_deref(), Some("vera@example.com"));
}

/// A port the account names is part of the origin; a port it does not name is
/// left out entirely, so that the scheme's default applies rather than port 0.
#[test]
fn the_port_the_account_names_is_part_of_the_origin() {
    let config = config_of(settings(c"jmap.example.com", 8443, None)).expect("a server");
    assert_eq!(config.origin, "https://jmap.example.com:8443");
}

/// Both non-`NONE` security methods mean the same thing here. They are names
/// about a protocol JMAP does not have — JMAP is HTTP, so there is no STARTTLS
/// and no alternate port — and the only bit really in that field is whether
/// the connection has to be encrypted.
#[test]
fn every_security_method_but_none_is_just_tls() {
    let config = config_of(settings(
        c"jmap.example.com",
        0,
        Some(CAMEL_NETWORK_SECURITY_METHOD_SSL_ON_ALTERNATE_PORT),
    ))
    .expect("a server");
    assert_eq!(config.origin, "https://jmap.example.com");
}

/// The security decision, and the reason this mapping is not a `format!`.
/// A `.source` keyfile or an account editor can switch encryption off; doing
/// so for a server that is not on this machine would put the password on the
/// network in the clear.
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

/// ...and the exception that makes development possible: `jmap-mockd` speaks
/// plain HTTP on loopback, where there is no network to leak onto.
#[test]
fn the_mock_server_is_reachable_without_tls() {
    let config = config_of(settings(
        c"127.0.0.1",
        8080,
        Some(CAMEL_NETWORK_SECURITY_METHOD_NONE),
    ))
    .expect("a server");
    assert_eq!(config.origin, "http://127.0.0.1:8080");

    // An IPv6 literal has to be bracketed, or the colons in it read as the
    // port separator.
    let config = config_of(settings(
        c"::1",
        8080,
        Some(CAMEL_NETWORK_SECURITY_METHOD_NONE),
    ))
    .expect("a server");
    assert_eq!(config.origin, "http://[::1]:8080");
}

/// The origin is assembled by concatenation, so the host is not just data:
/// anything but a bare host name or an IP literal could aim the client at
/// another server, or slip a plaintext endpoint past the check above.
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

/// An internationalised host name is converted to its ASCII form before it is
/// validated, because that is the form it goes on the wire in — Camel offers
/// the conversion for exactly this reason. Rejecting the UTF-8 spelling
/// instead would make a perfectly good account unusable.
#[test]
fn an_internationalised_host_is_punycoded_before_it_is_checked() {
    let config = config_of(settings(c"m\u{fc}nchen.example", 0, None)).expect("a server");
    assert_eq!(config.origin, "https://xn--mnchen-3ya.example");
}

/// A settings object nobody configured names no server. The interesting part
/// is that its host is the *empty string* — the construct default the property
/// overrides push in — where an unset `ESource` field is NULL. Read without
/// care, this account becomes a request to `https://`.
#[test]
fn an_unconfigured_account_names_no_server() {
    assert_eq!(
        config_of(settings(c"", 0, None)),
        Err(SourceError::MissingHost)
    );
}

/// The same rule one field along: a user name nobody filled in is absent, not
/// present and empty. `Some("")` would be sent as a login name.
#[test]
fn a_user_nobody_filled_in_is_absent_rather_than_empty() {
    let config = config_of(settings(c"jmap.example.com", 0, None)).expect("a server");
    assert_eq!(config.user, None);
}

/// Defence in depth: a service can in principle be handed a settings object of
/// another class, and Camel's network accessors assert on the type. Answering
/// "no server" is what those accessors would effectively answer anyway, minus
/// the criticals — and it must not be a crash in a process full of mail.
#[test]
fn settings_that_carry_no_network_name_no_server() {
    // SAFETY: `CamelOfflineSettings` is instantiable and has no construct
    // properties; it does not implement `CamelNetworkSettings`.
    let settings = unsafe { g_object_new(camel_offline_settings_get_type(), ptr::null()) };
    assert!(!settings.is_null(), "g_object_new returned NULL");
    assert_eq!(
        config_of(settings.cast::<CamelSettings>()),
        Err(SourceError::MissingHost)
    );
}

/// The other half of the conversion, and the reason the ASCII spelling is the
/// only one read: a host Camel *cannot* convert comes back from
/// `dup_host_ensure_ascii` unchanged rather than as NULL. So an unconvertible
/// host is not silently reported as a missing server — it reaches the
/// validator still holding what the account said, and is rejected there.
///
/// A settings property is a byte string, and nothing guarantees the account
/// that wrote it produced UTF-8; this is what that looks like arriving.
#[test]
fn a_host_camel_cannot_convert_is_rejected_rather_than_lost() {
    let host = CString::new(b"\xff\xfe.example".to_vec()).expect("no interior NUL");
    let config = config_of(settings(&host, 0, None));
    assert!(
        matches!(config, Err(SourceError::InvalidHost(_))),
        "an unconvertible host became {config:?}"
    );
}
