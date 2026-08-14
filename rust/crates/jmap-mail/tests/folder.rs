// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelJmapFolder`: the object a store hands out for one mailbox.
//!
//! Everything the store has answered with so far is a *description* of the
//! folders — a `CamelFolderInfo` forest, plain structs Camel frees and forgets.
//! A `CamelFolder` is the folder itself: the object `camel_store_get_folder_sync`
//! returns, that Evolution keeps open while the user reads it, and that every
//! later message operation is a method on. It is therefore the first place the
//! provider has to hold per-folder state, and the state that matters is the one
//! nothing in Camel's own model has a field for: the JMAP mailbox id. A Camel
//! path is a display-derived identifier this crate invented (see
//! `jmap-mail-sync`'s `path`), and no request can be built from it — `Email/query`
//! filters on `inMailbox`, which is the id.
//!
//! So the tests below are about the three things a folder is constructed from
//! and the one thing it is constructed *with*: the path Camel keys it by, the
//! name the user sees, the store it belongs to, and the mailbox id underneath.

mod common;

use std::ffi::CStr;

use common::Account;
use eds_sys::{
    CAMEL_FOLDER_FILTER_JUNK, CAMEL_FOLDER_FILTER_RECENT, CamelFolder, CamelStore,
    camel_folder_get_display_name, camel_folder_get_flags, camel_folder_get_full_name,
    camel_folder_get_parent_store, camel_folder_get_type, camel_offline_folder_get_type,
};
use glib_sys::{GFALSE, gchar};
use gobject_sys::{g_object_unref, g_type_is_a, g_type_name};
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_mail::folder::{JmapFolder, folder_type, new_folder};
use jmap_mail_sync::{FolderInfo, FolderRole};
use jmap_proto::Id;

/// One mailbox as the tree describes it — the only input a folder is built
/// from.
fn mailbox(path: &str, display_name: &str, role: Option<FolderRole>) -> FolderInfo {
    FolderInfo {
        id: Id::new("Mbx0001"),
        path: path.to_owned(),
        display_name: display_name.to_owned(),
        role,
        total: 0,
        unread: 0,
        subscribed: true,
        children: Vec::new(),
    }
}

/// The folder for one mailbox, owned by the caller.
///
/// A real store is needed for this and not merely convenient: Camel refuses to
/// construct a folder without one — `folder_set_parent_store` asserts
/// `CAMEL_IS_STORE`.
fn folder_of(account: &Account, mailbox: &FolderInfo) -> *mut CamelFolder {
    // SAFETY: `account.store` is a live `CamelStore` for as long as the
    // `Account` is.
    let folder = unsafe { new_folder(account.store, mailbox) };
    assert!(!folder.is_null(), "no folder for {}", mailbox.path);
    folder
}

