// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync::import_message`'s `received_at` parameter against a real JMAP
//! server.
//!
//! `tests/live_server.rs` already proves `import_message` round-trips
//! through real Stalwart, but every call site there passes `received_at:
//! None`. The one test that exercises a real value, `tests/import.rs`, only
//! runs against `jmap_mock::MockServer`, which just echoes back whatever
//! `receivedAt` it is handed rather than a real server independently
//! parsing and storing it. RFC 8621 section 4.8 lets a server default
//! `receivedAt` to the moment of import when the caller omits it, which is
//! exactly the wrong date for a message being copied between accounts
//! (`import_message`'s own doc comment explains why the date is sent at
//! all) — so whether real Stalwart actually keeps the client-supplied date
//! rather than silently defaulting had never been confirmed. This is that
//! confirmation.
//!
//! ## Running it
//!
//! Same environment as the other `jmap-mail-sync` live-server tests — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_received_at -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset — the same tolerance every write-path test in this repository
//! gives an unconfigured environment.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{Keywords, MailSync};
use jmap_proto::mail::role;
use jmap_proto::session::CAPABILITY_MAIL;

/// A value unique to this process invocation, so a concurrent or prior run's
/// leftover message can never be mistaken for this run's own.
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

/// Imports a message into the Inbox with a distinctive `received_at`, far
/// from the moment the test runs, and confirms the server hands that exact
/// value back rather than the time of import.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn an_imported_message_keeps_the_received_at_the_caller_gave_it() {
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

    // 2019-03-14T01:59:26Z, seconds since the epoch — nowhere near "now", so
    // a server silently defaulting to import time cannot pass by accident.
    let received_at: i64 = 1_552_528_766;
    let subject = format!("agent-mailrecvat-{}", unique_suffix());
    let message = format!(
        "From: agent-mailrecvat@example.invalid\r\n\
         To: agent-mailrecvat@example.invalid\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         It arrived through MailSync::import_message with a chosen date.\r\n"
    );

    let uid = sync
        .import_message(
            &inbox_id,
            message.into_bytes(),
            &Keywords::default(),
            Some(received_at),
        )
        .expect("Email/import failed against the real server");

    let (_, present) = sync
        .messages(&inbox_id)
        .expect("listing the Inbox failed against the real server");
    let row = present
        .iter()
        .find(|summary| summary.uid == uid)
        .expect("the newly imported message should be listed in the Inbox");
    assert_eq!(
        row.received_at,
        Some(received_at),
        "the real server should keep the caller's receivedAt rather than defaulting it"
    );

    sync.expunge_message(&uid, &inbox_id)
        .expect("expunging the message failed against the real server");
}
