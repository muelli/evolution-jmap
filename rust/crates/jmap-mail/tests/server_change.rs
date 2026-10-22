// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A folder whose account moves to a different server.
//!
//! The summary and the message cache use the `Email` id as the key. Two servers
//! can give the same id to different messages: the mock and Stalwart both count
//! up from one. After a change of host, a row or a cached body from the old
//! server must not show under the id of a message on the new server.

mod common;

use std::ffi::{CStr, CString};
use std::ptr;

use common::Account;
use eds_sys::{
    CAMEL_MESSAGE_SEEN, CAMEL_STORE_FOLDER_NONE, CamelFolder, camel_folder_get_folder_summary,
    camel_folder_get_message_sync, camel_folder_refresh_info_sync, camel_folder_summary_get,
    camel_message_info_get_folder_flagged, camel_message_info_get_subject,
    camel_message_info_set_flags, camel_mime_message_get_subject, camel_store_get_folder_sync,
};
use glib_sys::{GError, GFALSE, g_error_free};
use gobject_sys::g_object_unref;
use jmap_client::{Client, Credentials};
use jmap_mail_sync::MailSync;
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::role;

/// A server with one inbox that holds one message. Each new server gives the
/// same ids, so the message id of two servers is the same.
fn server_with(subject: &str, body: &str) -> (MockServer, Id) {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let state = server.state();
    let uid = {
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_email(EmailSeed::new(
            inbox,
            ("Bob", "bob@example.com"),
            subject,
            body,
            "2026-01-01T09:00:00Z",
        ))
    };
    (server, uid)
}

fn sync_against(server: &MockServer) -> MailSync {
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    MailSync::new(client, server.account_id())
}

fn open_inbox(account: &Account) -> *mut CamelFolder {
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live store, a NUL-terminated path, and an out-parameter that
    // is writable and NULL.
    let folder = unsafe {
        camel_store_get_folder_sync(
            account.store,
            c"Inbox".as_ptr(),
            CAMEL_STORE_FOLDER_NONE,
            ptr::null_mut(),
            &mut error,
        )
    };
    assert!(
        !folder.is_null() && error.is_null(),
        "the inbox did not open"
    );
    folder
}

fn refresh(folder: *mut CamelFolder) {
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder and an out-parameter that is writable and NULL.
    let ok = unsafe { camel_folder_refresh_info_sync(folder, ptr::null_mut(), &mut error) };
    if !error.is_null() {
        // SAFETY: an owned error from the call above.
        let message = unsafe { CStr::from_ptr((*error).message) }
            .to_string_lossy()
            .into_owned();
        // SAFETY: as above, freed once.
        unsafe { g_error_free(error) };
        panic!("the refresh failed: {message}");
    }
    assert!(ok != 0, "the refresh failed with no error");
}

/// The subject in the summary row of `uid`.
fn row_subject(folder: *mut CamelFolder, uid: &Id) -> Option<String> {
    let key = CString::new(uid.as_str()).unwrap();
    // SAFETY: a live folder; the row is a new reference that this function
    // releases, and the subject is copied before the release.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        let info = camel_folder_summary_get(summary, key.as_ptr());
        if info.is_null() {
            return None;
        }
        let subject = camel_message_info_get_subject(info);
        let subject =
            (!subject.is_null()).then(|| CStr::from_ptr(subject).to_string_lossy().into_owned());
        g_object_unref(info.cast());
        subject
    }
}

/// The subject of the message that the folder opens for `uid`.
fn opened_subject(folder: *mut CamelFolder, uid: &Id) -> String {
    let key = CString::new(uid.as_str()).unwrap();
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder, a NUL-terminated uid, and an out-parameter that
    // is writable and NULL. The message is a new reference that this function
    // releases after it copies the subject.
    unsafe {
        let message =
            camel_folder_get_message_sync(folder, key.as_ptr(), ptr::null_mut(), &mut error);
        assert!(
            !message.is_null() && error.is_null(),
            "the message did not open"
        );
        let subject = camel_mime_message_get_subject(message);
        let subject = CStr::from_ptr(subject).to_string_lossy().into_owned();
        g_object_unref(message.cast());
        subject
    }
}

