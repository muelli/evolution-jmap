// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The `ESource` an EDS backend is handed is the *only* description of the
// account it has, and everything that can go wrong here goes wrong quietly:
// a host read from the wrong extension is an address book that never
// connects, and a plaintext origin nobody noticed is Basic credentials on the
// wire. So this drives a real `ESource` built with the EDS setters rather
// than a hand-made struct.

use std::ffi::CString;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_RESOURCE, E_SOURCE_EXTENSION_SECURITY,
    ESource, ESourceAuthentication, ESourceResource, ESourceSecurity,
    e_source_authentication_set_host, e_source_authentication_set_port,
    e_source_authentication_set_user, e_source_get_extension, e_source_new_with_uid,
    e_source_resource_set_identity, e_source_security_set_secure,
};
use gobject_sys::g_object_unref;
use jmap_backend_core::source::{SourceConfig, SourceError};

/// An `ESource` that is not backed by the registry — `e_source_new_with_uid`
/// with a NULL D-Bus object is exactly what EDS itself uses for a source read
/// from a keyfile, so the extension machinery behaves as it does in a
/// backend.
struct TestSource(*mut ESource);

impl TestSource {
    fn new() -> Self {
        let uid = CString::new("jmap-test-source").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a NULL
        // GError out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    fn authentication(&self) -> *mut ESourceAuthentication {
        // SAFETY: the source is alive and the name is a header constant; the
        // extension is created on demand and owned by the source.
        unsafe { e_source_get_extension(self.0, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()) }.cast()
    }

    fn security(&self) -> *mut ESourceSecurity {
        // SAFETY: as above.
        unsafe { e_source_get_extension(self.0, E_SOURCE_EXTENSION_SECURITY.as_ptr()) }.cast()
    }

    fn resource(&self) -> *mut ESourceResource {
        // SAFETY: as above.
        unsafe { e_source_get_extension(self.0, E_SOURCE_EXTENSION_RESOURCE.as_ptr()) }.cast()
    }

    fn host(self, host: &str) -> Self {
        let host = CString::new(host).expect("no NUL in a test host");
        // SAFETY: a live extension and a NUL-terminated string, which the
        // setter copies.
        unsafe { e_source_authentication_set_host(self.authentication(), host.as_ptr()) };
        self
    }

    fn port(self, port: u16) -> Self {
        // SAFETY: a live extension.
        unsafe { e_source_authentication_set_port(self.authentication(), port) };
        self
    }

    fn user(self, user: &str) -> Self {
        let user = CString::new(user).expect("no NUL in a test user");
        // SAFETY: as `host`.
        unsafe { e_source_authentication_set_user(self.authentication(), user.as_ptr()) };
        self
    }

    fn secure(self, secure: bool) -> Self {
        // SAFETY: a live extension.
        unsafe { e_source_security_set_secure(self.security(), secure as _) };
        self
    }

    fn identity(self, identity: &str) -> Self {
        let identity = CString::new(identity).expect("no NUL in a test identity");
        // SAFETY: as `host`.
        unsafe { e_source_resource_set_identity(self.resource(), identity.as_ptr()) };
        self
    }

