// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `MailSync::file_message` against a real JMAP server.
//! `transfer_messages_to_sync` calls this function.
//!
//! The other live-server tests do not call `file_message`. Only
//! `tests/mailboxes.rs` and `tests/updates.rs` test the
//! `Filing::copied_into` and `Filing::moved` patches, against `jmap-mock`.
//! This file is their live-server counterpart.
//!
//! ## Running it
//!
//! See `docs/manual-test-live-server.md`. Set `JMAP_LIVE_SERVER_URL`,
//! `JMAP_LIVE_SERVER_WRITE_USER` and `JMAP_LIVE_SERVER_WRITE_PASSWORD`, then:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_filing -- --ignored
//! ```
//!
//! This crate has no `live-server` feature. `#[ignore]` keeps the test out of
//! a plain `cargo test`.
//!
//! The test skips when `JMAP_LIVE_SERVER_WRITE_USER` is unset. It panics when
//! that variable is set but `JMAP_LIVE_SERVER_WRITE_PASSWORD` or
//! `JMAP_LIVE_SERVER_URL` is unset.

use jmap_mail_sync::{Filing, Keywords, MailSync};
use jmap_proto::Id;
use jmap_proto::mail::role;
use jmap_proto::session::CAPABILITY_MAIL;
use jmap_test_support::{connect_for_write, unique_suffix};

/// Returns `true` when `mailbox` lists `uid`.
fn lists(sync: &MailSync, mailbox: &Id, uid: &Id) -> bool {
    let (_, summaries) = sync
        .messages(mailbox)
        .expect("listing a mailbox failed against the real server");
    summaries.iter().any(|summary| &summary.uid == uid)
}

/// Copies a message from the Inbox into a new folder, then moves it out of
/// the Inbox with `MailSync::file_message`. Each step checks the listing of
/// the two mailboxes. The cleanup at the end runs only when all assertions
/// pass.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn filing_a_message_into_another_folder_reaches_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let inbox_id = client
        .mailbox_get(&account_id)
        .unwrap()
        .list
        .into_iter()
        .find(|mailbox| mailbox.role.as_deref() == Some(role::INBOX))
        .expect("the write-test account needs an Inbox")
        .id
        .expect("the server named the Inbox");

    let sync = MailSync::new(client, account_id);

    let suffix = unique_suffix();
    let folder_name = format!("agent-mailsync-filing-{suffix}");
    let folder = sync
        .create_folder(None, &folder_name)
        .expect("Mailbox/set create failed against the real server");

    let subject = format!("agent-mailsync-filing-{suffix}");
    let message = format!(
        "From: agent-mailsync@example.invalid\r\n\
         To: agent-mailsync@example.invalid\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         It arrived through MailSync::import_message.\r\n"
    );

    let uid = sync
        .import_message(&inbox_id, message.into_bytes(), &Keywords::default(), None)
        .expect("Email/import failed against the real server");

    // After the copy, the message must be in the Inbox and in the new folder.
    sync.file_message(&uid, &Filing::copied_into(folder.id.clone()))
        .expect("copying the message failed against the real server");
    assert!(
        lists(&sync, &inbox_id, &uid),
        "the message should still be in the Inbox after a copy"
    );
    assert!(
        lists(&sync, &folder.id, &uid),
        "the message should be filed in the new folder after MailSync::file_message"
    );

    // Move the message out of the Inbox in one patch. The new folder must
    // keep it.
    sync.file_message(&uid, &Filing::moved(inbox_id.clone(), folder.id.clone()))
        .expect("moving the message failed against the real server");
    assert!(
        !lists(&sync, &inbox_id, &uid),
        "the message should have left the Inbox after MailSync::file_message moved it out"
    );
    assert!(
        lists(&sync, &folder.id, &uid),
        "the message should still be filed in the new folder after the move"
    );

    sync.expunge_message(&uid, &folder.id)
        .expect("expunging the message failed against the real server");
    sync.delete_folder(&folder.id)
        .expect("Mailbox/set destroy failed against the real server");
}
