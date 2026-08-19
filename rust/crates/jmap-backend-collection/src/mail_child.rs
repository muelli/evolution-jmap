// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The two mail sources of this account, following the account — the one child
//! [`crate::child_added`]'s rule cannot serve.
//!
//! [`follow_collection`](crate::child_added::follow_collection) binds a group
//! only when *both* sources already have it, because
//! `e_source_get_extension()` creates the group it cannot find and a collection
//! backend has no business writing into somebody else's source. That rule is
//! right for every child except two, and those two are exactly the ones that
//! reach no server without it.
//!
//! ## The transport arrives naming a provider and nothing else
//!
//! An account created through Evolution's assistant ends with three mail
//! sources. The receiving one is written by the setup module's `commit_changes`
//! — `jmap_config::mail::apply_server`, which is handed the account and the
//! scratch source together. The transport is not, and cannot be: Evolution hides
//! the *Sending Email* page for a `CAMEL_PROVIDER_IS_STORE_AND_TRANSPORT`
//! provider, so the backend that is its candidate is never shown the account and
//! never asked where the account sends through. What the transport carries when
//! it gets here is therefore the service name `jmap` and no `[Authentication]`
//! group at all — and JMAP submission needs a server exactly as much as
//! receiving does.
//!
//! This backend is the one place that holds both halves. It is handed the
//! account source, and `child_added` fires for every source parented to it,
//! mail sources included ([`crate::prepare_mail`] on why they are children of
//! the *account* without being cached resources of the *backend*). So the group
//! is created here, on a source this account owns, and filled from the account
//! it belongs to. Inventing a host would be the alternative and is not one.
//!
//! ## Which children this claims, and why the test is not "is it mail"
//!
//! `[Mail Account]` or `[Mail Transport]`, **and** a
//! [`backend-name`](crate::prepare_mail::MAIL_BACKEND_NAME) of `jmap`. The
//! second half is not decoration: an account may put its outgoing service on
//! another server with a password of its own — that is the case EDS's
//! `e_util_can_use_collection_as_credential_source` exists to allow — and
//! writing this collection's host onto an `smtp` transport would point it at a
//! server that does not speak SMTP and take its password away in the same move.
//! The identity is not claimed either, and its absence from the list is the same
//! decision [`crate::prepare_mail`] makes: it is a person, not a service, and it
//! reaches nothing.
//!
//! ## `[Security]` is the one field that is *not* the account's
//!
//! Everywhere else in this crate a child repeats what the account says. Here the
//! account's own spelling would be wrong, and quietly so.
//!
//! `ESourceSecurity` carries `method` plus the derived `secure`, and
//! `e_source_security_set_secure()` writes one of EDS's own two words — `"tls"`
//! or `"none"`. On a mail source that same key is additionally bound, by the
//! `ESourceCamel` extension, to `CamelNetworkSettings:security-method` through
//! `e_binding_transform_enum_nick_to_value` — which looks the string up as a
//! **`CamelNetworkSecurityMethod` enum nick**. `"tls"` is not one. The transform
//! then returns `FALSE`, the binding sets nothing, and the settings object keeps
//! the property's default, which in EDS 3.52 is `STARTTLS_ON_STANDARD_PORT`: a
//! security method nobody chose, shown to the user in Evolution's editor as
//! this account's, and one that a Camel release moving that default would turn
//! into a refusal to connect. So the boolean is bound to the *method* through a
//! `GBindingTransformFunc` of this module's, and what lands on the mail source is
//! [`MAIL_SECURITY_METHOD_TLS`] or [`MAIL_SECURITY_METHOD_NONE`] — the same two
//! strings `jmap_config::mail::apply_server` commits, which is what keeps a
//! source that is committed and then merely re-bound from changing shape.
//!
//! ## `[Authentication] Method` is bound too
//!
//! All four fields of the group are bound. `Method` was once excluded on the
//! theory that on a mail source `ESourceCamel` binds it to
//! `CamelNetworkSettings:auth-mechanism`, where it would name a SASL mechanism
//! Camel might try to use — and JMAP has none, so `jmap-mail` passes a NULL
//! mechanism to `camel_session_authenticate_sync`. But `jmap-mail` *reuses* that
//! `auth-mechanism` field as this project's credential-type selector:
//! `uses_api_token`/`uses_oauth2` read it back to choose Basic vs Bearer vs
//! OAuth 2.0. A mail child that did not follow the collection's `Method` would
//! therefore always authenticate as Basic — which is exactly why the transport
//! of a Bearer (API-token) account re-prompted for a password forever while its
//! receiving account, whose `Method` `jmap_config::mail::apply_server` writes
//! directly, connected. `"none"` (password → Basic) still reaches Camel as the
//! absent mechanism `ESourceCamel` converts NULL back to, so following the field
//! costs the Basic case nothing and is what makes Bearer and OAuth reach the
//! services at all.

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

