// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Which mail accounts authenticate with a manually pasted API token.
//
// This is the Camel-side sibling of `jmap-backend-core/tests/api_token.rs`.
// It matches `tests/oauth2.rs`, the sibling of
// `jmap-backend-core/tests/oauth2.rs`: same field
// (`CamelNetworkSettings:auth-mechanism`), same reason (Camel stores the
// value there). This file does not retest `method_is_api_token` itself (the
// literal value, and the values that are not it). It only proves that this
// crate reads the correct field into that already-tested function.

use std::ffi::CString;
use std::ptr;
use std::sync::Once;

use eds_sys::{
    CamelNetworkSettings, CamelSettings, camel_network_settings_set_auth_mechanism,
    camel_network_settings_set_host, camel_offline_settings_get_type,
};
use glib_sys::{G_LOG_LEVEL_CRITICAL, G_LOG_LEVEL_WARNING, g_log_set_always_fatal};
use gobject_sys::{g_object_new, g_object_unref};
use jmap_mail::api_token::uses_api_token;
use jmap_mail::settings::settings_type;

/// A settings object as `settings_type` constructs it. `auth-mechanism` is
/// set the way the account editor's combo box sets it, or left unset for
/// an ordinary password account.
fn settings(mechanism: Option<&str>) -> *mut CamelSettings {
    // SAFETY: `settings_type` registers the type and defines no construct
    // properties. The accessors below take an instance of this type.
    unsafe {
        let object = g_object_new(settings_type(), ptr::null());
        assert!(!object.is_null(), "g_object_new returned NULL");
        let network = object.cast::<CamelNetworkSettings>();
        // A host is set here, so this looks like a configured account, not
        // an obviously unset one. `uses_api_token` does not read this field.
        // The test asserts nothing about a field the function does not read.
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

/// The literal string that `jmap-config`'s `Authentication` combo box writes
/// for this project's API-token choice.
#[test]
fn the_api_token_method_is_read_off_the_auth_mechanism_field() {
    let settings = settings(Some("bearer"));
    // SAFETY: as above.
    assert!(unsafe { uses_api_token(settings) });
    // SAFETY: as above.
    unsafe { free(settings) };
}

/// A NULL settings pointer cannot be configured either way. `uses_api_token`
/// returns `false` for it, the same graceful result that `jmap-mail::server`'s
/// `network` already gives every other accessor for a NULL settings object.
#[test]
fn a_null_settings_object_is_not_api_token() {
    // SAFETY: a NULL settings pointer is documented as accepted.
    assert!(!unsafe { uses_api_token(ptr::null_mut()) });
}

/// Configure GLib critical and warning logs to abort the test process.
fn glib_complaints_are_failures() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: no preconditions; the call returns the previous mask.
        unsafe { g_log_set_always_fatal(G_LOG_LEVEL_CRITICAL | G_LOG_LEVEL_WARNING) };
    });
}

/// Settings of another class are not API-token settings. The answer must come
/// without a critical from Camel's accessor, so a critical aborts this test.
#[test]
fn settings_that_carry_no_network_are_not_api_token() {
    glib_complaints_are_failures();
    // SAFETY: `CamelOfflineSettings` is instantiable and has no construct
    // properties. It does not implement `CamelNetworkSettings`.
    let settings = unsafe { g_object_new(camel_offline_settings_get_type(), ptr::null()) };
    assert!(!settings.is_null(), "g_object_new returned NULL");
    let settings = settings.cast::<CamelSettings>();
    // SAFETY: a live settings object, freed below.
    assert!(!unsafe { uses_api_token(settings) });
    // SAFETY: this test owns the only reference.
    unsafe { free(settings) };
}
