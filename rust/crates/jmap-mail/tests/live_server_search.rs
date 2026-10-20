// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `search_by_expression`/`search_by_uids` against a real JMAP server.
//!
//! `jmap-mail-sync::MailSync::search` is already proven against real
//! Stalwart.
//!
//! This crate's own vfuncs on top of it are not: `folder.rs`'s
//! `server_matches`, `search_sexp::translate`'s e-sexp-to-JMAP-filter
//! translation, and `narrow`'s uid restriction for `search_by_uids`.
//!
//! Every test of that layer so far (`tests/search.rs`) runs against
//! `jmap-mockd`, not against a real server's own `Email/query` answer to a
//! translated filter. This test is the live-server counterpart. It has the
//! same shape as `tests/live_server_transfer.rs` and
//! `tests/live_server_expunge.rs`.
//!
//! This file only builds where `camel_folder_search_object` is defined. EDS
//! keeps that function up to version 3.52. EDS removes it in 3.60 and
//! replaces it with the base class's own `search_sync`. The test runner this
//! suite builds against is version 3.52.3.
//!
//! ## Running it
//!
//! This test needs the same environment as the other live-server tests. See
//! `docs/manual-test-live-server.md`:
//!
//! ```console
//! $ cargo test -p jmap-mail --features testing -- --ignored
//! ```
//!
//! The test skips instead of failing when
//! `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are unset.

#![cfg(camel_folder_search_object)]

mod common;

use std::env;
use std::ffi::{CStr, CString};
use std::ptr;

