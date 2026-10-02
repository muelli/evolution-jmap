// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `update_message_info`'s replay-over-an-outstanding-change logic, against a
//! real `Email/changes` delta rather than a hand-built [`MessageSummary`].
//!
//! `jmap-mail/tests/message_info.rs` (`a_listing_still_brings_a_queued_row_
//! what_the_server_changed`, `a_listing_takes_a_keyword_off_a_queued_row_
//! when_the_server_did`) proves `update_message_info`'s replay against a
//! synthetic listing built by hand. `jmap-mail/tests/live_server_refresh.rs`
//! proves the delta branch of `refresh_info_sync` against a real server, but
//! only for a folder with no local, unsaved change on any row. Neither test
//! combines the two: a row Camel still has queued (the user flagged it, not
//! yet synchronised) meeting a real `Email/changes` delta that reports
//! another client changed the same message's keywords in the meantime. This
//! is that combination.
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
    camel_folder_summary_get, camel_message_info_get_flags, camel_message_info_get_folder_flagged,
    camel_message_info_get_user_flag, camel_message_info_set_flags,
};
use glib_sys::{GError, GFALSE};
use gobject_sys::g_object_unref;
use jmap_client::{Client, Credentials};
use jmap_mail::folder::new_folder;
use jmap_mail_sync::{KeywordChange, Keywords, MailSync};
use jmap_proto::session::CAPABILITY_MAIL;

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-mail/tests/live_server_refresh.rs::connect_for_write`.
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

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_delta_replays_onto_a_row_still_queued_for_the_server() {
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
    let folder_name = format!("agent-keywordrace-{suffix}");
    let created = account
        .jmap()
        .create_folder(None, &folder_name)
        .expect("create_folder failed against the real server");

    let subject = format!("agent-keywordrace-{suffix}");
    let message = format!(
        "From: agent-keywordrace@example.invalid\r\n\
         To: agent-keywordrace@example.invalid\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         Body.\r\n"
    )
    .into_bytes();

    let uid = account
        .jmap()
        .import_message(&created.id, message, &Keywords::default(), None)
        .expect("import_message failed against the real server");

    // SAFETY: `account.store` is a live `CamelStore`, and `created` is a
    // `FolderInfo` a real discovery over that same store's connection just
    // returned.
    let folder = unsafe { new_folder(account.store, &created) };
    assert!(!folder.is_null(), "the folder would not construct");

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder, never refreshed before, and a writable,
    // currently-NULL out-parameter. The full-listing branch, already proven
    // live elsewhere; it only builds the row the second refresh meets.
    unsafe {
        assert_ne!(
            camel_folder_refresh_info_sync(folder, ptr::null_mut(), &mut error),
            GFALSE,
            "the first refresh failed against the real server"
        );
    }

    let uid_c = CString::new(uid.as_str()).expect("a uid with no NUL");

    // SAFETY: the refresh above built a row for the message; nothing else
    // touches it yet.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        let info = camel_folder_summary_get(summary, uid_c.as_ptr());
        assert!(
            !info.is_null(),
            "the first refresh left no row for the message"
        );

        assert_eq!(
            camel_message_info_get_folder_flagged(info),
            GFALSE,
            "a row a listing built should not start out queued for the server"
        );

        // The user flags the message locally. Camel's own setter is what
        // marks the row queued — the same call `jmap-mail/tests/
        // message_info.rs`'s mock tests make, now on a row the real server
        // itself built.
        camel_message_info_set_flags(info, CAMEL_MESSAGE_FLAGGED, CAMEL_MESSAGE_FLAGGED);
        assert_ne!(
            camel_message_info_get_folder_flagged(info),
            GFALSE,
            "Camel did not queue the row the user flagged"
        );
        g_object_unref(info.cast());
    }

    // From an independent connection, another client changes the same
    // message's keywords on the real server — a label this test's local
    // change never mentioned.
    let mover_client = connect_for_write().expect("the write-test account is still there");
    let mover_sync = MailSync::new(mover_client, account_id);
    let labelled = Keywords::from_iter([String::from("Urgent")]);
    let change = KeywordChange::between(&Keywords::default(), &labelled);
    mover_sync
        .set_keywords(&uid, &change)
        .expect("the other client's keyword change failed against the real server");

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: the folder's summary holds the first refresh's state, so this
    // is the delta branch — a real `Email/changes` request — and the row it
    // meets for this uid is the one just flagged and queued above.
    unsafe {
        assert_ne!(
            camel_folder_refresh_info_sync(folder, ptr::null_mut(), &mut error),
            GFALSE,
            "the second refresh failed against the real server"
        );
    }
    assert!(error.is_null(), "a refresh that worked set an error");

    // SAFETY: as above.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        let info = camel_folder_summary_get(summary, uid_c.as_ptr());
        assert!(!info.is_null(), "the delta removed the row this test needs");

        assert_ne!(
            camel_message_info_get_flags(info) & CAMEL_MESSAGE_FLAGGED,
            0,
            "the real delta overwrote the user's unsaved flag"
        );
        assert_ne!(
            camel_message_info_get_user_flag(info, c"Urgent".as_ptr()),
            GFALSE,
            "the label the other client added over the real server never arrived"
        );
        assert_ne!(
            camel_message_info_get_folder_flagged(info),
            GFALSE,
            "the row's still-unsynced local change should leave it queued"
        );
        g_object_unref(info.cast());
    }

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
