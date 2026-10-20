// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The provider's named authentication mechanism, as Camel reports it rather
//! than as this crate's own static value reports it.
//!
//! There is no `camel_sasl_register` call whose effect can be checked
//! directly. A mechanism exists exactly when `camel_sasl_authtype` finds a
//! registered, non-abstract `CamelSasl` subclass whose class carries a
//! `CamelServiceAuthType` with that `authproto` (evolution-data-server
//! 3.52.3, `src/camel/camel-sasl.c`, `sasl_build_class_table_rec`). Every
//! test in this file goes through that same lookup, which
//! `mail_ui_session_authenticate_sync` and `EMailConfigAuthCheck` also call.
//! A test that read `jmap_mail::sasl::auth_type()` back from this crate's
//! own static would prove only that Rust remembers what it was told.
//!
//! The entry point is called instead of `sasl_type()`, for the same reason
//! `tests/provider.rs` calls it. Registering the mechanism is
//! [`jmap_mail::provider::register`]'s job. A mechanism that this crate
//! declares but the module never registers would pass a narrower test, and
//! still leave every real account prompting for consent.

use std::ffi::{CStr, CString};
use std::ptr;

use eds_sys::{
    CamelNetworkSettings, CamelServiceAuthType, CamelSettings,
    camel_network_settings_set_auth_mechanism, camel_network_settings_set_host, camel_provider_get,
    camel_provider_init, camel_sasl_authtype, camel_sasl_is_xoauth2_alias,
};
use glib_sys::{GFALSE, GTRUE, g_list_length, g_list_nth_data};
use gobject_sys::{g_object_new, g_object_unref};
use jmap_backend_core::oauth2::OAUTH2_AUTH_METHOD;
use jmap_mail::module::camel_provider_module_init;
use jmap_mail::provider::PROTOCOL;
use jmap_mail::sasl::{MECHANISM, auth_type, mechanism_for};
use jmap_mail::settings::settings_type;

/// The module's entry point, called the way Camel calls it. It is
/// idempotent, so every test may start by calling it.
fn loaded() {
    // SAFETY: neither call takes arguments. `camel_provider_init` is
    // idempotent, and so is the entry point, because `provider::register`
    // runs behind a `OnceLock`.
    unsafe {
        camel_provider_init();
        camel_provider_module_init();
    }
}

/// `camel_sasl_authtype`, as Evolution calls it.
fn authtype(mechanism: &CStr) -> *mut CamelServiceAuthType {
    loaded();
    // SAFETY: the argument is a 'static NUL-terminated string. The returned
    // pointer is borrowed from a class that Camel holds, or it is NULL.
    unsafe { camel_sasl_authtype(mechanism.as_ptr()) }
}

/// The contract that this whole file rests on: after the module has loaded,
/// Camel knows a mechanism by this project's `EOAuth2Service` name, and that
/// mechanism does not require a password.
///
/// Both halves matter, and `mail_ui_session_authenticate_sync` checks them
/// in one `if`:
///
/// ```c
/// if (authtype != NULL && !authtype->need_password) { … one silent shot … }
/// ```
///
/// A NULL result here, or a TRUE `need_password`, sends the account to
/// `e_credentials_prompter_loop_prompt_sync`, the consent window, before the
/// service has been asked even once. That is GNOME/evolution#3382.
#[test]
fn camel_knows_a_mechanism_by_the_oauth2_service_name_that_needs_no_password() {
    let found = authtype(MECHANISM);
    assert!(
        !found.is_null(),
        "camel_sasl_authtype({MECHANISM:?}) found no mechanism: \
         the session will prompt for consent before ever asking the service"
    );
    // SAFETY: checked non-NULL. The struct is borrowed from a class that
    // Camel holds a reference to for the life of the process.
    let found = unsafe { &*found };
    assert_eq!(
        found.need_password, GFALSE,
        "the mechanism claims to need a password, so it gets a prompt and not a silent attempt"
    );
    // SAFETY: a mechanism's authproto is the 'static literal that its class
    // named.
    assert_eq!(unsafe { CStr::from_ptr(found.authproto) }, MECHANISM);
    assert!(
        !found.name.is_null() && !found.description.is_null(),
        "the account editor shows both of these"
    );
}

/// The name that Camel looks the mechanism up under is the name that EDS
/// answers for this project's OAuth 2.0 service, not merely a string that
/// happens to work.
///
/// `mail_ui_session_authenticate_sync` recovers a *rejected* silent attempt
/// through the consent window only if
/// `e_oauth2_services_is_oauth2_alias (registry, mechanism)` is true.
/// `mail_config_auth_check_host_changed_cb` finds the combo's entry with
/// `camel_sasl_authtype (e_oauth2_service_get_name (oauth2_service))`. Both
/// checks key on the service name. A mechanism under any other name would
/// take the silent attempt, then have no way back to a re-consent.
#[test]
fn the_mechanism_is_named_after_this_projects_oauth2_service() {
    assert_eq!(MECHANISM, OAUTH2_AUTH_METHOD);
}

/// Deriving from `CamelSaslXOAuth2` rather than from `CamelSasl` makes
/// Evolution treat a mechanism under a private name as the bearer-token
/// mechanism. `camel_sasl_is_xoauth2_alias` walks a class's parents looking
/// for `CAMEL_IS_SASL_XOAUTH2_CLASS`. `e_auth_combo_box_update_available`
/// uses that answer to decide whether the entry is offered or struck
/// through.
#[test]
fn the_mechanism_is_recognised_as_an_xoauth2_alias() {
    loaded();
    // SAFETY: a 'static NUL-terminated string.
    let alias = unsafe { camel_sasl_is_xoauth2_alias(MECHANISM.as_ptr()) };
    assert_eq!(
        alias, GTRUE,
        "the mechanism does not derive from CamelSaslXOAuth2, \
         so the account editor strikes its entry through"
    );
}

