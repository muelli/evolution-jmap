// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync::messages_since` against a real JMAP server — the
//! `Email/changes` state-token delta path that `get_folder_info_sync`'s
//! sibling, a folder's own summary refresh, actually uses for incremental
//! sync (a full `messages` re-listing is the fallback, not the common case).
//!
//! `jmap-mail-sync/tests/live_server.rs`, `live_server_keywords.rs`, and
//! `live_server_filing.rs` already prove import/expunge, keyword changes, and
//! filing round-trip against real Stalwart, but nothing there drives
//! `messages_since` itself — every assertion goes through `messages` (a full
//! listing) instead. `jmap-mail-sync/tests/updates.rs` proves the
//! present/absent classification logic against `jmap-mockd`; this file is its
//! live-server counterpart, following the same recipe as
//! `jmap-book-sync`/`jmap-cal-sync`'s own `live_server_changes.rs`.
//!
//! ## Running it
//!
//! Same environment as the other `jmap-mail-sync` live-server tests — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_changes -- --ignored
//! ```
//!
//! No `--features live-server` gate is needed here — like the other files,
//! this crate has no such feature, and `#[ignore]` alone already keeps it
//! out of a plain `cargo test`.
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset — the same tolerance every write-path test in this repository
//! gives an unconfigured environment. The cross-account test below also
//! needs `JMAP_LIVE_SERVER_RECIPIENT_USER`/`_PASSWORD` for a second account
//! in the same domain.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{KeywordChange, Keywords, MailSync, MessageFlags, MessageUpdate};
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

/// Mirrors `jmap-book-sync/tests/live_server_changes.rs::connect_recipient`
/// exactly.
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

/// The mailbox found by looking for its role, for a client whose account
/// needs an Inbox.
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

/// The delta, for a call that expects the mailbox to have moved.
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

/// Imports a message into the Inbox, then confirms `messages_since` reports
/// it as present from the state captured just before the import; sets a
/// keyword on it and confirms the edit shows up from the post-import state;
/// expunges it and confirms the removal shows up from the post-edit state.
/// Mirrors `tests/updates.rs`'s assertions, now against the real server
/// rather than `jmap-mockd`.
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

/// A state is opaque per RFC 8620, but not free-form: Stalwart's own state
/// strings fail syntax validation (`invalidArguments`) for almost any
/// hand-written guess, confirmed live in
/// `jmap-book-sync/tests/live_server_changes.rs`. A syntactically well-formed
/// state from a *different* account is the reliable way to reach
/// `cannotCalculateChanges` on the real server. Unlike `BookSync::get_changes`
/// and `CalSync::get_changes`, which propagate that error to the caller,
/// `MailSync::messages_since` catches it internally
/// (`SyncError::is_cannot_calculate_changes`) and relists the mailbox
/// instead, so this confirms the fallback actually fires on the real error
/// rather than assuming the mock's shape of it (`jmap-mock/src/setops.rs`)
/// matches.
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

    // A brand-new account's mailbox has an empty changelog, and the state
    // naming "no changes yet" turned out (checked live) to be accepted as
    // trivially valid for any other brand-new account too, since there is
    // nothing to diff against either way. Importing a message first gives
    // the owner a state that is a specific position in its own changelog,
    // which is what actually reaches cannotCalculateChanges when asked of an
    // account that never had that position.
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
