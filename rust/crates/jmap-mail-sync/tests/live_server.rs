// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync::import_message`/`expunge_message` against a real JMAP server —
//! the functions `append_message_sync`/`expunge_sync` actually call,
//! exercised end to end for the first time.
//!
//! `jmap-client/tests/live_server.rs` already proves `Email/import` and
//! `Mailbox/set` round-trip through `Client` directly, but nothing there
//! drives `MailSync` itself — the upload-then-import sequencing
//! `import_message` does, or `expunge_message`'s read-before-write
//! mailbox-membership decision. Only `jmap-mockd` has ever exercised this
//! crate's own functions (`jmap-mail-sync/tests/{import,expunge}.rs`); this
//! file is their live-server counterpart, following the same recipe as
//! `jmap-cal-sync`'s, `jmap-book-sync`'s, and `jmap-collection-sync`'s.
//!
//! ## Running it
//!
//! Same environment as the other sync crates' live-server tests — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up for
//! `jmap-client`'s write-path tests:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync -- --ignored
//! ```
//!
//! No `--features live-server` gate is needed here — like the other sync
//! crates, this crate has no such feature, and `#[ignore]` alone already
//! keeps it out of a plain `cargo test`.
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset — the same tolerance every write-path test in this repository gives
//! an unconfigured environment.

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

/// Imports a message into the Inbox via `MailSync::import_message`, confirms
/// it via `MailSync::messages`, then expunges it via
/// `MailSync::expunge_message` and confirms it is gone.
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
