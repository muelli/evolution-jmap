// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelFolderSummary`: the folder's own copy of what is in the mailbox.
//!
//! The previous increment built one row. This is the collection those rows go
//! into, and it is what turns a folder from an object with a name into a folder
//! with contents: Camel asks a summary for the message count it shows beside
//! the folder, for the uid list the message list is drawn from, and for the row
//! behind every line in it. A folder without one answers none of those, which
//! is why `CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY` — the flag that says the
//! question may be asked at all — could not be set before now.
//!
//! Filling it is not a copy of the listing either, because a listing arrives
//! more than once. The second one meets rows that are already there, and the
//! tests below are mostly about what happens then: which columns a refresh is
//! allowed to rewrite (the ones JMAP calls mutable, and no others), what
//! happens to a row whose message has left the mailbox, and what happens to the
//! marks the *user* made, which the server has never heard of and would
//! otherwise be undone by every refresh.

mod common;

use std::collections::BTreeSet;
use std::ffi::{CStr, CString};
use std::ptr;

use common::Account;
use eds_sys::{
    CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY, CAMEL_MESSAGE_DELETED, CAMEL_MESSAGE_FLAGGED,
    CAMEL_MESSAGE_SEEN, CamelFolder, CamelFolderSummary, CamelFolderSummaryClass,
    camel_folder_get_flags, camel_folder_get_folder_summary, camel_folder_has_summary_capability,
    camel_folder_summary_check_uid, camel_folder_summary_count, camel_folder_summary_free_array,
    camel_folder_summary_get, camel_folder_summary_get_array, camel_folder_summary_get_folder,
    camel_folder_summary_get_next_uid, camel_folder_summary_get_unread_count,
    camel_folder_summary_load, camel_folder_summary_save, camel_message_info_clone,
    camel_message_info_get_flags, camel_message_info_get_subject, camel_message_info_get_uid,
    camel_message_info_get_user_flag, camel_message_info_set_flags,
};
use glib_sys::{GError, GFALSE, GTRUE};
use gobject_sys::{GTypeInstance, g_object_unref};
use jmap_mail::folder::new_folder;
use jmap_mail::message_info::{message_info_type, server_keywords};
use jmap_mail::summary::apply_listing;
use jmap_mail_sync::{FolderInfo, Keywords, MessageFlags, MessageSummary};
use jmap_proto::Id;

/// A folder for a mailbox nobody has listed yet, together with the account it
/// hangs off — the store has to outlive the folder, so both are returned.
fn open_folder(account: &Account) -> *mut CamelFolder {
    let mailbox = FolderInfo {
        id: Id::new("Mbx0001"),
        path: "Inbox".to_owned(),
        display_name: "Inbox".to_owned(),
        role: None,
        total: 0,
        unread: 0,
        subscribed: true,
        children: Vec::new(),
    };
    // SAFETY: `account.store` is a live `CamelStore` for as long as the
    // `Account` is.
    let folder = unsafe { new_folder(account.store, &mailbox) };
    assert!(!folder.is_null(), "no folder for the mailbox");
    folder
}

/// The summary Camel would find on that folder.
///
/// # Safety
///
/// `folder` must be a live folder.
unsafe fn summary_of(folder: *mut CamelFolder) -> *mut CamelFolderSummary {
    // SAFETY: the accessor returns the summary the folder owns, borrowed.
    let summary = unsafe { camel_folder_get_folder_summary(folder) };
    assert!(!summary.is_null(), "the folder kept no summary");
    summary
}

/// A row with nothing in it but the one thing a row cannot be without.
fn message(uid: &str) -> MessageSummary {
    MessageSummary {
        uid: Id::new(uid),
        blob_id: None,
        thread_id: None,
        flags: MessageFlags::default(),
        tags: Vec::new(),
        size: 0,
        received_at: None,
        sent_at: None,
        subject: None,
        from: Vec::new(),
        to: Vec::new(),
        cc: Vec::new(),
        message_id: None,
        references: Vec::new(),
        preview: None,
    }
}

