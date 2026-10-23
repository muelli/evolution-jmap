// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Searches a JMAP folder.
//!
//! Search is the one folder operation whose *implementation* moved between
//! the EDS releases this provider targets.
//!
//! Evolution searches a folder for more than the search bar. Every
//! message-list view is an expression ("Unread Messages", "Hide Deleted
//! Messages"). A folder that cannot answer a search is a folder whose
//! message list does not draw.
//!
//! Up to EDS 3.52, answering was the *provider's* job. `CamelFolderClass`
//! left `search_by_expression` and `search_by_uids` NULL and asserted on a
//! class that had not filled them in. So [`crate::folder`] fills them with a
//! `CamelFolderSearch` over the local summary.
//!
//! From 3.58, that whole object is gone. The vfuncs were replaced by
//! `search_sync`. The base class installs an implementation of it over
//! `CamelStoreSearch`, which does the same job against the same rows.
//!
//! Porting across those two releases is not a rename to re-spell. It is the
//! claim that on the newer EDS, the provider should override *nothing* and
//! let the base class answer. A claim like that compiles cleanly even when
//! wrong: the symptom is not a build error, it is a folder that answers
//! every search with nothing.
//!
//! So this file asserts the claim behaviourally, on whichever entry point
//! the EDS in front of the test has, rather than arguing for it in a
//! comment: the same two rows, the same expression, and the same expected
//! answer on both legs.
//!
//! One case does reach the server. Up to EDS 3.52, before either vfunc
//! touches `CamelFolderSearch`, it tries `jmap-mail::search_sexp::translate`
//! on the expression. On a translatable expression (the shapes RFC 8621's
//! contains-only conditions express: `header-contains`, `body-contains`,
//! `and`, `or`, `not`), it asks `Email/query` over the whole mailbox instead.
//! This is the only way to search a body this provider never downloaded.
//!
//! Everything else, including a translatable expression asked of a
//! disconnected store, still answers from the summary exactly as before.
//! For that reason, the tests below that exercise the server path are gated
//! to the pre-3.58 leg. The tests shared with the 3.58 leg still test only
//! the local fallback.

mod common;

use std::ffi::CStr;
use std::ptr;

use common::Account;
#[cfg(camel_folder_search_object)]
use common::array::StringArray;
use eds_sys::{CamelFolder, camel_folder_get_folder_summary, camel_folder_summary_save};
use glib_sys::{GError, GPtrArray, gchar};
use gobject_sys::g_object_unref;
#[cfg(camel_folder_search_object)]
use jmap_client::{Client, Credentials};
use jmap_mail::folder::new_folder;
use jmap_mail::summary::apply_listing;
#[cfg(camel_folder_search_object)]
use jmap_mail_sync::MailSync;
use jmap_mail_sync::{FolderInfo, MessageFlags, MessageSummary};
#[cfg(camel_folder_search_object)]
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;

/// The mailbox the folder under test is a view of.
fn mailbox() -> FolderInfo {
    FolderInfo {
        id: Id::new("Mbx0001"),
        path: "Inbox".to_owned(),
        display_name: "Inbox".to_owned(),
        role: None,
        total: 0,
        unread: 0,
        subscribed: true,
        may_add_items: None,
        children: Vec::new(),
    }
}

