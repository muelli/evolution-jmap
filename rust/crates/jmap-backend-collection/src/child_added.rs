// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Keeps the connection of a child equal to the connection of its account.
//! [`crate::child_source`] writes the connection once; this module keeps it
//! correct after that.
//!
//! [`apply`](crate::child_source::apply) copies the
//! [`Connection`](jmap_collection_sync::child_source::Connection) of the
//! account onto a child when the child is created, and only then. If the user
//! later changes the server, port, user or TLS setting, the address books and
//! calendars continue to use the *old* server. Nothing shows an error, because
//! a child that names a host connects to it.
//!
//! Thus this module replaces the copy with a binding, as every collection
//! backend in EDS does. `e_ews_backend_child_added` binds the `[Authentication]`
//! host, user and method of the collection onto each child. EDS's
//! `collection_backend_child_added` binds the display name and the OAuth 2.0
//! support object in the same way.
//!
//! This module binds the five properties of a JMAP child connection: the four
//! of `[Authentication]` and the one of `[Security]`. It also binds a sixth
//! property that is not part of the connection: [`CREDENTIAL_PROPERTY`], the
//! uid of the account. With it, every child finds its OAuth 2.0 token under the one
//! key of the account, not under a key of its own (eds#663). All bindings go
//! from the account to the child, never back.
//!
//! ## Which sources are bound, and which stay unchanged
//!
//! `child_added` fires for *every* source whose parent is the collection. These
//! are the address books and calendars that this backend creates, and also the
//! mail account, identity and transport sources. The setup UI creates the mail
//! sources, and this backend neither writes nor caches them
//! ([`crate::prepare_mail`]). EWS binds them too, and that is correct: a mail
//! account of this collection uses the same server as its address books. But
//! it is also why nothing here assumes the shape of a source.
//!
//! Hence the rule of [`follow_collection`]: it binds a group only when *both*
//! sources already have it. EWS gets the `[Authentication]` of the collection
//! unconditionally, so this is a deliberate difference. The reason is that
//! `e_source_get_extension` **creates** an extension that it cannot find.
//!
//! On the child, that writes a group into a source of a different Evolution
//! component. On the collection, it writes a group into the account file of
//! the user, which [`crate::collection_source`] never does. A source without
//! `[Authentication]` names no host, so a binding has nothing useful to carry.
//!
//! The exception is [`crate::mail_child`], because the premise is not true
//! there. Nothing else writes a server onto the mail account and mail
//! transport of this account: the setup UI writes only the receiving one.
//! Thus a group created on them is in a source of this account, and has no
//! other origin. They also bind differently (`[Security]` in particular), so
//! they have a module, not a flag.
//!
//! ## `[Security]` is bound as the boolean, not as the string
//!
//! `ESourceSecurity` has `method` ("tls" or "none") and the derived `secure`.
//! Every JMAP backend reads `secure`
//! ([`jmap_backend_core::source::SourceConfig`]), and `set_method` notifies
//! both. Thus a binding of `secure` is as fast as a binding of `method`. It
//! also carries the value that the children use: an account with a third
//! method spelling reaches its children as the boolean they act on.

use std::ffi::CStr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_SECURITY, ESource,
    e_binding_bind_property, e_source_authentication_get_type, e_source_security_get_type,
};
use glib_sys::gpointer;
use gobject_sys::G_BINDING_SYNC_CREATE;
use jmap_backend_core::marshal::extension_if_present;

use crate::mail_child::{follow_server, mail_service_of};

/// The bound properties, per `ESource` extension: all of a
/// [`Connection`](jmap_collection_sync::child_source::Connection), and one
/// property that is not part of it: [`CREDENTIAL_PROPERTY`] (see the module
/// comment).
///
/// These are GObject property names, not the keyfile keys of
/// [`Setting`](jmap_collection_sync::Setting) (`secure` is not a keyfile key).
/// Thus this list is deliberately different from the one that
/// [`crate::child_source`] writes. `tests/child_added.rs` compares the two: it
/// changes the account and reads the child as the address book and calendar
/// backends read it.
pub const BOUND: [(&CStr, &[&CStr]); 2] = [
    (
        E_SOURCE_EXTENSION_AUTHENTICATION,
        &[c"host", c"port", c"user", c"method", CREDENTIAL_PROPERTY],
    ),
    (E_SOURCE_EXTENSION_SECURITY, &[c"secure"]),
];

/// The property name under which `apply` in `jmap_config::account` writes the
/// OAuth 2.0 token-cache key of the account. It is `credential-store-id` if the
/// installed EDS has it (eds#663), and `credential-name` if not. See the doc
/// comment of that module.
#[cfg(eds_credential_store_id)]
pub const CREDENTIAL_PROPERTY: &CStr = c"credential-store-id";
#[cfg(not(eds_credential_store_id))]
pub const CREDENTIAL_PROPERTY: &CStr = c"credential-name";

