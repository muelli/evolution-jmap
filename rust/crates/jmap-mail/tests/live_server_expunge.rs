// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `expunge_sync` against a real JMAP server.
//!
//! `jmap-mail-sync::MailSync::expunge_message` is already proven against a
//! real Stalwart server (`jmap-mail-sync/tests/live_server_expunge.rs`).
//! `jmap-mail`'s own `expunge_sync` vfunc, built on top of it, has only run
//! against `jmap-mockd` so far (`tests/expunge.rs`). That vfunc is
//! `expunge.rs`'s walk over the whole summary for rows marked
//! `CAMEL_MESSAGE_DELETED`, plus the immediate removal of the matching local
//! rows.
//!
//! This file is the live-server counterpart, in the same shape as
//! `tests/live_server_synchronize.rs` and `tests/live_server_transfer.rs`.
//! The folder's announcement of the removal is local, deterministic
//! behaviour. The mock already covers it, so it stays out of scope here.
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
//! The test is skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER` and
//! `_PASSWORD` are unset.

mod common;

use std::env;
use std::ffi::CString;
use std::ptr;

use common::Account;
use eds_sys::{
    CAMEL_MESSAGE_DELETED, camel_folder_expunge_sync, camel_folder_get_folder_summary,
    camel_folder_get_message_count, camel_folder_refresh_info_sync, camel_folder_summary_get,
    camel_message_info_set_flags,
};
use glib_sys::{GError, GFALSE};
use gobject_sys::g_object_unref;
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

/// Mirrors `jmap-mail/tests/live_server_synchronize.rs::connect_for_write`.
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

/// Imports two messages into a real folder. Marks one message deleted the
/// way Evolution's Delete key does, then calls a real
/// `camel_folder_expunge_sync`.
///
/// Confirms the deleted message's row disappears from the folder's local
/// summary right away, with no further refresh. Confirms an independent
/// connection sees that the real server destroyed the message. Confirms the
/// untouched message survives both locally and on the server.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn expunge_sync_removes_only_the_deleted_message_on_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    let sync = MailSync::new(client, account_id.clone());
    let account = Account::open();
    account.connect(sync);

    let suffix = unique_suffix();
    let folder_name = format!("agent-mailexpunge-{suffix}");
    let created = account
        .jmap()
        .create_folder(None, &folder_name)
        .expect("create_folder failed against the real server");

    let subject_deleted = format!("agent-mailexpunge-deleted-{suffix}");
    let subject_kept = format!("agent-mailexpunge-kept-{suffix}");
    let message = |subject: &str| {
        format!(
            "From: agent-mailexpunge@example.invalid\r\n\
             To: agent-mailexpunge@example.invalid\r\n\
             Subject: {subject}\r\n\
             MIME-Version: 1.0\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\
             \r\n\
             Body.\r\n"
        )
        .into_bytes()
    };

    let uid_deleted = account
        .jmap()
        .import_message(
            &created.id,
            message(&subject_deleted),
            &Keywords::default(),
            None,
        )
        .expect("import_message failed against the real server for the deleted message");
    let uid_kept = account
        .jmap()
        .import_message(
            &created.id,
            message(&subject_kept),
            &Keywords::default(),
            None,
        )
        .expect("import_message failed against the real server for the kept message");

    // SAFETY: `account.store` is a live `CamelStore`. `created` is a
    // `FolderInfo` that a real discovery over that same store's connection
    // just returned.
    let folder = unsafe { new_folder(account.store, &created) };
    assert!(!folder.is_null(), "the folder would not construct");

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: the folder is live and never refreshed before. `error` is a
    // writable out-parameter, currently NULL.
    unsafe {
        assert_ne!(
            camel_folder_refresh_info_sync(folder, ptr::null_mut(), &mut error),
            GFALSE,
            "the folder would not refresh"
        );
    }

    let uid_deleted_c = CString::new(uid_deleted.as_str()).expect("a uid with no NUL");
    // SAFETY: the refresh above created a row for the deleted message.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        let info = camel_folder_summary_get(summary, uid_deleted_c.as_ptr());
        assert!(
            !info.is_null(),
            "the refresh left no row for the message to delete"
        );
        camel_message_info_set_flags(info, CAMEL_MESSAGE_DELETED, CAMEL_MESSAGE_DELETED);
        g_object_unref(info.cast());
    }

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: the folder is live. `error` is a writable, currently NULL
    // out-parameter.
    let ok = unsafe { camel_folder_expunge_sync(folder, ptr::null_mut(), &mut error) };
    assert_ne!(ok, GFALSE, "expunge_sync failed against the real server");
    assert!(error.is_null(), "an expunge that worked set an error");

    // The row is gone from the folder's own local summary right away, with
    // no further refresh.
    // SAFETY: as above.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        let row = camel_folder_summary_get(summary, uid_deleted_c.as_ptr());
        assert!(
            row.is_null(),
            "the expunged message's row should be gone from the folder immediately"
        );
        assert_eq!(
            camel_folder_get_message_count(folder) as u32,
            1,
            "only the kept message's row should remain"
        );
    }

    // A fresh connection, separate from the one the expunge used, confirms
    // the real server agrees.
    let verify_client = connect_for_write().expect("the write-test account is still there");
    let verify_sync = MailSync::new(verify_client, account_id);
    let (_, messages) = verify_sync
        .messages(&created.id)
        .expect("listing the folder against the real server failed");
    assert!(
        !messages.iter().any(|listed| listed.uid == uid_deleted),
        "the real server should no longer list the expunged message"
    );
    assert!(
        messages.iter().any(|listed| listed.uid == uid_kept),
        "the real server should still list the untouched message"
    );

    // The server refuses to delete a non-empty mailbox. The surviving
    // message is deleted first.
    account
        .jmap()
        .expunge_message(&uid_kept, &created.id)
        .expect("expunge_message failed against the real server for the kept message");
    account
        .jmap()
        .delete_folder(&created.id)
        .expect("delete_folder failed against the real server");

    // SAFETY: this is the one reference that `new_folder` returned.
    unsafe { g_object_unref(folder.cast()) };
}
