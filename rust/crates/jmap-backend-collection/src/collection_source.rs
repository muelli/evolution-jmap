// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the account itself says, read off the collection `ESource`.
//!
//! [`crate::resource_id`] reads a *child*; this reads the source the children
//! hang off — the one `evolution-source-registry` handed the backend, which is
//! the whole description of the account. It is everything `populate` knows
//! before it contacts anything, and it comes in two answers that are
//! deliberately not one:
//!
//! - [`parts_of`] — which of mail, contacts and calendars the user left
//!   switched on, as [`Parts`], which is what gates every listing and every
//!   removal.
//! - [`server_of`] — where the server is, as a [`Server`]: the origin *this*
//!   backend fetches `/.well-known/jmap` from, and the [`Connection`] each child
//!   has to repeat in order to reach the same one.
//!
//! Two functions rather than one because they fail differently and are needed
//! at different moments. An account with no part enabled has nothing to
//! populate and must not be reported as broken merely because its host field is
//! also empty; `populate` asks [`parts_of`] first, returns if
//! [`Parts::any`] is false, and only then needs a server. Folding them together
//! would turn a switched-off account into an error dialog.
//!
//! ## Both answers come out of one read
//!
//! [`Server`] carries the assembled origin *and* the field-by-field connection
//! because both are needed and they must not be read twice. This backend
//! contacts the server itself, and each child assembles its own origin at the
//! far end from the fields copied into it ([`Child::settings`]). Two reads of
//! one source are two chances to disagree, and a disagreement here is an
//! account that discovers its collections from one server and fetches them from
//! another.
//!
//! The host rules — a bare host name or IP literal, and TLS unless the host is
//! loopback — are [`jmap_backend_core::source::origin`]'s, not a second copy.
//! They apply here for two reasons at once: this backend is the first thing to
//! contact the server, and it is what *writes* the host into the children. A
//! child re-validates what it was handed, but by then the string has been
//! written into a `.source` file per collection.
//!
//! ## Nothing here creates an extension
//!
//! `e_source_get_extension()` creates the extension it is asked for, and the
//! source in question is the user's *account* — the file EDS writes back to
//! disk. So `[Collection]`, `[Security]` and `[Authentication]` are each tested
//! for before they are read, and their absence is a documented answer rather
//! than an empty extension:
//!
//! - no `[Collection]` is [`Parts::ALL`], because
//!   `e_collection_backend_get_part_enabled()` answers `TRUE` for a source that
//!   has none;
//! - no `[Security]` is TLS, because `ESourceSecurity:secure` defaults to
//!   `FALSE` and reading that as "the user turned TLS off" would downgrade every
//!   hand-written account — the same rule, and the same reasoning, as
//!   [`SourceConfig`];
//! - no `[Authentication]` is [`SourceError::MissingHost`], which is what an
//!   empty one would have produced anyway, minus the edit to the user's file.
//!
//! [`Child::settings`]: jmap_collection_sync::Child::settings
//! [`SourceConfig`]: jmap_backend_core::source::SourceConfig

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_COLLECTION, E_SOURCE_EXTENSION_SECURITY,
    ESource, ESourceAuthentication, ESourceCollection, ESourceSecurity,
    e_source_authentication_get_host, e_source_authentication_get_method,
    e_source_authentication_get_port, e_source_authentication_get_type,
    e_source_authentication_get_user, e_source_collection_get_calendar_enabled,
    e_source_collection_get_contacts_enabled, e_source_collection_get_mail_enabled,
    e_source_collection_get_type, e_source_get_enabled, e_source_get_extension,
    e_source_has_extension, e_source_security_get_secure, e_source_security_get_type,
};
use glib_sys::GFALSE;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::{SourceError, origin};
use jmap_collection_sync::Parts;
use jmap_collection_sync::child_source::Connection;

/// Where the account says its server is, in the two shapes it is needed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    /// Scheme, host and port, validated: what this backend fetches the session
    /// document from. Assembled by [`jmap_backend_core::source::origin`], so it
    /// is the same string the address book and calendar backends would build
    /// from the [`connection`](Server::connection) beside it.
    pub origin: String,
    /// The same server field by field, for the children to repeat. Not derived
    /// from the origin: [`Child::settings`](jmap_collection_sync::Child::settings)
    /// writes `[Authentication]` and `[Security]` keys, and taking a URL apart
    /// again to get them back would be a second parser to disagree with.
    pub connection: Connection,
}

