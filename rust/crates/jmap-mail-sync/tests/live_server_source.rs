// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `MailSync::message_source` against a real JMAP server. The Camel
//! folder of `jmap-mail` calls this function to get the raw RFC 5322 bytes
//! of a message for the reading pane.
//!
//! `jmap-mail-sync/tests/source.rs` tests this function against `jmap-mock`
//! only. This file is its live-server counterpart.
//!
//! ## Running it
//!
//! See `docs/manual-test-live-server.md`. Set `JMAP_LIVE_SERVER_URL`,
//! `JMAP_LIVE_SERVER_WRITE_USER` and `JMAP_LIVE_SERVER_WRITE_PASSWORD`, then:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_source -- --ignored
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

/// Imports a message into the Inbox and gets its raw bytes with
/// `MailSync::message_source`. The source must contain the subject header,
/// and the body must be after the first blank line. The test does not
/// compare all bytes. `tests/source.rs` does the same check against the mock.
/// The cleanup at the end runs only when all assertions pass.
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
