// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests which accounts authenticate with OAuth 2.0, and what occurs when no
// token is available.
//
// This project did not invent the rule under test. It is a copy of EDS's
// `e_soup_session_setup_message_credentials` (3.52.3). Every WebDAV/CalDAV/
// CardDAV account in Evolution authenticates through that function. It reads
// `[Authentication] Method` and sends a Bearer token when that string is
// `"OAuth2"` or the name of a registered `EOAuth2Service`, and Basic otherwise.
// A JMAP account must give the same answer. If not, Evolution silently asks
// for a password for an account that the setup UI writes as OAuth2.

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
use jmap_tracing_capture::capture;

/// A live `ESource` with only a uid, plus the values that a test writes on it.
/// `jmap-config/tests/oauth2_service.rs` uses the same shape for the same
/// reason: you can construct an `ESource` without a registry, so the tests
/// call the real extension readers, not mocks.
struct TestSource(*mut ESource);

impl TestSource {
    fn new(uid: &str) -> Self {
        let uid = CString::new(uid).expect("no NUL in a test uid");
        let mut error = ptr::null_mut();
        // SAFETY: the documented arguments are a NUL-terminated uid, no D-Bus
        // object and a GError out-parameter.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    /// Sets `[Authentication] Method`. EDS has no unset state for this
    /// property, so the tests make "absent" by never creating the extension
    /// (see `new` above).
    fn with_method(self, method: &str) -> Self {
        let method = CString::new(method).expect("no NUL in a test method");
        // SAFETY: a live source. The extension name is EDS's own constant,
        // and the `_get_type` call registers the type first. Thus the lookup
        // creates and returns a real `ESourceAuthentication`. The setter
        // copies the string.
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

/// EDS writes "this source authenticates with OAuth 2.0, through any service
/// that claims it" as the literal string `OAuth2`. It is the only value that
/// the lookup recognises without a registered service.
#[test]
fn the_literal_oauth2_method_is_oauth2() {
    assert!(method_is_oauth2(Some("OAuth2")));
}

/// The three values that `e_oauth2_services_can_check_auth_method` always
/// rejects, plus the absent method. `"none"` is the important value: a *fresh*
/// `ESourceAuthentication` reads back as `"none"`. (`jmap-config`'s `account`
/// module documents that the property has no unset state.) Thus an ordinary
/// password account comes to this code as `Some("none")`, not as `None`. The
/// answer "yes, OAuth2" for it would break every account that exists today.
#[test]
fn the_password_methods_are_not_oauth2() {
    assert!(!method_is_oauth2(None));
    assert!(!method_is_oauth2(Some("")));
    assert!(!method_is_oauth2(Some("none")));
    assert!(!method_is_oauth2(Some("plain/password")));
}

/// A name that no `EOAuth2Service` in this process can have. It is
/// intentionally not `"JMAP"`. `module-jmap-backend.so` registers this crate's
/// own service, and that module can be installed on the test machine. Thus an
/// assertion that `"JMAP"` is *not* an alias passes or fails because of the
/// contents of the EDS module directory.
#[test]
fn an_unregistered_service_name_is_not_oauth2() {
    assert!(!method_is_oauth2(Some("not-a-registered-oauth2-service")));
}

/// The same question, asked as `connect_with` asks it: from a real `ESource`,
/// not from a string. This includes the case where the extension is absent.
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

/// A source without a registry has no token to give. Every source that a test
/// makes is such a source. So is a real source whose account has no consent
/// yet. The result must be an ordinary `Err`, not a crash. It must also ask
/// Evolution to authenticate, and not tell it that the stored credentials are
/// wrong. For an OAuth 2.0 source, `REQUIRED` opens the consent window.
/// `REJECTED` discards a refresh token that a transient failure did not make
/// invalid.
#[test]
fn a_source_with_no_token_is_a_required_not_a_rejected() {
    let source = TestSource::new("jmap-oauth2-no-token").with_method("OAuth2");

    // SAFETY: a live source and a NULL cancellable. An EDS vfunc can get a
    // NULL cancellable.
    let (result, _) = capture(|| unsafe { access_token(source.0, ptr::null_mut()) });
    let error = result.expect_err("no token exists");

    assert!(matches!(error, ConnectError::OAuth2(_)));
    assert_eq!(error.auth_result(), E_SOURCE_AUTHENTICATION_REQUIRED);

    let gerror = error.to_gerror();
    assert!(!gerror.is_null());
    unsafe {
        assert_eq!((*gerror).domain, eds_sys::e_client_error_quark());
        assert_eq!(
            (*gerror).code,
            eds_sys::E_CLIENT_ERROR_AUTHENTICATION_REQUIRED as i32
        );
        glib_sys::g_error_free(gerror);
    }
}

/// Compare with `a_source_with_no_token_is_a_required_not_a_rejected`: a secret
/// store failure must map to ERROR (never REQUIRED) and have the DBUS_ERROR
/// client code.
#[test]
fn an_oauth2_secret_store_failure_surfaces_error_and_dbus_client_error() {
    let error = ConnectError::SecretStore("the login keyring is locked".to_owned());
    assert_ne!(error.auth_result(), E_SOURCE_AUTHENTICATION_REQUIRED);
    assert_eq!(error.auth_result(), eds_sys::E_SOURCE_AUTHENTICATION_ERROR);

    let gerror = error.to_gerror();
    assert!(!gerror.is_null());
    unsafe {
        assert_eq!((*gerror).domain, eds_sys::e_client_error_quark());
        assert_eq!((*gerror).code, eds_sys::E_CLIENT_ERROR_DBUS_ERROR as i32);
        let message = jmap_backend_core::marshal::read_string((*gerror).message).unwrap();
        assert!(message.contains("the login keyring is locked"), "{message}");
        glib_sys::g_error_free(gerror);
    }
}

#[test]
fn access_token_traces_account_id_on_attempt() {
    let source = TestSource::new("jmap-oauth2-trace-uid").with_method("OAuth2");

    // SAFETY: a live source and NULL cancellable.
    let (_, captured) = capture(|| unsafe { access_token(source.0, ptr::null_mut()) });
    assert!(
        captured
            .iter()
            .any(|(_, name, value)| name == "account_id"
                && value == "Some(\"jmap-oauth2-trace-uid\")"),
        "expected account_id field, got {captured:?}"
    );
}

#[test]
fn access_token_failure_traces_structured_fields_and_consent_escalation() {
    let source = TestSource::new("jmap-oauth2-trace-failure").with_method("OAuth2");

    // SAFETY: a live source and NULL cancellable.
    let (result, captured) = capture(|| unsafe { access_token(source.0, ptr::null_mut()) });
    assert!(result.is_err());
    assert!(
        captured.iter().any(|(_, name, value)| name == "account_id"
            && value == "Some(\"jmap-oauth2-trace-failure\")"),
        "expected account_id field, got {captured:?}"
    );
    assert!(
        captured.iter().any(|(_, name, _)| name == "reason"),
        "expected reason field, got {captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|(_, name, _)| name == "escalates_to_consent"),
        "expected escalates_to_consent field, got {captured:?}"
    );
    assert!(
        captured.iter().any(|(_, name, _)| name == "error_code"),
        "expected error_code field, got {captured:?}"
    );
    assert!(
        captured.iter().any(|(_, name, _)| name == "store_failure"),
        "expected store_failure field, got {captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|(_, name, _)| name == "secret_not_found"),
        "expected secret_not_found field, got {captured:?}"
    );
}
