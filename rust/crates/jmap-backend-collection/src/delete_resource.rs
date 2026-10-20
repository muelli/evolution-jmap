// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deletes an address book or a calendar *on the server*. This is the opposite
//! operation of [`crate::create_resource`], and the destructive one.
//!
//! All other code in this crate, except the create, copies what the server has.
//! The create adds an object to the server. This module removes an object from
//! the server, when the user selects "Delete" on an address book or a calendar of
//! a JMAP account. `ECollectionBackendClass::delete_resource_sync` is that
//! request, and [`crate::backend`] sets the slot.
//!
//! As with the create, this module contains the EDS parts: it finds the
//! collection of the child `ESource`, and it sets the flag that makes the menu
//! item available. [`jmap_collection_sync::delete`] makes the decision between
//! these parts (which JMAP account, which `/set` destroy). That crate needs no
//! headers, and its tests use a running `jmap-mockd`.
//!
//! ## The obligation, from the EDS documentation of the vfunc
//!
//! `e_collection_backend_delete_resource_sync()`: *"After the server-side
//! resource is successfully deleted, the implementor must also remove @source
//! from the @backend's #ECollectionBackend:server."* Thus
//! [`crate::removal::remove_source`] comes after the destroy, never before it.
//! A populate uses the same `e_source_remove_sync` to remove an obsolete child,
//! and `ews_backend_delete_resource_sync` also ends with it.
//!
//! The order is the only error handling. Consider a removal of the source before
//! a destroy that then fails. The address book is then gone from the sidebar but
//! stays on the server. The next populate adds it again under a *new* uid, and
//! the offline cache of the old source is lost without a reason. Now consider a
//! destroy that succeeds before a removal that then fails. This is the
//! recoverable order: the child is stale, and the next populate finds that the
//! collection is gone and removes it through [`crate::removal`]. Until then, the
//! user has an error message.
//!
//! ## This override does not call the parent implementation
//!
//! The reason is the same as in [`crate::create_resource`], and it comes from the
//! same source. `collection_backend_delete_resource()` does only one thing:
//! `g_task_return_new_error (G_IO_ERROR_NOT_SUPPORTED, "%s does not support
//! deleting remote resources")`. The parent implementation *is* the refusal that
//! this override replaces.
//!
//! ## `remote-deletable` is a flag of the child, not of the account
//!
//! This is the opposite of `remote-creatable`, which [`crate::populate`] sets on
//! the account source. `server_side_source_remote_delete_sync()` examines the flag
//! of the *child* and refuses before it looks for a backend. Without
//! [`offer_deletion`], this vfunc is unreachable. If the flag is set on incorrect
//! sources, the result is worse.
//!
//! The `child_added` of [`crate::backend`] sets the flag. EDS calls `child_added`
//! for each source that appears under a collection: the children that a fan-out
//! wrote, the cached children that a populate exported before it contacted a
//! server, and the child that a create published. EDS also sets
//! `removable = FALSE` on each child there, for the same reason (one place for
//! all children). evolution-ews sets the flag at each of the three sites that
//! create a child. One shared place gives the same behaviour, and no site can be
//! forgotten.
//!
//! **This place writes the permissive default, not the final value.** It answers
//! the *identity* question above: is this source a collection that this backend
//! wrote? It does not read `myRights`, because a cached or newly published child
//! gets to it before any server contact, or without one. The only use of rights
//! here is that it does not offer a shared child (see [`offer_deletion`]).
//! After a fan-out writes a child (new or found again), [`crate::fan_out::adopt`]
//! changes that default to the per-resource `myRights.mayDelete` through
//! [`Collection::set_remote_deletable`](crate::fan_out::Collection::set_remote_deletable).
//! That is also the only place where a rights *change* on a child that exists
//! gets to the flag, because `child_added` fires only once in the life of a
//! source in a running registry.
//!
//! ## The source itself tells what can be deleted
//!
//! [`doomed_of`] returns `None` for each source that is not a child that this
//! backend wrote: a source without an `[Address Book]`/`[Calendar]` extension, or
//! without `[Resource] Identity`. Both the flag and the vfunc use this check.
//! This is necessary: `child_added` also fires for the mail sources of this
//! account, and the vfunc gets the source that the user selects. An incorrect
//! guess does not cause only an incorrect error message. It sends a destroy to a
//! JMAP server with an id from the keyfile of a different source.
//!
//! The kind is half of that identity, and the code never derives it from the id.
//! [`jmap_collection_sync::delete`] tells why an address book and a calendar can
//! have the same id.

use std::fmt;

use eds_sys::{ESource, e_server_side_source_set_remote_deletable};
use glib_sys::{GError, GTRUE};
use jmap_backend_core::error::{invalid_arg_gerror, to_gerror};
use jmap_backend_core::i18n::{translate, translate_with};
use jmap_backend_core::source::{self, ConnectTarget};
use jmap_client::Credentials;
use jmap_collection_sync::{
    DeleteFailure, Doomed, delete_collection, parse_resource_id, split_shared_identity,
};

