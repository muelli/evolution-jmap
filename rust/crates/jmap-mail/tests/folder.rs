// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelJmapFolder`: the object a store hands out for one mailbox.
//!
//! Everything the store has answered with so far is only a *description* of
//! the folders: a `CamelFolderInfo` forest, made of plain structs that Camel
//! frees and forgets. A `CamelFolder` is the folder itself: the object that
//! `camel_store_get_folder_sync` returns, that Evolution keeps open while the
//! user reads it, and that every later message operation is a method on. It
//! is therefore the first place where the provider must hold per-folder
//! state. The state that matters is the one field that nothing in Camel's own
//! model has: the JMAP mailbox id. A Camel path is a display-derived
//! identifier that this crate invented (see `jmap-mail-sync`'s `path`), and no
//! request can be built from the path alone: `Email/query` filters on
//! `inMailbox`, which is the id.
//!
//! The tests below check the three things that a folder is constructed from,
//! and the one thing it is constructed *with*: the path Camel keys it by, the
//! name the user sees, the store it belongs to, and the mailbox id underneath.

mod common;

use std::ffi::CStr;

use common::Account;
use eds_sys::{
    CAMEL_FOLDER_FILTER_JUNK, CAMEL_FOLDER_FILTER_RECENT, CAMEL_MESSAGE_ANSWERED,
    CAMEL_MESSAGE_DELETED, CAMEL_MESSAGE_DRAFT, CAMEL_MESSAGE_FLAGGED, CAMEL_MESSAGE_FORWARDED,
    CAMEL_MESSAGE_JUNK, CAMEL_MESSAGE_NOTJUNK, CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_USER, CamelFolder,
    CamelStore, camel_folder_cmp_uids, camel_folder_get_display_name, camel_folder_get_flags,
    camel_folder_get_full_name, camel_folder_get_parent_store, camel_folder_get_permanent_flags,
    camel_folder_get_type, camel_folder_sort_uids, camel_offline_folder_get_type,
};
use glib_sys::{GFALSE, g_ptr_array_add, g_ptr_array_new, g_ptr_array_unref, gchar};
use gobject_sys::{g_object_unref, g_type_is_a, g_type_name};
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_mail::folder::{JmapFolder, folder_type, new_folder};
use jmap_mail_sync::{FolderInfo, FolderRole};
use jmap_proto::Id;

/// One mailbox as the tree describes it. This is the only input that a
/// folder is built from.
fn mailbox(path: &str, display_name: &str, role: Option<FolderRole>) -> FolderInfo {
    FolderInfo {
        id: Id::new("Mbx0001"),
        path: path.to_owned(),
        display_name: display_name.to_owned(),
        role,
        total: 0,
        unread: 0,
        subscribed: true,
        may_add_items: None,
        children: Vec::new(),
    }
}

