// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Searching a JMAP folder's message *bodies*, the one part of a search that
//! cannot be answered out of the summary.
//!
//! `tests/search.rs` covers the expression entry point each EDS release has;
//! this covers the extra slot 3.58 introduced for a body term alone
//! (`CamelFolderClass::search_body_sync`, see `jmap_mail::search_body`).
//!
//! The vfunc is compiled on every EDS, because it names nothing older headers
//! lack, so every test here but the last drives it directly and asserts the
//! same answers on both legs. The last one is what the older leg genuinely
//! cannot have: that the function is *installed* in the class and reached
//! through `camel_folder_search_body_sync`, which is the claim a body search on
//! a 3.58+ EDS actually rests on.

mod common;

use std::ffi::{CStr, CString};
use std::ptr;

use common::Account;
use eds_sys::{CAMEL_SERVICE_ERROR_NOT_CONNECTED, CamelFolder, camel_service_error_quark};
use glib_sys::{GError, GFALSE, GPtrArray, g_error_free, g_ptr_array_add, g_ptr_array_free, gchar};
use glib_sys::{g_ptr_array_new, g_ptr_array_unref, gpointer};
use gobject_sys::g_object_unref;
use jmap_client::{Client, Credentials};
use jmap_mail::folder::new_folder;
use jmap_mail::search_body::search_body_sync;
use jmap_mail_sync::{FolderInfo, MailSync};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;

/// The words of a body term, owned for the length of one call: the input shape
/// the vfunc takes, built the way `tests/search.rs`'s `UidList` builds one.
struct Words {
    array: *mut GPtrArray,
    words: Vec<CString>,
}

impl Words {
    fn of(words: &[&str]) -> Self {
        let words: Vec<CString> = words
            .iter()
            .map(|word| CString::new(*word).expect("a word with no NUL"))
            .collect();
        // SAFETY: a fresh array, filled with pointers into strings this value
        // owns and outlives it by.
        let array = unsafe {
            let array = g_ptr_array_new();
            for word in &words {
                g_ptr_array_add(array, word.as_ptr() as gpointer);
            }
            array
        };
        Self { array, words }
    }
}

impl Drop for Words {
    fn drop(&mut self) {
        // SAFETY: the one array, allocated above; FALSE because the pointers
        // in it belong to `self.words`.
        unsafe { g_ptr_array_free(self.array, GFALSE) };
        self.words.clear();
    }
}

/// The description of the one mailbox every folder here is a view of.
fn mailbox(id: Id) -> FolderInfo {
    FolderInfo {
        id,
        path: "Inbox".to_owned(),
        display_name: "Inbox".to_owned(),
        role: None,
        total: 0,
        unread: 0,
        subscribed: true,
        children: Vec::new(),
    }
}

/// A folder on a live (mock) connection whose mailbox holds two messages with
/// distinct bodies, and no local summary rows at all.
///
/// The empty summary is the point: a body term that fell back to anything
/// local would find nothing here whatever the words were, so only a real round
/// trip can tell the two messages apart.
///
/// # Safety
///
/// The caller unrefs the folder before dropping the `Account` beside it.
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
    // SAFETY: `account.store` is a live `CamelStore` for as long as the
    // `Account` returned alongside it is.
    let folder = unsafe { new_folder(account.store, &info) };
    assert!(!folder.is_null(), "no folder for the mailbox");
    (server, account, folder, lunch, status)
}

/// The strings in an array the vfunc answered with, which the caller then
/// unrefs the way `CamelStoreSearch` does.
///
/// # Safety
///
/// `array` must be a live `GPtrArray` of NUL-terminated strings.
unsafe fn take(array: *mut GPtrArray) -> Vec<String> {
    // SAFETY: the contract above; the elements live as long as the array.
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

/// A word in one message's body and not the other's picks out that message.
#[test]
fn a_body_word_is_answered_by_the_server() {
    // SAFETY: the account outlives the folder, which this test unrefs.
    unsafe {
        let (_server, _account, folder, lunch, _status) = connected_folder_with_two_bodies();

        let words = Words::of(&["canteen"]);
        let mut uids: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        let ok = search_body_sync(
            folder,
            words.array,
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

/// A word in neither body is an empty answer rather than a refusal: "no
/// message has this" is something the server settles, and Camel takes it as
/// settled.
#[test]
fn a_word_no_body_has_is_an_empty_answer() {
    // SAFETY: as above.
    unsafe {
        let (_server, _account, folder, _lunch, _status) = connected_folder_with_two_bodies();

        let words = Words::of(&["escalator"]);
        let mut uids: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        let ok = search_body_sync(
            folder,
            words.array,
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

/// Every word has to be in the same body, not merely somewhere in the folder.
#[test]
fn several_words_have_to_share_a_body() {
    // SAFETY: as above.
    unsafe {
        let (_server, _account, folder, lunch, _status) = connected_folder_with_two_bodies();

        // Both words are in the folder, one per message.
        let split = Words::of(&["canteen", "green"]);
        let mut uids: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        let ok = search_body_sync(
            folder,
            split.array,
            ptr::addr_of_mut!(uids),
            ptr::null_mut(),
            ptr::addr_of_mut!(error),
        );
        assert!(error.is_null(), "the search reported an error");
        assert_ne!(ok, GFALSE, "the search refused");
        assert_eq!(take(uids), Vec::<String>::new());

        // Both words in the one body, which does match.
        let shared = Words::of(&["canteen", "o'clock"]);
        let mut uids: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        let ok = search_body_sync(
            folder,
            shared.array,
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

/// A folder with no connection behind it refuses, and refusing is what sends
/// the search to the messages already on disk. An empty answer here would be
/// the offline case reported as "no message matches".
#[test]
fn a_disconnected_folder_refuses_rather_than_answering_nothing() {
    let account = Account::open();
    // SAFETY: as above.
    unsafe {
        let folder = new_folder(account.store, &mailbox(Id::new("Mbx0001")));
        assert!(!folder.is_null(), "no folder for the mailbox");

        let words = Words::of(&["canteen"]);
        let mut uids: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        let ok = search_body_sync(
            folder,
            words.array,
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

/// A term of no words is refused too, and on a connected folder, so that the
/// refusal is the term's doing and not the connection's. Asking `Email/query`
/// for an empty body condition would be asking for whatever the server makes
/// of it, and answering "every message" would be worse.
#[test]
fn a_term_of_no_words_is_refused() {
    // SAFETY: as above.
    unsafe {
        let (_server, _account, folder, _lunch, _status) = connected_folder_with_two_bodies();

        for words in [Words::of(&[]), Words::of(&[""])] {
            let mut uids: *mut GPtrArray = ptr::null_mut();
            let mut error: *mut GError = ptr::null_mut();
            let ok = search_body_sync(
                folder,
                words.array,
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

/// And the vfunc is what Camel reaches: the same search again, this time
/// through `camel_folder_search_body_sync`, the wrapper `CamelStoreSearch`
/// itself calls.
///
/// This is the test the older leg cannot run, and the one the whole item is
/// for: a class that had not filled the slot in would be answered by the base
/// implementation, which has no index to consult here and so refuses.
#[cfg(camel_folder_search_body_sync)]
#[test]
fn the_slot_camel_calls_is_the_one_this_provider_filled() {
    // SAFETY: as above.
    unsafe {
        let (_server, _account, folder, lunch, _status) = connected_folder_with_two_bodies();

        let words = Words::of(&["canteen"]);
        let mut uids: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        let ok = eds_sys::camel_folder_search_body_sync(
            folder,
            words.array,
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
