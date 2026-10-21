// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// These tests check which accounts authenticate with a manually pasted API
// token, sent as `Authorization: Bearer`. This is the third choice, beside
// the OAuth 2.0 of `tests/oauth2.rs` and the ordinary Basic path.
//
// EDS does not recognise this spelling as it recognises `"OAuth2"`. The
// string must only differ from each OAuth 2.0 alias and from the `"none"`
// default of `ESourceAuthentication`. Thus a password account or an OAuth 2.0
// account never looks like an account that pastes a token.

use std::ffi::{CString, c_char};
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, ESource, e_source_authentication_get_type,
    e_source_authentication_set_method, e_source_get_extension, e_source_has_extension,
    e_source_new_with_uid,
};
use glib_sys::GFALSE;
use gobject_sys::g_object_unref;
use jmap_backend_core::api_token::{API_TOKEN_METHOD, method_is_api_token, source_uses_api_token};

/// This is the same fixture shape as in `tests/oauth2.rs`, for the same
/// reason. You can construct an `ESource` without a registry, so the tests
/// use the real extension readers, not mocks.
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
        // SAFETY: the source is live. The extension name is an EDS constant
        // and the first call registers the type, so the lookup creates and
        // returns a real `ESourceAuthentication`. The setter copies the
        // string.
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

#[test]
fn the_literal_api_token_method_is_api_token() {
    assert!(method_is_api_token(Some(
        API_TOKEN_METHOD.to_str().unwrap()
    )));
}

#[test]
fn the_other_methods_are_not_api_token() {
    assert!(!method_is_api_token(None));
    assert!(!method_is_api_token(Some("")));
    assert!(!method_is_api_token(Some("none")));
    assert!(!method_is_api_token(Some("OAuth2")));
}

/// This asks the same question as `connect_with` does: from a real `ESource`,
/// not from a string. It includes the case where the extension is absent.
#[test]
fn the_method_is_read_off_the_source() {
    let absent = TestSource::new("jmap-api-token-absent");
    // SAFETY: a live source.
    assert!(!unsafe { source_uses_api_token(absent.0) });

    let password = TestSource::new("jmap-api-token-password").with_method("none");
    // SAFETY: a live source.
    assert!(!unsafe { source_uses_api_token(password.0) });

    let oauth2 = TestSource::new("jmap-api-token-oauth2").with_method("OAuth2");
    // SAFETY: a live source.
    assert!(!unsafe { source_uses_api_token(oauth2.0) });

    let token =
        TestSource::new("jmap-api-token-yes").with_method(API_TOKEN_METHOD.to_str().unwrap());
    // SAFETY: a live source.
    assert!(unsafe { source_uses_api_token(token.0) });
}

/// The read must not add an `[Authentication]` group to a source that has none.
#[test]
fn the_read_does_not_add_the_extension_to_a_source_without_it() {
    let absent = TestSource::new("jmap-api-token-untouched");
    // SAFETY: a live source.
    assert!(!unsafe { source_uses_api_token(absent.0) });
    // SAFETY: a live source and an EDS constant.
    let added =
        unsafe { e_source_has_extension(absent.0, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()) };
    assert_eq!(added, GFALSE, "the read created the extension");
}
