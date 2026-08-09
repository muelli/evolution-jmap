// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The subscription half of a store: what happens on either side of the
//! `Mailbox/set` that carries it.
//!
//! `jmap-mail-sync` already puts the write on the wire. What is tested here is
//! the thing the store has to add and it cannot: the folder listing the store
//! keeps between calls. `CamelSubscribable` declares `folder_is_subscribed` as
//! one of its *non-blocking* methods — Evolution asks it while drawing the
//! folder tree, once per folder — so it can only ever be answered out of that
//! listing. A store that wrote the subscription to the server and left its own
//! listing saying the opposite would draw the tick back on the moment the user
//! took it off, and keep doing so until something refreshed the tree.

use std::sync::Arc;

use eds_sys::CAMEL_STORE_FOLDER_INFO_REFRESH;
use jmap_client::{Client, Credentials};
use jmap_mail::connect::StoreError;
use jmap_mail::store::JmapStore;
use jmap_mail_sync::MailSync;
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::mail::role;

/// No flags: the tree the store already has, with no request behind it — which
/// is exactly the reach a non-blocking `folder_is_subscribed` has.
const CACHED: eds_sys::CamelStoreGetFolderInfoFlags = 0;
const REFRESH: eds_sys::CamelStoreGetFolderInfoFlags = CAMEL_STORE_FOLDER_INFO_REFRESH;

fn sync_against(server: &MockServer) -> MailSync {
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    MailSync::new(client, server.account_id())
}

/// A server with an inbox and a sibling, and a store connected to it.
fn connected() -> (MockServer, Box<JmapStore>, Id) {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let inbox = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_mailbox("Sent", Some(role::SENT));
        inbox
    };
    let store = JmapStore::detached();
    store.store_connection(sync_against(&server));
    (server, store, inbox)
}

/// What the store's own listing says, with no request behind the question.
fn held(store: &JmapStore, path: &str) -> Option<bool> {
    let tree = store.folders(CACHED).expect("a listing");
    tree.find(path).map(|folder| folder.subscribed)
}

#[test]
fn the_write_reaches_the_server() {
    let (server, store, inbox) = connected();

    store.set_subscribed(&inbox, false).expect("unsubscribed");

    // Read back over a connection of its own, so the answer is the server's
    // and not the store's copy of it.
    let (_, tree) = sync_against(&server).folder_tree().expect("listed");
    assert_eq!(
        tree.find("Inbox").map(|folder| folder.subscribed),
        Some(false)
    );
}

/// The point of the increment: the listing the non-blocking read is answered
/// from agrees with the write that just succeeded, without anything having gone
/// back to the server.
#[test]
fn the_held_listing_agrees_with_the_write() {
    let (server, store, inbox) = connected();
    store.folders(CACHED).expect("listed");

    store.set_subscribed(&inbox, false).expect("unsubscribed");
    drop(server);

    assert_eq!(held(&store, "Inbox"), Some(false));
}

/// And it is the folder that was named that changes, not the tree around it.
#[test]
fn the_folders_next_to_it_are_left_alone() {
    let (_server, store, inbox) = connected();
    store.folders(CACHED).expect("listed");

    store.set_subscribed(&inbox, false).expect("unsubscribed");

    assert_eq!(held(&store, "Sent"), Some(true));
}

/// Subscribing again puts it back. Two writes in a row over one listing is what
/// a user changing their mind in the subscription editor does.
#[test]
fn subscribing_again_puts_the_folder_back() {
    let (server, store, inbox) = connected();
    store.folders(CACHED).expect("listed");

    store.set_subscribed(&inbox, false).expect("unsubscribed");
    store.set_subscribed(&inbox, true).expect("subscribed");
    drop(server);

    assert_eq!(held(&store, "Inbox"), Some(true));
}

/// The edit is to the tree the store holds, so a caller that took the previous
/// `Arc` out of `folders` still sees what it was handed. That is not a
/// compromise: a `CamelFolderInfo` forest built from a tree is copied out of it
/// while it is borrowed, and a tree that mutated underneath such a walk would
/// be the bug this arrangement rules out.
#[test]
fn a_tree_already_handed_out_is_not_edited_underneath_its_reader() {
    let (_server, store, inbox) = connected();
    let handed_out = store.folders(CACHED).expect("listed");

    store.set_subscribed(&inbox, false).expect("unsubscribed");

    assert_eq!(
        handed_out.find("Inbox").map(|folder| folder.subscribed),
        Some(true)
    );
    assert_eq!(held(&store, "Inbox"), Some(false));
    assert!(!Arc::ptr_eq(&handed_out, &store.folders(CACHED).unwrap()));
}

/// A store with nothing listed yet must not invent a listing to write into. A
/// tree assembled out of the one folder a write happened to name would be an
/// account with one folder in it — so the first `get_folder_info_sync` after
/// this still has to reach the server. Which is shown by moving the account on
/// afterwards: a mailbox created between the write and the listing is in the
/// answer only if the answer came from the server.
#[test]
fn a_store_that_has_not_listed_yet_does_not_gain_a_listing() {
    let (server, store, inbox) = connected();

    store.set_subscribed(&inbox, false).expect("unsubscribed");
    {
        let account_id = server.account_id();
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .create_mailbox("Receipts", None, None);
    }

    let tree = store.folders(CACHED).expect("listed");
    let paths: Vec<&str> = tree.iter().map(|folder| folder.path.as_str()).collect();
    assert_eq!(paths, ["Inbox", "Receipts", "Sent"]);
    assert_eq!(
        tree.find("Inbox").map(|folder| folder.subscribed),
        Some(false)
    );
}

/// The state the listing is current as of is *not* moved on by the edit. The
/// write did move the account on, so the next refresh finds a change and
/// rebuilds — one listing more than strictly needed, and the alternative is a
/// store inventing a state string the server never gave it and then asking
/// `Mailbox/changes` from it.
#[test]
fn a_refresh_after_the_write_still_rebuilds_from_the_server() {
    let (_server, store, inbox) = connected();
    let listed = store.folders(CACHED).expect("listed");

    store.set_subscribed(&inbox, false).expect("unsubscribed");
    let refreshed = store.folders(REFRESH).expect("refreshed");

    assert!(!Arc::ptr_eq(&listed, &refreshed));
    assert_eq!(
        refreshed.find("Inbox").map(|folder| folder.subscribed),
        Some(false)
    );
}

/// Camel drives a store it believes is connected, and the belief goes stale.
/// `NOT_CONNECTED` is what makes it connect and ask again.
#[test]
fn a_store_with_no_connection_cannot_change_a_subscription() {
    let store = JmapStore::detached();

    let failure = store
        .set_subscribed(&Id::new("M1"), false)
        .expect_err("no connection, no write");
    assert!(
        matches!(failure, StoreError::Disconnected),
        "expected a disconnected store, got {failure:?}"
    );
}

/// A folder another client deleted since the listing. Reported in the store's
/// own domain rather than the service's: nothing is wrong with the account.
#[test]
fn a_folder_the_account_no_longer_has_is_reported_as_missing() {
    let (_server, store, _inbox) = connected();

    let failure = store
        .set_subscribed(&Id::new("M404"), false)
        .expect_err("no such mailbox");
    assert!(
        matches!(failure, StoreError::NoFolder(_)),
        "expected a missing folder, got {failure:?}"
    );
}
