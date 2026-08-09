// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The folder the user adds to an account and the one they take away, from the
//! store's side of the two vfuncs.
//!
//! `jmap-mail-sync` already puts both writes on the wire and already decides
//! what the new folder *is*. What is tested here is the part only a store can
//! do, and it is the same part the subscription vfuncs needed: the folder
//! listing the store keeps between calls. Camel hands the `CamelFolderInfo` a
//! create answers with straight to Evolution's folder tree and then opens that
//! folder by path — through `get_folder_sync`, which is answered out of the
//! listing — so a store that made the folder on the server and left its own
//! listing without it would offer the user a folder it then refuses to open.
//! The same one step behind for a delete: a folder that is gone from the
//! account and still in the listing is one Camel will happily open again.
//!
//! Two things above that are *not* covered, and both are the vfunc rather than
//! what it decides. The `camel_store_folder_created`/`_deleted` emission at the
//! end of each needs a store with a `CamelSession` behind it —
//! `camel_store_folder_created` starts by taking the service's session and
//! queueing the emission on it — which is the same limit `subscriptions.rs`
//! documents. And the path from the user's "New Folder" menu item to the vfunc
//! is not reachable at all yet: Evolution offers those items for a store
//! carrying `CAMEL_STORE_CAN_EDIT_FOLDERS`, which this store does not set,
//! because the flag also offers Rename and there is no `rename_folder_sync`
//! behind it.

use eds_sys::{CamelStoreClass, camel_offline_store_get_type};
use gobject_sys::{g_type_class_peek, g_type_class_ref, g_type_class_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::connect::StoreError;
use jmap_mail::manage;
use jmap_mail::store::{JmapStore, store_type};
use jmap_mail_sync::{FolderTree, MailSync};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::mail::role;

/// No flags: the tree the store already holds, with no request behind it.
const CACHED: eds_sys::CamelStoreGetFolderInfoFlags = 0;

fn sync_against(server: &MockServer) -> MailSync {
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    MailSync::new(client, server.account_id())
}

/// A server with an inbox and one ordinary folder, and a store connected to it.
fn connected() -> (MockServer, Box<JmapStore>) {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_mailbox("Projects", None);
    }
    let store = JmapStore::detached();
    store.store_connection(sync_against(&server));
    (server, store)
}

/// The account as the server has it, read over a connection of its own so that
/// the answer is not the store's copy of it.
fn on_the_server(server: &MockServer) -> FolderTree {
    let (_, tree) = sync_against(server).folder_tree().expect("listed");
    tree
}

/// What the store's own listing holds, with no request behind the question.
fn held(store: &JmapStore) -> Vec<String> {
    let tree = store.folders(CACHED).expect("a listing");
    tree.iter().map(|folder| folder.path.clone()).collect()
}

// ---------------------------------------------------------------------------
// the folder the user makes

#[test]
fn the_new_folder_reaches_the_server() {
    let (server, store) = connected();

    manage::create_folder(&store, None, "Receipts").expect("created");

    assert!(on_the_server(&server).find("Receipts").is_some());
}

/// The point of doing it here rather than in the sync layer: the listing the
/// store answers `get_folder_sync` out of has the folder, without anything
/// having gone back to the server for it.
#[test]
fn the_held_listing_gains_the_new_folder() {
    let (server, store) = connected();
    store.folders(CACHED).expect("listed");

    manage::create_folder(&store, None, "Receipts").expect("created");
    drop(server);

    assert_eq!(held(&store), ["Inbox", "Projects", "Receipts"]);
}

/// Camel names the parent by path — the same string it names a folder by
/// everywhere else — and the new folder lands under it on both sides.
#[test]
fn a_folder_is_made_under_the_parent_camel_named() {
    let (server, store) = connected();
    store.folders(CACHED).expect("listed");

    let created = manage::create_folder(&store, Some("Projects"), "JMAP").expect("created");

    assert_eq!(created.path, "Projects/JMAP");
    let served = on_the_server(&server);
    assert_eq!(
        served.find("Projects/JMAP").map(|folder| folder.id.clone()),
        Some(created.id.clone())
    );
    assert_eq!(held(&store), ["Inbox", "Projects", "Projects/JMAP"]);
}

