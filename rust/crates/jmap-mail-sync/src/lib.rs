// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mail synchronisation, in the shape Camel asks for.
//!
//! One JMAP account is one `CamelJmapStore`, and this crate is what syncing it
//! means: which folders exist, what is in them, and what a message looks like.
//! Each entry point corresponds to one Camel vfunc — [`MailSync::folder_tree`]
//! to `get_folder_info_sync`, [`MailSync::messages`] to what a folder's
//! `CamelFolderSummary` is filled from — and more as the store grows.
//!
//! Like `jmap-book-sync` and `jmap-cal-sync`, it knows nothing about GObject
//! or the Camel headers, so the interesting half of the provider is testable
//! against `jmap-mockd` on any machine. The two mappings that have no
//! counterpart on the addressbook and calendar side, and are therefore where
//! the work is, are the path encoding — a mailbox name is a display string, a
//! Camel path is an identifier — and the tree itself, which JMAP models with parent
//! pointers and Camel with a linked forest.

pub(crate) mod date;
pub mod error;
pub mod folder;
pub mod message;
pub(crate) mod path;

use std::collections::BTreeMap;

use jmap_client::Client;
use jmap_proto::mail::EmailQueryFilter;
use jmap_proto::methods::Comparator;
use jmap_proto::{Id, State};

pub use error::SyncError;
pub use folder::{FolderInfo, FolderRole, FolderTree};
pub use message::{MessageFlags, MessageSummary, SUMMARY_PROPERTIES};

/// What a folder-list refresh found.
///
/// A delta is not applied folder by folder, so this is not one: a Camel path
/// is built from a mailbox's ancestors, and `Mailbox/changes` reporting a
/// renamed parent says nothing about the descendants whose paths just moved
/// with it. The account's mailbox list is one `Mailbox/get`, so the honest
/// answer to any change at all is the tree again — and the delta's real worth
/// is the case where it reports nothing, which is nearly every time it is
/// asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderUpdate {
    /// Nothing changed. The state carried is the one to ask from next time,
    /// which may be later than the one asked with.
    Unchanged(State),
    /// The tree as it is now, and the state that listing is current as of.
    Rebuilt { state: State, tree: FolderTree },
}

/// Synchronises one JMAP mail account.
pub struct MailSync {
    client: Client,
    account_id: Id,
}