/// The class overrides its parent's `auth_type` instead of inheriting it.
///
/// `CamelSaslXOAuth2`'s own class initialiser installs the generic
/// `"XOAUTH2"` auth type (`camel-sasl-xoauth2.c`). A subclass that left this
/// auth type in place would file itself under a key that Camel already has a
/// class for. Whichever of the two classes GLib walked last would win the
/// `"XOAUTH2"` key, and the other name would then answer NULL. So both
/// auth types must be findable, and `"XOAUTH2"` must not belong to this
/// project's mechanism.
#[test]
fn the_mechanism_does_not_take_over_camels_own_xoauth2_name() {
    let generic = authtype(c"XOAUTH2");
    assert!(
        !generic.is_null(),
        "Camel's own XOAUTH2 mechanism went missing"
    );
    assert_ne!(
        generic,
        auth_type(),
        "the subclass inherited its parent's auth type instead of overriding it"
    );
    // SAFETY: checked non-NULL, and borrowed from a class that Camel holds.
    assert_eq!(unsafe { CStr::from_ptr((*generic).authproto) }, c"XOAUTH2");
}

/// Milan Crha's stated bonus on GNOME/evolution#3382: the provider lists the
/// mechanism, so the account editor's *Authentication type* combo has an
/// entry to show. `auth_combo_box_rebuild_model` builds that model only from
/// `provider->authtypes`, so a NULL list produces a combo with no rows.
#[test]
fn the_provider_advertises_the_mechanism_to_the_account_editor() {
    loaded();
    let mut error = ptr::null_mut();
    // SAFETY: the protocol string is 'static, and this call owns the error
    // location. The provider comes back borrowed from Camel's table, which
    // is never cleared.
    let provider = unsafe { camel_provider_get(PROTOCOL.as_ptr(), &mut error) };
    assert!(!provider.is_null(), "the provider is not registered");
    // SAFETY: checked non-NULL and 'static.
    let authtypes = unsafe { (*provider).authtypes };
    // SAFETY: a GList that this module built, or NULL.
    assert_eq!(
        unsafe { g_list_length(authtypes) },
        1,
        "the provider should advertise exactly the one mechanism it has"
    );
    // SAFETY: as above, with an index inside the length just asserted.
    assert_eq!(
        unsafe { g_list_nth_data(authtypes, 0) }.cast::<CamelServiceAuthType>(),
        auth_type(),
        "the advertised mechanism is not the one this provider registered"
    );
}

/// A settings object as the account editor's combo leaves it, for the four
/// credential kinds a JMAP account can be configured with.
fn settings(mechanism: Option<&str>) -> *mut CamelSettings {
    // SAFETY: `settings_type` registers the type, and the type has no
    // construct properties of its own. The accessors below take an instance
    // of it.
    unsafe {
        let object = g_object_new(settings_type(), ptr::null());
        assert!(!object.is_null(), "g_object_new returned NULL");
        let network = object.cast::<CamelNetworkSettings>();
        camel_network_settings_set_host(network, c"jmap.example.com".as_ptr());
        if let Some(mechanism) = mechanism {
            let mechanism = CString::new(mechanism).expect("no NUL in a test mechanism");
            camel_network_settings_set_auth_mechanism(network, mechanism.as_ptr());
        }
        object.cast::<CamelSettings>()
    }
}

/// [`mechanism_for`] tested over the credential kinds a JMAP account can be
/// configured with. This is the decision that `connect_sync` makes before it
/// hands the account to the session.
///
/// The OAuth 2.0 row spells the method EDS's generic way, not with this
/// project's service name. `tests/oauth2.rs` records the same limit, for the
/// same reason: recognising the *alias* spelling needs a live query against
/// the registered `EOAuth2Service`s, which needs `module-jmap-backend.so`
/// loaded. This binary is a Camel provider with no EDS module in it. Which
/// spellings count as OAuth 2.0 is `jmap_backend_core::oauth2`'s question,
/// covered there. What this test covers is the next step: an account that
/// *is* OAuth 2.0 names this project's mechanism, and an account that is not
/// names none.
///
/// The point of the row is that it names this project's mechanism, not the
/// account's own string: `"OAuth2"` is not a mechanism, and no `CamelSasl`
/// carries that authproto. Passing the field through unchanged would send
/// the session looking for a mechanism that does not exist.
#[test]
fn only_an_oauth2_account_names_a_mechanism_and_it_names_ours() {
    loaded();
    for (method, expected) in [
        (None, None),
        (Some("none"), None),
        (Some("bearer"), None),
        (Some("PLAIN"), None),
        (Some("OAuth2"), Some(MECHANISM)),
    ] {
        let settings = settings(method);
        // SAFETY: a live settings object of this project's type, freed below.
        let named = unsafe { mechanism_for(settings) };
        let named = if named.is_null() {
            None
        } else {
            // SAFETY: non-NULL here is always the 'static MECHANISM literal.
            Some(unsafe { CStr::from_ptr(named) })
        };
        assert_eq!(
            named, expected,
            "an account whose auth-mechanism is {method:?} named the wrong mechanism"
        );
        // SAFETY: this test owns the only reference.
        unsafe { g_object_unref(settings.cast()) };
    }
}
