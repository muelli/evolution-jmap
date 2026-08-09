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
//!
//! Above the store's own two methods sits the interface itself, whose three
//! slots the store's copy of `CamelSubscribableInterface` fills. Two of the
//! three are reachable here: `folder_is_subscribed` takes no `CamelSubscribable`
//! machinery beyond the instance pointer, so it is called through the vtable the
//! way Camel calls it. The two sync vfuncs are not — they end in an emission
//! queued on the store's `CamelSession`, which a detached store does not have —
//! so what is tested of them here is everything up to that emission,
//! [`jmap_mail::subscribe::set_subscribed`], which is the whole of what they
//! decide. The emission is `tests/emissions.rs`.

use std::ffi::CString;
use std::sync::Arc;

use eds_sys::{
    CAMEL_STORE_FOLDER_INFO_REFRESH, CamelSubscribable, CamelSubscribableInterface,
    camel_subscribable_get_type,
};
use glib_sys::GFALSE;
use gobject_sys::{g_type_class_ref, g_type_class_unref, g_type_interface_peek, g_type_is_a};
use jmap_client::{Client, Credentials};
use jmap_mail::connect::StoreError;
use jmap_mail::store::{JmapStore, store_type};
use jmap_mail::subscribe;
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

// ---------------------------------------------------------------------------
// the interface

/// Claiming the interface is what makes `CAMEL_IS_SUBSCRIBABLE` true, which is
/// the test every caller makes before it asks a store anything about
/// subscriptions: Evolution's subscription editor offers the account at all
/// only for a store that answers it.
#[test]
fn the_store_implements_the_subscription_interface() {
    // SAFETY: plain type-system reads on a type `store_type` registers.
    unsafe {
        assert_ne!(
            g_type_is_a(store_type(), camel_subscribable_get_type()),
            GFALSE,
            "the store does not implement CamelSubscribable"
        );
    }
}

/// And filling the vtable is what makes it more than a claim. Camel installs no
/// default behind any of the three methods — `eds-sys`'s `tests/camel.rs` pins
/// that — so a slot left NULL is a call through a NULL pointer from inside the
/// wrapper, not a store that answers conservatively.
#[test]
fn the_stores_vtable_fills_all_three_slots() {
    // SAFETY: referencing the class is what runs the `interface_init` that
    // fills the vtable; the reference is released below, and the vtable is
    // owned by the class and only read.
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

/// `folder_is_subscribed`, called through the slot in the vtable — which is the
/// only way Camel ever reaches it. By name it would test a function that might
/// be installed nowhere.
fn asks_camel(store: &JmapStore, path: &str) -> bool {
    let path = CString::new(path).expect("a path with no NUL");

    // SAFETY: referencing the class runs the `interface_init` that fills the
    // vtable. The store is an instance of ours, which is what the vfunc's
    // contract asks for, and `path` is NUL-terminated and alive for the call.
    // The class reference is released after it, and the store outlives it.
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

/// The listing is the answer, and the write that just changed it is visible
/// through the vfunc immediately — the tick the user took off stays off.
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

/// A folder no listing mentions is not subscribed. FALSE rather than TRUE
/// because the question is whether the *user* asked to see it, and nothing
/// here says they did.
#[test]
fn a_folder_the_store_has_never_heard_of_is_not_subscribed() {
    let (_server, store, _inbox) = connected();
    store.folders(CACHED).expect("listed");

    assert!(!asks_camel(&store, "Receipts"));
}

/// And the vfunc does not go and look. Camel declares it non-blocking and
/// Evolution asks it once per folder while drawing the tree; a request from in
/// there would be a folder tree that blocks the UI thread once per row. The
/// store having no listing afterwards is what shows no listing was fetched.
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

/// The two sync vfuncs are handed a Camel path and nothing else, so resolving
/// it against the folder tree is theirs to do. What comes back is the folder as
/// it now is, which is what the `folder-subscribed` signal carries.
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

/// A path no mailbox answers to is reported in the store's own domain, which is
/// what keeps someone else's tidying from being shown as a broken account.
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

/// A mailbox another client created since the listing is subscribable without a
/// restart: the write looks again before it gives up, exactly as opening a
/// folder by path does. The cost is one `Mailbox/changes` on the path that was
/// about to fail anyway.
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

/// And with no connection there is nothing to resolve the path against.
/// `NOT_CONNECTED` is what makes Camel connect and ask again.
#[test]
fn a_store_with_no_connection_cannot_subscribe_by_path() {
    let store = JmapStore::detached();

    let failure = subscribe::set_subscribed(&store, "Inbox", true).expect_err("no connection");
    assert!(
        matches!(failure, StoreError::Disconnected),
        "expected a disconnected store, got {failure:?}"
    );
}