/// Which parts of the account are switched on, by
/// `e_collection_backend_get_part_enabled()`'s two rules.
///
/// # Safety
///
/// `source` must be a valid `ESource` — the collection source EDS handed the
/// backend. It is only read from, and nothing outlives the call.
pub unsafe fn parts_of(source: *mut ESource) -> Parts {
    // As in `SourceConfig::from_source`: `e_source_get_extension` finds an
    // extension class by walking the registered children of
    // `E_TYPE_SOURCE_EXTENSION`, so a type nothing has referenced yet is one it
    // cannot find. Referencing the GType registers it.
    // SAFETY: no arguments, and the type system initialises itself.
    unsafe { e_source_collection_get_type() };

    // SAFETY: a valid source by the contract above, and a header constant.
    let source_enabled = unsafe { e_source_get_enabled(source) } != GFALSE;

    // Tested for rather than fetched, so that reading the account does not
    // write to it — see the module comment.
    // SAFETY: as above.
    let collection =
        (unsafe { e_source_has_extension(source, E_SOURCE_EXTENSION_COLLECTION.as_ptr()) }
            != GFALSE)
            .then(|| {
                // SAFETY: the extension is present, so this returns the source's
                // own, which it owns and which outlives the call.
                unsafe {
                    let collection: *mut ESourceCollection =
                        e_source_get_extension(source, E_SOURCE_EXTENSION_COLLECTION.as_ptr())
                            .cast();
                    Parts {
                        mail: e_source_collection_get_mail_enabled(collection) != GFALSE,
                        contacts: e_source_collection_get_contacts_enabled(collection) != GFALSE,
                        calendars: e_source_collection_get_calendar_enabled(collection) != GFALSE,
                    }
                }
            });

    Parts::from_collection(source_enabled, collection)
}

/// The server this account names, or why it names none.
///
/// # Safety
///
/// As [`parts_of`].
pub unsafe fn server_of(source: *mut ESource) -> Result<Server, SourceError> {
    // SAFETY: no arguments; registers the types the lookups below need.
    unsafe {
        e_source_authentication_get_type();
        e_source_security_get_type();
    }

    // Absent means TLS, and it is asked before any `e_source_get_extension`:
    // `ESourceSecurity:secure` defaults to FALSE, so an unconditional read
    // cannot tell an account with no `[Security]` group from one whose owner
    // switched TLS off.
    // SAFETY: a valid source by the contract above, and a header constant.
    let secure = if unsafe { e_source_has_extension(source, E_SOURCE_EXTENSION_SECURITY.as_ptr()) }
        == GFALSE
    {
        true
    } else {
        // SAFETY: the extension is present, so this returns the source's own,
        // which outlives the call.
        let security: *mut ESourceSecurity =
            unsafe { e_source_get_extension(source, E_SOURCE_EXTENSION_SECURITY.as_ptr()).cast() };
        // SAFETY: a live extension of the type the name selects.
        (unsafe { e_source_security_get_secure(security) }) != GFALSE
    };

    // And the same guard again, for the same reason: an account with no
    // `[Authentication]` names no host, which is what an empty one would have
    // said too — without adding the group to the user's file.
    // SAFETY: as above.
    if unsafe { e_source_has_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()) }
        == GFALSE
    {
        return Err(SourceError::MissingHost);
    }

    // SAFETY: the extension is present and is owned by the source.
    let auth: *mut ESourceAuthentication = unsafe {
        e_source_get_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast()
    };
    // SAFETY: a live extension; each getter returns NULL or a NUL-terminated
    // string owned by it.
    let (host, user, auth_method, port) = unsafe {
        (
            read_string(e_source_authentication_get_host(auth)),
            read_string(e_source_authentication_get_user(auth)),
            read_string(e_source_authentication_get_method(auth)),
            e_source_authentication_get_port(auth),
        )
    };

    // The validation, and the one place it happens: the origin this backend
    // connects to and the host its children are given come out of the same
    // checked string.
    let origin = origin(host.as_deref(), port, secure)?;

    Ok(Server {
        origin,
        connection: Connection {
            // `origin` returned Ok, so the host is present; unwrapping it here
            // rather than earlier keeps `MissingHost` a single decision.
            host: host.expect("a validated origin was built from a host"),
            // The keyfile writes 0 for "not set", and a child given `Port=0`
            // would ask for port zero instead of the scheme's default.
            port: (port != 0).then_some(port),
            user,
            auth_method,
            secure,
        },
    })
}
