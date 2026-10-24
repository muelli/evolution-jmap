// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `disconnect_window.rs`'s bound on waiting out an in-flight call, proven
//! against a real JMAP server rather than a fake blocking [`Transport`].
//!
//! `disconnect_window.rs` proves that `JmapStore::drop_connection` no longer
//! waits out the whole network round trip of a `messages` call. It does
//! this with a fake transport that blocks the request itself before the
//! request is ever sent. That proves the store clones the connection's
//! `Arc` and drops its read guard before making the request. It cannot
//! prove what happens when a real network round trip to a real server is
//! genuinely in flight while the drop runs. This file reruns the same race
//! with the delayed-delivery technique that
//! `live_server_folder_listing_race.rs` already uses: a real request is
//! genuinely answered by Stalwart, but only its delivery back to the caller
//! is held. So `drop_connection` races an in-flight real round trip, not a
//! request that never left the process.
//!
//! ## Running it
//!
//! This test needs the same environment as the other live-server tests. See
//! `docs/manual-test-live-server.md`:
//!
//! ```console
//! $ cargo test -p jmap-mail --features testing -- --ignored
//! ```
//!
//! The test is skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER` or
//! `_PASSWORD` is unset.

mod common;

use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use common::race::DelayedDeliveryTransport;
use common::race::Gate;
use common::race::StorePtr;
use jmap_client::Client;
use jmap_mail::store::JmapStore;
use jmap_mail_sync::MailSync;
use jmap_proto::session::CAPABILITY_MAIL;
use jmap_test_support::{LiveAccount, connect_for_write, unique_suffix};

/// Mirrors `live_server_folder_listing_race.rs`'s `connect_for_write_with_gate`.
fn connect_for_write_with_gate(gate: Arc<Gate>) -> Option<Client> {
    // `messages()` sends `Email/get` just after it clones the connection's `Arc` and drops the
    // read guard. A hold on that response opens the window that `drop_connection` races.
    let transport = DelayedDeliveryTransport::new(gate, "Email/get");
    Some(LiveAccount::write()?.connect_with(Client::builder().transport(transport)))
}

/// Same scenario as `disconnect_window.rs`, against real Stalwart. A
/// `messages` listing's first request is genuinely in flight (the server
/// has answered it, but delivery is held) when `drop_connection` runs
/// concurrently. The drop must not wait out the round trip.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn dropping_the_connection_does_not_wait_out_a_message_listing_in_flight_on_the_real_server() {
    let gate = Arc::new(Gate::default());
    let Some(client) = connect_for_write_with_gate(gate.clone()) else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let sync = MailSync::new(client, account_id);

    let store = JmapStore::detached();
    store.store_connection(sync);

    let name = format!("agent-disconnectwindow-{}", unique_suffix());
    let created = store
        .create_folder(None, &name)
        .expect("create_folder should succeed against the real server");

    // SAFETY: the test joins both threads before `store` drops.
    let store_ptr = unsafe { StorePtr::new(&store) };
    let mailbox = created.id.clone();
    let listing = thread::spawn(move || store_ptr.borrow().messages(&mailbox));

    // The listing's first request (the account's `Email` state) has now
    // been genuinely answered by the real server and is being held back
    // from the caller.
    gate.wait_for_start();

    let (drop_tx, drop_rx) = mpsc::channel();
    let dropping = thread::spawn(move || {
        let start = Instant::now();
        let dropped = store_ptr.borrow().drop_connection();
        let _ = drop_tx.send((dropped, start.elapsed()));
    });

    let received = drop_rx.recv_timeout(Duration::from_secs(5));
    // Unparks both threads regardless of whether `drop_connection` answered
    // in time, so a red run cannot leave anything blocked past this test.
    gate.release();
    dropping.join().expect("the dropping thread panicked");
    let listed = listing
        .join()
        .expect("the listing thread panicked")
        .expect("the listing should still complete against the real server");

    let (dropped, elapsed) = received.expect(
        "drop_connection did not return within 5s while a messages call was still in flight \
         against the real server — it waited on the read lock instead of cloning the \
         connection out first",
    );
    assert!(dropped, "drop_connection reported nothing to drop");
    assert!(
        elapsed < Duration::from_millis(500),
        "drop_connection took {elapsed:?} while a real messages call was in flight; \
         it should not have waited on it at all"
    );
    assert!(
        listed.1.is_empty(),
        "the freshly created folder should have no messages yet"
    );

    // Cleanup needs its own connection: `drop_connection` above already took
    // this store's.
    let client = connect_for_write().expect("the write-test account was already confirmed above");
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let cleanup_sync = MailSync::new(client, account_id);
    let cleanup_store = JmapStore::detached();
    cleanup_store.store_connection(cleanup_sync);
    cleanup_store
        .delete_folder(&created.id)
        .expect("cleanup: delete_folder failed against the real server");
}