/// Every uid the summary holds a row for.
///
/// # Safety
///
/// `summary` must be a live summary.
unsafe fn uids(summary: *mut CamelFolderSummary) -> BTreeSet<String> {
    // SAFETY: the array is a snapshot the caller owns and frees; every element
    // is a NUL-terminated string that lives until it is freed with the array.
    unsafe {
        let array = camel_folder_summary_get_array(summary);
        assert!(!array.is_null(), "the summary listed no uids at all");
        let uids = (0..(*array).len)
            .map(|index| {
                let uid = *(*array).pdata.add(index as usize);
                CStr::from_ptr(uid.cast()).to_string_lossy().into_owned()
            })
            .collect();
        camel_folder_summary_free_array(array);
        uids
    }
}

/// The row for one uid, or `None` where the summary has none — the caller owns
/// what it gets back.
///
/// # Safety
///
/// `summary` must be a live summary.
unsafe fn row(
    summary: *mut CamelFolderSummary,
    uid: &str,
) -> Option<*mut eds_sys::CamelMessageInfo> {
    let uid = CString::new(uid).expect("a uid with no NUL in it");
    // SAFETY: the uid outlives both calls, and `get` returns a reference the
    // caller owns.
    unsafe {
        if camel_folder_summary_check_uid(summary, uid.as_ptr()) == GFALSE {
            return None;
        }
        let info = camel_folder_summary_get(summary, uid.as_ptr());
        assert!(!info.is_null(), "the summary knew the uid and had no row");
        Some(info)
    }
}

/// A folder is a name until it has somewhere to keep its contents. Camel gives
/// a summary a back-pointer to its folder — that is how it reaches the store's
/// database — and the flag is Camel's own precondition: it tests
/// `HAS_SUMMARY_CAPABILITY` before it asks a folder for a message count, so a
/// folder with a summary and without the flag is one whose contents are never
/// asked for.
#[test]
fn a_folder_keeps_a_summary_of_its_own() {
    let account = Account::open();
    let folder = open_folder(&account);

    // SAFETY: `folder` is a live folder this test owns.
    unsafe {
        let summary = summary_of(folder);
        assert_eq!(
            camel_folder_summary_get_folder(summary),
            folder,
            "a summary belongs to the folder it was made for"
        );
        assert_ne!(camel_folder_has_summary_capability(folder), GFALSE);
        assert_ne!(
            camel_folder_get_flags(folder) & CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY,
            0
        );
        assert_eq!(
            camel_folder_summary_count(summary),
            0,
            "a folder nobody has listed yet holds nothing"
        );
        g_object_unref(folder.cast());
    }
}

/// The listing, as rows. One row per message and no others: a summary is what
/// Camel draws the message list from, so a row too many is a message the user
/// sees and cannot open.
#[test]
fn a_listing_becomes_the_folders_rows() {
    let account = Account::open();
    let folder = open_folder(&account);

    // SAFETY: `folder` is live, and so is the summary borrowed from it.
    unsafe {
        let summary = summary_of(folder);
        apply_listing(summary, &[message("M1001"), message("M1002")]);

        assert_eq!(camel_folder_summary_count(summary), 2);
        assert_eq!(
            uids(summary),
            BTreeSet::from(["M1001".to_owned(), "M1002".to_owned()])
        );
        g_object_unref(folder.cast());
    }
}

/// A JMAP `Email` id is already everything Camel wants a uid to be: unique in
/// the mailbox, immutable, and the thing every later request names the message
/// by. A row under any other name is one nothing can ever be fetched for, and
/// Camel has a counter of its own it numbers rows from — so what is asserted
/// here is not only that the row can be found under the server's id but that
/// the counter never moved, which is the half that would still be wrong if a
/// row had been numbered and had happened to land on its own name.
#[test]
fn a_row_keeps_the_id_the_server_gave_it() {
    let account = Account::open();
    let folder = open_folder(&account);

    // SAFETY: `folder` is live, and the row borrowed below is unreffed here.
    unsafe {
        let summary = summary_of(folder);
        let before = camel_folder_summary_get_next_uid(summary);

        apply_listing(summary, &[message("M1001")]);

        let info = row(summary, "M1001").expect("a row for the message");
        assert_eq!(
            CStr::from_ptr(camel_message_info_get_uid(info)).to_string_lossy(),
            "M1001"
        );
        assert_eq!(
            camel_folder_summary_get_next_uid(summary),
            before,
            "the summary numbered a message the server had already named"
        );
        g_object_unref(info.cast());
        g_object_unref(folder.cast());
    }
}

