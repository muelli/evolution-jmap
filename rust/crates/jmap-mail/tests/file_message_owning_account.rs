// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapStore::file_message`, routed to the mailbox's own account when it
//! belongs to a shared account merged into the folder tree.
//!
//! `tests/transfer.rs` covers `JmapStore::file_message` through the real
//! Camel vfunc; this file calls it directly, the same way
//! `import_owning_account.rs` does for `import_message`, to reach the
//! id-collision case no FFI test needs a live folder for.

mod common;

use common::sync_against;
use jmap_mail::store::JmapStore;
use jmap_mail_sync::Filing;
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;

/// No flags: the tree the store already has, with no new request. Matches
/// `import_owning_account.rs`'s `CACHED`.
const CACHED: eds_sys::CamelStoreGetFolderInfoFlags = 0;

/// The id collision: a shared mailbox can have the same mailbox id as a
/// mailbox of the store's own account. `JmapStore::file_message` must use
/// the owner the caller gives it, not look the destination mailbox id up in
/// the merged tree (`jmap_mail_sync::FolderTree::owning_account`), because
/// that lookup finds the store's own folder first and answers `None` for it
/// (an ordinary folder of the store's own account carries no `account_id`),
/// which then files the message into the wrong account. The source folder
/// Camel opened already knows the right one, the same shape
/// `import_owning_account.rs` proves for `import_message`.
#[test]
fn a_shared_mailboxs_filing_reaches_its_own_account_when_the_mailbox_ids_collide() {
    let server = MockServer::builder().start();

    let shared_account_id = Id::new("agent-shared-owner");
    let (inbox_id, archive_id, message_uid) = {
        let state = server.state();
        let mut state = state.lock().unwrap();

        let own = state
            .account_mut(&server.account_id())
            .expect("the mock's own account");
        let own_inbox_id = own.seed_mailbox("Inbox", None);
        let own_archive_id = own.seed_mailbox("Archive", None);

        let sharer = state.add_account(shared_account_id.clone(), "Bob");
        sharer.is_personal = false;
        let shared_inbox_id = sharer.seed_mailbox("Inbox", None);
        let shared_archive_id = sharer.seed_mailbox("Archive", None);
        let message_uid = sharer.seed_email(EmailSeed::new(
            shared_inbox_id.clone(),
            ("Bob", "bob@example.com"),
            "Invoice 42",
            "text",
            "2026-01-15T09:00:00Z",
        ));

        assert_eq!(
            own_inbox_id, shared_inbox_id,
            "the two accounts must number their mailboxes from the same id"
        );
        assert_eq!(
            own_archive_id, shared_archive_id,
            "the two accounts must number their mailboxes from the same id"
        );
        (shared_inbox_id, shared_archive_id, message_uid)
    };

    let store = JmapStore::detached();
    store.store_connection(sync_against(&server));
    store.folders(CACHED).expect("listed");

    let filing = Filing::moved(inbox_id, archive_id.clone());
    store
        .file_message(&message_uid, Some(&shared_account_id), &filing)
        .expect("the filing to reach the shared mailbox's own account");

    let state = server.state();
    let state = state.lock().unwrap();
    assert!(
        state
            .account(&server.account_id())
            .unwrap()
            .emails
            .get(&message_uid)
            .is_none(),
        "the filing used the wrong account despite the id collision"
    );
    let moved = state
        .account(&shared_account_id)
        .unwrap()
        .emails
        .get(&message_uid)
        .expect("the message stays in the shared account");
    assert_eq!(
        moved
            .mailbox_ids
            .as_ref()
            .and_then(|ids| ids.get(&archive_id)),
        Some(&true),
        "the message must land in the shared mailbox's own account"
    );
}
