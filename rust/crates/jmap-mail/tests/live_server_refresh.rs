// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `refresh_info_sync`'s delta path against a real JMAP server.
//!
//! Every existing live-server test refreshes a freshly-created folder exactly
//! once, so its summary never holds a prior state and `refresh_info_sync`
//! (`jmap-mail/src/refresh.rs`) always takes the `None => store.messages(...)`
//! full-listing branch. The `Some(since) => store.messages_since(...)` branch
//! — a real `Email/changes` request — has only ever run against
//! `jmap-mockd` (`jmap-mail/tests/refresh.rs`). This is that confirmation: a
//! folder refreshed twice, with a server-side change between the two.
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
    camel_folder_get_folder_summary, camel_folder_get_message_count,
    camel_folder_refresh_info_sync, camel_folder_summary_get,
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

/// Imports one message, refreshes a real folder onto it (the full-listing
/// branch, since the folder has never been refreshed before), then — from an
/// independent connection — destroys that message and imports a second one,
/// moving the mailbox on since the state the first refresh recorded. A second
/// refresh of the same folder object then has a state to ask from, so it
/// takes the delta branch: a real `Email/changes` request. Confirms the
/// folder ends up holding only the second message's row, with no further
/// listing involved, and that an independent connection agrees.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_second_refresh_applies_a_real_delta() {
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
    let folder_name = format!("agent-mailrefresh-{suffix}");
    let created = account
        .jmap()
        .create_folder(None, &folder_name)
        .expect("create_folder failed against the real server");

    let subject_first = format!("agent-mailrefresh-first-{suffix}");
    let subject_second = format!("agent-mailrefresh-second-{suffix}");
    let message = |subject: &str| {
        format!(
            "From: agent-mailrefresh@example.invalid\r\n\
             To: agent-mailrefresh@example.invalid\r\n\
             Subject: {subject}\r\n\
             MIME-Version: 1.0\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\
             \r\n\
             Body.\r\n"
        )
        .into_bytes()
    };

    let uid_first = account
        .jmap()
        .import_message(
            &created.id,
            message(&subject_first),
            &Keywords::default(),
            None,
        )
        .expect("import_message failed against the real server for the first message");

    // SAFETY: `account.store` is a live `CamelStore`, and `created` is a
    // `FolderInfo` a real discovery over that same store's connection just
    // returned.
    let folder = unsafe { new_folder(account.store, &created) };
    assert!(!folder.is_null(), "the folder would not construct");

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder, never refreshed before, and a writable,
    // currently-NULL out-parameter. This is the full-listing branch, already
    // proven live elsewhere; it only builds the state the second refresh
    // needs.
    unsafe {
        assert_ne!(
            camel_folder_refresh_info_sync(folder, ptr::null_mut(), &mut error),
            GFALSE,
            "the first refresh failed against the real server"
        );
    }

    let uid_first_c = CString::new(uid_first.as_str()).expect("a uid with no NUL");
    // SAFETY: the refresh above built a row for the first message.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        let info = camel_folder_summary_get(summary, uid_first_c.as_ptr());
        assert!(
            !info.is_null(),
            "the first refresh left no row for the first message"
        );
        g_object_unref(info.cast());
    }

    // Move the mailbox on from an independent connection: destroy the first
    // message and import a second, so the state the first refresh recorded is
    // now behind the server's.
    let mover_client = connect_for_write().expect("the write-test account is still there");
    let mover_sync = MailSync::new(mover_client, account_id.clone());
    mover_sync
        .expunge_message(&uid_first, &created.id)
        .expect("expunge_message failed against the real server for the first message");
    let uid_second = mover_sync
        .import_message(
            &created.id,
            message(&subject_second),
            &Keywords::default(),
            None,
        )
        .expect("import_message failed against the real server for the second message");

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: as above. The folder's summary now holds the first refresh's
    // state, so this is the delta branch: a real `Email/changes` request.
    unsafe {
        assert_ne!(
            camel_folder_refresh_info_sync(folder, ptr::null_mut(), &mut error),
            GFALSE,
            "the second refresh failed against the real server"
        );
    }
    assert!(error.is_null(), "a refresh that worked set an error");

    let uid_second_c = CString::new(uid_second.as_str()).expect("a uid with no NUL");
    // SAFETY: as above.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        let row_first = camel_folder_summary_get(summary, uid_first_c.as_ptr());
        assert!(
            row_first.is_null(),
            "the delta should have removed the destroyed message's row"
        );
        let row_second = camel_folder_summary_get(summary, uid_second_c.as_ptr());
        assert!(
            !row_second.is_null(),
            "the delta should have added a row for the message that arrived"
        );
        g_object_unref(row_second.cast());
        assert_eq!(
            camel_folder_get_message_count(folder) as u32,
            1,
            "only the second message's row should remain"
        );
    }

    // Independent confirmation: a fresh connection, sharing nothing with the
    // folder's own, agrees on the mailbox's contents.
    let verify_client = connect_for_write().expect("the write-test account is still there");
    let verify_sync = MailSync::new(verify_client, account_id);
    let (_, messages) = verify_sync
        .messages(&created.id)
        .expect("listing the folder against the real server failed");
    assert!(
        !messages.iter().any(|listed| listed.uid == uid_first),
        "the real server should no longer list the destroyed message"
    );
    assert!(
        messages.iter().any(|listed| listed.uid == uid_second),
        "the real server should list the message that arrived"
    );

    // The server refuses to delete a non-empty mailbox, so the surviving
    // message goes first.
    account
        .jmap()
        .expunge_message(&uid_second, &created.id)
        .expect("expunge_message failed against the real server for the second message");
    account
        .jmap()
        .delete_folder(&created.id)
        .expect("delete_folder failed against the real server");

    // SAFETY: the one reference `new_folder` returned.
    unsafe { g_object_unref(folder.cast()) };
}
