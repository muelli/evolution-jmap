// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Which accounts authenticate with OAuth 2.0, and what happens when the token
// cannot be had.
//
// The rule under test is not this project's invention: it is transcribed from
// EDS's own `e_soup_session_setup_message_credentials` (3.52.3), the function
// every WebDAV/CalDAV/CardDAV account in Evolution authenticates through. It
// reads `[Authentication] Method` and sends a Bearer token when that string is
// `"OAuth2"` or the name of a registered `EOAuth2Service`, and Basic otherwise.
// A JMAP account has to answer the same question the same way, or an account
// the setup UI writes as OAuth2 would silently be asked for a password.

use std::ffi::{CString, c_char};
use std::ptr;

use eds_sys::{
    E_SOURCE_AUTHENTICATION_REQUIRED, E_SOURCE_EXTENSION_AUTHENTICATION, ESource,
    e_source_authentication_get_type, e_source_authentication_set_method, e_source_get_extension,
    e_source_new_with_uid,
};
use gobject_sys::g_object_unref;
use jmap_backend_core::connect::ConnectError;
use jmap_backend_core::oauth2::{access_token, method_is_oauth2, source_uses_oauth2};

/// A live `ESource` with nothing but a uid, plus whatever a test writes on it.
/// The same shape `jmap-config/tests/oauth2_service.rs` uses, and for the same
/// reason: an `ESource` is constructible without a registry, so the extension
/// readers can be driven for real rather than mocked.
struct TestSource(*mut ESource);

impl TestSource {
    fn new(uid: &str) -> Self {
        let uid = CString::new(uid).expect("no NUL in a test uid");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, no D-Bus object and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    /// Sets `[Authentication] Method`, or removes nothing when passed NULL —
    /// EDS has no unset state for this property, so "absent" is tested by
    /// never creating the extension at all (see `new` above).
    fn with_method(self, method: &str) -> Self {
        let method = CString::new(method).expect("no NUL in a test method");
        // SAFETY: a live source; the extension name is EDS's own constant and
        // the type is registered just above, so the lookup creates and returns
        // a real `ESourceAuthentication`. The setter copies the string.
        unsafe {
            e_source_authentication_get_type();
            let auth =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
            e_source_authentication_set_method(auth, method.as_ptr() as *const c_char);
        }
        self
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: this owns the only reference `e_source_new_with_uid` returned.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// EDS spells "this source authenticates with OAuth 2.0, by whichever service
/// claims it" as the literal string `OAuth2`, and it is the one value that
/// needs no registered service to be recognised.
#[test]
fn the_literal_oauth2_method_is_oauth2() {
    assert!(method_is_oauth2(Some("OAuth2")));
}

/// The three values `e_oauth2_services_can_check_auth_method` rejects outright,
/// plus the absent method. `"none"` is the important one: it is what a *fresh*
/// `ESourceAuthentication` reads back as — `jmap-config`'s `account` module
/// documents that the property has no unset state — so an ordinary
/// password account reaches this code as `Some("none")`, not as `None`, and
/// answering "yes, OAuth2" to it would break every account that exists today.
#[test]
fn the_password_methods_are_not_oauth2() {
    assert!(!method_is_oauth2(None));
    assert!(!method_is_oauth2(Some("")));
    assert!(!method_is_oauth2(Some("none")));
    assert!(!method_is_oauth2(Some("plain/password")));
}

/// A name no `EOAuth2Service` in this process can have. Deliberately not
/// `"JMAP"` — this crate's own service really is registered by
/// `module-jmap-backend.so`, which may be installed on the machine running the
/// tests, so asserting `"JMAP"` is *not* an alias would pass or fail depending
/// on what is in the EDS module directory.
#[test]
fn an_unregistered_service_name_is_not_oauth2() {
    assert!(!method_is_oauth2(Some("not-a-registered-oauth2-service")));
}

/// The same question asked the way `connect_with` asks it — off a real
/// `ESource` rather than off a string — including the case the extension is
/// absent entirely.
#[test]
fn the_method_is_read_off_the_source() {
    let absent = TestSource::new("jmap-oauth2-absent");
    // SAFETY: a live source.
    assert!(!unsafe { source_uses_oauth2(absent.0) });

    let password = TestSource::new("jmap-oauth2-password").with_method("none");
    // SAFETY: a live source.
    assert!(!unsafe { source_uses_oauth2(password.0) });

    let oauth2 = TestSource::new("jmap-oauth2-yes").with_method("OAuth2");
    // SAFETY: a live source.
    assert!(unsafe { source_uses_oauth2(oauth2.0) });
}

/// A source that is not backed by the registry — which is every source a test
/// builds, and also a real one whose account has never been consented to — has
/// no token to give. What matters is that this is an ordinary `Err` rather
/// than a crash, and that it asks Evolution to authenticate rather than
/// telling it the stored credentials were wrong: for an OAuth 2.0 source,
/// `REQUIRED` is what opens the consent window, and `REJECTED` would throw
/// away a refresh token that a transient failure had not invalidated.
#[test]
fn a_source_with_no_token_is_a_required_not_a_rejected() {
    let source = TestSource::new("jmap-oauth2-no-token").with_method("OAuth2");

    // SAFETY: a live source and a NULL cancellable, which is what an EDS vfunc
    // may be handed.
    let error = unsafe { access_token(source.0, ptr::null_mut()) }.expect_err("no token exists");

    assert!(matches!(error, ConnectError::OAuth2(_)));
    assert_eq!(error.auth_result(), E_SOURCE_AUTHENTICATION_REQUIRED);
}
