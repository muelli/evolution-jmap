// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Searches a JMAP folder's message bodies. The summary alone cannot answer
//! this part of a search.
//!
//! `tests/search.rs` covers the expression entry point that every EDS version
//! has. This file covers the extra slot that EDS 3.58 introduced for a body
//! term alone (`CamelFolderClass::search_body_sync`, see
//! `jmap_mail::search_body`).
//!
//! The vfunc compiles on every EDS version, because it names nothing that
//! older headers lack. Every test but the last calls the vfunc directly and
//! checks the same answers on both EDS versions. The last test checks what
//! only the newer EDS version has. The function is installed in the class and
//! reachable through `camel_folder_search_body_sync`. This is the claim a
//! body search on EDS 3.58 or later depends on.

mod common;

use std::ffi::CStr;
use std::ptr;

use common::Account;
use common::array::StringArray;
use eds_sys::{CAMEL_SERVICE_ERROR_NOT_CONNECTED, CamelFolder, camel_service_error_quark};
use glib_sys::g_ptr_array_unref;
use glib_sys::{GError, GFALSE, GPtrArray, g_error_free, gchar};
use gobject_sys::g_object_unref;
use jmap_client::{Client, Credentials};
use jmap_mail::folder::new_folder;
use jmap_mail::search_body::search_body_sync;
use jmap_mail_sync::{FolderInfo, MailSync};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;

/// The one mailbox that every folder in this file is a view of.
fn mailbox(id: Id) -> FolderInfo {
    FolderInfo {
        id,
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

/// A folder on a live (mock) connection. Its mailbox holds two messages with
/// distinct bodies, and the local summary has no rows.
///
/// The empty summary is deliberate. A body term that fell back to local data
/// would find nothing here, regardless of the words used. Only a real round
/// trip to the server can tell the two messages apart.
///
/// # Safety
///
/// The caller must unref the folder before dropping the `Account` beside it.
unsafe fn connected_folder_with_two_bodies() -> (MockServer, Account, *mut CamelFolder, Id, Id) {
    let server = MockServer::builder().start();
    let (mailbox_id, lunch, status) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&server.account_id()).unwrap();
        let mailbox = account.seed_mailbox("Inbox", Some("inbox"));
        let lunch = account.seed_email(EmailSeed::new(
            mailbox.clone(),
            ("Bob", "bob@example.com"),
            "Lunch?",
            "One o'clock at the canteen.",
            "2026-01-15T09:30:00Z",
        ));
        let status = account.seed_email(EmailSeed::new(
            mailbox.clone(),
            ("Carol", "carol@example.com"),
            "Status report",
            "Everything is green at the moment.",
            "2026-01-15T10:00:00Z",
        ));
        (mailbox, lunch, status)
    };

    let account = Account::open();
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    account.connect(MailSync::new(client, server.account_id()));

    let info = mailbox(mailbox_id);
    // SAFETY: `account.store` is a live `CamelStore` as long as the `Account`
    // returned alongside it exists.
    let folder = unsafe { new_folder(account.store, &info) };
    assert!(!folder.is_null(), "no folder for the mailbox");
    (server, account, folder, lunch, status)
}

/// Reads the strings from an array that the vfunc returned, then unrefs the
/// array the way `CamelStoreSearch` does.
///
/// # Safety
///
/// `array` must be a live `GPtrArray` of NUL-terminated strings.
unsafe fn take(array: *mut GPtrArray) -> Vec<String> {
    // SAFETY: meets the contract above. The elements live as long as the
    // array does.
    unsafe {
        let uids = (0..(*array).len)
            .map(|index| {
                let uid: *const gchar = (*array).pdata.add(index as usize).read().cast();
                CStr::from_ptr(uid).to_string_lossy().into_owned()
            })
            .collect();
        g_ptr_array_unref(array);
        uids
    }
}

/// A word in one message's body and not in the other's identifies that
/// message.
#[test]
fn a_body_word_is_answered_by_the_server() {
    // SAFETY: the account outlives the folder. This test unrefs the folder.
    unsafe {
        let (_server, _account, folder, lunch, _status) = connected_folder_with_two_bodies();

        let words = StringArray::of(&["canteen"]);
        let mut uids: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        let ok = search_body_sync(
            folder,
            words.as_ptr(),
            ptr::addr_of_mut!(uids),
            ptr::null_mut(),
            ptr::addr_of_mut!(error),
        );
        assert!(error.is_null(), "the search reported an error");
        assert_ne!(ok, GFALSE, "the search refused");
        assert!(!uids.is_null(), "the search answered with no array at all");
        assert_eq!(take(uids), vec![lunch.as_str().to_owned()]);

        g_object_unref(folder.cast());
    }
}

/// A word in neither body gives an empty answer, not a refusal. The server
/// decides that no message has the word, and Camel accepts that answer as
/// final.
#[test]
fn a_word_no_body_has_is_an_empty_answer() {
    // SAFETY: as above.
    unsafe {
        let (_server, _account, folder, _lunch, _status) = connected_folder_with_two_bodies();

        let words = StringArray::of(&["escalator"]);
        let mut uids: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        let ok = search_body_sync(
            folder,
            words.as_ptr(),
            ptr::addr_of_mut!(uids),
            ptr::null_mut(),
            ptr::addr_of_mut!(error),
        );
        assert!(error.is_null(), "the search reported an error");
        assert_ne!(ok, GFALSE, "the search refused");
        assert!(!uids.is_null(), "the search answered with no array at all");
        assert_eq!(take(uids), Vec::<String>::new());

        g_object_unref(folder.cast());
    }
}

