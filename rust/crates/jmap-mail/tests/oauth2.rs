// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Which mail accounts authenticate with OAuth 2.0.
//
// The Camel-side sibling of `jmap-backend-core/tests/oauth2.rs`: the same
// question, asked of `CamelNetworkSettings:auth-mechanism` rather than an
// `ESource`'s `[Authentication] Method`, because that is the field Camel
// keeps it on — see `jmap_mail::oauth2`'s module docs for why the two must
// agree. What is not repeated here is `jmap_backend_core::oauth2`'s own
// coverage of `method_is_oauth2` itself (the literal, the alias lookup, the
// values that are not OAuth 2.0); this file only proves that this crate
// reads the right field into that same, already-tested, function.
//
// Fetching a token itself — `oauth2::access_token` — is not driven by a test
// here the way `jmap-backend-core/tests/oauth2.rs` drives
// `e_source_get_oauth2_access_token_sync` against a bare `ESource`:
// `camel_session_get_oauth2_access_token_sync` is a `CamelSessionClass`
// vtable slot the base `CamelSession` leaves unimplemented — only
// `EMailSession`, Evolution's own subclass, fills it in (confirmed by
// reading `libemail-engine/e-mail-session.c` upstream: it relays straight to
// `e_source_get_oauth2_access_token_sync`, which is already covered by that
// EDS-side test). Calling it on the plain `CamelSession` this crate's other
// tests construct (see `tests/common`) would not exercise a failure path —
// it would trip Camel's own `g_return_val_if_fail` on a NULL vfunc, which is
// a property of that test double rather than of production Evolution.

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
        // than an obviously-unset one — not read by `uses_oauth2`, but
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

/// The literal spelling `jmap-config`'s own `Authentication` combo writes for
/// this project's OAuth 2.0 choice, and the one EDS's
/// `e_oauth2_services_is_oauth2_alias_static` recognises with no service
/// needing to be registered — see `jmap_backend_core::oauth2`'s tests for the
/// rest of that rule, which this crate reuses rather than repeats.
#[test]
fn the_oauth2_method_is_read_off_the_auth_mechanism_field() {
    let settings = settings(Some("OAuth2"));
    // SAFETY: as above.
    assert!(unsafe { uses_oauth2(settings) });
    // SAFETY: as above.
    unsafe { free(settings) };
}

/// A backend without settings cannot be configured either way; `uses_oauth2`
/// answering "no" for it is the same graceful non-crash `jmap-mail::server`'s
/// `network` already gives every other accessor a NULL settings object could
/// reach.
#[test]
fn a_null_settings_object_is_not_oauth2() {
    // SAFETY: a NULL settings pointer is documented as accepted.
    assert!(!unsafe { uses_oauth2(ptr::null_mut()) });
}
