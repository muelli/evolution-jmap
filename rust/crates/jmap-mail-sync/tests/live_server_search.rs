// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `MailSync::search` against a real JMAP server. `jmap-mail` calls this
//! function (from `folder.rs` and `search_body.rs`) to send a Camel search
//! expression as an `Email/query`.
//!
//! `jmap-mail-sync/tests/search.rs` and `jmap-mail/tests/search.rs` test
//! `search` against `jmap-mock` only. `search` adds the mailbox scope to the
//! filter of the caller with an AND (see the doc comment of
//! `MailSync::search`). A mock cannot show that a real server applies this
//! AND, and does not change it to an OR or drop the scope.
//!
//! ## Running it
//!
//! See `docs/manual-test-live-server.md`. Set `JMAP_LIVE_SERVER_URL`,
//! `JMAP_LIVE_SERVER_WRITE_USER` and `JMAP_LIVE_SERVER_WRITE_PASSWORD`, then:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_search -- --ignored
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
use jmap_proto::mail::{EmailQueryFilter, role};
use jmap_proto::methods::Filter;
use jmap_proto::session::CAPABILITY_MAIL;

/// Returns a value unique to this run, so that a message or folder from a
/// different run cannot match.
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

/// Two messages in different folders have the same subject token. A search
/// of each folder for the token must find only the message in that folder.
/// This shows that the real server applies the AND of the mailbox scope and
/// the filter. The cleanup at the end runs only when all assertions pass.
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
