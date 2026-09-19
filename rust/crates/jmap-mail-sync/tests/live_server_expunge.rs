// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync::expunge_message`'s multi-mailbox branch against a real JMAP
//! server.
//!
//! `expunge_message` reads a message's `mailboxIds` first and chooses
//! between two different writes: `Email/destroy` when this mailbox is the
//! message's only home, or an `Email/set` update stripping just this
//! mailbox's membership when another one remains (RFC 8621 section 4.6).
//! `tests/live_server.rs` and `tests/live_server_filing.rs` only ever
//! expunge a message that is filed in exactly one mailbox at the time, so
//! only the destroy branch has ever run against a real server; the update
//! branch is otherwise covered only against `jmap-mock`
//! (`tests/expunge.rs`), which cannot prove a real server's `Email/set`
//! response for a `mailboxIds/<id>: null` patch parses the way this crate
//! expects. This is that branch's live-server counterpart.
//!
//! ## Running it
//!
//! Same environment as the other `jmap-mail-sync` live-server tests — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_expunge -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset — the same tolerance every write-path test in this repository
//! gives an unconfigured environment.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{Filing, Keywords, MailSync};
use jmap_proto::Id;
use jmap_proto::mail::role;
use jmap_proto::session::CAPABILITY_MAIL;

/// A value unique to this process invocation, so a concurrent or prior run's
/// leftover message/folder can never be mistaken for this run's own.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-cal-sync/tests/live_server.rs::connect_for_write` exactly.
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

/// Whether `mailbox` lists `uid` among its messages.
fn lists(sync: &MailSync, mailbox: &Id, uid: &Id) -> bool {
    let (_, summaries) = sync
        .messages(mailbox)
        .expect("listing a mailbox failed against the real server");
    summaries.iter().any(|summary| &summary.uid == uid)
}

/// Copies a message into a second folder, then expunges it from the Inbox
/// while it is still filed in both. The message must survive in the second
/// folder and disappear from the Inbox, driving `expunge_message`'s
/// `Email/set` update branch rather than its destroy branch.
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

    // Copy it into the new folder — the message is now filed in both.
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

    // Expunge it from the Inbox while it is still filed in the new folder
    // too: this must take the `Email/set` update branch, not destroy.
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
