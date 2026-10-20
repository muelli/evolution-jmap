// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for folder create, delete, rename, and move, from the store's side of
//! the three vfuncs.
//!
//! `jmap-mail-sync` sends all three writes to the server and decides what the
//! new folder is. This file tests the part only the store does: the folder
//! listing the store keeps between calls. It is the same part the
//! subscription vfuncs need.
//!
//! Camel passes the `CamelFolderInfo` a create returns directly to
//! Evolution's folder tree, then opens that folder by path through
//! `get_folder_sync`. `get_folder_sync` reads the answer from the listing. A
//! store can create the folder on the server without updating its own
//! listing. That store then offers the user a folder it refuses to open. A
//! delete has the same risk: a folder removed from the account but still in
//! the listing is a folder Camel still opens. A rename carries the same risk
//! twice, for the folder and for every folder under it, because Camel opens a
//! folder by its path.
//!
//! This file does not cover one thing: what each vfunc announces. An
//! announcement needs a store with a `CamelSession` attached, because
//! `camel_store_folder_created` takes the service's session and queues the
//! emission on it. The stores below are `detached`. `tests/emissions.rs`
//! covers announcements. Its first run found that the rename was announcing
//! itself twice: once on its own, and once through the announcement Camel's
//! own wrapper already makes.
//!
//! At the bottom, this file covers the pair that decides whether the user can
//! reach any of this. The pair is: the three vfunc slots being filled, and
//! the store's flags word carrying `CAMEL_STORE_CAN_EDIT_FOLDERS`.

use eds_sys::{
    CAMEL_STORE_CAN_EDIT_FOLDERS, CAMEL_STORE_SUPPORTS_INITIAL_SETUP, CAMEL_STORE_VJUNK,
    CAMEL_STORE_VTRASH, CamelStoreClass, camel_offline_store_get_type, camel_store_get_flags,
};
use gobject_sys::{g_type_class_peek, g_type_class_ref, g_type_class_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::connect::StoreError;
use jmap_mail::manage;
use jmap_mail::store::{JmapStore, store_type};
use jmap_mail_sync::{FolderTree, MailSync};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::mail::role;

mod common;

/// No flags. The store returns its already-held tree, without a server request.
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

/// The account as the server holds it, read over its own connection so the
/// answer is not the store's cached copy.
fn on_the_server(server: &MockServer) -> FolderTree {
    let (_, tree) = sync_against(server).folder_tree().expect("listed");
    tree
}

/// What the store's own listing holds, with no new request sent.
fn held(store: &JmapStore) -> Vec<String> {
    let tree = store.folders(CACHED).expect("a listing");
    tree.iter().map(|folder| folder.path.clone()).collect()
}

// ---------------------------------------------------------------------------
// folder creation

#[test]
fn the_new_folder_reaches_the_server() {
    let (server, store) = connected();

    manage::create_folder(&store, None, "Receipts").expect("created");

    assert!(on_the_server(&server).find("Receipts").is_some());
}

/// Tests this at the store layer, not the sync layer: the listing that
/// answers `get_folder_sync` already has the folder, without a new request to
/// the server.
#[test]
fn the_held_listing_gains_the_new_folder() {
    let (server, store) = connected();
    store.folders(CACHED).expect("listed");

    manage::create_folder(&store, None, "Receipts").expect("created");
    drop(server);

    assert_eq!(held(&store), ["Inbox", "Projects", "Receipts"]);
}

/// Camel names the parent by path, the same string it uses to name a folder
/// everywhere else. The new folder lands under that parent on both sides.
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

/// A NULL or empty `parent_name` means the account itself.
/// `get_folder_info_sync`'s `top` already reads it this way, and so does
/// Camel's own wrapper.
#[test]
fn an_empty_parent_is_the_account_itself() {
    let (_server, store) = connected();

    let created = manage::create_folder(&store, Some(""), "Receipts").expect("created");

    assert_eq!(created.path, "Receipts");
}

/// The name Camel passes is the exact mailbox name. JMAP assigns the folder
/// an explicit `parentId`, so the name carries no hierarchy, unlike an IMAP
/// store, which reads hierarchy from the name. A `/` in the name is an
/// ordinary character, and the path carries its encoding.
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

/// A missing parent is an error from the store, not the server, the same as
/// opening a folder that does not exist.
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

/// A parent created by another client since the last listing is found, not
/// reported missing. `get_folder_sync` already takes this second look,
/// because the account has the folder.
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

/// A server refusal is not a store failure. A name already used by a sibling
/// folder returns the server's own error, and the listing stays unchanged.
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
// folder deletion

#[test]
fn the_removal_reaches_the_server() {
    let (server, store) = connected();

    manage::delete_folder(&store, "Projects").expect("removed");

    assert!(on_the_server(&server).find("Projects").is_none());
}

/// The answer is the deleted folder. The vfunc must hand this folder to
/// `camel_store_folder_deleted`, after the folder is no longer found
/// anywhere.
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

/// A path with no matching folder. Another client may have removed it first;
/// this is normal, and the store reports it as a missing folder.
#[test]
fn a_path_the_account_does_not_have_is_reported_as_missing() {
    let (_server, store) = connected();

    let failure = manage::delete_folder(&store, "Nowhere").expect_err("no such folder");

    assert!(
        matches!(failure, StoreError::NoFolder(path) if path == "Nowhere"),
        "expected a missing folder"
    );
}

/// RFC 8621 §2.5 lets the server refuse to destroy a mailbox that still holds
/// mail, unless the request also asks to remove the mail. Clicking "Delete
/// Folder" does not ask for that. The refusal is the server's own, and the
/// folder stays in the listing because it stays on the server.
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

    // No connection means no listing, so the path resolves to nothing before
    // the write is reached. This is still the answer Camel needs:
    // `NOT_CONNECTED` makes Camel connect and ask again.
    assert!(
        matches!(failure, StoreError::Disconnected),
        "expected a disconnected store, got {failure:?}"
    );
}

