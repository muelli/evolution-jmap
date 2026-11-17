// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapStore::search`, routed to the mailbox's own account when it belongs
//! to a shared account merged into the folder tree.
//!
//! `tests/search.rs` covers `JmapStore::search` through the real Camel
//! folder FFI; this file calls it directly, the same way `tests/messages.rs`
//! calls `JmapStore::messages` directly, since the owning-account routing
//! does not need a `CamelFolder` at all.

mod common;

use common::sync_against;
use jmap_mail::store::JmapStore;
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::EmailQueryFilter;
use jmap_proto::methods::Filter;

/// No flags: the tree the store already has, with no new request. Matches
/// `subscriptions.rs`'s `CACHED`.
const CACHED: eds_sys::CamelStoreGetFolderInfoFlags = 0;

/// ROADMAP item 179(d): `Email/query` is a method call under one
/// `accountId`, so searching a
/// [`jmap_mail_sync::FolderTree::merge_shared`] mailbox must reach that
/// mailbox's own account, not the account this store otherwise addresses.
/// As in `tests/messages.rs`, this lists first (`store.folders(CACHED)`):
/// `search` resolves the owner from the held listing only, not a fresh one,
/// since a real `search_by_expression`/`search_by_uids` call only ever
/// names a mailbox Camel already saw in a listing.
#[test]
fn search_reaches_the_shared_mailboxs_owning_account() {
    let server = MockServer::builder().start();

    let shared_account_id = Id::new("agent-shared-owner");
    let (shared_inbox_id, wanted) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let sharer = state.add_account(shared_account_id.clone(), "Bob");
        sharer.is_personal = false;
        let inbox = sharer.seed_mailbox("Inbox", None);
        let wanted = sharer.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Bob", "bob@example.com"),
            "Invoice 42",
            "text",
            "2026-01-15T09:00:00Z",
        ));
        sharer.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Bob", "bob@example.com"),
            "Lunch?",
            "text",
            "2026-01-15T09:00:00Z",
        ));
        (inbox, wanted)
    };

    let store = JmapStore::detached();
    store.store_connection(sync_against(&server));
    store.folders(CACHED).expect("listed");

    let filter = Filter::condition(EmailQueryFilter::default().subject("Invoice"));
    let matches = store
        .search(&shared_inbox_id, Some(&shared_account_id), filter)
        .expect("the search to reach the shared mailbox's own account");

    assert_eq!(
        matches,
        vec![wanted],
        "the match lives in the shared account, not this store's own"
    );
}

/// The id collision: a shared mailbox can have the same mailbox id as a
/// mailbox of the store's own account. `JmapStore::search` must use the
/// owner the caller gives it, not look the mailbox id up in the merged tree
/// ([`jmap_mail_sync::FolderTree::owning_account`]), because that lookup
/// finds the store's own folder first and answers `None` for it (an
/// ordinary folder of the store's own account carries no `account_id`),
/// which then searches the wrong account. The folder Camel opened already
/// knows the right one; `tests/messages.rs`'s own collision test proves
/// that through the real FFI path, so this test calls the store directly,
/// the same way `search_reaches_the_shared_mailboxs_owning_account` above
/// does.
#[test]
fn a_shared_mailboxs_search_reaches_its_own_account_when_the_mailbox_ids_collide() {
    let server = MockServer::builder().start();

    let shared_account_id = Id::new("agent-shared-owner");
    let (mailbox_id, wanted) = {
        let state = server.state();
        let mut state = state.lock().unwrap();

        let own = state
            .account_mut(&server.account_id())
            .expect("the mock's own account");
        let own_mailbox_id = own.seed_mailbox("Inbox", None);
        own.seed_email(EmailSeed::new(
            own_mailbox_id.clone(),
            ("Alice", "alice@example.com"),
            "Status report",
            "text",
            "2026-01-15T09:00:00Z",
        ));

        let sharer = state.add_account(shared_account_id.clone(), "Bob");
        sharer.is_personal = false;
        let shared_mailbox_id = sharer.seed_mailbox("Inbox", None);
        let wanted = sharer.seed_email(EmailSeed::new(
            shared_mailbox_id.clone(),
            ("Bob", "bob@example.com"),
            "Invoice 42",
            "text",
            "2026-01-15T09:00:00Z",
        ));

        assert_eq!(
            own_mailbox_id, shared_mailbox_id,
            "the two accounts must number their mailboxes from the same id"
        );
        (shared_mailbox_id, wanted)
    };

    let store = JmapStore::detached();
    store.store_connection(sync_against(&server));
    store.folders(CACHED).expect("listed");

    let filter = Filter::condition(EmailQueryFilter::default().subject("Invoice"));
    let matches = store
        .search(&mailbox_id, Some(&shared_account_id), filter)
        .expect("the search to reach the shared mailbox's own account");

    assert_eq!(
        matches,
        vec![wanted],
        "the search used the wrong account despite the id collision"
    );
}
