// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `MailSync::messages_since` against a real JMAP server. This is the
//! `Email/changes` delta path that `refresh_info_sync` in `jmap-mail` uses
//! for an incremental refresh. A full `messages` listing is the fallback.
//!
//! The other live-server tests of this crate check the result with
//! `messages` only. `jmap-mail-sync/tests/updates.rs` tests the present and
//! absent classification against `jmap-mock`. This file is its live-server
//! counterpart, with the same structure as `live_server_changes.rs` in
//! `jmap-book-sync` and `jmap-cal-sync`.
//!
//! ## Running it
//!
//! See `docs/manual-test-live-server.md`. Set `JMAP_LIVE_SERVER_URL`,
//! `JMAP_LIVE_SERVER_WRITE_USER` and `JMAP_LIVE_SERVER_WRITE_PASSWORD`, then:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_changes -- --ignored
//! ```
//!
//! This crate has no `live-server` feature. `#[ignore]` keeps the test out of
//! a plain `cargo test`.
//!
//! The tests skip when `JMAP_LIVE_SERVER_WRITE_USER` is unset. They panic when
//! that variable is set but `JMAP_LIVE_SERVER_WRITE_PASSWORD` or
//! `JMAP_LIVE_SERVER_URL` is unset. The cross-account test also skips when
//! `JMAP_LIVE_SERVER_RECIPIENT_USER` (a second account in the same domain) is
//! unset, and panics when only `JMAP_LIVE_SERVER_RECIPIENT_PASSWORD` is unset.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{KeywordChange, Keywords, MailSync, MessageFlags, MessageUpdate};
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

/// Same as `connect_recipient` in `jmap-book-sync/tests/live_server_changes.rs`.
fn connect_recipient() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_RECIPIENT_PASSWORD").expect(
        "JMAP_LIVE_SERVER_RECIPIENT_USER is set but JMAP_LIVE_SERVER_RECIPIENT_PASSWORD is not",
    );
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_RECIPIENT_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    Some(
        Client::builder()
            .rebase_urls_to_origin(rebase)
            .connect(&origin, Credentials::basic(user, password))
            .expect("could not fetch the session document for the recipient account"),
    )
}

/// Returns the id of the mailbox with the Inbox role. Panics when there is no
/// Inbox.
fn inbox_id(client: &Client, account_id: &jmap_proto::Id) -> jmap_proto::Id {
    client
        .mailbox_get(account_id)
        .unwrap()
        .list
        .into_iter()
        .find(|mailbox| mailbox.role.as_deref() == Some(role::INBOX))
        .expect("the account needs an Inbox")
        .id
        .expect("the server named the Inbox")
}

/// Returns the delta. Panics when the update is not `MessageUpdate::Changed`.
fn changed(
    update: MessageUpdate,
) -> (
    jmap_proto::State,
    Vec<jmap_mail_sync::MessageSummary>,
    Vec<jmap_proto::Id>,
) {
    match update {
        MessageUpdate::Changed {
            state,
            present,
            absent,
        } => (state, present, absent),
        MessageUpdate::Unchanged(state) => {
            panic!("expected a delta, the server reported nothing new at {state}")
        }
        MessageUpdate::Relisted { state, .. } => {
            panic!("expected a delta, the mailbox was listed again at {state}")
        }
    }
}