    fn config(&self) -> Result<SourceConfig, SourceError> {
        // SAFETY: the source is alive for the duration of the call.
        unsafe { SourceConfig::from_source(self.0) }
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: we hold the only reference.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

#[test]
fn a_configured_source_yields_an_origin_a_user_and_an_address_book() {
    let config = TestSource::new()
        .host("jmap.example.com")
        .secure(true)
        .user("vera@example.com")
        .identity("Ab1")
        .config()
        .expect("a complete https source is valid");

    assert_eq!(config.origin, "https://jmap.example.com");
    assert_eq!(config.user.as_deref(), Some("vera@example.com"));
    assert_eq!(config.resource_id.as_deref(), Some("Ab1"));
}

#[test]
fn security_defaults_to_tls_when_the_source_never_mentions_it() {
    // A source written by hand — the documented manual test recipe — may not
    // carry a [Security] group at all. `ESourceSecurity:secure` defaults to
    // FALSE, so reading the extension unconditionally would silently
    // downgrade every such account to plain HTTP.
    let config = TestSource::new()
        .host("jmap.example.com")
        .config()
        .expect("a source with no [Security] group is secure");

    assert_eq!(config.origin, "https://jmap.example.com");
}

#[test]
fn an_explicit_security_group_still_wins() {
    // The flip side: "secure=false" was written on purpose and must not be
    // read as "unspecified".
    let err = TestSource::new()
        .host("jmap.example.com")
        .secure(false)
        .config()
        .expect_err("an explicit opt-out of TLS is still an opt-out");

    assert_eq!(
        err,
        SourceError::InsecureTransport("jmap.example.com".into())
    );
}

#[test]
fn an_explicit_port_is_carried_into_the_origin() {
    let config = TestSource::new()
        .host("jmap.example.com")
        .secure(true)
        .port(8443)
        .config()
        .expect("a complete https source is valid");

    assert_eq!(config.origin, "https://jmap.example.com:8443");
}

#[test]
fn port_zero_means_unset_and_leaves_the_scheme_default() {
    let config = TestSource::new()
        .host("jmap.example.com")
        .secure(true)
        .port(0)
        .config()
        .expect("a complete https source is valid");

    assert_eq!(config.origin, "https://jmap.example.com");
}

#[test]
fn plaintext_to_a_remote_host_is_refused() {
    let err = TestSource::new()
        .host("jmap.example.com")
        .secure(false)
        .user("vera@example.com")
        .config()
        .expect_err("Basic credentials must not go out in the clear");

    assert_eq!(
        err,
        SourceError::InsecureTransport("jmap.example.com".into())
    );
}

#[test]
fn plaintext_to_loopback_is_allowed() {
    // The mock server and any local development instance are http on
    // 127.0.0.1; refusing that would make the backend untestable.
    for host in ["localhost", "127.0.0.1", "127.0.0.2", "::1"] {
        let config = TestSource::new()
            .host(host)
            .secure(false)
            .port(8080)
            .config()
            .unwrap_or_else(|e| panic!("{host} should be reachable in the clear: {e}"));
        assert!(config.origin.starts_with("http://"), "{}", config.origin);
        assert!(config.origin.ends_with(":8080"), "{}", config.origin);
    }
}

#[test]
fn an_ipv6_literal_is_bracketed_so_the_port_stays_a_port() {
    let config = TestSource::new()
        .host("2001:db8::1")
        .secure(true)
        .port(8443)
        .config()
        .expect("an IPv6 literal is a valid host");

    assert_eq!(config.origin, "https://[2001:db8::1]:8443");
}

#[test]
fn a_source_without_a_host_is_a_configuration_error() {
    let err = TestSource::new()
        .user("vera@example.com")
        .config()
        .expect_err("there is nothing to connect to");

    assert_eq!(err, SourceError::MissingHost);
}

#[test]
fn a_host_that_smuggles_a_path_or_a_scheme_is_rejected() {
    // The origin is built by string concatenation, so a host carrying its own
    // separators would let a `.source` file aim the client somewhere else
    // entirely — including at a plaintext endpoint past the TLS check.
    for host in [
        "jmap.example.com/../evil",
        "evil.example.com#jmap.example.com",
        "http://evil.example.com",
        "jmap.example.com:80",
        // Surrounding whitespace never reaches us — EDS strips it in the
        // property setter — but an interior space is passed straight through.
        "jmap.example.com evil.example.com",
        "user@evil.example.com",
    ] {
        let err = TestSource::new()
            .host(host)
            .secure(true)
            .config()
            .expect_err(&format!("{host} is not a bare host"));
        assert_eq!(err, SourceError::InvalidHost(host.into()));
    }
}

#[test]
fn an_empty_user_or_identity_is_absent_rather_than_empty() {
    // A key the user cleared comes back as absent, not as "": `ESource`
    // normalises the empty string to NULL in the setter. Pinned because the
    // reader relies on it — an empty address book id would otherwise be sent
    // to the server as a filter matching nothing.
    let config = TestSource::new()
        .host("jmap.example.com")
        .secure(true)
        .user("")
        .identity("")
        .config()
        .expect("a source with cleared optional keys is still usable");

    assert_eq!(config.user, None);
    assert_eq!(config.resource_id, None);
}

#[test]
fn the_refusal_reaches_evolution_as_a_tls_error_not_a_generic_one() {
    // Evolution shows the GError to the user; "not available in plain text"
    // is actionable, "other error" is not.
    let err = SourceError::InsecureTransport("jmap.example.com".into());
    let gerror = err.to_gerror();
    assert!(!gerror.is_null());

    // SAFETY: to_gerror returned a fresh GError we own.
    unsafe {
        assert_eq!((*gerror).domain, eds_sys::e_client_error_quark());
        assert_eq!(
            (*gerror).code,
            eds_sys::E_CLIENT_ERROR_TLS_NOT_AVAILABLE as i32
        );
        glib_sys::g_error_free(gerror);
    }

    let gerror = SourceError::MissingHost.to_gerror();
    // SAFETY: as above.
    unsafe {
        assert_eq!((*gerror).code, eds_sys::E_CLIENT_ERROR_INVALID_ARG as i32);
        glib_sys::g_error_free(gerror);
    }
}
