// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `synchronize_sync`'s dirty-row walk against a real JMAP server.
//!
//! `jmap-mail-sync/tests/live_server_keywords.rs` already proves
//! `MailSync::set_keywords` against real Stalwart, and
//! `jmap-mail/tests/synchronize.rs` already proves the walk from a dirty
//! `CamelMessageInfo` row to a `KeywordChange` against `jmap-mockd`. Neither
//! reaches the other's gap: the full stack from a real `CamelFolderSummary`
//! row, built by a real refresh, through `synchronize_sync`'s work-list walk
//! and `CamelJmapMessageInfo`'s own "what did we last write" column, to a
//! real server's `Email/set` answer, has never run against anything but the
//! mock. This is that confirmation, the same shape as
//! `jmap-mail/tests/live_server_folder.rs` for the folder cache.
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
use std::ffi::CString;
use std::ptr;

use common::Account;
use eds_sys::{
    CAMEL_MESSAGE_FLAGGED, camel_folder_get_folder_summary, camel_folder_refresh_info_sync,
    camel_folder_summary_get, camel_folder_synchronize_sync, camel_message_info_get_folder_flagged,
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

/// Mirrors `jmap-mail-sync/tests/live_server_keywords.rs::connect_for_write`.
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

/// Imports a message through `JmapStore::import_message`, opens a real
/// `CamelFolder` on it, refreshes it (a real listing builds the summary row),
/// flags the row through Camel's own setter, drives a real
/// `camel_folder_synchronize_sync`, then confirms two things: the row's own
/// dirty bit was cleared, and a second, independent connection sees
/// `$flagged` on the server. Cleans up via `delete_folder`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn synchronize_sync_carries_a_flag_change_to_the_real_server() {
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

    let name = format!("agent-mailsync-synchronize-{}", unique_suffix());
    let created = account
        .jmap()
        .create_folder(None, &name)
        .expect("create_folder failed against the real server");

    let subject = format!("agent-mailsync-synchronize-{}", unique_suffix());
    let source = format!(
        "From: agent-mailsync@example.invalid\r\n\
         To: agent-mailsync@example.invalid\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         Body.\r\n"
    )
    .into_bytes();
    let uid = account
        .jmap()
        .import_message(&created.id, source, &Keywords::default(), None)
        .expect("import_message failed against the real server");

    // SAFETY: `account.store` is a live `CamelStore`, and `created` is a
    // `FolderInfo` a real discovery over that same store's connection just
    // returned.
    let folder = unsafe { new_folder(account.store, &created) };
    assert!(!folder.is_null(), "the folder would not construct");

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder, never refreshed before, and an out-parameter
    // that is writable and currently NULL.
    unsafe {
        assert_ne!(
            camel_folder_refresh_info_sync(folder, ptr::null_mut(), &mut error),
            GFALSE,
            "the folder would not refresh"
        );
    }

    let uid_c = CString::new(uid.as_str()).expect("a uid with no NUL");
    // SAFETY: the folder has a summary once refreshed, and `uid_c` is
    // NUL-terminated and alive across the call.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        assert!(!summary.is_null(), "the folder has no summary");
        let row = camel_folder_summary_get(summary, uid_c.as_ptr());
        assert!(
            !row.is_null(),
            "the refresh left no row for the imported message"
        );

        camel_message_info_set_flags(row, CAMEL_MESSAGE_FLAGGED, CAMEL_MESSAGE_FLAGGED);
        assert_ne!(
            camel_message_info_get_folder_flagged(row),
            GFALSE,
            "changing a flag should have put the row on the work list"
        );
        g_object_unref(row.cast());
    }

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder with one dirty row, and an out-parameter that is
    // writable and currently NULL. FALSE for `expunge`: this test is only
    // about the flag.
    let ok = unsafe { camel_folder_synchronize_sync(folder, GFALSE, ptr::null_mut(), &mut error) };
    assert_ne!(
        ok, GFALSE,
        "synchronize_sync failed against the real server"
    );
    assert!(
        error.is_null(),
        "a synchronisation that worked set an error"
    );

    // SAFETY: as above.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        let row = camel_folder_summary_get(summary, uid_c.as_ptr());
        assert!(!row.is_null(), "the row disappeared across the synchronise");
        assert_eq!(
            camel_message_info_get_folder_flagged(row),
            GFALSE,
            "a successful synchronisation should have cleared the dirty bit"
        );
        g_object_unref(row.cast());
    }

    // Independent confirmation: a fresh connection, sharing nothing with the
    // one `synchronize_sync` used, sees the keyword the server now holds.
    let verify_client = connect_for_write().expect("the write-test account is still there");
    let verify_sync = MailSync::new(verify_client, account_id);
    let (_, messages) = verify_sync
        .messages(&created.id)
        .expect("listing the folder against the real server failed");
    let listed = messages
        .iter()
        .find(|message| message.uid == uid)
        .expect("the imported message should be listed");
    assert!(
        listed.flags.flagged,
        "the real server should hold $flagged for the message synchronize_sync just wrote"
    );

    // The server refuses to delete a non-empty mailbox, so the message goes
    // first.
    account
        .jmap()
        .expunge_message(&uid, &created.id)
        .expect("expunge_message failed against the real server");
    account
        .jmap()
        .delete_folder(&created.id)
        .expect("delete_folder failed against the real server");

    // SAFETY: the one reference `new_folder` returned.
    unsafe { g_object_unref(folder.cast()) };
}
