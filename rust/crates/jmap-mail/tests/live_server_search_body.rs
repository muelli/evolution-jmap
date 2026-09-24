// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `search_body_sync` against a real JMAP server.
//!
//! `src/search_body.rs`'s own unit tests cover `body_filter`'s translation
//! from Camel's word list to an `Email/query` filter, but nothing has driven
//! the vfunc's other half — `JmapStore::search` actually answering that
//! filter — against a real server. `tests/live_server_search.rs` proves the
//! sibling `search_by_expression`/`search_by_uids` path this way; this is the
//! same recipe for `search_body_sync`.
//!
//! The function is called directly rather than through Camel's vfunc
//! dispatch: `search_body.rs`'s own doc comment on `search_body_sync` notes
//! it is compiled on every EDS even though only 3.58 and newer have a slot to
//! install it into, "so keeping it out of the `#[cfg]` is what lets this
//! runner's own tests drive it" — this runner's EDS is 3.52.
//!
//! ## Running it
//!
//! Same environment as the other live-server tests — see
//! `docs/manual-test-live-server.md`:
//!
//! ```console
//! $ cargo test -p jmap-mail --features testing -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

mod common;

use std::env;
use std::ffi::{CStr, CString};
use std::ptr;

use common::Account;
use eds_sys::CamelFolder;
use glib_sys::{GError, GFALSE, GPtrArray, g_ptr_array_add, g_ptr_array_free, g_ptr_array_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::folder::new_folder;
use jmap_mail::search_body::search_body_sync;
use jmap_mail_sync::{Keywords, MailSync};
use jmap_proto::session::CAPABILITY_MAIL;

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-mail/tests/live_server_search.rs::connect_for_write`.
fn connect_for_write() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user, password))
        .expect("could not fetch the session document for the write-test account");
    Some(client)
}

/// A `GPtrArray` of words, owned for the length of one call — the input shape
/// `search_body_sync` takes. Mirrors `tests/live_server_search.rs::UidList`.
struct WordList {
    array: *mut GPtrArray,
    words: Vec<CString>,
}

impl WordList {
    fn of(words: &[&str]) -> Self {
        let words: Vec<CString> = words
            .iter()
            .map(|word| CString::new(*word).expect("a word with no NUL"))
            .collect();
        // SAFETY: a fresh array, filled with pointers into strings this value
        // owns and outlives it by.
        let array = unsafe {
            let array = glib_sys::g_ptr_array_new();
            for word in &words {
                g_ptr_array_add(array, word.as_ptr() as glib_sys::gpointer);
            }
            array
        };
        Self { array, words }
    }
}

impl Drop for WordList {
    fn drop(&mut self) {
        // SAFETY: the one array, allocated above; FALSE because the pointers
        // in it belong to `self.words`.
        unsafe { g_ptr_array_free(self.array, GFALSE) };
        self.words.clear();
    }
}

/// Calls `search_body_sync` for `words` against `folder` and collects the
/// uids it answers with.
///
/// # Safety
///
/// `folder` must be a live `JmapFolder` instance.
unsafe fn search(folder: *mut CamelFolder, words: &[&str]) -> Vec<String> {
    let list = WordList::of(words);
    let mut out_uids: *mut GPtrArray = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: `folder` is live per the caller's contract, `list.array` is a
    // live `GPtrArray` of NUL-terminated strings, and `out_uids`/`error` are
    // valid places to write.
    let ok = unsafe {
        search_body_sync(
            folder,
            list.array,
            &mut out_uids,
            ptr::null_mut(),
            &mut error,
        )
    };
    assert_eq!(
        ok,
        glib_sys::GTRUE,
        "search_body_sync reported an error against the real server"
    );
    assert!(error.is_null());
    assert!(
        !out_uids.is_null(),
        "the search answered with no array at all"
    );
    // SAFETY: `out_uids` is the array `search_body_sync` just returned, still
    // owned by this call.
    let uids = unsafe {
        (0..(*out_uids).len)
            .map(|index| {
                let uid = (*out_uids).pdata.add(index as usize).read();
                CStr::from_ptr(uid.cast()).to_string_lossy().into_owned()
            })
            .collect()
    };
    // SAFETY: the one array `search_body_sync` returned, never freed
    // elsewhere; its own free function releases the pooled strings in it.
    unsafe { g_ptr_array_unref(out_uids) };
    uids
}

/// Imports two messages with distinct bodies into one real folder, then asks
/// `search_body_sync` for a word each contains and a word only one contains,
/// confirming the answer comes from the real server's `Email/query` rather
/// than a mock.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_body_term_is_answered_by_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    let sync = MailSync::new(client, account_id);
    let account = Account::open();
    account.connect(sync);

    let suffix = unique_suffix();
    let folder_name = format!("agent-searchbody-{suffix}");
    let created = account
        .jmap()
        .create_folder(None, &folder_name)
        .expect("create_folder failed against the real server");

    let shared_word = format!("agentsearchbodyquarterly{suffix}");
    let only_word = format!("agentsearchbodystatus{suffix}");
    let message = |body: &str| {
        format!(
            "From: agent-searchbody@example.invalid\r\n\
             To: agent-searchbody@example.invalid\r\n\
             Subject: agent-searchbody-{suffix}\r\n\
             MIME-Version: 1.0\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\
             \r\n\
             {body}\r\n"
        )
        .into_bytes()
    };

    let both_words_uid = account
        .jmap()
        .import_message(
            &created.id,
            message(&format!("{shared_word} {only_word}")),
            &Keywords::default(),
            None,
        )
        .expect("import_message failed against the real server for the both-words message");
    let shared_only_uid = account
        .jmap()
        .import_message(
            &created.id,
            message(&shared_word),
            &Keywords::default(),
            None,
        )
        .expect("import_message failed against the real server for the shared-only message");

    // SAFETY: `account.store` is a live `CamelStore`, and `created` is a
    // `FolderInfo` a real discovery over that same store's connection just
    // returned. The mailbox id is set at construction, so no refresh is
    // needed before the server-side search path can resolve it.
    let folder: *mut CamelFolder = unsafe { new_folder(account.store, &created) };
    assert!(!folder.is_null(), "the folder would not construct");

    // A word both messages share finds both.
    // SAFETY: `folder` is the live instance just constructed.
    let mut found = unsafe { search(folder, &[&shared_word]) };
    found.sort();
    let mut expected = vec![
        both_words_uid.as_str().to_owned(),
        shared_only_uid.as_str().to_owned(),
    ];
    expected.sort();
    assert_eq!(found, expected);

    // A word only one message has finds only that one.
    // SAFETY: as above.
    assert_eq!(
        unsafe { search(folder, &[&only_word]) },
        vec![both_words_uid.as_str().to_owned()]
    );

    // Two words ANDed together find only the message with both.
    // SAFETY: as above.
    assert_eq!(
        unsafe { search(folder, &[&shared_word, &only_word]) },
        vec![both_words_uid.as_str().to_owned()]
    );

    // A word neither message has finds nothing — an authoritative empty
    // answer, not an error.
    // SAFETY: as above.
    assert_eq!(
        unsafe { search(folder, &[&format!("agentsearchbodyabsent{suffix}")]) },
        Vec::<String>::new()
    );

    account
        .jmap()
        .expunge_message(&both_words_uid, &created.id)
        .expect("expunge_message failed against the real server for the both-words message");
    account
        .jmap()
        .expunge_message(&shared_only_uid, &created.id)
        .expect("expunge_message failed against the real server for the shared-only message");
    account
        .jmap()
        .delete_folder(&created.id)
        .expect("delete_folder failed against the real server");

    // SAFETY: the one reference `new_folder` returned.
    unsafe { gobject_sys::g_object_unref(folder.cast()) };
}