/// The `[Security] Method` a mail source of this account carries when the
/// connection is encrypted: a `CamelNetworkSecurityMethod` enum nick, and
/// deliberately not the `"tls"` EDS's own `secure` setter writes.
///
/// Of Camel's three nicks this is the one that describes HTTPS — TLS from the
/// first byte, no in-band upgrade — and it is the spelling Evolution's server
/// settings page writes back through the same binding. The same constant, for
/// the same reason and with the argument written out at length, is
/// `jmap_config::mail::MAIL_SECURITY_METHOD_TLS`; `jmap-config`'s `tests/mail.rs`
/// holds the two against each other, as it does for
/// [`MAIL_BACKEND_NAME`].
pub const MAIL_SECURITY_METHOD_TLS: &CStr = c"ssl-on-alternate-port";

/// And when it is not encrypted, which is the one spelling both sides share:
/// `ESourceSecurity` documents `"none"` as its convention for "no security" and
/// `CamelNetworkSecurityMethod`'s first value has the same nick.
pub const MAIL_SECURITY_METHOD_NONE: &CStr = c"none";

/// The `[Authentication]` properties a mail source of this account follows —
/// [`BOUND`](crate::child_added::BOUND)'s four, `method` included.
///
/// `method` was once excluded here on the theory that on a mail source it names
/// a Camel SASL mechanism rather than a credentials provider. But `jmap-mail`
/// reuses that field (`CamelNetworkSettings:auth-mechanism`) as this project's
/// credential-type selector — `uses_api_token`/`uses_oauth2` read it to choose
/// Basic vs Bearer vs OAuth 2.0 — so a mail child that does not follow the
/// collection's `method` silently authenticates as Basic. That left the
/// transport of a Bearer (API-token) account prompting for a password forever
/// while the receiving account, whose `method` `jmap_config::mail::apply_server`
/// writes directly, worked. Following it here keeps the two in step.
pub const BOUND_MAIL_AUTHENTICATION: &[&CStr] = &[c"host", c"port", c"user", c"method"];

/// The two extensions that make a source one of an account's mail *services*.
///
/// In the order they are talked about everywhere else, and without the identity:
/// a person reaches no server.
const SERVICES: [&CStr; 2] = [
    E_SOURCE_EXTENSION_MAIL_ACCOUNT,
    E_SOURCE_EXTENSION_MAIL_TRANSPORT,
];

/// Which of this account's mail services `child` is, if it is one of them —
/// a source carrying `[Mail Account]` or `[Mail Transport]` that names *this*
/// provider.
///
/// Nothing here creates an extension: each is tested for before it is read, so a
/// child of another kind is left exactly as it arrived. That matters more than
/// usual, because `collection_backend_child_is_mail()` reads a source carrying
/// `[Mail Account]` as a receiving account of the user's — a group written here
/// by accident would show up in Evolution as a second inbox.
///
/// # Safety
///
/// `child` must be a valid `ESource` — one of the collection's children. It is
/// only read from, and nothing outlives the call.
pub unsafe fn mail_service_of(child: *mut ESource) -> Option<&'static CStr> {
    // As everywhere an extension is looked up by name: `e_source_get_extension`
    // and `e_source_has_extension` both answer off the registered children of
    // `E_TYPE_SOURCE_EXTENSION`, so a type nothing has referenced yet is one
    // neither can find. Referencing the GType registers it.
    // SAFETY: no arguments, and the type system initialises itself.
    unsafe {
        e_source_mail_account_get_type();
        e_source_mail_transport_get_type();
    }

    SERVICES.into_iter().find(|extension| {
        // SAFETY: a valid source by this function's contract, and a header
        // constant; both mail extensions derive from `ESourceBackend`.
        let Some(backend) = (unsafe { extension_if_present::<ESourceBackend>(child, extension) })
        else {
            return false;
        };
        // SAFETY: a live extension; the getter returns NULL or a
        // NUL-terminated string owned by it.
        let name = unsafe { e_source_backend_get_backend_name(backend) };
        // SAFETY: as above, and the string is only read from within this scope.
        !name.is_null() && unsafe { CStr::from_ptr(name) } == MAIL_BACKEND_NAME
    })
}