/// Imports, flags and expunges a message. After each step, `messages_since`
/// from the previous state must report the change: present after the import
/// and the edit, absent after the expunge. `tests/updates.rs` makes the same
/// assertions against `jmap-mock`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn messages_since_reports_a_create_an_edit_and_a_removal_against_the_real_server() {
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

    let (state_before_create, held) = sync
        .messages(&inbox_id)
        .expect("listing the Inbox failed before the import");

    let subject = format!("agent-mailsync-changes-{}", unique_suffix());
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

    let (state_after_create, present, absent) = changed(
        sync.messages_since(&inbox_id, &state_before_create, held.len())
            .expect("messages_since after the import failed against the real server"),
    );
    assert!(
        present.iter().any(|summary| summary.uid == uid),
        "the newly imported message should be reported present since before it existed: {:?}",
        present
            .iter()
            .map(|summary| &summary.uid)
            .collect::<Vec<_>>()
    );
    assert!(
        !absent.contains(&uid),
        "a brand-new message must not also be reported absent"
    );

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

    let (state_after_edit, present, _) = changed(
        sync.messages_since(&inbox_id, &state_after_create, held.len() + 1)
            .expect("messages_since after the edit failed against the real server"),
    );
    let edited = present
        .iter()
        .find(|summary| summary.uid == uid)
        .unwrap_or_else(|| {
            panic!(
                "the edited message should be reported present since right after its import: {:?}",
                present
                    .iter()
                    .map(|summary| &summary.uid)
                    .collect::<Vec<_>>()
            )
        });
    assert!(
        edited.flags.flagged,
        "the message reported by messages_since should carry the edit"
    );

    sync.expunge_message(&uid, &inbox_id)
        .expect("expunging the message failed against the real server");

    let (_, present, absent) = changed(
        sync.messages_since(&inbox_id, &state_after_edit, held.len() + 1)
            .expect("messages_since after the removal failed against the real server"),
    );
    assert!(
        absent.contains(&uid),
        "the expunged message should be reported absent since right after its edit: {absent:?}"
    );
    assert!(
        !present.iter().any(|summary| summary.uid == uid),
        "an expunged message must not also be reported present"
    );
}

/// Gives `messages_since` a state from a different account. The mailbox must
/// be listed again (`MessageUpdate::Relisted`).
///
/// RFC 8620 makes a state opaque. Stalwart rejects most hand-written states
/// with `invalidArguments` (see `jmap-book-sync/tests/live_server_changes.rs`).
/// A valid state from a different account causes `cannotCalculateChanges`.
///
/// `BookSync::get_changes` and `CalSync::get_changes` return that error to
/// the caller. `MailSync::messages_since` catches it and lists the mailbox
/// again. This test makes sure that the fallback occurs on the real error,
/// not only on the mock error of `jmap-mock/src/setops.rs`.
///
/// The test does not expunge the message that it imports.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn messages_since_relists_the_mailbox_for_another_accounts_state() {
    let Some(owner) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let Some(other) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the write-path test"
        );
        return;
    };

    let owner_account_id = owner
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let owner_inbox_id = inbox_id(&owner, &owner_account_id);
    let owner_sync = MailSync::new(owner, owner_account_id);

    // A live check showed that the "no changes yet" state of a new account
    // is valid for each other new account too. An import moves the owner to
    // a position in its own changelog. The other account does not have that
    // position, thus the server returns cannotCalculateChanges.
    let subject = format!("agent-mailsync-cannotcalc-{}", unique_suffix());
    let message = format!(
        "From: agent-mailsync@example.invalid\r\n\
         To: agent-mailsync@example.invalid\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         It exists only to advance the owner's state.\r\n"
    );
    owner_sync
        .import_message(
            &owner_inbox_id,
            message.into_bytes(),
            &Keywords::default(),
            None,
        )
        .expect("Email/import failed against the real server");
    let (owner_state, _) = owner_sync
        .messages(&owner_inbox_id)
        .expect("listing the owner's Inbox failed against the real server");

    let other_account_id = other
        .primary_account(CAPABILITY_MAIL)
        .expect("the recipient account needs the mail capability");
    let other_inbox_id = inbox_id(&other, &other_account_id);
    let other_sync = MailSync::new(other, other_account_id);

    let update = other_sync
        .messages_since(&other_inbox_id, &owner_state, 0)
        .expect("messages_since should relist rather than propagate cannotCalculateChanges");
    assert!(
        matches!(update, MessageUpdate::Relisted { .. }),
        "expected the mailbox relisted for another account's state, got: {update:?}"
    );
}