/// Every word must appear in the same body, not merely somewhere in the
/// folder.
#[test]
fn several_words_have_to_share_a_body() {
    // SAFETY: as above.
    unsafe {
        let (_server, _account, folder, lunch, _status) = connected_folder_with_two_bodies();

        // Both words appear in the folder, one in each message.
        let split = StringArray::of(&["canteen", "green"]);
        let mut uids: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        let ok = search_body_sync(
            folder,
            split.as_ptr(),
            ptr::addr_of_mut!(uids),
            ptr::null_mut(),
            ptr::addr_of_mut!(error),
        );
        assert!(error.is_null(), "the search reported an error");
        assert_ne!(ok, GFALSE, "the search refused");
        assert_eq!(take(uids), Vec::<String>::new());

        // Both words are in the same body, which matches.
        let shared = StringArray::of(&["canteen", "o'clock"]);
        let mut uids: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        let ok = search_body_sync(
            folder,
            shared.as_ptr(),
            ptr::addr_of_mut!(uids),
            ptr::null_mut(),
            ptr::addr_of_mut!(error),
        );
        assert!(error.is_null(), "the search reported an error");
        assert_ne!(ok, GFALSE, "the search refused");
        assert_eq!(take(uids), vec![lunch.as_str().to_owned()]);

        g_object_unref(folder.cast());
    }
}

/// A folder with no connection refuses the search. The refusal sends the
/// search to the messages already stored on disk. An empty answer here would
/// wrongly report the offline case as "no message matches".
#[test]
fn a_disconnected_folder_refuses_rather_than_answering_nothing() {
    let account = Account::open();
    // SAFETY: as above.
    unsafe {
        let folder = new_folder(account.store, &mailbox(Id::new("Mbx0001")));
        assert!(!folder.is_null(), "no folder for the mailbox");

        let words = StringArray::of(&["canteen"]);
        let mut uids: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        let ok = search_body_sync(
            folder,
            words.as_ptr(),
            ptr::addr_of_mut!(uids),
            ptr::null_mut(),
            ptr::addr_of_mut!(error),
        );
        assert_eq!(ok, GFALSE, "a disconnected folder answered a body search");
        assert!(uids.is_null(), "a refusal still wrote an array");
        assert!(!error.is_null(), "the refusal said nothing");
        assert_eq!((*error).domain, camel_service_error_quark());
        assert_eq!((*error).code, CAMEL_SERVICE_ERROR_NOT_CONNECTED as i32);
        g_error_free(error);

        g_object_unref(folder.cast());
    }
}

/// A term with no words is refused too, on a connected folder. This isolates
/// the refusal as a result of the empty term, not of the connection state.
/// Asking `Email/query` with an empty body condition depends on what the
/// server does with it. Answering with every message would be an even worse
/// result.
#[test]
fn a_term_of_no_words_is_refused() {
    // SAFETY: as above.
    unsafe {
        let (_server, _account, folder, _lunch, _status) = connected_folder_with_two_bodies();

        for words in [StringArray::of::<&str>(&[]), StringArray::of(&[""])] {
            let mut uids: *mut GPtrArray = ptr::null_mut();
            let mut error: *mut GError = ptr::null_mut();
            let ok = search_body_sync(
                folder,
                words.as_ptr(),
                ptr::addr_of_mut!(uids),
                ptr::null_mut(),
                ptr::addr_of_mut!(error),
            );
            assert_eq!(ok, GFALSE, "an empty body term was answered");
            assert!(uids.is_null(), "a refusal still wrote an array");
            assert!(!error.is_null(), "the refusal said nothing");
            g_error_free(error);
        }

        g_object_unref(folder.cast());
    }
}

/// Confirms Camel reaches the vfunc. Runs the same search again, this time
/// through `camel_folder_search_body_sync`, the wrapper that `CamelStoreSearch`
/// itself calls.
///
/// The older EDS version cannot run this test. It is the purpose of this
/// whole file: a class that had not filled the slot would be answered by the
/// base implementation, which has no index to consult and so refuses.
#[cfg(camel_folder_search_body_sync)]
#[test]
fn the_slot_camel_calls_is_the_one_this_provider_filled() {
    // SAFETY: as above.
    unsafe {
        let (_server, _account, folder, lunch, _status) = connected_folder_with_two_bodies();

        let words = StringArray::of(&["canteen"]);
        let mut uids: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        let ok = eds_sys::camel_folder_search_body_sync(
            folder,
            words.as_ptr(),
            ptr::addr_of_mut!(uids),
            ptr::null_mut(),
            ptr::addr_of_mut!(error),
        );
        assert!(error.is_null(), "the search reported an error");
        assert_ne!(ok, GFALSE, "the search refused");
        assert!(!uids.is_null(), "the search answered with no array at all");
        assert_eq!(take(uids), vec![lunch.as_str().to_owned()]);

        g_object_unref(folder.cast());
    }
}
