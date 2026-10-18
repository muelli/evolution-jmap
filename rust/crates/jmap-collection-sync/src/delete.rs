// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deleting a collection *from the server* — [`create`](crate::create)'s mirror,
//! and the destructive half of it.
//!
//! Evolution's "Delete" on an address book or a calendar of a JMAP account
//! reaches `ECollectionBackendClass::delete_resource_sync`, and this is the
//! decision inside it: which JMAP account the collection lives in, and which
//! `/set` call destroys it. As with [`create_collection`](crate::create_collection)
//! it lives here rather than in the collection backend so that it is testable
//! against a running `jmap-mockd` with no EDS headers in sight; what the backend
//! keeps is reading the doomed collection off the child `ESource` and removing
//! that source afterwards.
//!
//! ## The kind is carried, never inferred from the id
//!
//! [`Doomed`] is a *pair* — kind and id — for the reason
//! [`children`](crate::children) spells out: JMAP ids are scoped to an account
//! and an **object type** (RFC 8620 §1.2), so an `AddressBook` and a `Calendar`
//! may both be `X1`, and on a server that numbers its objects from one they will
//! be. A delete that took only the id and picked a `/set` call by anything other
//! than the kind the source itself states would destroy the wrong object and
//! report success. The kind comes out of the same `[Address Book]`/`[Calendar]`
//! extension `dup_resource_id` reads, so it is the source's own answer rather
//! than a guess.
//!
//! ## A refusal is an error, not a shrug
//!
//! Unlike [`crate::resources`]'s read direction, there is no "try again next
//! populate" here. If the destroy fails, the collection is still on the server,
//! and the vfunc must say so: answering `TRUE` would have EDS remove the child
//! source for a collection that goes on existing, which the next populate
//! rediscovers and writes a *new* source for — a delete that appeared to work,
//! undid itself, and lost the source's uid and offline cache on the way.

use jmap_client::Client;
use jmap_proto::Id;

use crate::children::{ChildKind, split_shared_identity};
use crate::layout::CollectionLayout;

/// The collection a delete is about: which kind, and which one of that kind.
///
/// The kind is half of the identity and not a hint — see the module comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doomed {
    pub kind: ChildKind,
    /// What `[Resource] Identity` carries on the child source: the JMAP id of
    /// one of the user's own collections, or `<accountId>/<collectionId>` for
    /// one shared with the user.
    pub collection_id: Id,
}

/// Why a collection could not be deleted.
///
/// The same two shapes [`CreateFailure`](crate::CreateFailure) has, and for the
/// same reasons: an account that serves no collections of that kind is this
/// code's answer, and everything else is the server's.
#[derive(Debug)]
pub enum DeleteFailure {
    /// The login holds no JMAP account serving collections of that kind, so
    /// there is nothing there to delete.
    Unserved(ChildKind),
    /// The collection is a shared one, and the account it was shared from no
    /// longer shares collections of that kind with this login.
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
/// The account is resolved through [`CollectionLayout`] exactly as a discovery
/// and a create resolve it, and for a sharper reason than either: falling back
/// to the session's primary account would send a destroy naming this id to an
/// account in which that id is some *other* collection.
///
/// A shared collection's identity, `<accountId>/<collectionId>`, names its
/// account. The destroy goes there, and only if the session still lists that
/// account as shared with this login and offering this kind. A withdrawn share
/// is [`DeleteFailure::Unshared`], never a retry against the primary account.
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
