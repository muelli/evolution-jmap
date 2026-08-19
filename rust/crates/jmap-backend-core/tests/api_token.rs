// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Which accounts authenticate with a manually pasted API token, sent as
// `Authorization: Bearer` — the third choice beside `tests/oauth2.rs`'s
// OAuth 2.0 and the ordinary Basic path.
//
// This is not a spelling EDS itself recognises the way `"OAuth2"` is: it
// only has to be a string that is neither an OAuth 2.0 alias nor
// `ESourceAuthentication`'s own `"none"` default, so an ordinary password
// account and an OAuth 2.0 account are never mistaken for one that pastes a
// token.

use std::ffi::{CString, c_char};
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, ESource, e_source_authentication_get_type,
    e_source_authentication_set_method, e_source_get_extension, e_source_new_with_uid,
};
use gobject_sys::g_object_unref;
use jmap_backend_core::api_token::{API_TOKEN_METHOD, method_is_api_token, source_uses_api_token};

/// The same fixture shape `tests/oauth2.rs` uses, and for the same reason: an
/// `ESource` is constructible without a registry, so the extension readers
/// can be driven for real rather than mocked.
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
        // SAFETY: a live source; the extension name is EDS's own constant and
        // the type is registered just above, so the lookup creates and
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

/// The same question asked the way `connect_with` asks it — off a real
/// `ESource` rather than off a string — including the case the extension is
/// absent entirely.
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
