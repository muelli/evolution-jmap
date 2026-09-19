// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync::search` against a real JMAP server — the function
//! `jmap-mail`'s `folder.rs` calls to delegate a translatable Camel search
//! expression to `Email/query` (`docs/RFC-SUPPORT.md` item 1), exercised end
//! to end for the first time.
//!
//! `tests/live_server.rs` covers `import_message`/`expunge_message` and
//! `tests/live_server_folder.rs` covers `create_folder`/`delete_folder`, but
//! nothing had driven `search` itself against a real server: only
//! `jmap-mail/tests/search.rs` exercises it, and only against `jmap-mockd`.
//! What a mock cannot confirm is that `search`'s own AND — the mailbox scope
//! folded in alongside the caller's filter (`MailSync::search`'s doc comment)
//! — actually holds on a real server's `Email/query`, rather than the two
//! conditions silently becoming an OR or the scope being dropped. This file
//! is `search`'s live-server counterpart, following the same recipe as the
//! other files in this suite.
//!
//! ## Running it
//!
//! Same environment as the other `jmap-mail-sync` live-server tests — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_search -- --ignored
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
use jmap_mail_sync::{Keywords, MailSync};
use jmap_proto::mail::{EmailQueryFilter, role};
use jmap_proto::methods::Filter;
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

/// Imports a message with `subject` into `mailbox`.
fn import(sync: &MailSync, mailbox: &jmap_proto::Id, subject: &str) -> jmap_proto::Id {
    let message = format!(
        "From: agent-mailsync@example.invalid\r\n\
         To: agent-mailsync@example.invalid\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         It arrived through MailSync::import_message.\r\n"
    );
    sync.import_message(mailbox, message.into_bytes(), &Keywords::default(), None)
        .expect("Email/import failed against the real server")
}

/// Two messages share one distinctive subject token but live in different
/// folders. Searching either folder for that token should surface only the
/// message actually filed there — proving `search`'s mailbox scope and the
/// caller's filter are ANDed together on a real server, not just in the
/// mock's own translation of the request.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn searching_a_folder_finds_only_messages_filed_there_on_the_real_server() {
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
    let folder_name = format!("agent-mailsync-search-{suffix}");
    let folder = sync
        .create_folder(None, &folder_name)
        .expect("Mailbox/set create failed against the real server");

    let token = format!("agent-mailsync-search-token-{suffix}");
    let in_inbox = import(&sync, &inbox_id, &format!("{token} inbox copy"));
    let in_folder = import(&sync, &folder.id, &format!("{token} folder copy"));

    let found_in_inbox = sync
        .search(
            &inbox_id,
            Filter::condition(EmailQueryFilter::default().subject(token.clone())),
        )
        .expect("Email/query failed against the real server");
    assert!(
        found_in_inbox.contains(&in_inbox),
        "searching the Inbox for the shared token should find the message filed there"
    );
    assert!(
        !found_in_inbox.contains(&in_folder),
        "searching the Inbox for the shared token should not find the message filed in the other folder"
    );

    let found_in_folder = sync
        .search(
            &folder.id,
            Filter::condition(EmailQueryFilter::default().subject(token.clone())),
        )
        .expect("Email/query failed against the real server");
    assert!(
        found_in_folder.contains(&in_folder),
        "searching the new folder for the shared token should find the message filed there"
    );
    assert!(
        !found_in_folder.contains(&in_inbox),
        "searching the new folder for the shared token should not find the message filed in the Inbox"
    );

    sync.expunge_message(&in_inbox, &inbox_id)
        .expect("expunging the Inbox message failed against the real server");
    sync.expunge_message(&in_folder, &folder.id)
        .expect("expunging the folder message failed against the real server");
    sync.delete_folder(&folder.id)
        .expect("Mailbox/set destroy failed against the real server");
}
