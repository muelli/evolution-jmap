// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mail synchronisation, in the shape Camel asks for.
//!
//! One JMAP account is one `CamelJmapStore`, and this crate is what syncing it
//! means: which folders exist, what is in them, and what a message looks like.
//! Each entry point corresponds to one Camel vfunc — [`MailSync::folder_tree`]
//! to `get_folder_info_sync`, and more as the store grows.
//!
//! Like `jmap-book-sync` and `jmap-cal-sync`, it knows nothing about GObject
//! or the Camel headers, so the interesting half of the provider is testable
//! against `jmap-mockd` on any machine. The two mappings that have no
//! counterpart on the addressbook and calendar side, and are therefore where
//! the work is, are the path encoding — a mailbox name is a display string, a
//! Camel path is an identifier — and the tree itself, which JMAP models with parent
//! pointers and Camel with a linked forest.

pub mod error;
pub mod folder;
pub(crate) mod path;

use jmap_client::Client;
use jmap_proto::{Id, State};

pub use error::SyncError;
pub use folder::{FolderInfo, FolderRole, FolderTree};

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
}
