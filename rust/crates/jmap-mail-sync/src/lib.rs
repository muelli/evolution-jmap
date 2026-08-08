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
use jmap_proto::Id;

pub use error::SyncError;
pub use folder::{FolderInfo, FolderRole, FolderTree};

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

    /// Every folder of the account — `get_folder_info_sync`.
    ///
    /// The whole tree in one `Mailbox/get`, not a subtree per call: JMAP has no
    /// way to ask for one level, mailbox lists are small, and Camel's
    /// `CAMEL_STORE_FOLDER_INFO_RECURSIVE` asks for all of it anyway.
    pub fn folder_tree(&self) -> Result<FolderTree, SyncError> {
        FolderTree::from_mailboxes(&self.client.mailboxes(&self.account_id)?)
    }
}
