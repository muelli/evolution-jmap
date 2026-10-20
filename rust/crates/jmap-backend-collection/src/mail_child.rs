// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The two mail sources of this account follow the account. They are the one
//! exception that the rule of [`crate::child_added`] cannot serve.
//!
//! [`follow_collection`](crate::child_added::follow_collection) binds a group
//! only when *both* sources already have it. The reason is that
//! `e_source_get_extension()` creates a group that it cannot find, and a
//! collection backend must not write into the source of a different component.
//! That rule is correct for all children except two, and these two reach no
//! server without the groups.
//!
//! ## The transport arrives with a provider name only
//!
//! An account from the Evolution assistant has three mail sources. The setup
//! module's `commit_changes` (`jmap_config::mail::apply_server`) writes the
//! receiving source, because it gets the account and the scratch source
//! together. Evolution hides the *Sending Email* page for a
//! `CAMEL_PROVIDER_IS_STORE_AND_TRANSPORT` provider, so the transport backend
//! never sees the account. Thus the transport arrives with the service name
//! `jmap` and no `[Authentication]` group. JMAP submission needs a server, as
//! receiving does.
//!
//! This backend is the one place that has both halves. It has the account
//! source, and `child_added` fires for every child of the account, the mail
//! sources included. ([`crate::prepare_mail`] tells why they are children of
//! the *account* but not cached resources of the *backend*.) Thus this module
//! creates the group on a source of this account and fills it from the
//! account. To invent a host is not an acceptable alternative.
//!
//! ## Which children this module claims, and why "is it mail" is not the test
//!
//! A child must have `[Mail Account]` or `[Mail Transport]`, **and** a
//! [`backend-name`](crate::prepare_mail::MAIL_BACKEND_NAME) of `jmap`. The
//! second condition is necessary. An account can send through a different
//! server with its own password: EDS's
//! `e_util_can_use_collection_as_credential_source` exists for that case. The
//! host of this collection on an `smtp` transport would send it to a server
//! that does not speak SMTP, and would remove its password.
//! The identity is not claimed, for the same reason as in
//! [`crate::prepare_mail`]: it is a person, not a service, and it connects to
//! nothing.
//!
//! ## `[Security]` is the one field that is *not* a copy of the account
//!
//! `ESourceSecurity` has `method` and the derived `secure`.
//! `e_source_security_set_secure()` writes one of two EDS words: `"tls"` or
//! `"none"`. On a mail source, the `ESourceCamel` extension also binds that key
//! to `CamelNetworkSettings:security-method` through
//! `e_binding_transform_enum_nick_to_value`. That transform expects a
//! **`CamelNetworkSecurityMethod` enum nick**, and `"tls"` is not one.
//!
//! The transform then returns `FALSE`, and the settings object keeps the
//! default of the property. In EDS 3.52 that default is
//! `STARTTLS_ON_STANDARD_PORT`. Nobody chose that method, but the Evolution
//! editor shows it as the method of this account. A Camel release that
//! changes the default can make the connection fail.
//!
//! Thus this module binds the boolean to the *method* through its own
//! `GBindingTransformFunc`. The mail source gets [`MAIL_SECURITY_METHOD_TLS`]
//! or [`MAIL_SECURITY_METHOD_NONE`]. `jmap_config::mail::apply_server` commits
//! the same two strings, so a committed source keeps its shape when it is bound
//! again.
//!
//! ## `[Authentication] Method` is bound too
//!
//! All five properties of [`BOUND_MAIL_AUTHENTICATION`] are bound, `Method`
//! included. On a mail source, `ESourceCamel` binds `Method` to
//! `CamelNetworkSettings:auth-mechanism`. JMAP has no SASL mechanism, so
//! `jmap-mail` gives a NULL mechanism to `camel_session_authenticate_sync`.
//! But `jmap-mail` also uses that field as the credential-type selector:
//! `uses_api_token`/`uses_oauth2` read it to choose Basic, Bearer or OAuth 2.0.
//!
//! A mail child without this binding always authenticates as Basic. That is
//! why the transport of a Bearer (API-token) account asked for a password
//! again and again. `"none"` (password, thus Basic) still reaches Camel as the
//! absent mechanism that `ESourceCamel` converts NULL back to. Thus the
//! binding costs the Basic case nothing, and Bearer and OAuth 2.0 need it.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_MAIL_ACCOUNT,
    E_SOURCE_EXTENSION_MAIL_TRANSPORT, E_SOURCE_EXTENSION_SECURITY, ESource, ESourceBackend,
    ESourceSecurity, e_binding_bind_property, e_binding_bind_property_full,
    e_source_authentication_get_type, e_source_backend_get_backend_name, e_source_get_extension,
    e_source_mail_account_get_type, e_source_mail_transport_get_type, e_source_security_get_type,
    e_source_security_set_method,
};
use glib_sys::{GFALSE, GTRUE, gboolean, gpointer};
use gobject_sys::{
    G_BINDING_SYNC_CREATE, GBinding, GValue, g_value_get_boolean, g_value_set_static_string,
};
use jmap_backend_core::marshal::extension_if_present;
use jmap_backend_core::trampoline::guard;