/// Reads a folder's name back the way Camel does, as a borrowed C string.
///
/// # Safety
///
/// `folder` must be a live folder.
unsafe fn name(text: *const gchar) -> String {
    assert!(!text.is_null(), "the folder has no name");
    // SAFETY: the accessors return a NUL-terminated string owned by the
    // folder, which outlives the copy made here.
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

/// The parent Camel dispatches from. `CamelOfflineFolder` rather than
/// `CamelFolder`, for the same reason the store is a `CamelOfflineStore`: it is
/// the class that knows how to keep a folder's content on disk for a
/// disconnected client, and a folder that derives from `CamelFolder` directly
/// would have to grow that itself.
#[test]
fn the_folder_type_is_an_offline_folder() {
    let gtype = folder_type();
    assert_ne!(gtype, 0, "registration returned the invalid GType");

    // SAFETY: plain type-system reads on a registered type.
    unsafe {
        assert_eq!(CStr::from_ptr(g_type_name(gtype)), JmapFolder::NAME);
        assert_ne!(
            g_type_is_a(gtype, camel_offline_folder_get_type()),
            GFALSE,
            "a folder of an offline store must be an offline folder"
        );
        assert_ne!(g_type_is_a(gtype, camel_folder_get_type()), GFALSE);
    }
}

/// The three things Camel keys, shows and routes a folder by. The path is the
/// key — it is what `camel_store_get_folder_sync` was called with and what the
/// store's cache would look it up by — and the display name is the mailbox
/// name as the server spells it, which is not derivable from the path: the
/// encoding that makes a name a path component is not reversible by anything
/// that reads it.
#[test]
fn a_folder_is_the_camel_view_of_one_mailbox() {
    let account = Account::open();
    let mailbox = mailbox("Work/Q3 Plans", "Q3 Plans", None);
    let folder = folder_of(&account, &mailbox);

    // SAFETY: `folder` is a live folder this test owns.
    unsafe {
        assert_eq!(name(camel_folder_get_full_name(folder)), "Work/Q3 Plans");
        assert_eq!(name(camel_folder_get_display_name(folder)), "Q3 Plans");
        assert_eq!(
            camel_folder_get_parent_store(folder).cast::<CamelStore>(),
            account.store,
            "a folder belongs to the store it was made for"
        );
        g_object_unref(folder.cast());
    }
}

/// The state that makes it a *JMAP* folder. Nothing in `CamelFolder` has a
/// field for it, and nothing can reconstruct it: the path is an identifier this
/// crate invented from the mailbox's name, and `Email/query` filters on the id.
/// A folder that lost it could describe itself and fetch nothing.
#[test]
fn a_folder_knows_the_mailbox_its_requests_filter_on() {
    let account = Account::open();
    let folder = folder_of(
        &account,
        &mailbox("Inbox", "Inbox", Some(FolderRole::Inbox)),
    );

    // SAFETY: `folder` is a live folder of this type, and the borrow ends
    // before the unref.
    unsafe {
        let jmap = JmapFolder::borrow(folder).expect("a folder of ours");
        assert_eq!(jmap.mailbox(), Some(&Id::new("Mbx0001")));
        g_object_unref(folder.cast());
    }
}

/// What the inbox is, in Camel's vocabulary. `FILTER_RECENT` is what runs the
/// user's incoming filters over new mail, and `FILTER_JUNK` what runs the junk
/// test over it; Camel's own IMAPX sets exactly these two, on the folder it
/// finds by name. This provider knows which mailbox is the inbox from its JMAP
/// role instead, which is the same decision made from data rather than from a
/// convention about a name.
#[test]
fn new_mail_is_filtered_where_it_arrives() {
    let account = Account::open();
    let inbox = folder_of(
        &account,
        &mailbox("Inbox", "Inbox", Some(FolderRole::Inbox)),
    );
    let ordinary = folder_of(&account, &mailbox("Receipts", "Receipts", None));

    // SAFETY: both folders are live and owned here.
    unsafe {
        let filtered = CAMEL_FOLDER_FILTER_RECENT | CAMEL_FOLDER_FILTER_JUNK;
        assert_eq!(
            camel_folder_get_flags(inbox) & filtered,
            filtered,
            "the inbox is where incoming mail is filtered"
        );
        assert_eq!(
            camel_folder_get_flags(ordinary) & filtered,
            0,
            "a mailbox mail is moved into is not filtered again"
        );
        g_object_unref(inbox.cast());
        g_object_unref(ordinary.cast());
    }
}

/// A JMAP string is a JSON string, so a mailbox name can carry a NUL that RFC
/// 8621 forbids. Handing the bytes to Camel would truncate the name there — a
/// folder called `Work` sitting beside the real `Work` — so the NUL is rewritten
/// rather than obeyed, exactly as the `CamelFolderInfo` forest does it.
#[test]
fn a_name_with_a_nul_in_it_does_not_truncate_the_folder() {
    let account = Account::open();
    let folder = folder_of(&account, &mailbox("Work%2FSecret", "Work\0Secret", None));

    // SAFETY: `folder` is a live folder this test owns.
    unsafe {
        assert_eq!(
            name(camel_folder_get_display_name(folder)),
            "Work\u{fffd}Secret"
        );
        g_object_unref(folder.cast());
    }
}

/// A nested subfolder hierarchy retains full paths, display names, and mailbox ids.
#[test]
fn folder_properties_and_custom_subfolder_nesting() {
    let account = Account::open();
    let subfolder_info = FolderInfo {
        id: Id::new("MbxSub99"),
        path: "Archive/2026/Q1".to_owned(),
        display_name: "Q1".to_owned(),
        role: None,
        total: 15,
        unread: 3,
        subscribed: true,
        children: Vec::new(),
    };
    let folder = folder_of(&account, &subfolder_info);

    // SAFETY: `folder` is a live folder this test owns.
    unsafe {
        assert_eq!(name(camel_folder_get_full_name(folder)), "Archive/2026/Q1");
        assert_eq!(name(camel_folder_get_display_name(folder)), "Q1");
        assert_eq!(
            camel_folder_get_parent_store(folder).cast::<CamelStore>(),
            account.store
        );
        let jmap = JmapFolder::borrow(folder).expect("a folder of ours");
        assert_eq!(jmap.mailbox(), Some(&Id::new("MbxSub99")));
        g_object_unref(folder.cast());
    }
}