/// Binds every property of [`BOUND`] from `collection` onto `child`, for the
/// groups that both sources already have.
///
/// The binding is one-way and `G_BINDING_SYNC_CREATE`. Thus the child gets the
/// values of the account immediately, which is necessary for a *cached* child
/// with old values. Changes on the child never reach the account.
///
/// Nothing is returned and nothing fails. A group that one side does not have
/// has nothing to carry, and every property above exists on its extension. A
/// misspelt name causes a `g_critical` from GLib, and `tests/child_added.rs`
/// finds it first.
///
/// # Safety
///
/// `collection` and `child` must be valid `ESource`s (the account source of the
/// backend and one of its children). Neither is referenced here. The binding
/// that EDS creates holds what it needs, and removes itself when either object
/// is finalized.
pub unsafe fn follow_collection(collection: *mut ESource, child: *mut ESource) {
    // `e_source_get_extension` and `e_source_has_extension` find only
    // registered children of `E_TYPE_SOURCE_EXTENSION`. A reference to the
    // GType registers it.
    // SAFETY: no arguments, and the type system initialises itself.
    unsafe {
        e_source_authentication_get_type();
        e_source_security_get_type();
    }

    // Every child gets the OAuth 2.0 client registration of the account, the
    // mail services included, so this comes before the branch below. See
    // `follow_oauth2` on why this group is created on the child.
    // SAFETY: valid sources by the contract of this function.
    unsafe { follow_oauth2(collection, child) };

    // The two children of this account with different rules, and the only
    // children on which other groups are created. See `crate::mail_child`.
    // SAFETY: a valid source by the contract of this function, only read from.
    if unsafe { mail_service_of(child) }.is_some() {
        // SAFETY: valid sources by the contract of this function, which is
        // also the contract of `follow_server`.
        unsafe { follow_server(collection, child) };
        return;
    }

    for (name, properties) in BOUND {
        // Test for the group on both sides, and get neither unless both have
        // it. See the module comment.
        // SAFETY: valid sources by the contract of this function, and a header
        // constant.
        let (Some(from), Some(to)) = (unsafe { extension_if_present(collection, name) }, unsafe {
            extension_if_present(child, name)
        }) else {
            continue;
        };

        for property in properties {
            // SAFETY: two live extension objects and two NUL-terminated
            // property names of them; the binding is `(transfer none)` and
            // owned by the two objects it joins.
            unsafe { bind(from, to, property) };
        }
    }
}

/// Binds the OAuth 2.0 client registration of the account
/// ([`jmap_config::oauth2::EXTENSION_NAME`], whichever group that is on this
/// EDS) onto a child, and creates the group on the child.
///
/// This is the second exception to the rule that both sides must have the
/// group. The reason is the same as for [`crate::mail_child`]: the group is
/// created in a source of this account, and has no other origin.
///
/// The child must have the group because EDS gives the `EOAuth2Service` the
/// source that asks for the token.
/// `e_source_registry_server_get_access_token_sync`
/// (e-source-registry-server.c) gives the asking source directly to
/// `e_oauth2_service_get_access_token_sync`. Only the interactive prompter uses
/// the collection as the credential source; the silent refresh does not.
///
/// The OAuth2 services of EDS have compile-time client ids, so they do not see
/// this problem. Here, the client id is per-account state in that group. A
/// child without it has no client id, token endpoint or scope for a refresh.
/// The first expired access token then causes a full re-consent, for example
/// for a calendar child or at send time through the transport.
///
/// This is true for EDS 3.52 through 3.62. On current EDS master, the same
/// function first prefers the collection source, gated on
/// `e_util_can_use_collection_as_credential_source`. That gate declines when
/// the `[Authentication] Host` and `User` of the child are both set and differ
/// from the collection. EDS documents this for a mail service on a different
/// server. Thus the collection is a *preference*, and the child must still
/// have the registration.
///
/// This function never creates the group on the account. An account without
/// the group has no registration to carry, and its `.source` file belongs to
/// the user.
///
/// # Safety
///
/// As [`follow_collection`], whose helper this is.
unsafe fn follow_oauth2(collection: *mut ESource, child: *mut ESource) {
    // Registers the extension type, so that the name lookups below find it.
    // This is the one function in this crate for that purpose.
    jmap_config::oauth2::ensure_registered();

    let name = jmap_config::oauth2::EXTENSION_NAME;
    // SAFETY: a valid source by the contract of the caller, and a
    // NUL-terminated name; only read from.
    let Some(from) = (unsafe { extension_if_present::<gobject_sys::GObject>(collection, name) })
    else {
        return;
    };
    // SAFETY: a valid source; `e_source_get_extension` creates the (registered)
    // extension on demand, and this account owns the child.
    let to: *mut gobject_sys::GObject =
        unsafe { eds_sys::e_source_get_extension(child, name.as_ptr()) }.cast();

    for property in jmap_config::oauth2::PROPERTY_NAMES {
        // SAFETY: two live extension objects of the same class, which installs
        // every property in `PROPERTY_NAMES`; NUL-terminated names.
        unsafe { bind(from.cast(), to.cast(), property) };
    }
}

/// Binds one property from the extension of the account onto the extension of
/// the child.
///
/// # Safety
///
/// Both pointers must be live `ESourceExtension`s carrying a property of that
/// name, and `property` must be NUL-terminated.
unsafe fn bind(from: gpointer, to: gpointer, property: &CStr) {
    // SAFETY: the contract above; the return is `(transfer none)`.
    unsafe {
        e_binding_bind_property(
            from,
            property.as_ptr(),
            to,
            property.as_ptr(),
            G_BINDING_SYNC_CREATE,
        )
    };
}
