// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for folder-list refresh races and disconnect non-blocking invariants:
//! - Row A2-3: two concurrent refreshes where the older answer arrives last must
//!   not overwrite a newer tree.
//! - Row S6-4: a disconnect during a stalled pushed refresh must not wait for the
//!   network round trip.

mod common;

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use common::race::DelayedDeliveryTransport;
use common::race::Gate;
use common::race::StorePtr;
use eds_sys::CAMEL_STORE_FOLDER_INFO_REFRESH;
use jmap_client::{Client, Credentials};
use jmap_mail::store::JmapStore;
use jmap_mail_sync::MailSync;
use jmap_mock::MockServer;

/// Row A2-3: two overlapping refreshes where the older answer arrives last must
/// leave the newer tree.
#[test]
fn overlapping_refreshes_keep_newer_tree_when_older_arrives_last() {
    let server = MockServer::builder().start();
    let gate = Arc::new(Gate::default());
    let transport = DelayedDeliveryTransport::new(gate.clone(), "Mailbox/changes");
    let client = Client::builder()
        .transport(transport)
        .connect(server.origin(), Credentials::none())
        .expect("connected");
    let sync = MailSync::new(client, server.account_id());

    let store = JmapStore::detached();
    store.store_connection(sync);

    // Primes the listing with the account's initial tree (Mailbox/get, un-gated).
    store.folders(0).expect("initial listing");

    // Start Refresh 1 in background. It asks Mailbox/changes for the initial state.
    // The server answers, and the gate holds the delivery back.
    let store_ptr = unsafe { StorePtr::new(&store) };
    let refresh1 =
        thread::spawn(move || store_ptr.borrow().folders(CAMEL_STORE_FOLDER_INFO_REFRESH));

    gate.wait_for_start();

    // Now modify the server: create a new mailbox.
    server.with_account_mut(server.account_id(), |account| {
        account.create_mailbox("LaterFolder", None, None);
    });

    // Run Refresh 2 (un-gated because DelayedDeliveryTransport delays once).
    // It sees "LaterFolder" on the server and updates the store's listing.
    let second_tree = store
        .folders(CAMEL_STORE_FOLDER_INFO_REFRESH)
        .expect("second refresh should succeed");
    let second_paths: Vec<&str> = second_tree.iter().map(|f| f.path.as_str()).collect();
    assert!(
        second_paths.contains(&"LaterFolder"),
        "second refresh must see the newly created folder"
    );

    // Now release Refresh 1. Its older (pre-create) answer arrives last.
    gate.release();
    let first_result = refresh1
        .join()
        .expect("refresh1 panicked")
        .expect("refresh1 should succeed");

    // The store's held listing must NOT lose LaterFolder to the older answer.
    let held = store.folders(0).expect("held listing");
    let held_paths: Vec<&str> = held.iter().map(|f| f.path.as_str()).collect();
    assert!(
        held_paths.contains(&"LaterFolder"),
        "the older refresh arriving last must not overwrite the newer tree; held: {held_paths:?}"
    );

    let first_paths: Vec<&str> = first_result.iter().map(|f| f.path.as_str()).collect();
    assert!(
        first_paths.contains(&"LaterFolder"),
        "refresh1 result must also see the newer tree; result: {first_paths:?}"
    );
}

/// Row S6-4: `disconnect_sync` (via `drop_connection`) during a stalled pushed
/// refresh must return promptly without waiting on the network request.
#[test]
fn disconnect_during_stalled_pushed_refresh_returns_promptly() {
    let server = MockServer::builder().start();
    let gate = Arc::new(Gate::default());
    let transport = DelayedDeliveryTransport::new(gate.clone(), "Mailbox/changes");
    let client = Client::builder()
        .transport(transport)
        .connect(server.origin(), Credentials::none())
        .expect("connected");
    let sync = MailSync::new(client, server.account_id());

    let store = JmapStore::detached();
    store.store_connection(sync);

    // Primes the listing so cached_tree() returns Some.
    store.folders(0).expect("initial listing");

    // Start pushed_folder_list_change in the background. It calls folders(REFRESH).
    // The server answers, and the gate holds the delivery back.
    let store_ptr = unsafe { StorePtr::new(&store) };
    let pushed = thread::spawn(move || store_ptr.borrow().pushed_folder_list_change());

    gate.wait_for_start();

    // While the pushed refresh is stalled at the network barrier, disconnect the store.
    // drop_connection must return promptly without blocking on the stalled network request.
    let (tx, rx) = std::sync::mpsc::channel();
    let disconnect_thread = thread::spawn(move || {
        let dropped = store_ptr.borrow().drop_connection();
        tx.send(dropped).unwrap();
    });

    let disconnected = rx.recv_timeout(Duration::from_secs(5));
    assert!(
        disconnected.is_ok(),
        "drop_connection blocked waiting for the stalled pushed refresh network request"
    );
    assert!(disconnected.unwrap());
    disconnect_thread
        .join()
        .expect("disconnect thread panicked");

    // Now release the gate so the stalled pushed refresh thread can finish.
    gate.release();
    let _ = pushed.join().expect("pushed refresh panicked");

    assert!(!store.is_connected());
}
