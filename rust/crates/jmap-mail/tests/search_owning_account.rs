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
        .search(&shared_inbox_id, filter)
        .expect("the search to reach the shared mailbox's own account");

    assert_eq!(
        matches,
        vec![wanted],
        "the match lives in the shared account, not this store's own"
    );
}
