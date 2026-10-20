// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the multi-mailbox branch of `MailSync::expunge_message` against a
//! real JMAP server.
//!
//! `expunge_message` reads the `mailboxIds` of the message, then selects one
//! of two writes. When this mailbox is the only mailbox of the message, it
//! sends `Email/destroy`. When a different mailbox remains, it sends an
//! `Email/set` update that removes only this mailbox (RFC 8621 section 4.6).
//!
//! `tests/live_server.rs` and `tests/live_server_filing.rs` expunge only
//! messages that are in one mailbox, thus they test only the destroy branch.
//! `tests/expunge.rs` tests the update branch against `jmap-mock` only. A mock
//! cannot show that this crate parses the `Email/set` response of a real
//! server to a `mailboxIds/<id>: null` patch. This file tests the update
//! branch against a real server.
//!
//! ## Running it
//!
//! See `docs/manual-test-live-server.md`. Set `JMAP_LIVE_SERVER_URL`,
//! `JMAP_LIVE_SERVER_WRITE_USER` and `JMAP_LIVE_SERVER_WRITE_PASSWORD`, then:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_expunge -- --ignored
//! ```
//!
//! The test skips when `JMAP_LIVE_SERVER_WRITE_USER` is unset. It panics when
//! that variable is set but `JMAP_LIVE_SERVER_WRITE_PASSWORD` or
//! `JMAP_LIVE_SERVER_URL` is unset.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{Filing, Keywords, MailSync};
use jmap_proto::Id;
use jmap_proto::mail::role;
use jmap_proto::session::CAPABILITY_MAIL;

/// Returns a value unique to this run, so that a message or folder from a
/// different run cannot match.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Same as `connect_for_write` in `live_server.rs`.
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

/// Returns `true` when `mailbox` lists `uid`.
fn lists(sync: &MailSync, mailbox: &Id, uid: &Id) -> bool {
    let (_, summaries) = sync
        .messages(mailbox)
        .expect("listing a mailbox failed against the real server");
    summaries.iter().any(|summary| &summary.uid == uid)
}

/// Copies a message into a second folder, then expunges it from the Inbox.
/// The message must stay in the second folder and leave the Inbox. This runs
/// the `Email/set` update branch of `expunge_message`, not the destroy branch.
/// The cleanup at the end runs only when all assertions pass.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn expunging_a_message_filed_in_two_mailboxes_only_removes_the_one_membership() {
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
    let folder_name = format!("agent-mailsync-expunge-{suffix}");
    let folder = sync
        .create_folder(None, &folder_name)
        .expect("Mailbox/set create failed against the real server");

    let subject = format!("agent-mailsync-expunge-{suffix}");
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

    // After the copy, the message is in the Inbox and in the new folder.
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

    // The message is also in the new folder, thus this expunge must use the
    // `Email/set` update branch, not destroy.
    sync.expunge_message(&uid, &inbox_id)
        .expect("expunging the message from the Inbox failed against the real server");
    assert!(
        !lists(&sync, &inbox_id, &uid),
        "the message should have left the Inbox after being expunged from it"
    );
    assert!(
        lists(&sync, &folder.id, &uid),
        "the message should still be filed in the new folder, since expunging \
         it from the Inbox while it was filed elsewhere too must not destroy it"
    );

    sync.expunge_message(&uid, &folder.id)
        .expect("expunging the message from the new folder failed against the real server");
    sync.delete_folder(&folder.id)
        .expect("Mailbox/set destroy failed against the real server");
}
