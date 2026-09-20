// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `append_message_sync` against a real JMAP server.
//!
//! `jmap-mail-sync::MailSync::import_message` is already proven against real
//! Stalwart (the setup step of most of that crate's live-server tests), but
//! `jmap-mail`'s own `append_message_sync` vfunc on top of it — Camel's own
//! writer serialising the `CamelMimeMessage`, `row_keywords` reading the
//! `CamelMessageInfo` argument, and the decision that nothing is added to the
//! folder's local summary until the next listing — has only ever run against
//! `jmap-mockd` (`tests/append.rs`). This is the live-server counterpart, the
//! same shape as `tests/live_server_synchronize.rs` and
//! `tests/live_server_transfer.rs`.
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
    CAMEL_MESSAGE_FLAGGED, CAMEL_MESSAGE_SEEN, CamelDataWrapper,
    camel_data_wrapper_construct_from_data_sync, camel_folder_append_message_sync,
    camel_folder_get_folder_summary, camel_folder_get_message_sync, camel_folder_refresh_info_sync,
    camel_folder_summary_get, camel_message_info_new, camel_message_info_set_date_received,
    camel_message_info_set_flags, camel_mime_message_get_subject, camel_mime_message_new,
};
use glib_sys::{GError, GFALSE, gssize};
use gobject_sys::g_object_unref;
use jmap_client::{Client, Credentials};
use jmap_mail::folder::new_folder;
use jmap_mail_sync::MailSync;
use jmap_proto::Id;
use jmap_proto::session::CAPABILITY_MAIL;

/// A fixed instant a `UTCDate` can spell exactly: 2026-01-15T09:30:00Z.
const RECEIVED_AT: i64 = 1_768_469_400;

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

/// Builds and parses the `CamelMimeMessage` a caller hands the append vfunc.
struct Message(*mut eds_sys::CamelMimeMessage);

impl Message {
    fn parsed(source: &[u8]) -> Self {
        // SAFETY: a fresh message is a valid `CamelDataWrapper`, `source` is a
        // live buffer of the length given, and the error out-parameter is a
        // local that starts NULL.
        unsafe {
            let message = camel_mime_message_new();
            let mut error: *mut GError = ptr::null_mut();
            let parsed = camel_data_wrapper_construct_from_data_sync(
                message.cast::<CamelDataWrapper>(),
                source.as_ptr().cast(),
                source.len() as gssize,
                ptr::null_mut(),
                &mut error,
            );
            assert_ne!(parsed, GFALSE, "the fixture message would not parse");
            Self(message)
        }
    }
}