use crate::prepare_mail::MAIL_BACKEND_NAME;

/// The `[Security] Method` of a mail source of this account when the
/// connection is encrypted. It is a `CamelNetworkSecurityMethod` enum nick, not
/// the `"tls"` that the EDS `secure` setter writes.
///
/// Of the three Camel nicks, this one describes HTTPS: TLS from the first
/// byte, with no in-band upgrade. The Evolution server settings page writes the
/// same value through the same binding.
/// `jmap_config::mail::MAIL_SECURITY_METHOD_TLS` is the same constant, and
/// `tests/mail.rs` of `jmap-config` compares the two, as it does for
/// [`MAIL_BACKEND_NAME`].
pub const MAIL_SECURITY_METHOD_TLS: &CStr = c"ssl-on-alternate-port";

/// The `[Security] Method` when the connection is not encrypted. Both sides
/// use this spelling: `ESourceSecurity` documents `"none"` for "no security",
/// and the first `CamelNetworkSecurityMethod` value has the same nick.
pub const MAIL_SECURITY_METHOD_NONE: &CStr = c"none";

/// The `[Authentication]` properties that a mail source of this account
/// follows: the five of [`BOUND`](crate::child_added::BOUND), `method` and
/// `credential-name` included.
///
/// `jmap-mail` uses `method` (`CamelNetworkSettings:auth-mechanism`) as the
/// credential-type selector. If a mail child does not follow it, the child
/// authenticates as Basic. See the module comment.
///
/// `credential-name`/`credential-store-id` follows for a related reason. A
/// mail source without the uid of the account there finds its OAuth 2.0 token
/// under its own key, not the key of the account (eds#663).
pub const BOUND_MAIL_AUTHENTICATION: &[&CStr] = &[
    c"host",
    c"port",
    c"user",
    c"method",
    crate::child_added::CREDENTIAL_PROPERTY,
];

/// The two extensions that make a source one of the mail *services* of an
/// account. The identity is not included: a person reaches no server.
const SERVICES: [&CStr; 2] = [
    E_SOURCE_EXTENSION_MAIL_ACCOUNT,
    E_SOURCE_EXTENSION_MAIL_TRANSPORT,
];

/// Which mail service of this account `child` is, if any: a source with
/// `[Mail Account]` or `[Mail Transport]` that names *this* provider.
///
/// This function creates no extension. It tests for each extension before it
/// reads it, so a child of a different kind stays unchanged. This is important
/// because `collection_backend_child_is_mail()` reads a source with
/// `[Mail Account]` as a receiving account. An accidental group would show in
/// Evolution as a second inbox.
///
/// # Safety
///
/// `child` must be a valid `ESource`: one of the children of the collection.
/// It is only read from, and nothing outlives the call.
pub unsafe fn mail_service_of(child: *mut ESource) -> Option<&'static CStr> {
    // `e_source_get_extension` and `e_source_has_extension` find only
    // registered children of `E_TYPE_SOURCE_EXTENSION`. A reference to the
    // GType registers it.
    // SAFETY: no arguments, and the type system initialises itself.
    unsafe {
        e_source_mail_account_get_type();
        e_source_mail_transport_get_type();
    }

    SERVICES.into_iter().find(|extension| {
        // SAFETY: a valid source by the contract of this function, and a header
        // constant; both mail extensions derive from `ESourceBackend`.
        let Some(backend) = (unsafe { extension_if_present::<ESourceBackend>(child, extension) })
        else {
            return false;
        };
        // SAFETY: a live extension; the getter returns NULL or a
        // NUL-terminated string that the extension owns.
        let name = unsafe { e_source_backend_get_backend_name(backend) };
        // SAFETY: as above, and the string is only read from within this scope.
        !name.is_null() && unsafe { CStr::from_ptr(name) } == MAIL_BACKEND_NAME
    })
}