/// A row with nothing on it but its uid and the flags the test is about.
fn message(uid: &str, flags: MessageFlags) -> MessageSummary {
    MessageSummary {
        uid: Id::new(uid),
        blob_id: None,
        thread_id: None,
        flags,
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

/// A folder holding two rows: `Msg0001` seen, `Msg0002` not.
///
/// The summary is written to the store's database before the search runs.
/// This is not tidiness. From 3.58, the base class's `search_sync` evaluates
/// the expression through `CamelStoreSearch`, which reads the store's
/// `CamelStoreDB`, not the summary's in-memory rows. A folder whose rows had
/// never been saved would answer nothing on that leg, for a reason that has
/// nothing to do with this provider.
///
/// `camel_folder_summary_save` is what Camel itself calls. It writes the rows
/// and the folder record together.
///
/// # Safety
///
/// `account` must outlive the returned folder.
unsafe fn folder_with_two_rows(account: &Account) -> *mut CamelFolder {
    let mailbox = mailbox();
    // SAFETY: `account.store` is a live `CamelStore` for as long as the
    // `Account` is.
    let folder = unsafe { new_folder(account.store, &mailbox) };
    assert!(!folder.is_null(), "no folder for the mailbox");

    let seen = MessageFlags {
        seen: true,
        ..MessageFlags::default()
    };
    let listed = [
        message("Msg0001", seen),
        message("Msg0002", MessageFlags::default()),
    ];

    // SAFETY: the folder is live and owns the summary the listing is applied
    // to. `summary_save` is Camel's own call, and it reports through its
    // `GError`.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        assert!(!summary.is_null(), "the folder kept no summary");
        apply_listing(summary, &listed);

        let mut error: *mut GError = ptr::null_mut();
        assert_ne!(
            camel_folder_summary_save(summary, ptr::addr_of_mut!(error)),
            glib_sys::GFALSE,
            "the summary would not save"
        );
        assert!(error.is_null(), "saving the summary reported an error");
    }

    folder
}

/// The uids `folder` answers `expression` with, in the order Camel gave them.
///
/// The two arms ask the same question of the two APIs. The ownership rules
/// differ as much as the signatures do. Up to 3.52, the array comes back as
/// the return value and goes back through `camel_folder_search_free`. From
/// 3.58, it is an out-parameter the caller unrefs, left NULL with success
/// reported when nothing matched.
///
/// # Safety
///
/// `folder` must point at a live `CamelFolder`.
unsafe fn search(folder: *mut CamelFolder, expression: &CStr) -> Vec<String> {
    let mut error: *mut GError = ptr::null_mut();

    #[cfg(camel_folder_search_object)]
    // SAFETY: the folder is live, by this function's contract. The expression
    // outlives the call. The array returned is freed through the function
    // Camel pairs with this one.
    let uids = unsafe {
        let result = eds_sys::camel_folder_search_by_expression(
            folder,
            expression.as_ptr(),
            ptr::null_mut(),
            ptr::addr_of_mut!(error),
        );
        assert!(error.is_null(), "the search reported an error");
        assert!(
            !result.is_null(),
            "the search answered with no array at all"
        );
        let uids = collect(result);
        eds_sys::camel_folder_search_free(folder, result);
        uids
    };

    #[cfg(not(camel_folder_search_object))]
    // SAFETY: as above. `out` is written by the call and owned by this
    // function. A NULL with success is Camel's way of saying "no matches".
    let uids = unsafe {
        let mut out: *mut GPtrArray = ptr::null_mut();
        let ok = eds_sys::camel_folder_search_sync(
            folder,
            expression.as_ptr(),
            ptr::addr_of_mut!(out),
            ptr::null_mut(),
            ptr::addr_of_mut!(error),
        );
        assert!(error.is_null(), "the search reported an error");
        assert_ne!(ok, glib_sys::GFALSE, "the search failed");
        if out.is_null() {
            Vec::new()
        } else {
            let uids = collect(out);
            glib_sys::g_ptr_array_unref(out);
            uids
        }
    };

    uids
}

/// The strings in one of Camel's uid arrays.
///
/// # Safety
///
/// `array` must be a live `GPtrArray` of NUL-terminated strings.
unsafe fn collect(array: *mut GPtrArray) -> Vec<String> {
    // SAFETY: the contract above. Every element lives at least as long as
    // the array, which the caller frees after this returns.
    unsafe {
        (0..(*array).len)
            .filter_map(|index| {
                let uid: *const gchar = (*array).pdata.add(index as usize).read().cast();
                (!uid.is_null()).then(|| CStr::from_ptr(uid).to_string_lossy().into_owned())
            })
            .collect()
    }
}

/// A search over a flag returns the rows that carry it and no others.
///
/// The expression is the one Evolution's own "Unread Messages" view is built
/// from, read the other way round. It is deliberately a *discriminating*
/// search, not `(match-all #t)`. A search that quietly ignored its expression
/// would answer every row: that would pass the test below and fail this one.
#[test]
fn a_search_over_a_system_flag_selects_the_rows_that_carry_it() {
    let account = Account::open();
    // SAFETY: the account outlives the folder, which this test unrefs.
    unsafe {
        let folder = folder_with_two_rows(&account);

        let seen = search(folder, c"(match-all (system-flag \"Seen\"))");
        assert_eq!(
            seen,
            vec!["Msg0001".to_owned()],
            "the seen row, and only it"
        );

        let unseen = search(folder, c"(match-all (not (system-flag \"Seen\")))");
        assert_eq!(
            unseen,
            vec!["Msg0002".to_owned()],
            "the unseen row, and only it"
        );

        g_object_unref(folder.cast());
    }
}

/// A search that matches everything returns both rows.
///
/// The companion to the test above. Together they pin that the folder answers
/// with the rows it holds, and that the answer depends on the expression. On
/// its own, either test could pass against a broken search: this one against
/// a search that returns all rows regardless, that one against a search that
/// filters, but over the wrong set.
#[test]
fn a_search_that_matches_everything_returns_every_row() {
    let account = Account::open();
    // SAFETY: as above.
    unsafe {
        let folder = folder_with_two_rows(&account);

        let mut all = search(folder, c"(match-all #t)");
        all.sort();
        assert_eq!(all, vec!["Msg0001".to_owned(), "Msg0002".to_owned()]);

        g_object_unref(folder.cast());
    }
}

/// A folder wired to a live (mock) connection. Its mailbox holds two
/// server-side messages with distinct subjects: one contains "Lunch", one
/// does not. The folder keeps no local summary rows at all.
///
/// The empty summary is deliberate. A search that silently fell back to the
/// local `CamelFolderSearch` over an empty summary would find nothing,
/// regardless of the expression. Only a genuine round trip to the server can
/// tell the two subjects apart here.
///
/// The `MockServer` is part of the return value, not merely used while
/// building it. Dropping it tears down the listener, and every request the
/// folder makes happens after this function returns.
///
/// # Safety
///
/// The caller must not keep the returned folder past its own use of it, and
/// the `Account` it hangs off of must still be alive. The caller unrefs the
/// folder.
#[cfg(camel_folder_search_object)]
unsafe fn connected_folder_with_two_subjects() -> (MockServer, Account, *mut CamelFolder, Id, Id) {
    let server = MockServer::builder().start();
    let (mailbox_id, lunch, other) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&server.account_id()).unwrap();
        let mailbox = account.seed_mailbox("Inbox", Some("inbox"));
        let lunch = account.seed_email(EmailSeed::new(
            mailbox.clone(),
            ("Bob", "bob@example.com"),
            "Lunch?",
            "One o'clock.",
            "2026-01-15T09:30:00Z",
        ));
        let other = account.seed_email(EmailSeed::new(
            mailbox.clone(),
            ("Carol", "carol@example.com"),
            "Status report",
            "All green.",
            "2026-01-15T10:00:00Z",
        ));
        (mailbox, lunch, other)
    };

    let account = Account::open();
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    account.connect(MailSync::new(client, server.account_id()));

    let info = FolderInfo {
        id: mailbox_id,
        path: "Inbox".to_owned(),
        display_name: "Inbox".to_owned(),
        role: None,
        total: 0,
        unread: 0,
        subscribed: true,
        may_add_items: None,
        children: Vec::new(),
    };
    // SAFETY: `account.store` is a live `CamelStore` for as long as the
    // `Account` returned alongside it is.
    let folder = unsafe { new_folder(account.store, &info) };
    assert!(!folder.is_null(), "no folder for the mailbox");
    (server, account, folder, lunch, other)
}