impl Drop for Message {
    fn drop(&mut self) {
        // SAFETY: the one reference, taken at construction.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// A detached row, carrying flags and a received date the way Camel builds
/// one for a message about to be appended.
struct Row(*mut eds_sys::CamelMessageInfo);

impl Row {
    fn new() -> Self {
        // SAFETY: NULL for the summary is the documented way to build a row
        // that belongs to none, which is what an append's argument is.
        let info = unsafe { camel_message_info_new(ptr::null_mut()) };
        // SAFETY: a live row this value owns.
        unsafe {
            camel_message_info_set_flags(
                info,
                CAMEL_MESSAGE_SEEN | CAMEL_MESSAGE_FLAGGED,
                CAMEL_MESSAGE_SEEN | CAMEL_MESSAGE_FLAGGED,
            );
            camel_message_info_set_date_received(info, RECEIVED_AT);
        }
        Self(info)
    }
}

impl Drop for Row {
    fn drop(&mut self) {
        // SAFETY: the one reference, taken at construction.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// Appends a real message with a flagged, read, dated row through
/// `camel_folder_append_message_sync` against a real folder. Confirms the row
/// is absent from the folder's own local summary until the next refresh
/// (append does not write it), that a real listing afterwards holds the
/// keywords and `receivedAt` the row carried, and that `get_message_sync`
/// reads the same subject back out. Cleans up via `expunge_message` and
/// `delete_folder`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn append_message_sync_carries_a_message_and_its_row_to_the_real_server() {
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
    let name = format!("agent-mailappend-{suffix}");
    let created = account
        .jmap()
        .create_folder(None, &name)
        .expect("create_folder failed against the real server");

    let subject = format!("agent-mailappend-{suffix}");
    let source = format!(
        "From: agent-mailappend@example.invalid\r\n\
         To: agent-mailappend@example.invalid\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         Body.\r\n"
    )
    .into_bytes();

    // SAFETY: `account.store` is a live `CamelStore`, and `created` is a
    // `FolderInfo` a real discovery over that same store's connection just
    // returned.
    let folder = unsafe { new_folder(account.store, &created) };
    assert!(!folder.is_null(), "the folder would not construct");

    let message = Message::parsed(&source);
    let row = Row::new();
    let mut appended_uid: *mut glib_sys::gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder, a live message, a live row, and two
    // out-parameters that are writable and currently NULL.
    let ok = unsafe {
        camel_folder_append_message_sync(
            folder,
            message.0,
            row.0,
            &mut appended_uid,
            ptr::null_mut(),
            &mut error,
        )
    };
    assert_ne!(
        ok, GFALSE,
        "append_message_sync failed against the real server"
    );
    assert!(error.is_null(), "an append that worked set an error");
    assert!(
        !appended_uid.is_null(),
        "a successful append should report the uid the server minted"
    );
    // SAFETY: an owned, NUL-terminated string Camel handed over.
    let uid = unsafe {
        std::ffi::CStr::from_ptr(appended_uid)
            .to_string_lossy()
            .into_owned()
    };
    // SAFETY: the string this out-parameter owns.
    unsafe { glib_sys::g_free(appended_uid.cast()) };

    let uid_c = CString::new(uid.as_str()).expect("a uid with no NUL");

    // The append does not write the folder's own summary: the row appears
    // only once the folder is listed, same as a message arriving any other
    // way.
    // SAFETY: a live folder; a summary with no rows is still a live summary.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        assert!(!summary.is_null(), "the folder has no summary");
        let row = camel_folder_summary_get(summary, uid_c.as_ptr());
        assert!(
            row.is_null(),
            "an append should not have written the folder's local summary"
        );
    }

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder, and an out-parameter that is writable and
    // currently NULL.
    unsafe {
        assert_ne!(
            camel_folder_refresh_info_sync(folder, ptr::null_mut(), &mut error),
            GFALSE,
            "the folder would not refresh"
        );
    }
    // SAFETY: as above; the refresh above should have built a row.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        let refreshed_row = camel_folder_summary_get(summary, uid_c.as_ptr());
        assert!(
            !refreshed_row.is_null(),
            "the refresh left no row for the appended message"
        );
        g_object_unref(refreshed_row.cast());
    }

    // Independent confirmation: a fresh connection, sharing nothing with the
    // one the append used, sees the keywords and the date the row carried.
    let verify_client = connect_for_write().expect("the write-test account is still there");
    let verify_sync = MailSync::new(verify_client, account_id);
    let (_, messages) = verify_sync
        .messages(&created.id)
        .expect("listing the folder against the real server failed");
    let listed = messages
        .iter()
        .find(|listed| listed.uid.as_str() == uid.as_str())
        .expect("the appended message should be listed");
    assert!(
        listed.flags.seen,
        "the real server should hold $seen for the row's flag"
    );
    assert!(
        listed.flags.flagged,
        "the real server should hold $flagged for the row's flag"
    );
    assert_eq!(
        listed.received_at,
        Some(RECEIVED_AT),
        "the real server should hold the row's own received date"
    );

    // The message that went up is the message that comes back down.
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder, a NUL-terminated uid alive across the call, and
    // an out-parameter that is writable and currently NULL.
    unsafe {
        let reopened =
            camel_folder_get_message_sync(folder, uid_c.as_ptr(), ptr::null_mut(), &mut error);
        assert!(!reopened.is_null(), "the appended message would not open");
        let reopened_subject = camel_mime_message_get_subject(reopened);
        assert!(
            !reopened_subject.is_null(),
            "the reopened message lost its subject"
        );
        assert_eq!(
            std::ffi::CStr::from_ptr(reopened_subject).to_string_lossy(),
            subject,
            "the reopened message is not the one that went up"
        );
        g_object_unref(reopened.cast());
    }

    // The server refuses to delete a non-empty mailbox, so the message goes
    // first.
    account
        .jmap()
        .expunge_message(&Id::from(uid), &created.id)
        .expect("expunge_message failed against the real server");
    account
        .jmap()
        .delete_folder(&created.id)
        .expect("delete_folder failed against the real server");

    // SAFETY: the one reference `new_folder` returned.
    unsafe { g_object_unref(folder.cast()) };
}