impl MailSync {
    pub fn new(client: Client, account_id: Id) -> Self {
        Self { client, account_id }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn account_id(&self) -> &Id {
        &self.account_id
    }

    /// Every folder of the account, and the state the listing is current as
    /// of — `get_folder_info_sync`.
    ///
    /// The whole tree in one `Mailbox/get`, not a subtree per call: JMAP has no
    /// way to ask for one level, mailbox lists are small, and Camel's
    /// `CAMEL_STORE_FOLDER_INFO_RECURSIVE` asks for all of it anyway.
    ///
    /// The state comes back with it because a folder list without one can only
    /// ever be re-fetched in full; it is what [`MailSync::folder_tree_since`]
    /// takes.
    pub fn folder_tree(&self) -> Result<(State, FolderTree), SyncError> {
        let response = self.client.mailbox_get(&self.account_id)?;
        Ok((response.state, FolderTree::from_mailboxes(&response.list)?))
    }

    /// The folder tree again, but only if the account's mailboxes moved since
    /// `since` — the refresh half of `get_folder_info_sync`.
    ///
    /// One `Mailbox/changes` for the common case, which is a store asking
    /// whether anything happened and being told no. Anything else costs the
    /// full listing, for the reason [`FolderUpdate`] gives.
    ///
    /// A state the server cannot calculate from — too old, or from some other
    /// server entirely — is answered with the listing rather than reported.
    /// The EDS meta backends pass that condition up because EDS knows how to
    /// diff a collection against its cache; Camel has nothing of the kind, so
    /// a store that reported it would be a folder tree that never recovers.
    pub fn folder_tree_since(&self, since: &State) -> Result<FolderUpdate, SyncError> {
        match self.client.all_changes(&self.account_id, "Mailbox", since) {
            Ok(changes) if changes.is_empty() => Ok(FolderUpdate::Unchanged(changes.new_state)),
            Ok(_) => self.rebuild(),
            Err(error) if error.is_cannot_calculate_changes() => self.rebuild(),
            Err(error) => Err(error.into()),
        }
    }

    /// The listing, labelled with its own state rather than the delta's: the
    /// tree is what was walked, and the account may have moved again between
    /// the two calls.
    fn rebuild(&self) -> Result<FolderUpdate, SyncError> {
        let (state, tree) = self.folder_tree()?;
        Ok(FolderUpdate::Rebuilt { state, tree })
    }

    /// Every message in one mailbox, oldest first — what a folder's summary is
    /// filled from.
    ///
    /// Two steps, not the one round-trip `Email/query`+`Email/get`
    /// back-reference the client also offers: chaining them sends every
    /// matching id straight into the `/get`, and a mailbox may hold more ids
    /// than one `/get` is allowed to name. Asking first and fetching second is
    /// what makes the fetch divisible.
    ///
    /// Oldest first because that is the order a summary is built in and the
    /// order Camel numbers messages in, and `receivedAt` rather than the `Date`
    /// header because the header is the sender's clock — a message with a wrong
    /// one would sort into the wrong place forever.
    pub fn messages(&self, mailbox: &Id) -> Result<Vec<MessageSummary>, SyncError> {
        let ids = self.message_ids(mailbox)?;

        // `/get` may answer in any order (RFC 8620 §5.1), so the query's order
        // is restored below rather than assumed here.
        let mut by_uid: BTreeMap<Id, MessageSummary> = BTreeMap::new();
        for chunk in ids.chunks(self.objects_in_get()) {
            for email in self
                .client
                .email_get(&self.account_id, chunk, Some(SUMMARY_PROPERTIES))?
            {
                let summary = MessageSummary::from_email(&email)?;
                by_uid.insert(summary.uid.clone(), summary);
            }
        }

        // An id the query named and the `/get` did not answer for is a message
        // deleted between the two calls: it is gone, which is not a failure and
        // not something to keep a row for. `remove` also settles the other side
        // of the same race — a message that shifted position and came back on
        // two pages is listed once.
        Ok(ids.iter().filter_map(|id| by_uid.remove(id)).collect())
    }

    /// The ids of a mailbox's messages, oldest first, however many pages the
    /// server answers in.
    fn message_ids(&self, mailbox: &Id) -> Result<Vec<Id>, SyncError> {
        let mut ids: Vec<Id> = Vec::new();

        for _ in 0..MAX_QUERY_PAGES {
            let response = self.client.email_query(
                &self.account_id,
                EmailQueryFilter::in_mailbox(mailbox.clone()),
                Some(vec![Comparator::ascending("receivedAt")]),
                None,
                ids.len() as i64,
            )?;
            let capped = response.limit.is_some();
            let answered = !response.ids.is_empty();
            ids.extend(response.ids);
            // No cap means the whole rest of the result set is in hand; a cap
            // that came back empty means the rest of it is nothing.
            if !capped || !answered {
                return Ok(ids);
            }
        }
        Err(SyncError::protocol(
            "Email/query never stopped reporting a limited answer",
        ))
    }

    /// How many ids one `Email/get` of this account may name.
    ///
    /// The server's `maxObjectsInGet` if it published one — asking for more is
    /// a `requestTooLarge` that fails the whole call rather than a short
    /// answer — and otherwise a conservative guess, because RFC 8620 §2
    /// requires the limit to be there and a server that omits it has told us
    /// nothing about what it will take.
    ///
    /// Capped from above as well as below: a server may advertise a limit far
    /// larger than a mailbox, and one `/get` for fifty thousand messages is a
    /// response Evolution waits on with the folder half-open. Chunking bounds
    /// what is in flight at the cost of round-trips it would otherwise make
    /// anyway.
    fn objects_in_get(&self) -> usize {
        let advertised = self
            .client
            .session()
            .max_objects_in_get()
            .and_then(|limit| usize::try_from(limit).ok())
            .filter(|limit| *limit > 0)
            .unwrap_or(FALLBACK_OBJECTS_IN_GET);
        advertised.min(MAX_OBJECTS_PER_GET)
    }
}

/// A server that answers every `Email/query` with a limited page, without ever
/// running out of ids, would otherwise hang the calling thread. Far above any
/// real mailbox at any real page size; reaching it means the server is broken.
const MAX_QUERY_PAGES: usize = 1024;

/// What to assume when the server does not publish `maxObjectsInGet`. Small
/// enough that no plausible server rejects it, at the price of more round-trips
/// for a server that broke the rules.
const FALLBACK_OBJECTS_IN_GET: usize = 50;

/// The most this client asks for in one `Email/get`, however much the server
/// allows.
const MAX_OBJECTS_PER_GET: usize = 500;
