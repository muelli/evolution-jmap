// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the `received_at` parameter of `MailSync::import_message` against a
//! real JMAP server.
//!
//! `tests/live_server.rs` gives `received_at: None` only. `tests/import.rs`
//! gives a real value, but `jmap_mock::MockServer` only returns the
//! `receivedAt` that it gets. RFC 8621 section 4.8 lets a server set
//! `receivedAt` to the import time when the caller omits it. That date is
//! wrong for a message that moves between accounts (see the doc comment of
//! `import_message`). This test makes sure that Stalwart keeps the date that
//! the client sends.
//!
//! ## Running it
//!
//! See `docs/manual-test-live-server.md`. Set `JMAP_LIVE_SERVER_URL`,
//! `JMAP_LIVE_SERVER_WRITE_USER` and `JMAP_LIVE_SERVER_WRITE_PASSWORD`, then:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_received_at -- --ignored
//! ```
//!
//! The test skips when `JMAP_LIVE_SERVER_WRITE_USER` is unset. It panics when
//! that variable is set but `JMAP_LIVE_SERVER_WRITE_PASSWORD` or
//! `JMAP_LIVE_SERVER_URL` is unset.

use jmap_mail_sync::{Keywords, MailSync};
use jmap_proto::mail::role;
use jmap_proto::session::CAPABILITY_MAIL;
use jmap_test_support::{connect_for_write, unique_suffix};

/// Imports a message with a `received_at` far from the current time, and
/// makes sure that the server returns that value, not the import time.
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

    // 2019-03-14T01:59:26Z in seconds since the epoch. A server that uses the
    // import time cannot pass by accident.
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
