// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Which mail accounts authenticate with OAuth 2.0.
//
// This is the Camel-side sibling of `jmap-backend-core/tests/oauth2.rs`. It
// asks the same question of `CamelNetworkSettings:auth-mechanism` instead of
// an `ESource`'s `[Authentication] Method`, because that is the field Camel
// keeps it on. See `jmap_mail::oauth2`'s module docs for why the two fields
// must agree. This file does not repeat `jmap_backend_core::oauth2`'s own
// coverage of `method_is_oauth2` itself (the literal, the alias lookup, the
// values that are not OAuth 2.0). This file only proves that this crate
// reads the right field into that same, already-tested, function.
//
// Fetching a token itself (`oauth2::access_token`) is not driven by a test
// here, unlike `jmap-backend-core/tests/oauth2.rs`, which drives
// `e_source_get_oauth2_access_token_sync` against a bare `ESource`.
// `camel_session_get_oauth2_access_token_sync` is a `CamelSessionClass`
// vtable slot that the base `CamelSession` leaves unimplemented. Only
// `EMailSession`, Evolution's own subclass, fills it in. This is confirmed
// by reading `libemail-engine/e-mail-session.c` upstream. It relays
// straight to `e_source_get_oauth2_access_token_sync`, which that EDS-side
// test already covers. Calling it on the plain `CamelSession` this crate's
// other tests construct (see `tests/common`) would not exercise a failure
// path. It would trip Camel's own `g_return_val_if_fail` on a NULL vfunc,
// a property of that test double, not of production Evolution.

use std::ffi::CString;
use std::ptr;

use eds_sys::{
    CamelNetworkSettings, CamelSettings, camel_network_settings_set_auth_mechanism,
    camel_network_settings_set_host,
};
use gobject_sys::{g_object_new, g_object_unref};
use jmap_mail::oauth2::uses_oauth2;
use jmap_mail::settings::settings_type;

/// A settings object as `settings_type` constructs it, with `auth-mechanism`
/// set the way the account editor's combo box would leave it. For the
/// ordinary password account, `auth-mechanism` stays unset.
fn settings(mechanism: Option<&str>) -> *mut CamelSettings {
    // SAFETY: the type is registered by `settings_type` and has no construct
    // properties of its own; the accessors below take an instance of it.
    unsafe {
        let object = g_object_new(settings_type(), ptr::null());
        assert!(!object.is_null(), "g_object_new returned NULL");
        let network = object.cast::<CamelNetworkSettings>();
        // A host, so this could plausibly be a configured account rather
        // than an obviously-unset one. `uses_oauth2` does not read this
        // field. Asserting nothing about it is the point.
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
fn an_account_with_no_auth_mechanism_set_is_not_oauth2() {
    let settings = settings(None);
    // SAFETY: a live settings object, freed below.
    assert!(!unsafe { uses_oauth2(settings) });
    // SAFETY: this test owns the only reference.
    unsafe { free(settings) };
}

#[test]
fn a_plain_password_mechanism_is_not_oauth2() {
    let settings = settings(Some("PLAIN"));
    // SAFETY: as above.
    assert!(!unsafe { uses_oauth2(settings) });
    // SAFETY: as above.
    unsafe { free(settings) };
}

/// The literal spelling that `jmap-config`'s own `Authentication` combo
/// writes for this project's OAuth 2.0 choice. It is also the spelling
/// EDS's `e_oauth2_services_is_oauth2_alias_static` recognises with no
/// service needing to be registered. See `jmap_backend_core::oauth2`'s
/// tests for the rest of that rule, which this crate reuses rather than
/// repeats.
#[test]
fn the_oauth2_method_is_read_off_the_auth_mechanism_field() {
    let settings = settings(Some("OAuth2"));
    // SAFETY: as above.
    assert!(unsafe { uses_oauth2(settings) });
    // SAFETY: as above.
    unsafe { free(settings) };
}

/// A backend without settings cannot be configured either way. `uses_oauth2`
/// answers "no" for it, the same graceful non-crash that `jmap-mail::server`'s
/// `network` already gives for every other accessor on a NULL settings
/// object.
#[test]
fn a_null_settings_object_is_not_oauth2() {
    // SAFETY: a NULL settings pointer is documented as accepted.
    assert!(!unsafe { uses_oauth2(ptr::null_mut()) });
}
