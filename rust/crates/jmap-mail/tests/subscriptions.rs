// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The subscription half of a store: what happens around the `Mailbox/set`
//! call that carries it.
//!
//! `jmap-mail-sync` already covers the write on the wire. This file covers the
//! thing the store must add: the folder listing it keeps between calls.
//! `CamelSubscribable` declares `folder_is_subscribed` as a *non-blocking*
//! method. Evolution calls it once per folder while it draws the folder tree,
//! so the answer can only come from that listing. A store can write a
//! subscription to the server but leave its own listing unchanged. Then the
//! old tick shows again, until something refreshes the tree.
//!
//! Above the store's two methods sits the interface itself. The store's copy
//! of `CamelSubscribableInterface` fills all three of its slots, but only two
//! are reachable here:
//!
//! - `folder_is_subscribed` needs no `CamelSubscribable` machinery beyond the
//!   instance pointer, so the test calls it through the vtable, as Camel does.
//! - The two sync vfuncs end in an emission queued on the store's
//!   `CamelSession`, which a detached store does not have. This file tests
//!   everything up to that emission: [`jmap_mail::subscribe::set_subscribed`],
//!   the whole decision the vfuncs make. `tests/emissions.rs` covers the
//!   emission itself.

mod common;

use std::ffi::CString;
use std::sync::Arc;

use common::sync_against;
use eds_sys::{
    CAMEL_STORE_FOLDER_INFO_REFRESH, CamelSubscribable, CamelSubscribableInterface,
    camel_subscribable_get_type,
};
use glib_sys::GFALSE;
use gobject_sys::{g_type_class_ref, g_type_class_unref, g_type_interface_peek, g_type_is_a};
use jmap_mail::connect::StoreError;
use jmap_mail::store::{JmapStore, store_type};
use jmap_mail::subscribe;
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::mail::role;

/// No flags: the tree the store already has, with no new request.
/// This matches the reach of the non-blocking `folder_is_subscribed` method.
const CACHED: eds_sys::CamelStoreGetFolderInfoFlags = 0;
const REFRESH: eds_sys::CamelStoreGetFolderInfoFlags = CAMEL_STORE_FOLDER_INFO_REFRESH;

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

/// The point of the test: the write updates the listing that answers the
/// non-blocking read. No new request to the server is needed.
#[test]
fn the_held_listing_agrees_with_the_write() {
    let (server, store, inbox) = connected();
    store.folders(CACHED).expect("listed");

    store.set_subscribed(&inbox, false).expect("unsubscribed");
    drop(server);

    assert_eq!(held(&store, "Inbox"), Some(false));
}

/// Only the named folder changes. The rest of the tree stays the same.
#[test]
fn the_folders_next_to_it_are_left_alone() {
    let (_server, store, inbox) = connected();
    store.folders(CACHED).expect("listed");

    store.set_subscribed(&inbox, false).expect("unsubscribed");

    assert_eq!(held(&store, "Sent"), Some(true));
}

/// Subscribing again restores the folder. A user who changes their mind in
/// the subscription editor makes two writes over one listing.
#[test]
fn subscribing_again_puts_the_folder_back() {
    let (server, store, inbox) = connected();
    store.folders(CACHED).expect("listed");

    store.set_subscribed(&inbox, false).expect("unsubscribed");
    store.set_subscribed(&inbox, true).expect("subscribed");
    drop(server);

    assert_eq!(held(&store, "Inbox"), Some(true));
}

/// The edit changes the tree the store holds. A caller that took the previous
/// `Arc` from `folders` still sees the old state it was handed.
///
/// This is not a flaw: building a `CamelFolderInfo` forest copies data out of
/// the tree while it is borrowed. A tree that mutates during that walk would
/// be the bug this design prevents.
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

/// A store with no listing yet must not invent one to write into. A tree
/// built from the single folder a write names would show an account with
/// only one folder. So the first `get_folder_info_sync` call after the write
/// must still reach the server.
///
/// The test proves this by changing the account afterward: a mailbox created
/// between the write and the listing appears in the answer only if the answer
/// came from the server.
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