/// The rest of the row comes along with it: the columns `message_info` fills
/// are what a summary is read for, and the unread count Camel shows beside the
/// folder is maintained out of the flags word as rows go in.
#[test]
fn a_row_carries_what_the_message_said() {
    let account = Account::open();
    let folder = open_folder(&account);

    let mut read = message("M1001");
    read.subject = Some("Q3 plans".to_owned());
    read.flags.seen = true;
    let unread = message("M1002");

    // SAFETY: `folder` is live and the row is unreffed before it goes.
    unsafe {
        let summary = summary_of(folder);
        apply_listing(summary, &[read, unread]);

        let info = row(summary, "M1001").expect("a row for the message");
        assert_eq!(
            CStr::from_ptr(camel_message_info_get_subject(info)).to_string_lossy(),
            "Q3 plans"
        );
        assert_ne!(camel_message_info_get_flags(info) & CAMEL_MESSAGE_SEEN, 0);
        assert_eq!(
            camel_folder_summary_get_unread_count(summary),
            1,
            "one of the two messages was read"
        );
        g_object_unref(info.cast());
        g_object_unref(folder.cast());
    }
}

/// A mailbox is listed again every time the folder is refreshed, and the second
/// listing is mostly the first one. Adding what is already there twice would be
/// every message shown twice.
#[test]
fn a_second_listing_does_not_list_the_same_message_twice() {
    let account = Account::open();
    let folder = open_folder(&account);

    // SAFETY: `folder` is live.
    unsafe {
        let summary = summary_of(folder);
        apply_listing(summary, &[message("M1001"), message("M1002")]);
        apply_listing(summary, &[message("M1001"), message("M1002")]);

        assert_eq!(camel_folder_summary_count(summary), 2);
        g_object_unref(folder.cast());
    }
}

/// A message that left the mailbox — deleted, or moved somewhere else, which in
/// JMAP is the same `Email/set` — is a row Camel would otherwise keep showing
/// and be unable to open. `Email/query` answering without it is the only notice
/// there is.
#[test]
fn a_message_that_left_the_mailbox_loses_its_row() {
    let account = Account::open();
    let folder = open_folder(&account);

    // SAFETY: `folder` is live.
    unsafe {
        let summary = summary_of(folder);
        apply_listing(
            summary,
            &[message("M1001"), message("M1002"), message("M1003")],
        );
        apply_listing(summary, &[message("M1001"), message("M1003")]);

        assert_eq!(
            uids(summary),
            BTreeSet::from(["M1001".to_owned(), "M1003".to_owned()])
        );
        assert_eq!(camel_folder_summary_count(summary), 2);
        g_object_unref(folder.cast());
    }
}

/// What a refresh is allowed to rewrite. RFC 8621 §4.1 makes every property of
/// an `Email` immutable except `keywords` and `mailboxIds`, so a message that
/// was read since the last listing is the same row with one bit different — and
/// rewriting the whole row would be re-deriving a dozen columns that cannot
/// have changed, on every message, on every refresh.
#[test]
fn a_message_read_since_the_last_listing_is_marked_read_on_the_row_it_has() {
    let account = Account::open();
    let folder = open_folder(&account);

    let mut read = message("M1001");
    read.flags.seen = true;

    // SAFETY: `folder` is live and both rows are unreffed here.
    unsafe {
        let summary = summary_of(folder);
        apply_listing(summary, &[message("M1001")]);
        let before = row(summary, "M1001").expect("a row for the message");
        assert_eq!(camel_message_info_get_flags(before) & CAMEL_MESSAGE_SEEN, 0);

        apply_listing(summary, &[read]);

        let after = row(summary, "M1001").expect("a row for the message");
        assert_ne!(camel_message_info_get_flags(after) & CAMEL_MESSAGE_SEEN, 0);
        assert_eq!(after, before, "the row was replaced rather than updated");
        g_object_unref(before.cast());
        g_object_unref(after.cast());
        g_object_unref(folder.cast());
    }
}