/// A translatable expression is answered by the server, not the (empty)
/// local summary.
#[cfg(camel_folder_search_object)]
#[test]
fn a_translatable_expression_is_delegated_to_the_server() {
    // SAFETY: the account outlives the folder, which this test unrefs.
    unsafe {
        let (_server, _account, folder, lunch, _other) = connected_folder_with_two_subjects();

        let mut error: *mut GError = ptr::null_mut();
        let result = eds_sys::camel_folder_search_by_expression(
            folder,
            c"(header-contains \"Subject\" \"Lunch\")".as_ptr(),
            ptr::null_mut(),
            ptr::addr_of_mut!(error),
        );
        assert!(error.is_null(), "the search reported an error");
        assert!(
            !result.is_null(),
            "the search answered with no array at all"
        );
        assert_eq!(collect(result), vec![lunch.as_str().to_owned()]);
        eds_sys::camel_folder_search_free(folder, result);

        g_object_unref(folder.cast());
    }
}

/// The same server round trip, narrowed to a uid subset. This is the extra
/// argument `search_by_uids` takes, which the local path gets for free from
/// `camel_folder_search_search`.
#[cfg(camel_folder_search_object)]
#[test]
fn a_translatable_expression_over_uids_is_narrowed_to_them() {
    // SAFETY: as above.
    unsafe {
        let (_server, _account, folder, lunch, other) = connected_folder_with_two_subjects();

        // Both uids are in the restriction, so the match must come from the
        // filter, not merely from the restriction excluding the other row.
        let restrict = StringArray::of(&[&lunch, &other]);
        let mut error: *mut GError = ptr::null_mut();
        let result = eds_sys::camel_folder_search_by_uids(
            folder,
            c"(header-contains \"Subject\" \"Lunch\")".as_ptr(),
            restrict.as_ptr(),
            ptr::null_mut(),
            ptr::addr_of_mut!(error),
        );
        assert!(error.is_null(), "the search reported an error");
        assert!(
            !result.is_null(),
            "the search answered with no array at all"
        );
        assert_eq!(collect(result), vec![lunch.as_str().to_owned()]);
        eds_sys::camel_folder_search_free(folder, result);

        // A restriction that excludes the matching row leaves nothing. The
        // same expression matches when unrestricted.
        let excluding = StringArray::of(&[&other]);
        let mut error: *mut GError = ptr::null_mut();
        let result = eds_sys::camel_folder_search_by_uids(
            folder,
            c"(header-contains \"Subject\" \"Lunch\")".as_ptr(),
            excluding.as_ptr(),
            ptr::null_mut(),
            ptr::addr_of_mut!(error),
        );
        assert!(error.is_null(), "the search reported an error");
        assert!(
            !result.is_null(),
            "the search answered with no array at all"
        );
        assert_eq!(collect(result), Vec::<String>::new());
        eds_sys::camel_folder_search_free(folder, result);

        g_object_unref(folder.cast());
    }
}