/// The edit does not advance the state the listing is current as of. The
/// write does advance the account state, so the next refresh finds a change
/// and rebuilds the tree. This costs one extra listing, but the alternative is
/// worse: the store would invent a state string the server never gave it, then
/// ask `Mailbox/changes` using that invented string.
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

/// Camel may believe a store is connected after the belief goes stale.
/// `NOT_CONNECTED` makes Camel reconnect and ask again.
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

/// A folder that another client deleted since the listing. The store reports
/// this in its own error domain, not the service's: nothing is wrong with the
/// account.
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

// ---------------------------------------------------------------------------
// the interface

/// Claiming the interface makes `CAMEL_IS_SUBSCRIBABLE` true. Every caller
/// checks this before it asks a store about subscriptions: Evolution's
/// subscription editor offers the account only if the store answers true.
#[test]
fn the_store_implements_the_subscription_interface() {
    // SAFETY: a plain type-system read on a type that `store_type` registers.
    unsafe {
        assert_ne!(
            g_type_is_a(store_type(), camel_subscribable_get_type()),
            GFALSE,
            "the store does not implement CamelSubscribable"
        );
    }
}

/// Filling the vtable makes the claim real. Camel installs no default behind
/// any of the three methods (`eds-sys`'s `tests/camel.rs` confirms this). So a
/// NULL slot causes a call through a NULL pointer inside the wrapper, not a
/// conservative answer from the store.
#[test]
fn the_stores_vtable_fills_all_three_slots() {
    // SAFETY: referencing the class runs the `interface_init` call that fills
    // the vtable. The reference is released below. The vtable belongs to the
    // class and the code only reads it.
    unsafe {
        let class = g_type_class_ref(store_type());
        let vtable = g_type_interface_peek(class, camel_subscribable_get_type())
            .cast::<CamelSubscribableInterface>();
        assert!(!vtable.is_null(), "the store has no copy of the vtable");

        assert!((*vtable).folder_is_subscribed.is_some());
        assert!((*vtable).subscribe_folder_sync.is_some());
        assert!((*vtable).unsubscribe_folder_sync.is_some());

        g_type_class_unref(class);
    }
}

/// Calls `folder_is_subscribed` through the vtable slot, the only way Camel
/// reaches it. Calling it by name would test a function that might not be
/// installed anywhere.
fn asks_camel(store: &JmapStore, path: &str) -> bool {
    let path = CString::new(path).expect("a path with no NUL");

    // SAFETY: referencing the class runs `interface_init`, which fills the
    // vtable. The store is an instance of our type, as the vfunc's contract
    // requires. `path` is NUL-terminated and stays alive for the call. The
    // class reference is released after the call, and the store outlives it.
    unsafe {
        let class = g_type_class_ref(store_type());
        let vtable = g_type_interface_peek(class, camel_subscribable_get_type())
            .cast::<CamelSubscribableInterface>();
        assert!(
            !vtable.is_null(),
            "the store does not implement CamelSubscribable"
        );
        let vfunc = (*vtable)
            .folder_is_subscribed
            .expect("the store cannot say whether a folder is subscribed");
        let answer = vfunc(
            (store as *const JmapStore)
                .cast_mut()
                .cast::<CamelSubscribable>(),
            path.as_ptr(),
        );
        g_type_class_unref(class);
        answer != GFALSE
    }
}

/// The listing is the answer. A write that changes it is visible through the
/// vfunc at once: the tick the user removed stays off.
#[test]
fn camel_is_told_what_the_held_listing_says() {
    let (server, store, inbox) = connected();
    store.folders(CACHED).expect("listed");

    assert!(asks_camel(&store, "Inbox"));
    store.set_subscribed(&inbox, false).expect("unsubscribed");
    drop(server);

    assert!(!asks_camel(&store, "Inbox"));
    assert!(asks_camel(&store, "Sent"));
}