/// The marks the server has never heard of. `CAMEL_MESSAGE_DELETED` is one the
/// user made locally — JMAP has no deleted keyword, so nothing in a listing
/// says anything about it — and a refresh that cleared it because the server
/// was silent would undo a deletion the user is waiting to have expunged.
/// `FLAGGED` is the opposite case in the same word: the server does speak to
/// it, so a listing that stops carrying `$flagged` takes it off.
#[test]
fn a_refresh_does_not_undo_a_mark_the_server_never_saw() {
    let account = Account::open();
    let folder = open_folder(&account);

    let mut flagged = message("M1001");
    flagged.flags.flagged = true;

    // SAFETY: `folder` is live and every row taken is unreffed.
    unsafe {
        let summary = summary_of(folder);
        apply_listing(summary, &[flagged]);

        let info = row(summary, "M1001").expect("a row for the message");
        camel_message_info_set_flags(info, CAMEL_MESSAGE_DELETED, CAMEL_MESSAGE_DELETED);
        g_object_unref(info.cast());

        apply_listing(summary, &[message("M1001")]);

        let info = row(summary, "M1001").expect("a row for the message");
        let flags = camel_message_info_get_flags(info);
        assert_ne!(
            flags & CAMEL_MESSAGE_DELETED,
            0,
            "the refresh undid a deletion the user had made"
        );
        assert_eq!(
            flags & CAMEL_MESSAGE_FLAGGED,
            0,
            "the server stopped saying the message was flagged"
        );
        g_object_unref(info.cast());
        g_object_unref(folder.cast());
    }
}

/// Labels are keywords too — the ones Camel has no bit for — so the same rule
/// applies to them, and it has to be applied to the set rather than to one
/// name: a keyword the listing no longer carries is a label the user took off
/// in some other client, and Camel is only told about it by its absence.
///
/// Taken off one at a time and then down to none, because the last one is the
/// case an implementation gets wrong: a listing that carries no keywords at
/// all looks exactly like a listing with nothing to say about them, and it is
/// not — user flags have no way to spell "absent", so an empty set is the
/// whole answer rather than a missing one.
#[test]
fn a_label_taken_off_at_the_server_comes_off_the_row() {
    let account = Account::open();
    let folder = open_folder(&account);

    let mut labelled = message("M1001");
    labelled.tags = vec!["urgent".to_owned(), "personal".to_owned()];
    let mut relabelled = message("M1001");
    relabelled.tags = vec!["personal".to_owned()];

    // SAFETY: `folder` is live and every row taken is unreffed.
    unsafe {
        let summary = summary_of(folder);
        apply_listing(summary, &[labelled]);
        apply_listing(summary, &[relabelled]);

        let info = row(summary, "M1001").expect("a row for the message");
        assert_eq!(
            camel_message_info_get_user_flag(info, c"urgent".as_ptr()),
            GFALSE,
            "the label was taken off at the server"
        );
        assert_eq!(
            camel_message_info_get_user_flag(info, c"personal".as_ptr()),
            GTRUE
        );
        g_object_unref(info.cast());

        apply_listing(summary, &[message("M1001")]);

        let info = row(summary, "M1001").expect("a row for the message");
        assert_eq!(
            camel_message_info_get_user_flag(info, c"personal".as_ptr()),
            GFALSE,
            "the last label was taken off at the server too"
        );
        g_object_unref(info.cast());
        g_object_unref(folder.cast());
    }
}

/// A mailbox that lost everything. Camel's own answer to an empty listing is an
/// empty folder, not an untouched one — a folder whose rows all vanished is
/// exactly what emptying the trash looks like.
#[test]
fn a_mailbox_that_emptied_empties_the_summary() {
    let account = Account::open();
    let folder = open_folder(&account);

    // SAFETY: `folder` is live.
    unsafe {
        let summary = summary_of(folder);
        apply_listing(summary, &[message("M1001"), message("M1002")]);
        apply_listing(summary, &[]);

        assert_eq!(camel_folder_summary_count(summary), 0);
        assert!(row(summary, "M1001").is_none());
        g_object_unref(folder.cast());
    }
}

// ---------------------------------------------------------------------------
// what the listing is reported to have changed

/// The rows are only half of what a listing produces. The other half is the
/// diff: Camel draws a message list once and then redraws it from the `changed`
/// signal, so a folder that filled its summary and said nothing is a folder
/// whose new mail appears when the user next clicks away and back.
#[test]
fn a_first_listing_reports_every_row_as_added() {
    let account = Account::open();
    let folder = open_folder(&account);

    // SAFETY: `folder` is live.
    unsafe {
        let summary = summary_of(folder);
        let changes = apply_listing(summary, &[message("M1001"), message("M1002")]);

        assert!(!changes.is_empty());
        assert_eq!(changes.added(), ["M1001", "M1002"]);
        assert!(changes.removed().is_empty());
        assert!(changes.changed().is_empty());
        g_object_unref(folder.cast());
    }
}

