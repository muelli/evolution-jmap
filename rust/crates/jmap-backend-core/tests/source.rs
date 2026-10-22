// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The `ESource` that an EDS backend gets is its *only* description of the
// account. Every failure here is silent. A host read from the wrong extension
// gives an address book that never connects. A plaintext origin that nobody
// sees sends Basic credentials over the network unencrypted. Thus these tests
// use a real `ESource` made with the EDS setters, not a hand-made struct.

use std::ffi::CString;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_RESOURCE, E_SOURCE_EXTENSION_SECURITY,
    ESource, ESourceAuthentication, ESourceResource, ESourceSecurity,
    e_source_authentication_set_host, e_source_authentication_set_port,
    e_source_authentication_set_user, e_source_get_extension, e_source_has_extension,
    e_source_new_with_uid, e_source_resource_set_identity, e_source_security_set_secure,
};
use glib_sys::GFALSE;
use gobject_sys::g_object_unref;
use jmap_backend_core::source::{ConnectTarget, SourceConfig, SourceError, destination_address};

/// An `ESource` that is not backed by the registry. EDS itself uses
/// `e_source_new_with_uid` with a NULL D-Bus object for a source read from a
/// keyfile. Thus the extensions behave as they do in a backend.
struct TestSource(*mut ESource);

