// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `MailSync::import_message` and `MailSync::expunge_message` against a
//! real JMAP server. `append_message_sync` and `expunge_sync` call these
//! functions.
//!
//! `jmap-client/tests/live_server.rs` tests `Email/import` and `Mailbox/set`
//! through `Client` directly. It does not test `MailSync`: the upload before
//! the import in `import_message`, or the mailbox-membership read before the
//! write in `expunge_message`. `tests/{import,expunge}.rs` test these
//! functions against `jmap-mock` only. This file is the live-server
//! counterpart, with the same structure as the `jmap-cal-sync`,
//! `jmap-book-sync` and `jmap-collection-sync` live-server tests.
//!
//! ## Running it
//!
//! See `docs/manual-test-live-server.md`. Set `JMAP_LIVE_SERVER_URL`,
//! `JMAP_LIVE_SERVER_WRITE_USER` and `JMAP_LIVE_SERVER_WRITE_PASSWORD`, then:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync -- --ignored
//! ```
//!
//! This crate has no `live-server` feature. `#[ignore]` keeps the test out of
//! a plain `cargo test`.
//!
//! The test skips when `JMAP_LIVE_SERVER_WRITE_USER` is unset. It panics when
//! that variable is set but `JMAP_LIVE_SERVER_WRITE_PASSWORD` or
//! `JMAP_LIVE_SERVER_URL` is unset.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{Keywords, MailSync};
use jmap_proto::mail::role;
use jmap_proto::session::CAPABILITY_MAIL;

/// Returns a value unique to this run, so that a message from a different run
/// cannot match.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Connects as the write-test account. Returns `None` when
/// `JMAP_LIVE_SERVER_WRITE_USER` is unset.
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

/// Imports a message into the Inbox, finds it with `MailSync::messages`, then
/// expunges it and makes sure that it is gone.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn importing_then_expunging_a_message_round_trips_through_the_real_server() {
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

    let subject = format!("agent-mailsync-{}", unique_suffix());
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

    let (_, present) = sync
        .messages(&inbox_id)
        .expect("listing the Inbox failed against the real server");
    assert!(
        present.iter().any(|summary| summary.uid == uid),
        "the newly imported message should be listed in the Inbox"
    );

    sync.expunge_message(&uid, &inbox_id)
        .expect("expunging the message failed against the real server");

    let (_, remaining) = sync
        .messages(&inbox_id)
        .expect("listing the Inbox failed after expunge");
    assert!(
        !remaining.iter().any(|summary| summary.uid == uid),
        "the expunged message should no longer be listed in the Inbox"
    );
}
