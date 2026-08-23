// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync::set_keywords` against a real JMAP server — the function
//! `CamelFolder::set_message_flags` actually calls, exercised end to end for
//! the first time.
//!
//! `jmap-mail-sync/tests/live_server.rs` already proves
//! `import_message`/`expunge_message` against real Stalwart, and
//! `jmap-mail-sync/tests/keywords.rs` already proves `KeywordChange`'s own
//! diff logic against `jmap-mockd`, but nothing had driven `set_keywords`
//! itself against a real server: this is the single most-executed write in
//! an ordinary mail client's life (mark read, star, flag), and until now it
//! had never touched anything but the mock.
//!
//! ## Running it
//!
//! Same environment as the other `jmap-mail-sync` live-server tests — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_keywords -- --ignored
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
use jmap_mail_sync::{KeywordChange, Keywords, MailSync, MessageFlags};
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

/// Imports a message into the Inbox, flags it via `MailSync::set_keywords`,
/// confirms the flag via `MailSync::messages`, then flips flagged off and
/// seen on in a second change and confirms that too — proving both that a
/// keyword change reaches the server and that it names only what changed
/// (the unrelated flag from the first change does not linger or get
/// reasserted). Cleans up via `MailSync::expunge_message`.
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

    // Unflag it and mark it seen, in one patch — proves the diff names both
    // the removal and the addition, not just whichever came first.
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
