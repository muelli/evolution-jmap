// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync::file_message` against a real JMAP server — the function
//! `transfer_messages_to_sync` actually calls, exercised end to end for the
//! first time.
//!
//! `tests/live_server.rs` covers `import_message`/`expunge_message`,
//! `tests/live_server_folder.rs` covers `create_folder`/`delete_folder`, and
//! `tests/live_server_keywords.rs` covers `set_keywords` — but nothing had
//! driven `file_message` itself against a real server: only `jmap-mockd` has
//! ever exercised the `Filing::copied_into`/`Filing::moved` patches
//! (`tests/mailboxes.rs`, `tests/updates.rs`). This is their live-server
//! counterpart.
//!
//! ## Running it
//!
//! Same environment as the other `jmap-mail-sync` live-server tests — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_filing -- --ignored
//! ```
//!
//! No `--features live-server` gate is needed here — like the other files,
//! this crate has no such feature, and `#[ignore]` alone already keeps it
//! out of a plain `cargo test`.
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

/// Imports a message into the Inbox, creates a second folder, copies the
/// message into it via `MailSync::file_message` and confirms both mailboxes
/// now list it, then moves it out of the Inbox via a second `file_message`
/// call and confirms the Inbox no longer lists it while the new folder
/// still does. Cleans up via `expunge_message` and `delete_folder`.
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

    // Copy it into the new folder — the message should now be filed in both.
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

    // Move it out of the Inbox into the new folder, in one patch — the Inbox
    // should stop listing it while the new folder keeps it.
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
