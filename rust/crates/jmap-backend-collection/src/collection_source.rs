// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads the settings of the account from the collection `ESource`.
//!
//! [`crate::resource_id`] reads a *child*. This module reads the parent source
//! of the children: the source that `evolution-source-registry` gives the
//! backend, which fully describes the account. It is all that `populate` knows
//! before it connects. It gives three answers, deliberately separate:
//!
//! - [`parts_of`]: which of mail, contacts and calendars are enabled, as
//!   [`Parts`]. This controls every listing and every removal.
//! - [`server_of`]: the server, as a [`Server`]. It contains the target that
//!   *this* backend connects to, and the [`Connection`] that each child copies
//!   to reach the same server.
//! - [`user_of`]: the user that the account authenticates as. `populate` needs
//!   this one field of `[Authentication]`, and it must not come from
//!   [`server_of`]. See [`user_of`] for the reason.
//!
//! They are separate functions because they fail differently and are
//! necessary at different times. An account with no part enabled has nothing
//! to populate. It is not broken because its host field is also empty.
//! `populate` calls [`parts_of`] first, and returns if [`Parts::any`] is false.
//! Only after that does it need a server. One combined function would show an
//! error dialog for a disabled account.
//!
//! ## Both answers come from one read
//!
//! [`Server`] has the connect target *and* the connection field by field. Both
//! are necessary, and two reads must not occur. This backend connects to the
//! server itself. Each child assembles its own origin from the fields copied
//! into it ([`Child::settings`]). Two reads of one source can disagree. The
//! account then discovers its collections on one server and fetches them from
//! a different server.
//!
//! The host rules come from [`jmap_backend_core::source::connect_target`];
//! this module has no second copy. The rules are a bare host name or IP
//! literal, and TLS unless the host is loopback. They apply here for two
//! reasons. This backend connects to the server first, and it *writes* the
//! host into the children. A child validates the host again, but by then the
//! host is in one `.source` file per collection.
//!
//! ## Nothing here creates an extension
//!
//! `e_source_get_extension()` creates the extension that it is asked for. Here
//! the source is the *account* of the user, which EDS writes to disk. Thus this
//! module tests for `[Collection]`, `[Security]` and `[Authentication]` before
//! it reads them. Each absence has a documented meaning, not an empty
//! extension:
//!
//! - no `[Collection]` is [`Parts::ALL`], because
//!   `e_collection_backend_get_part_enabled()` answers `TRUE` for a source
//!   without one;
//! - no `[Security]` is TLS, because `ESourceSecurity:secure` defaults to
//!   `FALSE`. To read that as "TLS off" would make every hand-written account
//!   less secure. [`SourceConfig`] has the same rule for the same reason;
//! - no `[Authentication]` is [`SourceError::MissingHost`]. An empty group
//!   gives the same result, but would change the file of the user.
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
    e_source_collection_get_type, e_source_get_enabled, e_source_security_get_secure,
    e_source_security_get_type,
};
use glib_sys::GFALSE;
use jmap_backend_core::marshal::{extension_if_present, read_string};
use jmap_backend_core::source::{ConnectTarget, SourceError, connect_target};
use jmap_collection_sync::Parts;
use jmap_collection_sync::child_source::Connection;

/// The server of the account, in the two shapes that are necessary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    /// Where this backend gets the session document: an explicit endpoint, or
    /// a bare domain for `_jmap._tcp` SRV autodiscovery.
    /// [`jmap_backend_core::source::connect_target`] decides it from the same
    /// fields as the [`connection`](Server::connection). Thus an address book
    /// or calendar backend that connects again gets the same answer.
    pub target: ConnectTarget,
    /// The same server field by field, for the children to copy. It is not
    /// derived from the target.
    /// [`Child::settings`](jmap_collection_sync::Child::settings) writes
    /// `[Authentication]` and `[Security]` keys. A parser that takes a URL
    /// apart again could disagree.
    pub connection: Connection,
    /// Whether this account rebases the URLs of the session onto the origin it
    /// connected through. See [`jmap_backend_core::rebase::rebase_urls`].
    pub rebase_urls: bool,
}