/// A folder absent from the listing is not subscribed. The answer is FALSE,
/// not TRUE, because the question is whether the *user* asked to see it, and
/// nothing here confirms that.
#[test]
fn a_folder_the_store_has_never_heard_of_is_not_subscribed() {
    let (_server, store, _inbox) = connected();
    store.folders(CACHED).expect("listed");

    assert!(!asks_camel(&store, "Receipts"));
}

/// The vfunc never makes a request. Camel declares it non-blocking, and
/// Evolution calls it once per folder while it draws the tree. A request from
/// inside the vfunc would block the UI thread once per row. The store having
/// no listing afterward proves no listing was fetched.
#[test]
fn the_non_blocking_answer_makes_no_request() {
    let (_server, store, _inbox) = connected();

    assert!(!asks_camel(&store, "Inbox"));
    assert!(
        store.held_folders().is_none(),
        "the non-blocking read went and listed the account"
    );
}

// ---------------------------------------------------------------------------
// the write, by the name Camel calls the folder

/// The two sync vfuncs receive only a Camel path, so resolving it against the
/// folder tree is their job. The result is the folder's current state, the
/// same data the `folder-subscribed` signal carries.
#[test]
fn the_path_camel_names_is_resolved_to_the_mailbox_written() {
    let (server, store, _inbox) = connected();
    store.folders(CACHED).expect("listed");

    let folder = subscribe::set_subscribed(&store, "Inbox", false).expect("unsubscribed");

    assert_eq!(folder.path, "Inbox");
    assert!(!folder.subscribed, "the answer describes the new state");
    assert_eq!(held(&store, "Inbox"), Some(false));

    // Read back over a connection of its own, so the answer is the server's.
    let (_, tree) = sync_against(&server).folder_tree().expect("listed");
    assert_eq!(
        tree.find("Inbox").map(|folder| folder.subscribed),
        Some(false)
    );
}

/// Subscribing again by path puts it back, and says so.
#[test]
fn subscribing_by_path_puts_the_folder_back() {
    let (_server, store, _inbox) = connected();
    store.folders(CACHED).expect("listed");

    subscribe::set_subscribed(&store, "Inbox", false).expect("unsubscribed");
    let folder = subscribe::set_subscribed(&store, "Inbox", true).expect("subscribed");

    assert!(folder.subscribed);
    assert_eq!(held(&store, "Inbox"), Some(true));
}

/// A path with no matching mailbox is reported in the store's own error
/// domain. This keeps another client's folder cleanup from looking like a
/// broken account.
#[test]
fn a_path_no_folder_answers_to_is_reported_as_missing() {
    let (_server, store, _inbox) = connected();
    store.folders(CACHED).expect("listed");

    let failure = subscribe::set_subscribed(&store, "Receipts", true).expect_err("no such folder");
    assert!(
        matches!(&failure, StoreError::NoFolder(path) if path == "Receipts"),
        "expected a missing folder, got {failure:?}"
    );
}

/// A mailbox that another client created since the listing is subscribable
/// without a restart. The write retries before it gives up, the same as
/// opening a folder by path does. The cost is one `Mailbox/changes` call on a
/// path that was about to fail anyway.
#[test]
fn a_folder_created_since_the_listing_is_looked_for_again() {
    let (server, store, _inbox) = connected();
    store.folders(CACHED).expect("listed");
    {
        let account_id = server.account_id();
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .create_mailbox("Receipts", None, None);
    }

    let folder = subscribe::set_subscribed(&store, "Receipts", false).expect("unsubscribed");

    assert_eq!(folder.path, "Receipts");
    assert_eq!(held(&store, "Receipts"), Some(false));
}

/// With no connection, there is nothing to resolve the path against.
/// `NOT_CONNECTED` makes Camel connect and ask again.
#[test]
fn a_store_with_no_connection_cannot_subscribe_by_path() {
    let store = JmapStore::detached();

    let failure = subscribe::set_subscribed(&store, "Inbox", true).expect_err("no connection");
    assert!(
        matches!(failure, StoreError::Disconnected),
        "expected a disconnected store, got {failure:?}"
    );
}