/// The folder for one mailbox, owned by the caller.
///
/// A real store is required here, not merely convenient: Camel refuses to
/// construct a folder without one. `folder_set_parent_store` asserts
/// `CAMEL_IS_STORE`.
fn folder_of(account: &Account, mailbox: &FolderInfo) -> *mut CamelFolder {
    // SAFETY: `account.store` is a live `CamelStore` for as long as the
    // `Account` is live.
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

/// The parent class that Camel dispatches from. This is `CamelOfflineFolder`
/// rather than `CamelFolder`, for the same reason the store is a
/// `CamelOfflineStore`: it is the class that knows how to keep a folder's
/// content on disk for a disconnected client. A folder that derived from
/// `CamelFolder` directly would have to implement that itself.
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

/// The three things that Camel keys, shows, and routes a folder by. The path
/// is the key: it is what `camel_store_get_folder_sync` was called with, and
/// what the store's cache looks it up by. The display name is the mailbox
/// name as the server spells it. The display name cannot be derived from the
/// path: the encoding that turns a name into a path component is not
/// reversible by anything that reads it.
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

/// The state that makes a folder a *JMAP* folder. Nothing in `CamelFolder` has
/// a field for the mailbox id, and nothing can reconstruct it: the path is an
/// identifier that this crate invented from the mailbox's name, and
/// `Email/query` filters on the id, not the path. A folder that lost the id
/// could describe itself but fetch nothing.
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

/// The message cache that every later read checks before asking the server.
/// The account that this test opens has a real cache directory. A folder
/// built on it must come back with a working cache, not the `None` that
/// results when folder construction cannot find a directory.
#[test]
fn a_folder_on_an_account_with_a_cache_directory_has_one() {
    let account = Account::open();
    let folder = folder_of(&account, &mailbox("Inbox", "Inbox", None));

    // SAFETY: `folder` is a live folder of this type, and the borrow ends
    // before the unref.
    unsafe {
        let jmap = JmapFolder::borrow(folder).expect("a folder of ours");
        assert!(
            jmap.cache().is_some(),
            "the account has a cache directory to open one in"
        );
        g_object_unref(folder.cast());
    }
}

/// What the inbox is, in Camel's vocabulary. `FILTER_RECENT` runs the user's
/// incoming filters over new mail, and `FILTER_JUNK` runs the junk test over
/// it. Camel's own IMAPX sets exactly these two flags, on the folder it finds
/// by name. This provider instead identifies the inbox from its JMAP role:
/// the same decision, made from data rather than from a naming convention.
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

/// A JMAP string is a JSON string, so a mailbox name can carry a NUL, although
/// RFC 8621 forbids this. Handing the raw bytes to Camel would truncate the
/// name there, producing a folder called `Work` that sits beside the real
/// `Work`. Instead, the code rewrites the NUL rather than passing it through,
/// exactly as the `CamelFolderInfo` forest does.
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

/// A nested subfolder hierarchy keeps its full paths, display names, and
/// mailbox ids.
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
        may_add_items: None,
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

/// How Camel orders two uids. Camel's own default comparator parses each uid
/// as a decimal number, because that is what an IMAP uid is. A JMAP id is an
/// opaque string instead. Under that default, every id that does not start
/// with a digit would parse as 0, and every pair of such messages would
/// compare equal. Byte order is an arbitrary choice, but it is a total order,
/// and `sort_uids` is stable only on top of a total order.
#[test]
fn uids_compare_as_strings_not_as_numbers() {
    let account = Account::open();
    let folder = folder_of(&account, &mailbox("Inbox", "Inbox", None));

    let ids = [c"Mc", c"Ma2", c"Ma10", c"Mb"];
    // SAFETY: `folder` is a live folder this test owns. The uid strings are
    // static, and the array only borrows them until it is freed below.
    unsafe {
        assert!(camel_folder_cmp_uids(folder, c"Ma".as_ptr(), c"Mb".as_ptr()) < 0);
        assert!(camel_folder_cmp_uids(folder, c"Mb".as_ptr(), c"Ma".as_ptr()) > 0);
        assert_eq!(
            camel_folder_cmp_uids(folder, c"Ma".as_ptr(), c"Ma".as_ptr()),
            0
        );

        let array = g_ptr_array_new();
        for id in ids {
            g_ptr_array_add(array, id.as_ptr().cast_mut().cast());
        }
        camel_folder_sort_uids(folder, array);
        let sorted: Vec<String> = (0..(*array).len as usize)
            .map(|i| name((*(*array).pdata.add(i)).cast()))
            .collect();
        assert_eq!(sorted, ["Ma10", "Ma2", "Mb", "Mc"]);

        g_ptr_array_unref(array);
        g_object_unref(folder.cast());
    }
}

/// The flags that a folder can keep on a message between sessions: the flags
/// that the server keeps as keywords (including the user's labels, `USER`),
/// plus `DELETED`, which Camel's own summary keeps until the expunge. Camel's
/// default answer is 0, which tells a caller that nothing survives, not even
/// the read state.
#[test]
fn permanent_flags_are_the_ones_the_server_keeps() {
    let account = Account::open();
    let folder = folder_of(&account, &mailbox("Inbox", "Inbox", None));

    // SAFETY: `folder` is a live folder this test owns.
    let flags = unsafe { camel_folder_get_permanent_flags(folder) };
    assert_eq!(
        flags,
        CAMEL_MESSAGE_ANSWERED
            | CAMEL_MESSAGE_DELETED
            | CAMEL_MESSAGE_DRAFT
            | CAMEL_MESSAGE_FLAGGED
            | CAMEL_MESSAGE_SEEN
            | CAMEL_MESSAGE_FORWARDED
            | CAMEL_MESSAGE_JUNK
            | CAMEL_MESSAGE_NOTJUNK
            | CAMEL_MESSAGE_USER
    );

    // SAFETY: the test's own reference.
    unsafe { g_object_unref(folder.cast()) };
}