/// The listing that found what it found last time. A refresh is a poll — Camel
/// runs one on a timer and one every time the folder is opened — so the common
/// answer is that nothing happened, and reporting it as a change would redraw
/// the user's message list, and lose their scroll position, every minute.
#[test]
fn a_listing_that_found_nothing_new_reports_nothing() {
    let account = Account::open();
    let folder = open_folder(&account);

    // SAFETY: `folder` is live.
    unsafe {
        let summary = summary_of(folder);
        apply_listing(summary, &[message("M1001"), message("M1002")]);
        let changes = apply_listing(summary, &[message("M1001"), message("M1002")]);

        assert!(changes.is_empty(), "an unchanged mailbox reported a change");
        assert!(changes.added().is_empty());
        assert!(changes.removed().is_empty());
        assert!(changes.changed().is_empty());
        g_object_unref(folder.cast());
    }
}

/// The two columns a refresh may rewrite are the two it may report. A message
/// read in another client is the row Camel has to redraw in bold-no-longer, and
/// nothing else about it moved.
#[test]
fn a_row_whose_flags_moved_is_reported_as_changed() {
    let account = Account::open();
    let folder = open_folder(&account);

    let mut read = message("M1001");
    read.flags.seen = true;

    // SAFETY: `folder` is live.
    unsafe {
        let summary = summary_of(folder);
        apply_listing(summary, &[message("M1001"), message("M1002")]);
        let changes = apply_listing(summary, &[read, message("M1002")]);

        assert_eq!(changes.changed(), ["M1001"]);
        assert!(changes.added().is_empty());
        assert!(changes.removed().is_empty());
        g_object_unref(folder.cast());
    }
}

/// A label is the same kind of change through a column Camel has no bit for,
/// and it has to count as one: Evolution colours the message list by label.
#[test]
fn a_row_whose_labels_moved_is_reported_as_changed() {
    let account = Account::open();
    let folder = open_folder(&account);

    let mut labelled = message("M1001");
    labelled.tags = vec!["urgent".to_owned()];

    // SAFETY: `folder` is live.
    unsafe {
        let summary = summary_of(folder);
        apply_listing(summary, &[message("M1001")]);
        let changes = apply_listing(summary, &[labelled]);

        assert_eq!(changes.changed(), ["M1001"]);
        g_object_unref(folder.cast());
    }
}

/// The row that went. Camel takes a message off the list on this and no other
/// notice, so a removal reported as nothing is a line the user can click and
/// not open.
#[test]
fn a_message_that_left_the_mailbox_is_reported_as_removed() {
    let account = Account::open();
    let folder = open_folder(&account);

    // SAFETY: `folder` is live.
    unsafe {
        let summary = summary_of(folder);
        apply_listing(summary, &[message("M1001"), message("M1002")]);
        let changes = apply_listing(summary, &[message("M1002")]);

        assert_eq!(changes.removed(), ["M1001"]);
        assert!(changes.added().is_empty());
        assert!(changes.changed().is_empty());
        g_object_unref(folder.cast());
    }
}

/// Nothing is ever recent, and that is a decision rather than an omission.
/// Camel's fourth list is what runs the user's incoming filters, and a JMAP
/// listing cannot tell a message that just arrived from one that was always
/// there: the first refresh of an account finds the whole mailbox. Reporting
/// those as recent would file, or delete, every message the user already had.
#[test]
fn nothing_a_listing_found_is_recent() {
    let account = Account::open();
    let folder = open_folder(&account);

    // SAFETY: `folder` is live.
    unsafe {
        let summary = summary_of(folder);
        let changes = apply_listing(summary, &[message("M1001")]);

        assert!(changes.recent().is_empty());
        g_object_unref(folder.cast());
    }
}

/// Both mutable columns in one listing. A message read *and* relabelled between
/// two refreshes is ordinary — Evolution's own "mark as read and file it" rule
/// does both at once — and it is the case a verdict built with `||` gets wrong:
/// short-circuiting on the flags means the labels are never written at all, and
/// the row keeps a label that was taken off days ago.
#[test]
fn a_row_that_moved_in_both_columns_has_both_of_them_written() {
    let account = Account::open();
    let folder = open_folder(&account);

    let mut labelled = message("M1001");
    labelled.tags = vec!["urgent".to_owned()];
    let mut read = message("M1001");
    read.flags.seen = true;

    // SAFETY: `folder` is live and every row taken is unreffed.
    unsafe {
        let summary = summary_of(folder);
        apply_listing(summary, &[labelled]);
        let changes = apply_listing(summary, &[read]);

        let info = row(summary, "M1001").expect("a row for the message");
        assert_ne!(camel_message_info_get_flags(info) & CAMEL_MESSAGE_SEEN, 0);
        assert_eq!(
            camel_message_info_get_user_flag(info, c"urgent".as_ptr()),
            GFALSE,
            "the flags were written and the labels were not"
        );
        assert_eq!(changes.changed(), ["M1001"]);
        g_object_unref(info.cast());
        g_object_unref(folder.cast());
    }
}

