// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The folder the user adds to an account, the one they take away, and the one
//! they rename or move — from the store's side of the three vfuncs.
//!
//! `jmap-mail-sync` already puts all three writes on the wire and already
//! decides what the new folder *is*. What is tested here is the part only a
//! store can do, and it is the same part the subscription vfuncs needed: the
//! folder listing the store keeps between calls. Camel hands the
//! `CamelFolderInfo` a create answers with straight to Evolution's folder tree
//! and then opens that folder by path — through `get_folder_sync`, which is
//! answered out of the listing — so a store that made the folder on the server
//! and left its own listing without it would offer the user a folder it then
//! refuses to open. The same one step behind for a delete: a folder that is
//! gone from the account and still in the listing is one Camel will happily
//! open again. And a rename is both at once, for the folder and for everything
//! under it, since every one of those paths is a key Camel opens a folder by.
//!
//! One thing above is not covered *here*, and it is the vfunc rather than what
//! it decides: what each of the three announces needs a store with a
//! `CamelSession` behind it, since `camel_store_folder_created` starts by
//! taking the service's session and queueing the emission on it, and the stores
//! below are `detached`. That is `tests/emissions.rs`, whose first run found
//! that the rename had been announcing itself on top of the announcement
//! Camel's own wrapper already makes.
//!
//! What is covered, at the bottom, is the pair that decides whether the user
//! ever reaches any of it: the three slots being filled, and the store's flags
//! word carrying `CAMEL_STORE_CAN_EDIT_FOLDERS`.

use eds_sys::{
    CAMEL_STORE_CAN_EDIT_FOLDERS, CAMEL_STORE_VJUNK, CAMEL_STORE_VTRASH, CamelStoreClass,
    camel_offline_store_get_type, camel_store_get_flags,
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

/// The third of them, and the one the other two waited on: Evolution offers
/// New, Rename and Delete Folder behind one flag, so a store that set the flag
/// while this slot was NULL would put a menu item in front of the user that
/// reaches a slot Camel refuses to call.
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

/// And the flag the three are offered behind, which this provider does not set
/// and does not have to: `camel_store_init` turns `CAN_EDIT_FOLDERS` on for
/// every store there is, and a provider that cannot edit folders is the one
/// with a line to write. What is pinned here is therefore the whole word, so
/// that Camel changing its defaults — or this provider growing a line that
/// clears a bit — is a red test rather than a menu item that quietly goes away.
///
/// On a store Camel constructed, because flags are instance state: a detached
/// store is not a `CamelStore` to ask.
///
/// `VTRASH` and `VJUNK` are in the word too, and they are Camel's defaults
/// rather than a decision this provider has taken: they ask Camel to build the
/// account a virtual Trash and Junk out of the messages flagged as such.
/// Whether a JMAP account wants those or the mailboxes its server gives roles
/// to is what `get_trash_folder_sync` and `get_junk_folder_sync` still wait on.
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
        flags,
        CAMEL_STORE_CAN_EDIT_FOLDERS | CAMEL_STORE_VTRASH | CAMEL_STORE_VJUNK,
        "the store's flags are no longer Camel's defaults"
    );
}

// ---------------------------------------------------------------------------
// and the one they rename

/// The path Camel hands over is the whole new path, and its last component is
/// the name the user typed into the rename dialog — verbatim, the same reading
/// a create makes of `folder_name`.
#[test]
fn the_rename_reaches_the_server() {
    let (server, store) = connected();

    manage::rename_folder(&store, "Projects", "Work").expect("renamed");

    let served = on_the_server(&server);
    assert!(served.find("Work").is_some());
    assert!(served.find("Projects").is_none());
}

/// The answer is the folder as it now is — what the vfunc hands
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

/// A move is a rename to a path under another parent — Camel has no separate
/// vfunc for one — and the parent is named by path, like everything else.
#[test]
fn a_move_is_a_rename_to_a_path_under_another_parent() {
    let (server, store) = connected();
    store.folders(CACHED).expect("listed");

    let moved = manage::rename_folder(&store, "Projects", "Inbox/Projects").expect("moved");

    assert_eq!(moved.path, "Inbox/Projects");
    assert!(on_the_server(&server).find("Inbox/Projects").is_some());
    assert_eq!(held(&store), ["Inbox", "Inbox/Projects"]);
}

/// Whatever hung under the folder moves with it, at paths rebuilt from the new
/// one. Evolution keys every open folder by path, so a descendant left at its
/// old one is a folder Camel would open twice.
///
/// The renamed folder joins its siblings at the end, which is `FolderTree`'s
/// judgement and is why the order here is not alphabetical: sibling order is
/// the server's — sortOrder, then name — and this side has been told about one
/// folder, not about where the account now sorts it.
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

/// The decision this vfunc turns on. Camel spells a move as a rename to a path
/// whose last component is the folder's *existing* one — Evolution's drag and
/// drop builds it from the old path — so a component that did not change is not
/// a new name, and reading it as one would rename `Bills/2026` to
/// `Bills%2F2026` for the crime of being dragged.
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

/// A component that *did* change is the name the user typed, and it is taken as
/// one character for character — including a `/`, which Evolution's rename
/// dialog refuses but nothing here relies on.
///
/// The limit that comes with it, stated rather than hidden: a typed name this
/// crate has to encode ends up at a path that is not the one Camel asked for.
/// The name is what the user asked for and the answer carries the real path, so
/// the folder tree is right; a `CamelFolder` Camel had already rekeyed to the
/// requested path is not, until the account is listed again.
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

/// The new parent is resolved the same way and reported the same way — and
/// nothing is written, because a folder moved under a parent that is not there
/// is a folder nothing can reach.
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

/// And a parent another client made since the last listing is looked for again,
/// the second look every other folder vfunc takes.
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

/// The server refusing is not the store failing, and the listing says what the
/// account says: the folder is still where it was.
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
