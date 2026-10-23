// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests the one credential choice that the backends and the account link share.

use std::ffi::{CString, c_char};
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, ESource, e_source_authentication_get_type,
    e_source_authentication_set_method, e_source_get_extension, e_source_new_with_uid,
};
use gobject_sys::g_object_unref;
use jmap_backend_core::api_token::API_TOKEN_METHOD;
use jmap_backend_core::connect::{AuthScheme, ConnectError, resolve_credentials};
use jmap_client::Credentials;
use jmap_tracing_capture::capture;

/// A live `ESource` with only a uid, as in `tests/oauth2.rs`.
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

    fn with_method(self, method: &str) -> Self {
        let method = CString::new(method).expect("no NUL in a test method");
        // SAFETY: a live source and EDS's own extension name. The `_get_type`
        // call registers the type, so the lookup creates a real extension.
        // The setter copies the string.
        unsafe {
            e_source_authentication_get_type();
            let auth =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
            e_source_authentication_set_method(auth, method.as_ptr() as *const c_char);
        }
        self
    }

    fn scheme(&self) -> AuthScheme {
        // SAFETY: a live source.
        unsafe { AuthScheme::of(self.0) }
    }

    fn resolve(
        &self,
        user: Option<&str>,
        password: impl FnOnce() -> Option<String>,
    ) -> Result<Credentials, ConnectError> {
        // SAFETY: a live source and a NULL cancellable.
        unsafe { resolve_credentials(self.0, self.scheme(), user, password, ptr::null_mut()) }
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: this owns the only reference `e_source_new_with_uid` returned.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

fn never_asked() -> Option<String> {
    panic!("the password must not be read for this scheme")
}

#[test]
fn the_scheme_is_read_off_the_source() {
    let api_token = API_TOKEN_METHOD.to_str().unwrap();
    assert_eq!(
        TestSource::new("scheme-absent").scheme(),
        AuthScheme::Password
    );
    let none = TestSource::new("scheme-none").with_method("none");
    assert_eq!(none.scheme(), AuthScheme::Password);
    let oauth2 = TestSource::new("scheme-oauth2").with_method("OAuth2");
    assert_eq!(oauth2.scheme(), AuthScheme::OAuth2);
    let token = TestSource::new("scheme-token").with_method(api_token);
    assert_eq!(token.scheme(), AuthScheme::ApiToken);
}

#[test]
fn a_password_account_logs_in_as_its_user() {
    let source = TestSource::new("resolve-password");

    let basic = source.resolve(Some("alice"), || Some("secret".into()));
    assert!(matches!(
        basic,
        Ok(Credentials::Basic { ref user, ref password }) if user == "alice" && password == "secret"
    ));
    let missing = source.resolve(Some("alice"), || None);
    assert!(matches!(missing, Err(ConnectError::CredentialsRequired)));
    let anonymous = source.resolve(None, || Some("secret".into()));
    assert!(matches!(anonymous, Ok(Credentials::None)));
}

#[test]
fn an_api_token_account_sends_the_stored_token_without_a_user() {
    let source = TestSource::new("resolve-token").with_method(API_TOKEN_METHOD.to_str().unwrap());

    let bearer = source.resolve(None, || Some("t0k3n".into()));
    assert!(matches!(bearer, Ok(Credentials::Bearer(ref token)) if token == "t0k3n"));
    let missing = source.resolve(Some("alice"), || None);
    assert!(matches!(missing, Err(ConnectError::CredentialsRequired)));
}

/// An OAuth 2.0 account never reads the stored password. The account link
/// looks it up in libsecret, a blocking call that this account does not need.
#[test]
fn an_oauth2_account_asks_eds_for_a_token_and_never_reads_the_password() {
    let source = TestSource::new("resolve-oauth2").with_method("OAuth2");

    let (result, _) = capture(|| source.resolve(Some("alice"), never_asked));
    assert!(matches!(result, Err(ConnectError::OAuth2(_))));
}