use crate::create_resource::collection_of;
use crate::resource_id::resource_id_of;

/// The reason why a delete failed.
#[derive(Debug)]
pub enum DeleteError {
    /// The source is not a collection that this backend wrote, so this account
    /// has nothing to destroy for it. Evolution cannot cause this error, because
    /// [`offer_deletion`] uses the same check, and thus such a source has no menu
    /// item. It is an error and not a silent success, because it shows that a
    /// different problem exists.
    NotOurs,
    /// The account could not supply a login: it has a server that it must not
    /// contact, or it has no credentials.
    Login(crate::authenticate::LoginError),
    /// The connection or the destroy failed, or the server of the account has no
    /// account for this kind of collection.
    Server(DeleteFailure),
    /// The server no longer has the collection, and EDS did not remove the source
    /// of the collection. Contains the reason from EDS.
    Stale(String),
}

impl From<crate::authenticate::LoginError> for DeleteError {
    fn from(failure: crate::authenticate::LoginError) -> Self {
        Self::Login(failure)
    }
}

impl From<DeleteFailure> for DeleteError {
    fn from(failure: DeleteFailure) -> Self {
        Self::Server(failure)
    }
}

impl From<jmap_client::Error> for DeleteError {
    fn from(error: jmap_client::Error) -> Self {
        Self::Server(DeleteFailure::Client(error))
    }
}

impl fmt::Display for DeleteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOurs => f.write_str(&translate(
                // TRANSLATORS: shown when Evolution asks a JMAP account to
                // delete something that is not one of the address books or
                // calendars that account created.
                c"this is not an address book or a calendar of this JMAP account",
            )),
            Self::Login(failure) => failure.fmt(f),
            // `Unserved` comes from `jmap-collection-sync`, which builds without
            // gettext. Its `Display` is for developers, so this code writes the
            // text for the user, with the translated nouns of all other account
            // errors.
            Self::Server(DeleteFailure::Unserved(kind)) => f.write_str(&translate_with(
                // TRANSLATORS: %1$s is "address book" or "calendar". Shown when
                // the JMAP server behind an account offers no account of that
                // kind at all, so the collection cannot be there to delete.
                c"this account's JMAP server has no %1$s to delete",
                &[collection_of(*kind).noun().as_str()],
            )),
            Self::Server(DeleteFailure::Unshared(kind)) => f.write_str(&translate_with(
                // TRANSLATORS: %1$s is "address book" or "calendar". Shown when
                // the user deletes a JMAP address book or calendar that someone
                // shared with them, and that person no longer shares it.
                c"this %1$s is no longer shared with this JMAP account, so it cannot be deleted",
                &[collection_of(*kind).noun().as_str()],
            )),
            Self::Server(failure) => failure.fmt(f),
            // The msgid below is intentionally one long line. For a `\`
            // continuation, Rust removes the newline *and* the leading spaces of
            // the next line, but xgettext removes neither. Thus no translation
            // matches a wrapped literal.
            Self::Stale(reason) => f.write_str(&translate_with(
                // TRANSLATORS: %1$s is a developer-facing reason from Evolution
                // Data Server. Shown when the collection was deleted on the
                // server and the local source describing it stayed behind.
                c"the collection was deleted on the server, but the source for it could not be removed: %1$s",
                &[reason.as_str()],
            )),
        }
    }
}

impl std::error::Error for DeleteError {}

impl DeleteError {
    /// Returns the `GError` that the vfunc uses to report this error.
    ///
    /// The split is the same as in [`CreateError::to_gerror`], for the same
    /// reason. A `jmap_client::Error` goes through the mapping that all other
    /// operations in these backends use. Thus a 401 on a delete becomes the
    /// usual Evolution error code for a 401. All other errors become
    /// `E_CLIENT_ERROR_INVALID_ARG`.
    ///
    /// [`CreateError::to_gerror`]: crate::create_resource::CreateError::to_gerror
    pub fn to_gerror(&self) -> *mut GError {
        match self {
            Self::Login(failure) => failure.to_gerror(),
            Self::Server(DeleteFailure::Client(error)) => to_gerror(error),
            other => invalid_arg_gerror(&other.to_string()),
        }
    }
}

/// Returns the collection of `source`, or `None` for a source that this backend
/// did not write.
///
/// This function reads through [`resource_id_of`] and [`parse_resource_id`], not
/// directly from the extensions. Thus the delete uses the same string that
/// `dup_resource_id` gives EDS for this child. A second implementation of that
/// read could give a different answer, and then the delete would destroy a
/// different object.
///
/// # Safety
///
/// `source` must be NULL or a valid `ESource` that outlives the call. The
/// function only reads it and adds no extension to it. This is important,
/// because callers also use it for sources of other parts of Evolution.
pub unsafe fn doomed_of(source: *mut ESource) -> Option<Doomed> {
    // SAFETY: the contract of the caller is the contract of `resource_id_of`.
    let resource_id = unsafe { resource_id_of(source) }?;
    let (kind, collection_id) = parse_resource_id(&resource_id)?;
    Some(Doomed {
        kind,
        collection_id,
    })
}