use common::Account;
use eds_sys::CamelFolder;
use glib_sys::{GError, GFALSE, GPtrArray, g_ptr_array_add, g_ptr_array_free, gchar, gpointer};
use jmap_client::{Client, Credentials};
use jmap_mail::folder::new_folder;
use jmap_mail_sync::{Keywords, MailSync};
use jmap_proto::session::CAPABILITY_MAIL;

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-mail/tests/live_server_expunge.rs::connect_for_write`.
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

/// A `GPtrArray` of uids, owned for the length of one call. This is the
/// input shape `search_by_uids` takes. Mirrors `tests/search.rs::UidList`.
struct UidList {
    array: *mut GPtrArray,
    uids: Vec<CString>,
}

impl UidList {
    fn of(uids: &[&str]) -> Self {
        let uids: Vec<CString> = uids
            .iter()
            .map(|uid| CString::new(*uid).expect("a uid with no NUL"))
            .collect();
        // SAFETY: a fresh array, filled with pointers into strings this value
        // owns. Those strings outlive the array.
        let array = unsafe {
            let array = glib_sys::g_ptr_array_new();
            for uid in &uids {
                g_ptr_array_add(array, uid.as_ptr() as gpointer);
            }
            array
        };
        Self { array, uids }
    }
}

impl Drop for UidList {
    fn drop(&mut self) {
        // SAFETY: the array allocated above. The call passes FALSE because
        // the pointers in it belong to `self.uids`.
        unsafe { g_ptr_array_free(self.array, GFALSE) };
        self.uids.clear();
    }
}

/// The uids a search answer names, in order.
///
/// # Safety
///
/// `array` must be a live `GPtrArray` of NUL-terminated strings.
unsafe fn collect(array: *mut GPtrArray) -> Vec<String> {
    // SAFETY: the contract above. Every element lives at least as long as
    // the array. The caller frees the array after this call returns.
    unsafe {
        (0..(*array).len)
            .map(|index| {
                let uid: *const gchar = (*array).pdata.add(index as usize).read().cast();
                CStr::from_ptr(uid).to_string_lossy().into_owned()
            })
            .collect()
    }
}

/// Imports two messages with distinct subjects into one real folder.
///
/// Drives a real `camel_folder_search_by_expression` with a
/// `header-contains` expression. `search_sexp::translate` turns this
/// expression into an `Email/query` filter. The test confirms the
/// server-side match set names only the message whose subject contains the
/// searched word.
///
/// The test also confirms that `search_by_uids` narrows the same
/// expression correctly. It narrows to the matching message when both uids
/// are offered. It narrows to nothing when the restriction excludes the
/// matching message.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_translatable_expression_is_answered_by_the_real_server() {
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
    let folder_name = format!("agent-mailsearch-{suffix}");
    let created = account
        .jmap()
        .create_folder(None, &folder_name)
        .expect("create_folder failed against the real server");

    let lunch_subject = format!("agent-mailsearch-lunch-{suffix}");
    let other_subject = format!("agent-mailsearch-status-{suffix}");
    let message = |subject: &str| {
        format!(
            "From: agent-mailsearch@example.invalid\r\n\
             To: agent-mailsearch@example.invalid\r\n\
             Subject: {subject}\r\n\
             MIME-Version: 1.0\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\
             \r\n\
             Body.\r\n"
        )
        .into_bytes()
    };

    let lunch_uid = account
        .jmap()
        .import_message(
            &created.id,
            message(&lunch_subject),
            &Keywords::default(),
            None,
        )
        .expect("import_message failed against the real server for the lunch message");
    let other_uid = account
        .jmap()
        .import_message(
            &created.id,
            message(&other_subject),
            &Keywords::default(),
            None,
        )
        .expect("import_message failed against the real server for the other message");

    // SAFETY: `account.store` is a live `CamelStore`. `created` is a
    // `FolderInfo` that a real discovery over the same store's connection
    // just returned. Construction sets the mailbox id, so the server-side
    // search path can resolve it without a refresh.
    let folder: *mut CamelFolder = unsafe { new_folder(account.store, &created) };
    assert!(!folder.is_null(), "the folder would not construct");

    let expression = c"(header-contains \"Subject\" \"lunch\")";

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder, a NUL-terminated expression live for the call,
    // and a writable, currently-NULL out-parameter.
    let result = unsafe {
        eds_sys::camel_folder_search_by_expression(
            folder,
            expression.as_ptr(),
            ptr::null_mut(),
            &mut error,
        )
    };
    assert!(
        error.is_null(),
        "search_by_expression reported an error against the real server"
    );
    assert!(
        !result.is_null(),
        "the search answered with no array at all"
    );
    // SAFETY: `result` is the array just returned, still owned by this call.
    assert_eq!(
        unsafe { collect(result) },
        vec![lunch_uid.as_str().to_owned()]
    );
    // SAFETY: the folder and array match. This call frees the array that
    // `camel_folder_search_by_expression` returned.
    unsafe { eds_sys::camel_folder_search_free(folder, result) };

    // Narrowed to both uids. The match must come from the filter, not from
    // the restriction excluding the other row.
    let restrict = UidList::of(&[lunch_uid.as_str(), other_uid.as_str()]);
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: as above, plus a live `GPtrArray` of NUL-terminated uids.
    let result = unsafe {
        eds_sys::camel_folder_search_by_uids(
            folder,
            expression.as_ptr(),
            restrict.array,
            ptr::null_mut(),
            &mut error,
        )
    };
    assert!(
        error.is_null(),
        "search_by_uids reported an error against the real server"
    );
    assert!(
        !result.is_null(),
        "the search answered with no array at all"
    );
    // SAFETY: as above.
    assert_eq!(
        unsafe { collect(result) },
        vec![lunch_uid.as_str().to_owned()]
    );
    // SAFETY: as above.
    unsafe { eds_sys::camel_folder_search_free(folder, result) };

    // Narrowed to exclude the matching row. The restriction wins even though
    // the expression alone matches.
    let excluding = UidList::of(&[other_uid.as_str()]);
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: as above.
    let result = unsafe {
        eds_sys::camel_folder_search_by_uids(
            folder,
            expression.as_ptr(),
            excluding.array,
            ptr::null_mut(),
            &mut error,
        )
    };
    assert!(
        error.is_null(),
        "search_by_uids reported an error against the real server"
    );
    assert!(
        !result.is_null(),
        "the search answered with no array at all"
    );
    // SAFETY: as above.
    assert_eq!(unsafe { collect(result) }, Vec::<String>::new());
    // SAFETY: as above.
    unsafe { eds_sys::camel_folder_search_free(folder, result) };

    // The server refuses to delete a non-empty mailbox, so the test deletes
    // both messages first.
    account
        .jmap()
        .expunge_message(&lunch_uid, &created.id)
        .expect("expunge_message failed against the real server for the lunch message");
    account
        .jmap()
        .expunge_message(&other_uid, &created.id)
        .expect("expunge_message failed against the real server for the other message");
    account
        .jmap()
        .delete_folder(&created.id)
        .expect("delete_folder failed against the real server");

    // SAFETY: the one reference `new_folder` returned.
    unsafe { gobject_sys::g_object_unref(folder.cast()) };
}
