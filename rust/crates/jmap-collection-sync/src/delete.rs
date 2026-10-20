// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deletion of a collection *from the server*: the opposite of
//! [`create`](crate::create), and the destructive half.
//!
//! Evolution's "Delete" on an address book or a calendar of a JMAP account
//! calls `ECollectionBackendClass::delete_resource_sync`. This module makes the
//! decision in it: which JMAP account has the collection, and which `/set` call
//! destroys it. As for [`create_collection`](crate::create_collection), the code
//! is here and not in the collection backend, so tests can run it against a
//! running `jmap-mockd` without the EDS headers. The backend keeps two steps:
//! - it reads the collection to delete from the child `ESource`;
//! - it removes that source after the destroy.
//!
//! ## The kind is carried, never inferred from the id
//!
//! [`Doomed`] is a *pair* of kind and id, for the reason that
//! [`children`](crate::children) gives. JMAP ids are scoped to an account and
//! an **object type** (RFC 8620 §1.2). Thus an `AddressBook` and a `Calendar`
//! can both be `X1`, and on a server that numbers its objects from one, they
//! will be. A delete that selects the `/set` call by something other than the
//! kind that the source states destroys the wrong object and reports success.
//! The kind comes from the same `[Address Book]`/`[Calendar]` extension that
//! `dup_resource_id` reads, so it is the answer of the source, not a guess.
//!
//! ## A refusal is an error
//!
//! The read direction of [`crate::resources`] can try again at the next
//! populate. This module cannot. If the destroy fails, the collection is still
//! on the server, and the vfunc must report this. If it answers `TRUE`, EDS
//! removes the child source of a collection that still exists. The next
//! populate then finds the collection again and writes a *new* source for it.
//! The delete seems to succeed, then reverses itself, and the source loses its
//! uid and offline cache.

use jmap_client::Client;
use jmap_proto::Id;

use crate::children::{ChildKind, split_shared_identity};
use crate::layout::CollectionLayout;

/// The collection to delete: its kind, and which collection of that kind.
///
/// The kind is half of the identity, not a hint (see the module comment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doomed {
    pub kind: ChildKind,
    /// The value of `[Resource] Identity` on the child source: the JMAP id of a
    /// collection of the user, or `<accountId>/<collectionId>` for a collection
    /// shared with the user.
    pub collection_id: Id,
}

/// Why a collection could not be deleted.
///
/// The two variants of [`CreateFailure`](crate::CreateFailure), for the same
/// reasons, and [`DeleteFailure::Unshared`] for shared collections. This code
/// gives `Unserved` and `Unshared`. All other failures come from the server.
#[derive(Debug)]
pub enum DeleteFailure {
    /// The login has no JMAP account that serves collections of that kind, so
    /// no collection exists to delete.
    Unserved(ChildKind),
    /// The collection is shared, and its account no longer shares collections
    /// of that kind with this login.
    Unshared(ChildKind),
    /// The server refused the destroy, or could not be reached.
    Client(jmap_client::Error),
}

impl From<jmap_client::Error> for DeleteFailure {
    fn from(error: jmap_client::Error) -> Self {
        Self::Client(error)
    }
}

impl std::fmt::Display for DeleteFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unserved(kind) => write!(
                f,
                "this account's JMAP server offers no {} to delete one from",
                match kind {
                    ChildKind::AddressBook => "contacts",
                    ChildKind::Calendar => "calendars",
                }
            ),
            Self::Unshared(kind) => write!(
                f,
                "the account this {} was shared from no longer shares it with this login",
                match kind {
                    ChildKind::AddressBook => "address book",
                    ChildKind::Calendar => "calendar",
                }
            ),
            Self::Client(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for DeleteFailure {}

/// Destroys `doomed` on the server.
///
/// [`CollectionLayout`] resolves the account, as for a discovery and a create.
/// Here the reason is stronger: in the primary account of the session, the same
/// id can be a *different* collection, and the destroy would delete it.
///
/// The identity of a shared collection, `<accountId>/<collectionId>`, names its
/// account. The destroy goes to that account only if the session still lists it
/// as shared with this login and offering this kind. A withdrawn share gives
/// [`DeleteFailure::Unshared`], never a retry against the primary account.
pub fn delete_collection(client: &Client, doomed: &Doomed) -> Result<(), DeleteFailure> {
    let session = client.session();
    let kind = doomed.kind;
    let identity = doomed.collection_id.to_string();
    let refuse = |error: DeleteFailure| {
        tracing::warn!(?kind, collection_id = identity, %error, "collection destroy failed");
        error
    };
    let (account, collection) = match split_shared_identity(&identity) {
        Some((shared, collection)) => {
            let account = CollectionLayout::shared_accounts(session)
                .into_iter()
                .find(|service| service.account.id.as_str() == shared && service.offers(kind))
                .map(|service| service.account)
                .ok_or_else(|| refuse(DeleteFailure::Unshared(kind)))?;
            (account, Id::new(collection))
        }
        None => {
            let layout = CollectionLayout::from_session(session);
            let account = layout
                .account_for(kind)
                .cloned()
                .ok_or_else(|| refuse(DeleteFailure::Unserved(kind)))?;
            (account, doomed.collection_id.clone())
        }
    };
    let collection_id = collection.to_string();

    let account_id = account.id.to_string();
    tracing::debug!(account_id, ?kind, collection_id, "destroying collection");

    match doomed.kind {
        ChildKind::AddressBook => {
            client
                .address_book_destroy(&account.id, &collection)
                .inspect_err(|error| {
                    tracing::warn!(account_id, ?kind, collection_id, %error, "collection destroy failed");
                })?;
        }
        ChildKind::Calendar => {
            client
                .calendar_destroy(&account.id, &collection)
                .inspect_err(|error| {
                    tracing::warn!(account_id, ?kind, collection_id, %error, "collection destroy failed");
                })?;
        }
    }

    Ok(())
}