/// Which parts of the account are enabled, by the two rules of
/// `e_collection_backend_get_part_enabled()`.
///
/// # Safety
///
/// `source` must be a valid `ESource`: the collection source that EDS gave the
/// backend. It is only read from, and nothing outlives the call.
pub unsafe fn parts_of(source: *mut ESource) -> Parts {
    // `e_source_get_extension` finds only registered children of
    // `E_TYPE_SOURCE_EXTENSION` (as in `SourceConfig::from_source`). A
    // reference to the GType registers it.
    // SAFETY: no arguments, and the type system initialises itself.
    unsafe { e_source_collection_get_type() };

    // SAFETY: a valid source by the contract above, and a header constant.
    let source_enabled = unsafe { e_source_get_enabled(source) } != GFALSE;

    // Test for the extension and do not get it, so that a read does not write
    // to the account. See the module comment.
    // SAFETY: as above.
    let collection =
        unsafe { extension_if_present::<ESourceCollection>(source, E_SOURCE_EXTENSION_COLLECTION) }
            .map(|collection| {
                // SAFETY: the extension is present, so it belongs to the
                // source, which owns it and which outlives the call.
                unsafe {
                    Parts {
                        mail: e_source_collection_get_mail_enabled(collection) != GFALSE,
                        contacts: e_source_collection_get_contacts_enabled(collection) != GFALSE,
                        calendars: e_source_collection_get_calendar_enabled(collection) != GFALSE,
                    }
                }
            });

    Parts::from_collection(source_enabled, collection)
}

/// The user that the account authenticates as, or `None` for anonymous.
///
/// `populate` needs this one field of `[Authentication]`, and deliberately does
/// not call [`server_of`]. The choice between the two EDS "authenticate me"
/// calls depends only on whether a password is necessary. A broken host is for
/// `authenticate_sync` to report, because that vfunc has a `GError`. An
/// account with a user and an invalid host must get that error. A failed read
/// in `populate` must not make it anonymous.
///
/// `None` covers both forms of "no user": no `[Authentication]` group, and an
/// empty `User=` key. The second is important.
/// [`jmap_backend_core::connect::credentials`] decides what the account
/// authenticates *as*, and reads an empty user as anonymous through the same
/// [`read_string`]. If `populate` disagreed, it would ask EDS for a password
/// that `credentials` then discards.
///
/// # Safety
///
/// As [`parts_of`].
pub unsafe fn user_of(source: *mut ESource) -> Option<String> {
    // SAFETY: no arguments; registers the type the lookup below needs.
    unsafe { e_source_authentication_get_type() };

    // Test for the group and do not get it, as everywhere in this module:
    // `e_source_get_extension` would add the group to the account file.
    // SAFETY: a valid source by the contract above, and a header constant.
    let auth = unsafe {
        extension_if_present::<ESourceAuthentication>(source, E_SOURCE_EXTENSION_AUTHENTICATION)
    }?;
    // SAFETY: a live extension; the getter returns NULL or a string it owns.
    unsafe { read_string(e_source_authentication_get_user(auth)) }
}

/// The server that this account names, or why it names none.
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

    // Absent means TLS, and this test comes before any
    // `e_source_get_extension`. `ESourceSecurity:secure` defaults to FALSE, so
    // an unconditional read cannot tell an account without `[Security]` from
    // an account with TLS off.
    // SAFETY: a valid source by the contract above, and a header constant.
    let secure = match unsafe {
        extension_if_present::<ESourceSecurity>(source, E_SOURCE_EXTENSION_SECURITY)
    } {
        None => true,
        // SAFETY: a live extension of the type the name selects.
        Some(security) => unsafe { e_source_security_get_secure(security) != GFALSE },
    };

    // The same test, for the same reason. An account without
    // `[Authentication]` names no host, as an empty group does, and the file
    // of the user does not change.
    // SAFETY: as above.
    let Some(auth) = (unsafe {
        extension_if_present::<ESourceAuthentication>(source, E_SOURCE_EXTENSION_AUTHENTICATION)
    }) else {
        return Err(SourceError::MissingHost);
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

    // The one validation. The connect target of this backend and the host of
    // its children come from the same checked fields.
    let target = connect_target(host.as_deref(), port, secure)?;

    // SAFETY: `source` is valid by the contract of this function.
    let rebase_urls = unsafe { jmap_backend_core::rebase::rebase_urls(source) };

    Ok(Server {
        target,
        rebase_urls,
        connection: Connection {
            // `connect_target` returned Ok, so the host is present. The unwrap
            // is here, not earlier, so that `MissingHost` has one origin.
            host: host.expect("a validated target was built from a host"),
            // The keyfile writes 0 for "not set". A child with `Port=0` would
            // use port zero, not the default port of the scheme.
            port: (port != 0).then_some(port),
            user,
            auth_method,
            secure,
        },
    })
}