/// Binds where a mail service of this account reaches its server, creating the
/// groups on the mail source if it has none.
///
/// Called from [`follow_collection`](crate::child_added::follow_collection) for
/// the children [`mail_service_of`] claims, and instead of the rule that applies
/// to every other child — see the module comment on why those two children are
/// the exception, and on `[Security]`, which is bound as a different property at
/// the far end than it is read from at this one.
///
/// Nothing is returned and nothing fails, as in `child_added`: a group the
/// *account* lacks is a group with nothing to carry, and is left absent on both
/// sides rather than invented on either.
///
/// # Safety
///
/// `collection` and `child` must be valid `ESource`s — the backend's account
/// source and one of its mail services. Neither is referenced here; the bindings
/// EDS creates hold what they need and drop themselves when either object is
/// finalized.
pub unsafe fn follow_server(collection: *mut ESource, child: *mut ESource) {
    // SAFETY: no arguments; registers the types the lookups below need.
    unsafe {
        e_source_authentication_get_type();
        e_source_security_get_type();
    }

    // Tested for on the account and never fetched from it: this is the user's
    // own file, and an account that names no server has nothing to pass on.
    // SAFETY: valid sources by this function's contract, and a header constant.
    if let Some(from) =
        unsafe { extension_if_present(collection, E_SOURCE_EXTENSION_AUTHENTICATION) }
    {
        // SAFETY: created on demand on the mail source, which is the point of
        // this module, and owned by the source either way.
        let to =
            unsafe { e_source_get_extension(child, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()) };

        for property in BOUND_MAIL_AUTHENTICATION {
            // SAFETY: two live extension objects and a NUL-terminated property
            // name both carry; the binding is `(transfer none)` and owned by the
            // two objects it joins.
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
    // is what is written into it.
    let security: *mut ESourceSecurity =
        unsafe { e_source_get_extension(child, E_SOURCE_EXTENSION_SECURITY.as_ptr()) }.cast();

    // SAFETY: valid sources by this function's contract, and a header constant.
    if let Some(from) = unsafe { extension_if_present(collection, E_SOURCE_EXTENSION_SECURITY) } {
        // `G_BINDING_SYNC_CREATE`, so this also *is* the initial write: the
        // transform runs once here and again on every later change.
        // SAFETY: two live extension objects, a boolean property of the one and
        // a string property of the other, and a transform function with the
        // signature `GBindingTransformFunc` names; no user data, so nothing to
        // free.
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
        // No `[Security]` on the account is TLS — `collection_source::server_of`
        // reads it that way, because `ESourceSecurity:secure` defaults to FALSE
        // and reading that as "the user turned TLS off" would downgrade every
        // hand-written account. Written rather than bound, since there is no
        // group to bind from and adding one to the user's file is what this
        // crate goes out of its way never to do; a mail source left with no
        // `[Security]` at all would take `CamelNetworkSettings`' own default
        // instead, which is a third answer nobody chose.
        // SAFETY: a live extension, and a static NUL-terminated string the
        // setter copies.
        unsafe { e_source_security_set_method(security, MAIL_SECURITY_METHOD_TLS.as_ptr()) };
    }
}

/// The account's `secure` as a mail source's `[Security] Method`: a
/// `CamelNetworkSecurityMethod` nick rather than one of EDS's own two words.
///
/// A panic becomes `FALSE`, which is how a `GBindingTransformFunc` says "no
/// value": the binding then leaves the target alone, so the mail source keeps
/// the method it had rather than acquiring a half-written one — and a panic
/// crossing from here into GLib's binding machinery, which runs inside
/// `evolution-source-registry`, is the thing that must not happen at all.
///
/// # Safety
///
/// The arguments are `GBindingTransformFunc`'s: `from_value` holds the source
/// property's value — a boolean, since this is only ever installed on
/// `ESourceSecurity:secure` — and `to_value` is initialised to the target
/// property's type, a string.
unsafe extern "C" fn secure_to_camel_method(
    binding: *mut GBinding,
    from_value: *const GValue,
    to_value: *mut GValue,
    user_data: gpointer,
) -> gboolean {
    let _ = (binding, user_data);

    guard("secure_to_camel_method", GFALSE, || {
        // SAFETY: a `GValue` holding the boolean this binding's source property
        // is, by the contract above.
        let secure = unsafe { g_value_get_boolean(from_value) } != GFALSE;
        let method = if secure {
            MAIL_SECURITY_METHOD_TLS
        } else {
            MAIL_SECURITY_METHOD_NONE
        };

        // `set_static_string` and not `set_string`: both constants are `'static`
        // and NUL-terminated, so there is nothing for the `GValue` to copy or
        // free.
        // SAFETY: a `GValue` initialised to the string type the target property
        // is, and a pointer valid for the life of the process.
        unsafe { g_value_set_static_string(to_value, method.as_ptr()) };
        GTRUE
    })
}