/// A NULL or empty `parent_name` is the account itself, the reading
/// `get_folder_info_sync`'s `top` already has and the one Camel's own wrapper
/// makes.
#[test]
fn an_empty_parent_is_the_account_itself() {
    let (_server, store) = connected();

    let created = manage::create_folder(&store, Some(""), "Receipts").expect("created");

    assert_eq!(created.path, "Receipts");
}

/// The name Camel passes is the *mailbox* name, verbatim: JMAP puts the folder
/// under an explicit `parentId`, so there is no hierarchy to read out of the
/// name the way an IMAP store has to. A `/` in it is therefore a character of
/// the name and the path is what carries the encoding of it.
#[test]
fn the_name_is_a_mailbox_name_and_the_path_is_the_encoded_one() {
    let (server, store) = connected();

    let created = manage::create_folder(&store, None, "Bills/2026").expect("created");

    assert_eq!(created.display_name, "Bills/2026");
    assert_eq!(created.path, "Bills%2F2026");
    assert_eq!(
        on_the_server(&server)
            .find("Bills%2F2026")
            .map(|folder| folder.display_name.clone()),
        Some("Bills/2026".to_owned())
    );
}

/// A parent the account does not have is the store's own domain rather than the
/// service's, exactly as opening a folder that is not there is.
#[test]
fn a_parent_the_account_does_not_have_is_reported_as_missing() {
    let (_server, store) = connected();

    let failure = manage::create_folder(&store, Some("Nowhere"), "JMAP")
        .expect_err("no such parent, no create");

    assert!(
        matches!(failure, StoreError::NoFolder(path) if path == "Nowhere"),
        "expected a missing folder"
    );
}

/// And a parent another client made since the last listing is found rather than
/// reported missing — the second look `get_folder_sync` already takes, for the
/// same reason: the account plainly has the folder.
#[test]
fn a_parent_created_since_the_listing_is_looked_for_again() {
    let (server, store) = connected();
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

    let created = manage::create_folder(&store, Some("Receipts"), "2026").expect("created");

    assert_eq!(created.path, "Receipts/2026");
}

/// The server refusing is not the store failing: a name a sibling already has
/// comes back as the server's own error, and nothing joins the listing.
#[test]
fn a_refused_create_leaves_the_listing_alone() {
    let (_server, store) = connected();
    store.folders(CACHED).expect("listed");

    let failure = manage::create_folder(&store, None, "Projects").expect_err("a name in use");

    assert!(
        matches!(failure, StoreError::Client(_)),
        "expected the server's own refusal, got {failure:?}"
    );
    assert_eq!(held(&store), ["Inbox", "Projects"]);
}

#[test]
fn a_store_with_no_connection_cannot_make_a_folder() {
    let store = JmapStore::detached();

    let failure = manage::create_folder(&store, None, "Receipts").expect_err("no connection");

    assert!(
        matches!(failure, StoreError::Disconnected),
        "expected a disconnected store, got {failure:?}"
    );
}

// ---------------------------------------------------------------------------
// and the one they remove

#[test]
fn the_removal_reaches_the_server() {
    let (server, store) = connected();

    manage::delete_folder(&store, "Projects").expect("removed");

    assert!(on_the_server(&server).find("Projects").is_none());
}

/// The answer is the folder that went, because that is what the vfunc has to
/// hand `camel_store_folder_deleted` — after the folder is no longer anywhere
/// to be looked up.
#[test]
fn the_answer_is_the_folder_that_went() {
    let (server, store) = connected();
    let listed = store.folders(CACHED).expect("listed");
    let expected = listed.find("Projects").expect("a folder to remove").clone();

    let removed = manage::delete_folder(&store, "Projects").expect("removed");
    drop(server);

    assert_eq!(removed, expected);
}

#[test]
fn the_held_listing_loses_the_folder() {
    let (server, store) = connected();
    store.folders(CACHED).expect("listed");

    manage::delete_folder(&store, "Projects").expect("removed");
    drop(server);

    assert_eq!(held(&store), ["Inbox"]);
}

