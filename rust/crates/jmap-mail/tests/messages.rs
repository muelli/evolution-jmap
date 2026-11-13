// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapStore::messages` and `JmapStore::messages_since`: the listing and the
//! delta that fill a folder's summary, routed to the mailbox's own account
//! when it belongs to a shared account merged into the folder tree.

mod common;

use common::sync_against;
use jmap_mail::store::JmapStore;
use jmap_mail_sync::MessageUpdate;
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;

/// No flags: the tree the store already has, with no new request. Matches
/// `subscriptions.rs`'s `CACHED`.
const CACHED: eds_sys::CamelStoreGetFolderInfoFlags = 0;

/// ROADMAP item 179(d): `Email/query` and `Email/get` are method calls under
/// one `accountId`, so listing a
/// [`jmap_mail_sync::FolderTree::merge_shared`] mailbox must reach that
/// mailbox's own account, not the account this store otherwise addresses.
/// Unlike `expunge.rs`'s own shared-owner test, this lists first
/// (`store.folders(CACHED)`): `messages` resolves the owner from the held
/// listing only, not a fresh one, so a disconnect never waits out an
/// in-flight listing (`disconnect_window.rs`); a real `refresh_info_sync`
/// call only ever names a mailbox Camel already saw in a listing.
#[test]
fn messages_lists_the_shared_mailboxs_owning_account() {
    let server = MockServer::builder().start();

    let shared_account_id = Id::new("agent-shared-owner");
    let shared_inbox_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let sharer = state.add_account(shared_account_id.clone(), "Bob");
        sharer.is_personal = false;
        let inbox = sharer.seed_mailbox("Inbox", None);
        sharer.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Bob", "bob@example.com"),
            "Lunch?",
            "One o'clock.",
            "2026-01-15T09:30:00Z",
        ));
        inbox
    };

    let store = JmapStore::detached();
    store.store_connection(sync_against(&server));
    store.folders(CACHED).expect("listed");

    let (_, messages) = store
        .messages(&shared_inbox_id)
        .expect("the listing to reach the shared mailbox's own account");

    assert_eq!(
        messages.len(),
        1,
        "the message lives in the shared account, not this store's own"
    );
}

/// ROADMAP item 179(d), the delta half: `Email/changes` and `Email/get` are
/// also method calls under one `accountId`, so [`JmapStore::messages_since`]
/// needs the same routing as [`JmapStore::messages`], resolved from the held
/// listing for the same reason.
#[test]
fn messages_since_reads_the_shared_mailboxs_owning_account() {
    let server = MockServer::builder().start();

    let shared_account_id = Id::new("agent-shared-owner");
    let shared_inbox_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let sharer = state.add_account(shared_account_id.clone(), "Bob");
        sharer.is_personal = false;
        sharer.seed_mailbox("Inbox", None)
    };

    let store = JmapStore::detached();
    store.store_connection(sync_against(&server));
    store.folders(CACHED).expect("listed");

    let (since, _) = store
        .messages(&shared_inbox_id)
        .expect("the first listing of the shared mailbox");

    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let sharer = state.account_mut(&shared_account_id).unwrap();
        sharer.deliver_email(EmailSeed::new(
            shared_inbox_id.clone(),
            ("Bob", "bob@example.com"),
            "Lunch?",
            "One o'clock.",
            "2026-01-15T09:30:00Z",
        ));
    }

    let update = store
        .messages_since(&shared_inbox_id, &since, 0)
        .expect("the delta to reach the shared mailbox's own account");

    match update {
        MessageUpdate::Changed { present, .. } => {
            assert_eq!(present.len(), 1, "the new message must be reported");
        }
        MessageUpdate::Relisted { messages, .. } => {
            assert_eq!(messages.len(), 1, "the new message must be reported");
        }
        other => panic!("expected the new message to be reported, got {other:?}"),
    }
}