// ---------------------------------------------------------------------------
// vfunc slots

/// Both slots are NULL on `CamelStore` itself. `camel_store_create_folder_sync`
/// refuses to call a store that leaves them unset. Filling the slots is the
/// difference between an account whose folders can be edited, and one that
/// only warns.
#[test]
fn the_store_fills_both_folder_management_slots() {
    // SAFETY: referencing the class runs `class_init`, which fills the slots.
    // The code releases the reference below, and only reads the class.
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

/// The third slot, and the one the other two depend on. Evolution offers New,
/// Rename, and Delete Folder behind one flag. A store that set the flag while
/// this slot was NULL would show the user a menu item that reaches a slot
/// Camel refuses to call.
#[test]
fn the_store_fills_the_rename_slot_too() {
    // SAFETY: as above.
    unsafe {
        let class = g_type_class_ref(store_type()).cast::<CamelStoreClass>();
        assert!(
            (*class).rename_folder_sync.is_some(),
            "the store cannot rename a folder"
        );

        let parent = g_type_class_peek(camel_offline_store_get_type()).cast::<CamelStoreClass>();
        assert!(!parent.is_null(), "the parent class is not initialised");
        assert!(
            (*parent).rename_folder_sync.is_none(),
            "CamelOfflineStore grew an implementation of its own; the override \
             above is no longer the only thing filling the slot"
        );

        g_type_class_unref(class.cast());
    }
}

/// The flag that all three slots are offered behind. This provider does not
/// set this flag and does not need to: `camel_store_init` turns
/// `CAN_EDIT_FOLDERS` on for every store. Only a provider that cannot edit
/// folders needs to write a line that clears it. This test pins the whole
/// flags word, so a changed Camel default, or a new line in this provider
/// that clears a bit, fails this test instead of silently removing a menu
/// item.
///
/// This test runs on a store that Camel constructed, because flags are
/// instance state. A detached store has no `CamelStore` to query.
///
/// `VTRASH` and `VJUNK` are two of the three bits this provider does clear
/// with a line of its own. `camel_store_init` normally asks Camel to build
/// every account a virtual Trash and Junk from flagged messages. A JMAP
/// account's trash and junk are instead mailboxes the server assigns a role
/// to. See `crate::store`'s `instance_init` for this decision, and
/// `tests/folders.rs` for the resulting listing. `SUPPORTS_INITIAL_SETUP` is
/// the third bit, and this provider sets it rather than clearing it: it is
/// the flag that `initial_setup_sync` is advertised behind. `tests/folders.rs`
/// covers what that vfunc answers.
#[test]
fn the_store_offers_folder_management() {
    let account = common::Account::open();

    // SAFETY: a live store, constructed by Camel through `g_initable_new`.
    let flags = unsafe { camel_store_get_flags(account.store) };

    assert!(
        flags & CAMEL_STORE_CAN_EDIT_FOLDERS != 0,
        "the store does not offer folder management: flags {flags:#x}"
    );
    assert_eq!(
        flags & (CAMEL_STORE_VTRASH | CAMEL_STORE_VJUNK),
        0,
        "the store asks Camel for virtual trash and junk folders: flags {flags:#x}"
    );
    assert_eq!(
        flags,
        CAMEL_STORE_CAN_EDIT_FOLDERS | CAMEL_STORE_SUPPORTS_INITIAL_SETUP,
        "the store's flags are no longer Camel's defaults minus the virtual folders"
    );
}

// ---------------------------------------------------------------------------
// folder rename

/// The path Camel hands over is the whole new path. Its last component is the
/// exact name the user typed into the rename dialog, read the same way a
/// create reads `folder_name`.
#[test]
fn the_rename_reaches_the_server() {
    let (server, store) = connected();

    manage::rename_folder(&store, "Projects", "Work").expect("renamed");

    let served = on_the_server(&server);
    assert!(served.find("Work").is_some());
    assert!(served.find("Projects").is_none());
}

/// The answer is the folder in its new state. The vfunc hands this to
/// `camel_store_folder_renamed`, which is how Evolution's folder tree learns
/// the new path.
#[test]
fn the_answer_is_the_folder_at_its_new_path() {
    let (server, store) = connected();
    let listed = store.folders(CACHED).expect("listed");
    let before = listed.find("Projects").expect("a folder").clone();

    let renamed = manage::rename_folder(&store, "Projects", "Work").expect("renamed");
    drop(server);

    assert_eq!(renamed.id, before.id);
    assert_eq!(renamed.path, "Work");
    assert_eq!(renamed.display_name, "Work");
}

#[test]
fn the_held_listing_follows_the_rename() {
    let (server, store) = connected();
    store.folders(CACHED).expect("listed");

    manage::rename_folder(&store, "Projects", "Work").expect("renamed");
    drop(server);

    assert_eq!(held(&store), ["Inbox", "Work"]);
}

/// A move is a rename to a path under another parent. Camel has no separate
/// vfunc for a move. The parent is named by path, like everywhere else.
#[test]
fn a_move_is_a_rename_to_a_path_under_another_parent() {
    let (server, store) = connected();
    store.folders(CACHED).expect("listed");

    let moved = manage::rename_folder(&store, "Projects", "Inbox/Projects").expect("moved");

    assert_eq!(moved.path, "Inbox/Projects");
    assert!(on_the_server(&server).find("Inbox/Projects").is_some());
    assert_eq!(held(&store), ["Inbox", "Inbox/Projects"]);
}

/// Every descendant of the folder moves with it, at paths rebuilt from the
/// new path. Evolution keys every open folder by its path. A descendant left
/// at its old path is a folder Camel would open twice.
///
/// The renamed folder joins its siblings at the end of the list. This is
/// `FolderTree`'s own ordering, and it is why the order here is not
/// alphabetical. Sibling order is set by the server: first by sortOrder, then
/// by name. This side only learns about the one renamed folder, not where the
/// account now sorts it.
#[test]
fn the_descendants_move_with_the_folder() {
    let (server, store) = connected();
    {
        let account_id = server.account_id();
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let projects = account.create_mailbox("Reports", None, None);
        account.create_mailbox("2026", None, Some(&projects));
    }
    store.folders(CACHED).expect("listed");

    manage::rename_folder(&store, "Reports", "Archive").expect("renamed");
    drop(server);

    assert_eq!(
        held(&store),
        ["Inbox", "Projects", "Archive", "Archive/2026"]
    );
}

/// The decision this vfunc turns on. Camel expresses a move as a rename to a
/// path whose last component is the folder's existing one, because
/// Evolution's drag and drop builds the path from the old path. An unchanged
/// last component is not a new name. Reading it as a new name would rename
/// `Bills/2026` to `Bills%2F2026` just because the folder was dragged.
#[test]
fn a_move_leaves_the_name_alone() {
    let (server, store) = connected();
    {
        let account_id = server.account_id();
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .create_mailbox("Bills/2026", None, None);
    }
    store.folders(CACHED).expect("listed");

    let moved =
        manage::rename_folder(&store, "Bills%2F2026", "Projects/Bills%2F2026").expect("moved");

    assert_eq!(moved.path, "Projects/Bills%2F2026");
    assert_eq!(moved.display_name, "Bills/2026");
    assert_eq!(
        on_the_server(&server)
            .find("Projects/Bills%2F2026")
            .map(|folder| folder.display_name.clone()),
        Some("Bills/2026".to_owned())
    );
}

/// The "unchanged" reading above holds only together with an actual parent
/// change. A drag always moves and never renames. The rename dialog always
/// keeps the parent and never moves. A same-parent call, the shape the
/// rename dialog always uses, is therefore always the typed name, even when
/// it happens to match the folder's own encoded component exactly. Without
/// this rule, that one case would read as "nothing to do" and silently drop
/// the user's rename.
#[test]
fn a_same_parent_call_is_always_the_typed_name_even_if_it_matches_the_encoding() {
    let (server, store) = connected();
    {
        let account_id = server.account_id();
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .create_mailbox("Bills/2026", None, None);
    }
    store.folders(CACHED).expect("listed");

    let renamed = manage::rename_folder(&store, "Bills%2F2026", "Bills%2F2026").expect("renamed");

    // The typed name itself needs encoding, because it has a `%` in it. The
    // real path is not the one Camel asked for, the same caveat
    // `a_new_last_component_is_the_name_the_user_typed` documents.
    assert_eq!(renamed.path, "Bills%252F2026");
    assert_eq!(renamed.display_name, "Bills%2F2026");
    assert_eq!(
        on_the_server(&server)
            .find("Bills%252F2026")
            .map(|folder| folder.display_name.clone()),
        Some("Bills%2F2026".to_owned())
    );
}

/// A changed component is the exact name the user typed, taken character for
/// character, including a `/`. Evolution's rename dialog refuses a `/`, but
/// this test does not rely on that refusal.
///
/// This comes with a limit, stated here rather than hidden: a typed name that
/// this crate must encode ends up at a path different from the one Camel
/// asked for. The answer carries the real path, so the folder tree is
/// correct. But a `CamelFolder` that Camel already rekeyed to the requested
/// path is not correct, until the account is listed again.
#[test]
fn a_new_last_component_is_the_name_the_user_typed() {
    let (server, store) = connected();
    store.folders(CACHED).expect("listed");

    let renamed = manage::rename_folder(&store, "Projects", "100%").expect("renamed");

    assert_eq!(renamed.display_name, "100%");
    assert_eq!(renamed.path, "100%25");
    assert_eq!(
        on_the_server(&server)
            .find("100%25")
            .map(|folder| folder.display_name.clone()),
        Some("100%".to_owned())
    );
}

#[test]
fn a_path_the_account_does_not_have_is_reported_as_missing_by_a_rename() {
    let (_server, store) = connected();

    let failure = manage::rename_folder(&store, "Nowhere", "Work").expect_err("no such folder");

    assert!(
        matches!(failure, StoreError::NoFolder(path) if path == "Nowhere"),
        "expected a missing folder"
    );
}

/// The new parent is resolved and reported the same way as before. Nothing is
/// written, because a folder moved under a missing parent would be a folder
/// nothing can reach.
#[test]
fn a_new_parent_the_account_does_not_have_is_reported_as_missing() {
    let (server, store) = connected();
    store.folders(CACHED).expect("listed");

    let failure =
        manage::rename_folder(&store, "Projects", "Nowhere/Projects").expect_err("no such parent");

    assert!(
        matches!(failure, StoreError::NoFolder(path) if path == "Nowhere"),
        "expected a missing folder"
    );
    assert!(on_the_server(&server).find("Projects").is_some());
}

/// A parent created by another client since the last listing is looked for
/// again. Every other folder vfunc takes this same second look.
#[test]
fn a_new_parent_created_since_the_listing_is_looked_for_again() {
    let (server, store) = connected();
    store.folders(CACHED).expect("listed");
    {
        let account_id = server.account_id();
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .create_mailbox("Archive", None, None);
    }

    let moved = manage::rename_folder(&store, "Projects", "Archive/Projects").expect("moved");

    assert_eq!(moved.path, "Archive/Projects");
}

/// A server refusal is not a store failure. The listing matches the account:
/// the folder is still where it was.
#[test]
fn a_refused_rename_leaves_the_listing_alone() {
    let (_server, store) = connected();
    store.folders(CACHED).expect("listed");

    let failure = manage::rename_folder(&store, "Projects", "Inbox").expect_err("a name in use");

    assert!(
        matches!(failure, StoreError::Client(_)),
        "expected the server's own refusal, got {failure:?}"
    );
    assert_eq!(held(&store), ["Inbox", "Projects"]);
}

#[test]
fn a_store_with_no_connection_cannot_rename_a_folder() {
    let store = JmapStore::detached();

    let failure = manage::rename_folder(&store, "Projects", "Work").expect_err("no connection");

    assert!(
        matches!(failure, StoreError::Disconnected),
        "expected a disconnected store, got {failure:?}"
    );
}