#[test]
fn a_refresh_from_another_server_replaces_a_row_that_shares_its_uid() {
    let (old, old_uid) = server_with("From the old server", "old");
    let (new, new_uid) = server_with("From the new server", "new");
    assert_eq!(old_uid, new_uid, "the two servers must reuse the id");

    let account = Account::open();
    account.connect(sync_against(&old));
    let folder = open_inbox(&account);
    refresh(folder);
    assert_eq!(
        row_subject(folder, &old_uid).as_deref(),
        Some("From the old server")
    );

    account.connect(sync_against(&new));
    refresh(folder);
    assert_eq!(
        row_subject(folder, &new_uid).as_deref(),
        Some("From the new server"),
        "the row of the old server stayed under the id of the new message"
    );

    // SAFETY: the reference that `open_inbox` took.
    unsafe { g_object_unref(folder.cast()) };
}

#[test]
fn a_body_cached_from_another_server_is_not_served_for_a_new_message() {
    // The old body is longer, so the size check of the cache cannot catch it.
    let (old, uid) = server_with("Old", "a long body from the old server, much longer");
    let (new, _) = server_with("New", "short");

    let account = Account::open();
    account.connect(sync_against(&old));
    let folder = open_inbox(&account);
    refresh(folder);
    assert_eq!(opened_subject(folder, &uid), "Old");

    account.connect(sync_against(&new));
    refresh(folder);
    assert_eq!(
        opened_subject(folder, &uid),
        "New",
        "the cached body of the old server opened under the id of the new message"
    );

    // SAFETY: the reference that `open_inbox` took.
    unsafe { g_object_unref(folder.cast()) };
}

/// Marks the row of `uid` as seen, the way the message list does. The change
/// waits in the row until the folder is synchronised.
fn mark_seen(folder: *mut CamelFolder, uid: &Id) {
    let key = CString::new(uid.as_str()).unwrap();
    // SAFETY: a live folder; the row is a new reference that this function
    // releases.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        let info = camel_folder_summary_get(summary, key.as_ptr());
        assert!(!info.is_null(), "the refresh left no row for the message");
        camel_message_info_set_flags(info, CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);
        g_object_unref(info.cast());
    }
}

/// Whether the row of `uid` still holds a change for the server.
fn holds_a_change(folder: *mut CamelFolder, uid: &Id) -> bool {
    let key = CString::new(uid.as_str()).unwrap();
    // SAFETY: as in `mark_seen`.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        let info = camel_folder_summary_get(summary, key.as_ptr());
        if info.is_null() {
            return false;
        }
        let flagged = camel_message_info_get_folder_flagged(info) != GFALSE;
        g_object_unref(info.cast());
        flagged
    }
}

/// The files of the message cache under `directory`, without the origin file.
fn cached_files(directory: &std::path::Path) -> usize {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .map(|path| match path.is_dir() {
            true => cached_files(&path),
            false => usize::from(!path.ends_with("jmap-origin")),
        })
        .sum()
}

#[test]
fn a_session_whose_api_url_spelling_changes_keeps_the_rows_and_the_cache() {
    let (server, uid) = server_with("Kept", "a body that stays cached");

    let account = Account::open();
    account.connect(sync_against(&server));
    let folder = open_inbox(&account);
    refresh(folder);
    assert_eq!(opened_subject(folder, &uid), "Kept");
    let cache = std::path::PathBuf::from(account.cache_dir());
    let cached = cached_files(&cache);
    assert!(cached > 0, "the opened message was not cached");
    mark_seen(folder, &uid);

    // The same endpoint under a different spelling: the next session
    // document gives `HTTP://host:port/jmap` as its `apiUrl`.
    let upper = server.origin().replacen("http://", "HTTP://", 1);
    server.state().lock().unwrap().advertise_origin = Some(upper);
    let sync = sync_against(&server);
    assert_ne!(
        sync.client().session().api_url,
        format!("{}/jmap", server.origin()),
        "the second session must spell its apiUrl differently"
    );
    account.connect(sync);
    let listed = server.method_calls().len();
    refresh(folder);
    let second = server.method_calls().split_off(listed);
    assert!(
        !second.iter().any(|call| call == "Email/query"),
        "a different spelling of the same apiUrl listed the mailbox again: {second:?}"
    );
    assert!(
        holds_a_change(folder, &uid),
        "a different spelling of the same apiUrl dropped a change not yet sent"
    );
    assert_eq!(
        cached_files(&cache),
        cached,
        "a different spelling of the same apiUrl cleared the message cache"
    );

    // SAFETY: the reference that `open_inbox` took.
    unsafe { g_object_unref(folder.cast()) };
}