/// The reason the summary is a subclass at all. Camel builds rows itself
/// whenever it reads a folder back off disk, and this class field is the only
/// thing that says which kind to build — so the answer here and the answer
/// `new_message_info` gives have to be the same one, or a folder would hold two
/// kinds of row depending on where each came from.
#[test]
fn the_summary_declares_the_provider_s_own_row_type() {
    let account = Account::open();
    let folder = open_folder(&account);

    // SAFETY: `folder` is live and owns its summary, whose class is owned by the
    // type system and outlives both.
    unsafe {
        let summary = summary_of(folder);
        let class = (*summary.cast::<GTypeInstance>())
            .g_class
            .cast::<CamelFolderSummaryClass>();
        assert!(!class.is_null(), "the summary has no class");

        assert_eq!((*class).message_info_type, message_info_type());
        g_object_unref(folder.cast());
    }
}

/// Camel clones a row when one has to exist outside the summary it belongs to,
/// and the summary the copy is being made for is what decides its type. Cloned
/// into one of ours, the copy is one of ours — and a copy that had forgotten the
/// keywords the server holds would be a row whose next flag change looked like
/// the removal of every one of them.
#[test]
fn a_row_cloned_into_the_summary_keeps_the_keywords_it_remembered() {
    let account = Account::open();
    let folder = open_folder(&account);
    let mut listed = message("M2001");
    listed.tags = vec!["Work".to_owned()];

    // SAFETY: `folder` is live; the row and its clone are both references this
    // test owns.
    unsafe {
        let summary = summary_of(folder);
        apply_listing(summary, std::slice::from_ref(&listed));
        let info = row(summary, "M2001").expect("a row for the message");

        let clone = camel_message_info_clone(info, summary);
        assert!(!clone.is_null(), "Camel cloned nothing");

        assert_eq!(
            server_keywords(clone),
            Some(Keywords::new(&listed.flags, &listed.tags))
        );
        g_object_unref(clone.cast());
        g_object_unref(info.cast());
        g_object_unref(folder.cast());
    }
}

/// And the whole point of storing the set at all: it has to be there after a
/// restart. A folder closed and opened again is a new summary over the same
/// rows, read back out of the store's database — the state in which a flag
/// change made this morning is diffed against a listing made yesterday.
#[test]
fn the_keywords_a_row_remembers_outlive_the_folder_that_listed_it() {
    let account = Account::open();
    let mut listed = message("M2002");
    listed.flags.seen = true;
    listed.tags = vec!["Read later".to_owned()];

    // SAFETY: every folder is live for as long as it is used and unreffed once;
    // the error out-parameter is only read while it is in scope.
    unsafe {
        let folder = open_folder(&account);
        apply_listing(summary_of(folder), std::slice::from_ref(&listed));
        let mut error: *mut GError = ptr::null_mut();
        assert_ne!(
            camel_folder_summary_save(summary_of(folder), ptr::addr_of_mut!(error)),
            GFALSE,
            "the summary would not save"
        );
        assert!(error.is_null());
        g_object_unref(folder.cast());

        // The same mailbox opened a second time: a folder with a summary that
        // holds nothing until it is loaded, which is what a restart amounts to.
        let reopened = open_folder(&account);
        assert_ne!(
            camel_folder_summary_load(summary_of(reopened), ptr::addr_of_mut!(error)),
            GFALSE,
            "the summary would not load"
        );
        assert!(error.is_null());

        let info = row(summary_of(reopened), "M2002").expect("a row read back off disk");
        assert_eq!(
            server_keywords(info),
            Some(Keywords::new(&listed.flags, &listed.tags)),
            "the row came back without the keywords it was stored with"
        );
        g_object_unref(info.cast());
        g_object_unref(reopened.cast());
    }
}