/// Offers `child` for deletion if it is a collection that this backend wrote.
/// Returns `true` if it offered the child.
///
/// `child_added` calls this function for each source that appears under this
/// collection. The module comment tells why that is the only place, and why a
/// source of a different owner must not be offered.
///
/// The function only *sets* the flag. It does not write `FALSE` to a source that
/// returns `None`. That source can belong to a different part of Evolution that
/// set the flag to `TRUE` for its own reasons. This function must not remove that
/// value.
///
/// # Safety
///
/// `child` must be NULL or a valid `EServerSideSource` that outlives the call.
/// Each child of a collection is one, because `evolution-source-registry` has no
/// other type. For a plain `ESource`, EDS logs a `g_return_if_fail` critical and
/// no undefined behaviour occurs. But the contract of this function still does
/// not permit it.
pub unsafe fn offer_deletion(child: *mut ESource) -> bool {
    // SAFETY: the contract of the caller is the contract of `doomed_of`,
    // including NULL.
    let doomed = unsafe { doomed_of(child) };
    // The fan-out decides for a shared child. It offers the child only if
    // `myRights.mayDelete` of the owner permits it. A delete removes the
    // collection of the owner for all users, so the identity is not sufficient.
    let deletable =
        doomed.is_some_and(|doomed| split_shared_identity(doomed.collection_id.as_str()).is_none());
    tracing::debug!(
        remote_deletable = deletable,
        "evaluating child source remote-deletable offer"
    );
    if !deletable {
        return false;
    }

    // SAFETY: a valid `EServerSideSource` because of the contract of this
    // function. It is not NULL, because `doomed_of` returned `Some` for it.
    unsafe { e_server_side_source_set_remote_deletable(child.cast(), GTRUE) };
    true
}

/// Connects with `target` and `credentials`, and destroys `doomed` on that server.
///
/// This is the opposite operation of [`crate::create_resource::create_on_server`].
/// It is a separate function for the same reason: the vfunc has one operation to
/// do with a [`Login`](crate::authenticate::Login), and one error type to report.
/// [`jmap_backend_core::source::connect`] makes the connection, so an account with
/// only a domain gets the same `_jmap._tcp` autodiscovery as a fan-out.
/// [`delete_collection`] does the destroy.
pub fn delete_on_server(
    target: &ConnectTarget,
    rebase_urls: bool,
    credentials: Credentials,
    doomed: &Doomed,
) -> Result<(), DeleteError> {
    let client = source::connect(target, rebase_urls, credentials)?;
    delete_collection(&client, doomed)?;
    tracing::debug!(
        collection_id = doomed.collection_id.as_str(),
        kind = ?doomed.kind,
        "deleted collection resource on server"
    );
    Ok(())
}

#[cfg(test)]
mod delete_error_display_tests {
    use jmap_backend_core::source::SourceError;
    use jmap_collection_sync::{ChildKind, DeleteFailure};

    use super::*;
    use crate::authenticate::LoginError;

    /// The `Display` of `DeleteError` writes text for each variant. Without this
    /// test, a `Display` that writes nothing still compiles, and a refused delete
    /// shows an empty error message.
    #[test]
    fn every_variant_displays_a_non_empty_message() {
        assert!(!DeleteError::NotOurs.to_string().is_empty());

        let login = DeleteError::Login(LoginError::Account(SourceError::MissingHost));
        assert_eq!(login.to_string(), SourceError::MissingHost.to_string());

        let unserved = DeleteError::Server(DeleteFailure::Unserved(ChildKind::Calendar));
        assert!(unserved.to_string().contains("calendar"));

        let unshared = DeleteError::Server(DeleteFailure::Unshared(ChildKind::AddressBook));
        assert!(unshared.to_string().contains("address book"));

        let client = DeleteError::Server(DeleteFailure::Client(jmap_client::Error::Protocol(
            "boom".to_owned(),
        )));
        assert!(!client.to_string().is_empty());

        let stale = DeleteError::Stale("gvfs mount busy".to_owned());
        assert!(stale.to_string().contains("gvfs mount busy"));
    }
}

#[cfg(test)]
mod delete_error_to_gerror_tests {
    use jmap_backend_core::marshal::read_string;

    use super::*;

    /// `NotOurs` goes through `invalid_arg_gerror`, as do all variants except
    /// `Login` and a server `Client` failure. Without this test, a `to_gerror`
    /// that always returns the null pointer causes no failure.
    #[test]
    fn not_ours_carries_the_invalid_arg_code_and_its_own_message() {
        let error = DeleteError::NotOurs;
        let gerror = error.to_gerror();
        assert!(!gerror.is_null());
        // SAFETY: a newly allocated GError that this test owns.
        unsafe {
            assert_eq!((*gerror).domain, eds_sys::e_client_error_quark());
            assert_eq!((*gerror).code, eds_sys::E_CLIENT_ERROR_INVALID_ARG as i32);
            let message = read_string((*gerror).message).unwrap();
            assert_eq!(message, error.to_string());
            glib_sys::g_error_free(gerror);
        }
    }
}
