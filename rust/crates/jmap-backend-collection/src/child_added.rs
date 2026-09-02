// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Keeping a child's connection following its account's — what
//! [`crate::child_source`] writes once, kept true afterwards.
//!
//! [`apply`](crate::child_source::apply) copies the account's
//! [`Connection`](jmap_collection_sync::child_source::Connection) onto a child
//! at the moment the child is created, and that is the last time anything
//! copies it. An account whose server, port, user or TLS setting the user then
//! edits is an account whose address books and calendars go on asking the *old*
//! server — silently, because a child that names a host is a child that connects
//! somewhere, and nothing about it looks wrong.
//!
//! So the copy is turned into a binding, which is what every collection backend
//! in EDS does: `e_ews_backend_child_added` binds the collection's
//! `[Authentication]` host, user and method onto each child's, and EDS's own
//! `collection_backend_child_added` binds the display name and the OAuth 2.0
//! support object the same way. This module is that, for the five properties a
//! JMAP child's connection is made of — the four of `[Authentication]` and the
//! one of `[Security]` — plus a sixth that is not part of the connection at
//! all: `credential-name`, the account's own uid, bound so every child looks
//! its OAuth 2.0 token up under the one key the account uses rather than one
//! each child would otherwise derive on its own (eds#663) — all bound from the
//! account to the child and never back.
//!
//! ## Which sources are bound, and which are left alone
//!
//! `child_added` fires for *every* source whose parent is the collection: the
//! address books and calendars this backend creates, and also the mail account,
//! identity and transport sources that the setup UI creates and that this
//! backend neither writes nor caches ([`crate::prepare_mail`]). Binding them
//! too is wanted and is what EWS does — a mail account of this collection
//! reaches the same server as its address books, so it should follow the same
//! fields — but it is also why nothing here assumes the shape of the source it
//! is handed.
//!
//! Hence the rule [`follow_collection`] applies: a group is bound only when
//! *both* sources already have it. That is a deliberate deviation from EWS,
//! which fetches the collection's `[Authentication]` unconditionally, and it
//! exists because `e_source_get_extension` **creates** the extension it cannot
//! find. On the child that would be a group written into a source belonging to
//! another part of Evolution; on the collection it would be a group written into
//! the user's own account file, which is the thing [`crate::collection_source`]
//! goes out of its way never to do. A source with no `[Authentication]` names no
//! host anyway, so there is nothing a binding could usefully carry to it.
//!
//! The exception is [`crate::mail_child`], and it is one because the premise
//! fails there: this account's *own* mail account and mail transport are sources
//! nothing else writes a server onto — the setup UI can reach only the receiving
//! one — so a group created on them is created in a source of this account's,
//! for want of anywhere else it could come from. They are also bound
//! differently, `[Security]` in particular, which is why they are a module and
//! not a flag.
//!
//! ## `[Security]` is bound as the boolean, not as the string
//!
//! `ESourceSecurity` carries `method` — "tls" or "none" — and the derived
//! `secure` that every JMAP backend actually reads
//! ([`jmap_backend_core::source::SourceConfig`]), and `set_method` notifies both.
//! Binding `secure` therefore propagates just as promptly as binding `method`
//! would, and it propagates the question the reader asks: an account set to some
//! third method spelling reaches its children as the answer they will act on
//! rather than as a string they would have to agree about.

use std::ffi::CStr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_SECURITY, ESource,
    e_binding_bind_property, e_source_authentication_get_type, e_source_security_get_type,
};
use glib_sys::gpointer;
use gobject_sys::G_BINDING_SYNC_CREATE;
use jmap_backend_core::marshal::extension_if_present;

use crate::mail_child::{follow_server, mail_service_of};

/// The properties bound, per `ESource` extension: everything a
/// [`Connection`](jmap_collection_sync::child_source::Connection) is, and one
/// property that is not — see the module comment on `credential-name`.
///
/// Spelled as GObject property names rather than as the keyfile keys
/// [`Setting`](jmap_collection_sync::Setting) uses — `secure` is not a keyfile
/// key at all — so this list is deliberately not the one
/// [`crate::child_source`] writes through. `tests/child_added.rs` holds the two
/// against each other by changing the account and reading the child back the way
/// the address book and calendar backends read it.
pub const BOUND: [(&CStr, &[&CStr]); 2] = [
    (
        E_SOURCE_EXTENSION_AUTHENTICATION,
        &[c"host", c"port", c"user", c"method", c"credential-name"],
    ),
    (E_SOURCE_EXTENSION_SECURITY, &[c"secure"]),
];

