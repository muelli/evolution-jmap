// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `MailSync::set_keywords` against a real JMAP server. When a folder
//! synchronizes, `synchronize_sync` in `jmap-mail` sends the flag changes of
//! the user through this function.
//!
//! `jmap-mail-sync/tests/keywords.rs` tests the diff of `KeywordChange`
//! against `jmap-mock` only. Marking a message as read, starred or flagged
//! is the most frequent write of a mail client. This file tests it against a
//! real server.
//!
//! ## Running it
//!
//! See `docs/manual-test-live-server.md`. Set `JMAP_LIVE_SERVER_URL`,
//! `JMAP_LIVE_SERVER_WRITE_USER` and `JMAP_LIVE_SERVER_WRITE_PASSWORD`, then:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_keywords -- --ignored
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
use jmap_mail_sync::{KeywordChange, Keywords, MailSync, MessageFlags};
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

/// Imports a message, flags it, then removes the flag and marks it seen in a
/// second change. `MailSync::messages` checks the flags after each change.
///
/// The test shows that a keyword change gets to the server, and that the
/// second change also removes the flag. The cleanup at the end runs only when
/// all assertions pass.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn setting_keywords_on_a_message_reaches_the_real_server() {
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

    let subject = format!("agent-mailsync-keywords-{}", unique_suffix());
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

    let flags_of = |sync: &MailSync| -> MessageFlags {
        let (_, summaries) = sync
            .messages(&inbox_id)
            .expect("listing the Inbox failed against the real server");
        summaries
            .into_iter()
            .find(|summary| summary.uid == uid)
            .expect("the message should still be listed in the Inbox")
            .flags
    };

    // Flag it.
    let flagged = Keywords::new(
        &MessageFlags {
            flagged: true,
            ..MessageFlags::default()
        },
        &[],
    );
    let change = KeywordChange::between(&Keywords::default(), &flagged);
    sync.set_keywords(&uid, &change)
        .expect("setting keywords failed against the real server");

    let after_first_change = flags_of(&sync);
    assert!(
        after_first_change.flagged,
        "the message should be flagged after MailSync::set_keywords"
    );
    assert!(
        !after_first_change.seen,
        "the message should not be seen yet"
    );

    // Remove the flag and set seen in one patch. The diff must contain the
    // removal and the addition.
    let seen = Keywords::new(
        &MessageFlags {
            seen: true,
            ..MessageFlags::default()
        },
        &[],
    );
    let change = KeywordChange::between(&flagged, &seen);
    sync.set_keywords(&uid, &change)
        .expect("the second keyword change failed against the real server");

    let after_second_change = flags_of(&sync);
    assert!(
        after_second_change.seen,
        "the message should be seen after the second MailSync::set_keywords"
    );
    assert!(
        !after_second_change.flagged,
        "the earlier flagged keyword should have been cleared, not left behind"
    );

    sync.expunge_message(&uid, &inbox_id)
        .expect("expunging the message failed against the real server");
}