/// Binds the server settings of a mail service of this account. Creates the
/// groups on the mail source if it does not have them.
///
/// [`follow_collection`](crate::child_added::follow_collection) calls this for
/// the children that [`mail_service_of`] claims, instead of the rule for all
/// other children. The module comment tells why, and why `[Security]` binds to
/// a different property on the mail source.
///
/// Nothing is returned and nothing fails. If the *account* does not have a
/// group, there is nothing to copy, and the group stays absent on both sides.
///
/// # Safety
///
/// `collection` and `child` must be valid `ESource`s: the account source of the
/// backend and one of its mail services. Neither is referenced here. The
/// bindings that EDS creates hold what they need, and remove themselves when
/// either object is finalized.
pub unsafe fn follow_server(collection: *mut ESource, child: *mut ESource) {
    // SAFETY: no arguments; registers the types the lookups below need.
    unsafe {
        e_source_authentication_get_type();
        e_source_security_get_type();
    }

    // Test for the group on the account, and never create it: the account is
    // the file of the user. An account without a server has nothing to copy.
    // SAFETY: valid sources by the contract of this function, and a header
    // constant.
    if let Some(from) =
        unsafe { extension_if_present(collection, E_SOURCE_EXTENSION_AUTHENTICATION) }
    {
        // SAFETY: created on demand on the mail source (the purpose of this
        // module), and owned by the source in both cases.
        let to =
            unsafe { e_source_get_extension(child, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()) };

        for property in BOUND_MAIL_AUTHENTICATION {
            // SAFETY: two live extension objects and a NUL-terminated property
            // name that both have; the binding is `(transfer none)` and owned
            // by the two objects it joins.
            unsafe {
                e_binding_bind_property(
                    from,
                    property.as_ptr(),
                    to,
                    property.as_ptr(),
                    G_BINDING_SYNC_CREATE,
                );
            }
        }
    }

    // SAFETY: created on demand and owned by the mail source; the branch below
    // writes into it.
    let security: *mut ESourceSecurity =
        unsafe { e_source_get_extension(child, E_SOURCE_EXTENSION_SECURITY.as_ptr()) }.cast();

    // SAFETY: valid sources by the contract of this function, and a header
    // constant.
    if let Some(from) = unsafe { extension_if_present(collection, E_SOURCE_EXTENSION_SECURITY) } {
        // `G_BINDING_SYNC_CREATE` makes this binding also the initial write.
        // SAFETY: two live extension objects, a boolean property of the one and
        // a string property of the other, and a transform function with the
        // signature that `GBindingTransformFunc` names; no user data, so
        // nothing to free.
        unsafe {
            e_binding_bind_property_full(
                from,
                c"secure".as_ptr(),
                security.cast(),
                c"method".as_ptr(),
                G_BINDING_SYNC_CREATE,
                Some(secure_to_camel_method),
                None,
                ptr::null_mut(),
                None,
            );
        }
    } else {
        // No `[Security]` on the account means TLS, as in
        // `collection_source::server_of`. There is no group to bind from, and
        // this crate never adds one to the file of the user. Thus write the
        // method directly. Without it, the mail source gets the
        // `CamelNetworkSettings` default, which nobody chose.
        // SAFETY: a live extension, and a static NUL-terminated string that the
        // setter copies.
        unsafe { e_source_security_set_method(security, MAIL_SECURITY_METHOD_TLS.as_ptr()) };
    }
}

/// Converts the `secure` of the account to the `[Security] Method` of a mail
/// source: a `CamelNetworkSecurityMethod` nick, not one of the two EDS words.
///
/// A panic becomes `FALSE`, which tells GLib "no value". The binding then does
/// not change the target, so the mail source keeps its previous method. A
/// panic must never cross into the GLib binding code, which runs inside
/// `evolution-source-registry`.
///
/// # Safety
///
/// The arguments are those of `GBindingTransformFunc`. `from_value` holds the
/// value of the source property: a boolean, because this function is installed
/// only on `ESourceSecurity:secure`. `to_value` is initialised to the type of
/// the target property, a string.
unsafe extern "C" fn secure_to_camel_method(
    binding: *mut GBinding,
    from_value: *const GValue,
    to_value: *mut GValue,
    user_data: gpointer,
) -> gboolean {
    let _ = (binding, user_data);

    guard("secure_to_camel_method", GFALSE, || {
        // SAFETY: a `GValue` that holds the boolean of the source property of
        // this binding, by the contract above.
        let secure = unsafe { g_value_get_boolean(from_value) } != GFALSE;
        let method = if secure {
            MAIL_SECURITY_METHOD_TLS
        } else {
            MAIL_SECURITY_METHOD_NONE
        };

        // `set_static_string`: both constants are `'static` and NUL-terminated,
        // so the `GValue` has nothing to copy or free.
        // SAFETY: a `GValue` initialised to the string type of the target
        // property, and a pointer valid for the life of the process.
        unsafe { g_value_set_static_string(to_value, method.as_ptr()) };
        GTRUE
    })
}
