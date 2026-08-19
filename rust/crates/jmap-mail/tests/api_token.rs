// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Which mail accounts authenticate with a manually pasted API token.
//
// The Camel-side sibling of `jmap-backend-core/tests/api_token.rs`, exactly
// the way `tests/oauth2.rs` is the sibling of `jmap-backend-core/tests/
// oauth2.rs` — same field (`CamelNetworkSettings:auth-mechanism`), same
// reason (that is where Camel keeps it). `method_is_api_token` itself (the
// literal, the values that are not it) is not re-covered here; this file
// only proves this crate reads the right field into that already-tested
// function.

use std::ffi::CString;
use std::ptr;

use eds_sys::{
    CamelNetworkSettings, CamelSettings, camel_network_settings_set_auth_mechanism,
    camel_network_settings_set_host,
};
use gobject_sys::{g_object_new, g_object_unref};
use jmap_mail::api_token::uses_api_token;
use jmap_mail::settings::settings_type;

/// A settings object as `settings_type` constructs it, with `auth-mechanism`
/// set the way the account editor's combo box would leave it — or left
/// unset, for the ordinary password account.
fn settings(mechanism: Option<&str>) -> *mut CamelSettings {
    // SAFETY: the type is registered by `settings_type` and has no construct
    // properties of its own; the accessors below take an instance of it.
    unsafe {
        let object = g_object_new(settings_type(), ptr::null());
        assert!(!object.is_null(), "g_object_new returned NULL");
        let network = object.cast::<CamelNetworkSettings>();
        // A host, so this could plausibly be a configured account rather
        // than an obviously-unset one — not read by `uses_api_token`, but
        // asserting nothing about the field it does not read is the point.
        camel_network_settings_set_host(network, c"jmap.example.com".as_ptr());
        if let Some(mechanism) = mechanism {
            let mechanism = CString::new(mechanism).expect("no NUL in a test mechanism");
            camel_network_settings_set_auth_mechanism(network, mechanism.as_ptr());
        }
        object.cast::<CamelSettings>()
    }
}

/// # Safety
///
/// `settings` must be a valid `CamelSettings` this test owns the only
/// reference to.
unsafe fn free(settings: *mut CamelSettings) {
    // SAFETY: the contract above.
    unsafe { g_object_unref(settings.cast()) };
}

#[test]
fn an_account_with_no_auth_mechanism_set_is_not_api_token() {
    let settings = settings(None);
    // SAFETY: a live settings object, freed below.
    assert!(!unsafe { uses_api_token(settings) });
    // SAFETY: this test owns the only reference.
    unsafe { free(settings) };
}

#[test]
fn an_oauth2_mechanism_is_not_api_token() {
    let settings = settings(Some("OAuth2"));
    // SAFETY: as above.
    assert!(!unsafe { uses_api_token(settings) });
    // SAFETY: as above.
    unsafe { free(settings) };
}

/// The literal spelling `jmap-config`'s own `Authentication` combo writes for
/// this project's API-token choice.
#[test]
fn the_api_token_method_is_read_off_the_auth_mechanism_field() {
    let settings = settings(Some("bearer"));
    // SAFETY: as above.
    assert!(unsafe { uses_api_token(settings) });
    // SAFETY: as above.
    unsafe { free(settings) };
}

/// A backend without settings cannot be configured either way; `uses_api_token`
/// answering "no" for it is the same graceful non-crash `jmap-mail::server`'s
/// `network` already gives every other accessor a NULL settings object could
/// reach.
#[test]
fn a_null_settings_object_is_not_api_token() {
    // SAFETY: a NULL settings pointer is documented as accepted.
    assert!(!unsafe { uses_api_token(ptr::null_mut()) });
}