/// A path no folder answers to. Another client having removed it first is
/// ordinary, and the store's own domain is where that is reported.
#[test]
fn a_path_the_account_does_not_have_is_reported_as_missing() {
    let (_server, store) = connected();

    let failure = manage::delete_folder(&store, "Nowhere").expect_err("no such folder");

    assert!(
        matches!(failure, StoreError::NoFolder(path) if path == "Nowhere"),
        "expected a missing folder"
    );
}

/// RFC 8621 §2.5 has the server refuse to destroy a mailbox that still holds
/// mail unless it is asked to take the mail with it, which is not what a click
/// on "Delete Folder" says. The refusal is the server's own, and the folder
/// stays in the listing because it stays on the server.
#[test]
fn a_folder_the_server_will_not_remove_stays_in_the_listing() {
    let (server, store) = connected();
    let listed = store.folders(CACHED).expect("listed");
    let projects = listed.find("Projects").expect("a folder").id.clone();
    {
        let account_id = server.account_id();
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_email(EmailSeed::new(
                projects,
                ("Bob", "bob@example.com"),
                "Lunch?",
                "text",
                "2026-01-15T09:00:00Z",
            ));
    }

    let failure = manage::delete_folder(&store, "Projects").expect_err("mail in the folder");

    assert!(
        matches!(failure, StoreError::Client(_)),
        "expected the server's own refusal, got {failure:?}"
    );
    assert!(on_the_server(&server).find("Projects").is_some());
    assert_eq!(held(&store), ["Inbox", "Projects"]);
}

#[test]
fn a_store_with_no_connection_cannot_remove_a_folder() {
    let store = JmapStore::detached();

    let failure = manage::delete_folder(&store, "Projects").expect_err("no connection");

    // No connection means no listing either, so the path resolves to nothing
    // before the write is even reached — and that is still the answer Camel
    // needs, since `NOT_CONNECTED` is what makes it connect and ask again.
    assert!(
        matches!(failure, StoreError::Disconnected),
        "expected a disconnected store, got {failure:?}"
    );
}

// ---------------------------------------------------------------------------
// the vfunc slots

/// Both slots are NULL on `CamelStore` itself — `camel_store_create_folder_sync`
/// refuses to call a store that has not filled them in — so installing them is
/// the difference between an account whose folders can be edited and one that
/// warns.
#[test]
fn the_store_fills_both_folder_management_slots() {
    // SAFETY: referencing the class runs the `class_init` that fills the slots;
    // the reference is released below, and the class is only read.
    unsafe {
        let class = g_type_class_ref(store_type()).cast::<CamelStoreClass>();
        assert!(
            (*class).create_folder_sync.is_some(),
            "the store cannot make a folder"
        );
        assert!(
            (*class).delete_folder_sync.is_some(),
            "the store cannot remove a folder"
        );

        let parent = g_type_class_peek(camel_offline_store_get_type()).cast::<CamelStoreClass>();
        assert!(!parent.is_null(), "the parent class is not initialised");
        assert!(
            (*parent).create_folder_sync.is_none() && (*parent).delete_folder_sync.is_none(),
            "CamelOfflineStore grew implementations of its own; the overrides \
             above are no longer the only things filling the slots"
        );

        g_type_class_unref(class.cast());
    }
}

/// The one folder id nothing may be built from: `rename_folder_sync` is still
/// unfilled, and this pins the pairing the module documents — the store must
/// not claim `CAMEL_STORE_CAN_EDIT_FOLDERS` while one of the three vfuncs
/// Evolution offers behind that flag is missing.
#[test]
fn the_store_does_not_yet_claim_it_can_rename() {
    // SAFETY: as above.
    unsafe {
        let class = g_type_class_ref(store_type()).cast::<CamelStoreClass>();
        assert!(
            (*class).rename_folder_sync.is_none(),
            "rename_folder_sync is filled in; CAMEL_STORE_CAN_EDIT_FOLDERS is \
             now the thing that turns folder management on"
        );
        g_type_class_unref(class.cast());
    }
}
