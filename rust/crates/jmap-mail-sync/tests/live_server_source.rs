// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync::message_source` against a real JMAP server — the function
//! `jmap-mail`'s Camel folder calls to fetch a message's raw RFC 5322 bytes
//! for the reading pane, exercised end to end for the first time.
//!
//! This is the exact sync-layer function behind item 9's Fastmail
//! blob-download saga (redirect-auth, the `Accept` header, the
//! `REBASE_URLS` cross-account-poisoning root cause) — everything that
//! closed that item was either a raw `jmap-client`-level probe/example or
//! the operator's own real Evolution session; no automated regression test
//! has ever driven `MailSync::message_source` itself against a real server.
//! Only `jmap-mockd` has ever exercised it (`jmap-mail-sync/tests/
//! source.rs`); this file is its live-server counterpart, following the
//! same recipe as `live_server.rs`'s `import_message`/`expunge_message`
//! test.
//!
//! ## Running it
//!
//! Same environment as this crate's other live-server tests — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_source -- --ignored
//! ```
//!
//! No `--features live-server` gate is needed here — like this crate's
//! other live-server files, `#[ignore]` alone already keeps it out of a
//! plain `cargo test`.
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

/// Mirrors `live_server.rs::connect_for_write` exactly.
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

/// Imports a message into the Inbox via `MailSync::import_message`, fetches
/// its raw bytes back via `MailSync::message_source`, confirms the RFC 5322
/// headers and body round-trip byte-for-byte (the same shape `tests/
/// source.rs::the_source_of_a_message_is_the_rfc_5322_bytes_the_server_holds`
/// checks against the mock), then expunges it.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn message_source_returns_the_exact_bytes_imported_against_the_real_server() {
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

    let subject = format!("agent-mailsync-source-{}", unique_suffix());
    let message = format!(
        "From: agent-mailsync@example.invalid\r\n\
         To: agent-mailsync@example.invalid\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         It arrived through MailSync::message_source.\r\n"
    );

    let uid = sync
        .import_message(&inbox_id, message.into_bytes(), &Keywords::default(), None)
        .expect("Email/import failed against the real server");

    let source = sync
        .message_source(&uid)
        .expect("message_source failed against the real server");
    let source = String::from_utf8(source).expect("the server serves a text message");

    assert!(
        source.contains(&format!("Subject: {subject}")),
        "missing the imported subject: {source}"
    );
    assert!(
        source.contains("It arrived through MailSync::message_source."),
        "missing the imported body: {source}"
    );
    let (headers, body) = source
        .split_once("\r\n\r\n")
        .expect("a header/body split, same shape as the mock's own answer");
    assert!(
        !headers.contains("It arrived through"),
        "the body leaked into the header block: {source}"
    );
    assert!(
        body.contains("It arrived through MailSync::message_source."),
        "the body did not arrive whole: {body}"
    );

    sync.expunge_message(&uid, &inbox_id)
        .expect("expunging the message failed against the real server");
}