/// Binds every property of [`BOUND`] from `collection` onto `child`, for the
/// groups both sources already have.
///
/// One-way and `G_BINDING_SYNC_CREATE`: the child is brought into line the
/// moment it is bound — which is the whole point for a *cached* child, whose
/// values are whatever the account said when it was last written — and nothing
/// a child does afterwards reaches the account.
///
/// Nothing is returned and nothing fails: a group either side lacks is a group
/// with nothing to carry, and every property named above exists on the extension
/// it is named under. A misspelling would be a `g_critical` from GLib rather
/// than an error here, which is what `tests/child_added.rs` exists to catch
/// first.
///
/// # Safety
///
/// `collection` and `child` must be valid `ESource`s — the backend's account
/// source and one of its children. Neither is referenced here; the binding EDS
/// creates holds what it needs and drops itself when either object is finalized.
pub unsafe fn follow_collection(collection: *mut ESource, child: *mut ESource) {
    // As everywhere an extension is looked up by name: `e_source_get_extension`
    // walks the registered children of `E_TYPE_SOURCE_EXTENSION`, and
    // `e_source_has_extension` answers off the same table, so a type nothing has
    // referenced yet is one neither can find. Referencing the GType registers
    // it.
    // SAFETY: no arguments, and the type system initialises itself.
    unsafe {
        e_source_authentication_get_type();
        e_source_security_get_type();
    }

    // Every child — the mail services included, so before the fork below —
    // carries the account's client registration; see `follow_oauth2` on why
    // this group alone is created on the child.
    // SAFETY: valid sources by this function's contract.
    unsafe { follow_oauth2(collection, child) };

    // The two children of this account that are bound by another set of rules,
    // and the only ones any *other* group is created on — see
    // `crate::mail_child`.
    // SAFETY: a valid source by this function's contract, only read from.
    if unsafe { mail_service_of(child) }.is_some() {
        // SAFETY: valid sources by this function's contract, which is
        // `follow_server`'s too.
        unsafe { follow_server(collection, child) };
        return;
    }

    for (name, properties) in BOUND {
        // Both sides tested for, and neither fetched unless both answer — see
        // the module comment on why an absent group is left absent.
        // SAFETY: valid sources by this function's contract, and a header
        // constant.
        let (Some(from), Some(to)) = (unsafe { extension_if_present(collection, name) }, unsafe {
            extension_if_present(child, name)
        }) else {
            continue;
        };

        for property in properties {
            // SAFETY: two live extension objects and two NUL-terminated
            // property names of theirs; the binding is `(transfer none)` and
            // owned by the two objects it joins.
            unsafe { bind(from, to, property) };
        }
    }
}

/// Binds the account's `[JMAP OAuth2]` client registration onto a child,
/// creating the group on the child — the second exception to the
/// both-sides-or-neither rule, and one for [`crate::mail_child`]'s own reason:
/// a group created here is created in a source of this account's, for want of
/// anywhere else it could come from.
///
/// It has to be on the child because EDS hands the `EOAuth2Service` whichever
/// source asked for the token: `e_source_registry_server_get_access_token_sync`
/// (e-source-registry-server.c, EDS 3.52) passes the asking source straight
/// into `e_oauth2_service_get_access_token_sync` — the silent-refresh path
/// never resolves the collection as the credential source (only the
/// interactive prompter does). EDS's own OAuth2 services never feel this,
/// their client ids being compile-time constants; ours is per-account state in
/// `[JMAP OAuth2]`, and a child without it is a child whose token refresh has
/// no client id, no token endpoint and no scope — so the first expired access
/// token became a full re-consent (observed live 2026-08-26: the registry
/// prepared a refresh form for a memory-only calendar child's uid, and mail's
/// connect escalated to the consent window; that same send-time re-consent
/// (`jmap-mail`'s `connect_sync` not authenticating before the shell's
/// prompt-first path) is the mechanism seen here, through the transport).
///
/// The account's side is never invented: an account without the group has no
/// registration to carry, and its `.source` file is the user's — the same
/// rule the rest of this module keeps.
///
/// # Safety
///
/// As [`follow_collection`], whose helper this is.
unsafe fn follow_oauth2(collection: *mut ESource, child: *mut ESource) {
    // Registers the extension type, so the name lookups below can answer —
    // same reasoning as the `get_type` calls in `follow_collection`, spelled
    // through the one caller this crate is meant to use for it.
    jmap_config::oauth2::ensure_registered();

    let name = jmap_config::oauth2::EXTENSION_NAME;
    // SAFETY: a valid source by the caller's contract, and a NUL-terminated
    // name; only read from.
    let Some(from) = (unsafe { extension_if_present::<gobject_sys::GObject>(collection, name) })
    else {
        return;
    };
    // SAFETY: a valid source; `e_source_get_extension` creates the (registered)
    // extension on demand, and the child is this account's to write.
    let to: *mut gobject_sys::GObject =
        unsafe { eds_sys::e_source_get_extension(child, name.as_ptr()) }.cast();

    for property in jmap_config::oauth2::PROPERTY_NAMES {
        // SAFETY: two live extension objects of the same class, which installs
        // every property in `PROPERTY_NAMES`; NUL-terminated names.
        unsafe { bind(from.cast(), to.cast(), property) };
    }
}

/// One property, from the account's extension onto the child's.
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