impl TestSource {
    fn new() -> Self {
        let uid = CString::new("jmap-test-source").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: the documented arguments are a NUL-terminated uid, a NULL
        // D-Bus object and a GError out-parameter.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    fn authentication(&self) -> *mut ESourceAuthentication {
        // SAFETY: the source is live and the name is a header constant. The
        // source creates the extension on demand and owns it.
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

    fn has_extension(&self, name: &std::ffi::CStr) -> bool {
        // SAFETY: a live source and a NUL-terminated name.
        unsafe { e_source_has_extension(self.0, name.as_ptr()) != GFALSE }
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: this struct holds the only reference.
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

    // No port named: RFC 8620 §2.2 SRV autodiscovery applies.
    assert_eq!(
        config.target,
        ConnectTarget::Domain("jmap.example.com".into())
    );
    assert_eq!(config.user.as_deref(), Some("vera@example.com"));
    assert_eq!(config.resource_id.as_deref(), Some("Ab1"));
}

#[test]
fn security_defaults_to_tls_when_the_source_never_mentions_it() {
    // A source written by hand (the documented manual test recipe) can have
    // no [Security] group. `ESourceSecurity:secure` defaults to FALSE. Thus an
    // unconditional read of the extension silently changes every such account
    // to plain HTTP.
    let config = TestSource::new()
        .host("jmap.example.com")
        .config()
        .expect("a source with no [Security] group is secure");

    assert_eq!(
        config.target,
        ConnectTarget::Domain("jmap.example.com".into())
    );
}

#[test]
fn reading_a_source_with_no_authentication_group_does_not_create_one() {
    // A source with no `[Authentication]` group yet. The test does not call
    // `.host()`/`.user()`/`.port()`, because they get the extension and thus
    // create it. `from_source` must fail (there is nothing to connect to), and
    // its read must not create a group.
    let source = TestSource::new();

    source
        .config()
        .expect_err("a source with no host is invalid");

    assert!(
        !source.has_extension(E_SOURCE_EXTENSION_AUTHENTICATION),
        "reading a source must not create the [Authentication] group it lacks"
    );
}

#[test]
fn reading_a_source_with_no_resource_group_does_not_create_one() {
    // A source with a host but no `[Resource]` group: the "default address
    // book/calendar" case in the module docs of `from_source`. A read of its
    // (absent) resource identity must not create the group.
    let source = TestSource::new().host("jmap.example.com").secure(true);

    let config = source
        .config()
        .expect("a source with a host and no [Resource] group is valid");

    assert_eq!(config.resource_id, None);
    assert!(
        !source.has_extension(E_SOURCE_EXTENSION_RESOURCE),
        "reading a source must not create the [Resource] group it lacks"
    );
}

#[test]
fn an_explicit_security_group_still_wins() {
    // The opposite case: someone wrote "secure=false" intentionally. It must
    // not be read as "unspecified".
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

    // A named port states an endpoint, not a domain to autodiscover.
    assert_eq!(
        config.target,
        ConnectTarget::Origin("https://jmap.example.com:8443".into())
    );
}

#[test]
fn port_zero_means_unset_and_is_srv_eligible() {
    let config = TestSource::new()
        .host("jmap.example.com")
        .secure(true)
        .port(0)
        .config()
        .expect("a complete https source is valid");

    assert_eq!(
        config.target,
        ConnectTarget::Domain("jmap.example.com".into())
    );
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
    // The mock server and all local development instances use http on
    // 127.0.0.1. If the backend refuses that, nobody can test it.
    for host in ["localhost", "127.0.0.1", "127.0.0.2", "::1"] {
        let config = TestSource::new()
            .host(host)
            .secure(false)
            .port(8080)
            .config()
            .unwrap_or_else(|e| panic!("{host} should be reachable in the clear: {e}"));
        let ConnectTarget::Origin(origin) = &config.target else {
            panic!(
                "an explicit port names an endpoint, not a domain: {:?}",
                config.target
            );
        };
        assert!(origin.starts_with("http://"), "{origin}");
        assert!(origin.ends_with(":8080"), "{origin}");
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

    assert_eq!(
        config.target,
        ConnectTarget::Origin("https://[2001:db8::1]:8443".into())
    );
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
    // The origin is a string concatenation. A host with its own separators
    // lets a `.source` file send the client to a different place, including
    // a plaintext endpoint that the TLS check does not see.
    for host in [
        "jmap.example.com/../evil",
        "evil.example.com#jmap.example.com",
        "http://evil.example.com",
        "jmap.example.com:80",
        // EDS removes surrounding whitespace in the property setter, so it
        // never gets here. But an interior space passes through unchanged.
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
    // A key that the user cleared reads back as absent, not as "": `ESource`
    // normalises the empty string to NULL in the setter. The reader needs
    // this. Without it, the server gets an empty address book id as a filter
    // that matches nothing.
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
    // Evolution shows the GError to the user. The user can act on "not
    // available in plain text", but not on "other error".
    let err = SourceError::InsecureTransport("jmap.example.com".into());
    let gerror = err.to_gerror();
    assert!(!gerror.is_null());

    // SAFETY: to_gerror returned a fresh GError that this test owns.
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

#[test]
fn destination_address_reads_the_explicit_host_and_port() {
    let source = TestSource::new().host("jmap.example.com").port(8443);

    // SAFETY: a live source, only read from.
    let address = unsafe { destination_address(source.0) };

    assert_eq!(address, Some(("jmap.example.com".into(), 8443)));
}

#[test]
fn destination_address_defaults_an_unset_port_to_443() {
    // A bare-domain, SRV-eligible account (`ConnectTarget::Domain`) never has
    // a port in its `[Authentication]` group. EDS reads that back as 0, "not
    // set". But a network monitor that watches port 0 learns nothing.
    let source = TestSource::new().host("jmap.example.com");

    // SAFETY: as above.
    let address = unsafe { destination_address(source.0) };

    assert_eq!(address, Some(("jmap.example.com".into(), 443)));
}

#[test]
fn destination_address_is_none_without_a_host() {
    let source = TestSource::new().user("vera@example.com");

    // SAFETY: as above.
    let address = unsafe { destination_address(source.0) };

    assert_eq!(address, None);
}

#[test]
fn destination_address_does_not_create_an_authentication_group_it_lacks() {
    let source = TestSource::new();

    // SAFETY: as above.
    let address = unsafe { destination_address(source.0) };

    assert_eq!(address, None);
    assert!(
        !source.has_extension(E_SOURCE_EXTENSION_AUTHENTICATION),
        "reading a source must not create the [Authentication] group it lacks"
    );
}

#[test]
fn an_identity_with_an_account_names_a_collection_in_that_account() {
    // A calendar or address book that another user shares is in that user's
    // account, so the collection writes `<accountId>/<collectionId>`. JMAP
    // ids never contain `/` (RFC 8620 §1.2), so the split is unambiguous.
    let config = TestSource::new()
        .host("jmap.example.com")
        .identity("A2/C1")
        .config()
        .expect("a shared collection's source is valid");

    assert_eq!(config.account_id(), Some("A2"));
    assert_eq!(config.collection_id(), Some("C1"));
}

#[test]
fn a_bare_identity_names_a_collection_in_the_primary_account() {
    let config = TestSource::new()
        .host("jmap.example.com")
        .identity("Ab1")
        .config()
        .expect("a valid source");

    assert_eq!(config.account_id(), None);
    assert_eq!(config.collection_id(), Some("Ab1"));
}

#[test]
fn a_malformed_identity_is_kept_whole_rather_than_guessed_at() {
    // Kept whole, so the server refuses it under the name in the keyfile.
    // The backend must not silently open a different collection.
    for identity in ["/C1", "A2/", "A2/C1/X", "/"] {
        let config = TestSource::new()
            .host("jmap.example.com")
            .identity(identity)
            .config()
            .expect("a valid source");

        assert_eq!(config.account_id(), None, "{identity}");
        assert_eq!(config.collection_id(), Some(identity), "{identity}");
    }
}

#[test]
fn a_source_without_an_identity_names_no_account_and_no_collection() {
    let config = TestSource::new()
        .host("jmap.example.com")
        .config()
        .expect("a valid source");

    assert_eq!(config.account_id(), None);
    assert_eq!(config.collection_id(), None);
}

/// A source that two threads share, as the backend's worker thread and the
/// main loop share it in EDS.
struct Shared(*mut ESource);
// SAFETY: an `ESource` and its extensions guard their state with a lock.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

/// Runs `read` on this thread while another thread calls `write` 20,000
/// times with the two long values in turn. Each `read` must give one of
/// `allowed`. A borrowed getter copies freed memory and gives a torn value.
fn read_while_another_thread_writes(
    source: &TestSource,
    values: [String; 2],
    write: unsafe fn(*mut ESource, *const std::ffi::c_char),
    read: impl Fn(*mut ESource) -> Option<String>,
    allowed: &[Option<String>],
) {
    let shared = std::sync::Arc::new(Shared(source.0));
    let names: Vec<CString> = values
        .iter()
        .map(|value| CString::new(value.as_str()).expect("no NUL"))
        .collect();
    let writer = {
        let shared = std::sync::Arc::clone(&shared);
        std::thread::spawn(move || {
            for round in 0..20_000 {
                // SAFETY: a live source and a NUL-terminated value.
                unsafe { write(shared.0, names[round % 2].as_ptr()) };
            }
        })
    };
    while !writer.is_finished() {
        let value = read(shared.0);
        assert!(
            allowed.contains(&value) || values.iter().any(|v| value.as_deref() == Some(v)),
            "a torn value of {:?} bytes",
            value.map(|v| v.len())
        );
    }
    writer.join().expect("writer");
}

fn long_values(stem: &str) -> [String; 2] {
    ["a".repeat(4000) + stem, "b".repeat(9000) + stem]
}

#[test]
fn from_source_on_another_thread_copies_a_user_the_main_thread_is_replacing() {
    let source = TestSource::new()
        .host("jmap.example.com")
        .user("vera@example.com");
    unsafe fn set_user(source: *mut ESource, user: *const std::ffi::c_char) {
        // SAFETY: a live source and a NUL-terminated user, by the contract.
        unsafe {
            let auth = e_source_get_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr());
            e_source_authentication_set_user(auth.cast(), user);
        }
    }
    read_while_another_thread_writes(
        &source,
        long_values("@example.com"),
        set_user,
        // SAFETY: a live source.
        |source| {
            unsafe { SourceConfig::from_source(source) }
                .expect("a valid source")
                .user
        },
        &[Some("vera@example.com".into())],
    );
}

#[test]
fn from_source_on_another_thread_copies_an_identity_the_main_thread_is_replacing() {
    let source = TestSource::new().host("jmap.example.com").identity("Ab1");
    unsafe fn set_identity(source: *mut ESource, identity: *const std::ffi::c_char) {
        // SAFETY: a live source and a NUL-terminated identity, by the contract.
        unsafe {
            let resource = e_source_get_extension(source, E_SOURCE_EXTENSION_RESOURCE.as_ptr());
            e_source_resource_set_identity(resource.cast(), identity);
        }
    }
    read_while_another_thread_writes(
        &source,
        long_values("Ab1"),
        set_identity,
        // SAFETY: a live source.
        |source| {
            unsafe { SourceConfig::from_source(source) }
                .expect("a valid source")
                .resource_id
        },
        &[Some("Ab1".into())],
    );
}

#[test]
fn destination_address_on_another_thread_copies_a_host_the_main_thread_is_replacing() {
    let source = TestSource::new().host("jmap.example.com");
    unsafe fn set_host(source: *mut ESource, host: *const std::ffi::c_char) {
        // SAFETY: a live source and a NUL-terminated host, by the contract.
        unsafe {
            let auth = e_source_get_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr());
            e_source_authentication_set_host(auth.cast(), host);
        }
    }
    read_while_another_thread_writes(
        &source,
        long_values(".example"),
        set_host,
        // SAFETY: a live source.
        |source| unsafe { destination_address(source) }.map(|(host, _)| host),
        &[Some("jmap.example.com".into())],
    );
}
