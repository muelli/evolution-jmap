// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A refresh's response can arrive late, after a local edit made by the
//! connection in the meantime. `JmapStore::folders` must not let the late
//! answer erase the edit.
//!
//! `folders(REFRESH)` snapshots the listing, makes a `Mailbox/changes` round
//! trip, then writes the result back unconditionally. `create_folder` and
//! its three siblings edit the *current* listing in place, under the same
//! lock, as soon as the server confirms the write. This edit has no ordering
//! against a refresh that started earlier and is still waiting on its own
//! response. A refresh's `Mailbox/changes` answer can be computed before the
//! create, so it truthfully says nothing changed. If that answer is
//! *delivered* after the create, it overwrites the listing with the
//! pre-create tree. This silently loses the folder Camel was just told
//! exists.
//!
//! This is proven with a transport that wraps a real one against a real
//! [`MockServer`]. The `Mailbox/changes` request completes for real, so its
//! answer is the server's honest one for the state at the time. Its delivery
//! back to the caller is held until a concurrent `create_folder` has
//! finished.

mod common;

use std::sync::Arc;
use std::thread;

use common::race::DelayedDeliveryTransport;
use common::race::Gate;
use common::race::StorePtr;
use jmap_client::{Client, Credentials};
use jmap_mail::store::JmapStore;
use jmap_mail_sync::MailSync;
use jmap_mock::MockServer;

#[test]
fn a_folder_created_while_a_refresh_is_in_flight_is_not_lost_to_a_stale_answer() {
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
    // Primes the listing with the account's current (empty) tree. Uses a
    // plain request the gate does not touch.
    store.folders(0).expect("initial listing");

    // SAFETY: the test joins the background thread before `store` drops.
    let store_ptr = unsafe { StorePtr::new(&store) };
    let refresh = thread::spawn(move || {
        store_ptr
            .borrow()
            .folders(eds_sys::CAMEL_STORE_FOLDER_INFO_REFRESH)
    });

    // The server has now answered the refresh's `Mailbox/changes` request.
    // The answer truthfully says nothing has changed yet, and the gate
    // holds it back.
    gate.wait_for_start();

    let created = store
        .create_folder(None, "New")
        .expect("create_folder should succeed while the refresh is parked");
    assert_eq!(created.display_name, "New");

    // Releases the gate. The stale "nothing changed" answer lands now.
    gate.release();
    let refreshed = refresh
        .join()
        .expect("the refresh thread panicked")
        .expect("refresh should still succeed");

    let paths: Vec<&str> = refreshed
        .iter()
        .map(|folder| folder.path.as_str())
        .collect();
    assert!(
        paths.contains(&"New"),
        "the folder created while the refresh was in flight was lost to the refresh's \
         stale (pre-create) answer landing after it; store's tree has {paths:?}"
    );

    // The store's own held listing agrees. The loss this test guards
    // against is the refresh's write silently replacing it.
    let held = store.folders(0).expect("cached listing");
    let held_paths: Vec<&str> = held.iter().map(|folder| folder.path.as_str()).collect();
    assert!(
        held_paths.contains(&"New"),
        "store's cached listing lost the created folder; has {held_paths:?}"
    );
}

/// Two racing *first* listings (the slot starts empty, so each has no
/// generation to compare against) can lose a folder the same way, even
/// though no `REFRESH` is involved.
///
/// `folders(0)` with an empty slot has `baseline_generation: None`. The
/// staleness check only compares generations when a baseline exists
/// (`(Some(current), Some(baseline))`); a `None` baseline is never stale, so
/// its answer always overwrites the slot. If a second first listing writes
/// the slot first, and a `create_folder` then edits it, a first listing
/// still in flight overwrites that edit when it lands, because its own
/// baseline was `None`.
#[test]
fn a_folder_created_between_two_racing_first_listings_is_not_lost_to_the_slower_one() {
    let server = MockServer::builder().start();
    let gate = Arc::new(Gate::default());
    let transport = DelayedDeliveryTransport::new(gate.clone(), "Mailbox/get");
    let client = Client::builder()
        .transport(transport)
        .connect(server.origin(), Credentials::none())
        .expect("connected");
    let sync = MailSync::new(client, server.account_id());

    let store = JmapStore::detached();
    store.store_connection(sync);

    // SAFETY: the test joins the background thread before `store` drops.
    let store_ptr = unsafe { StorePtr::new(&store) };
    let slow_first_listing = thread::spawn(move || store_ptr.borrow().folders(0));

    // The server has answered the slow listing's `Mailbox/get` honestly (an
    // empty tree). The gate holds the answer back before it reaches the
    // store.
    gate.wait_for_start();

    // A second, un-gated first listing reaches the empty slot first and
    // populates it.
    let fast_first_listing = store.folders(0).expect("the second first listing");
    assert!(fast_first_listing.iter().next().is_none());

    let created = store
        .create_folder(None, "New")
        .expect("create_folder should succeed once a listing exists");
    assert_eq!(created.display_name, "New");

    // Releases the gate. The slow listing's stale (pre-create) answer lands
    // now, with no baseline generation to notice the create happened first.
    gate.release();
    let slow_result = slow_first_listing
        .join()
        .expect("the slow listing thread panicked")
        .expect("the slow listing should still succeed");

    let paths: Vec<&str> = slow_result
        .iter()
        .map(|folder| folder.path.as_str())
        .collect();
    assert!(
        paths.contains(&"New"),
        "the folder created between the two racing first listings was lost to the slower \
         one's stale answer landing after it; store's tree has {paths:?}"
    );

    let held = store.folders(0).expect("cached listing");
    let held_paths: Vec<&str> = held.iter().map(|folder| folder.path.as_str()).collect();
    assert!(
        held_paths.contains(&"New"),
        "store's cached listing lost the created folder; has {held_paths:?}"
    );
}
