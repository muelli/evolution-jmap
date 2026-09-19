// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync` reading and writing a *shared* mailbox's actual message
//! content, i.e. one owned by a different real account, through an ACL
//! grant.
//!
//! `jmap-client/tests/live_server.rs`'s
//! `mailbox_shared_with_a_second_real_account_is_visible_only_after_the_grant`
//! already proves the grant changes what `Mailbox/get` *lists* (visibility
//! and `myRights`), but never calls `Email/query`/`Email/set` through the
//! granted account to see whether the shared mailbox's actual messages are
//! readable, let alone writable once `mayAddItems` is granted. This is
//! exactly the path a real Evolution session takes once a shared mailbox is
//! added as a source, so it is worth pinning at the `MailSync` level, the
//! same way `jmap-book-sync/tests/live_server_shared_book.rs` and
//! `jmap-cal-sync/tests/live_server_shared_calendar.rs` already did for a
//! shared address book and a shared calendar.
//!
//! ## A real Stalwart limitation this test works around, not a client bug
//!
//! The obvious write to test would be `MailSync::import_message` — the
//! function `append_message_sync` calls — the same way the address book and
//! calendar sibling tests use `save_contact`/`save_component`. Confirmed
//! against real Stalwart while writing this test: `import_message` always
//! uploads its blob to `uploadUrl` with the *target* account id (RFC 8620
//! §6.1, `jmap-mail-sync/src/lib.rs`'s own `import_message` doc comment),
//! and Stalwart accepts an upload to an account the caller only has an
//! item-level ACL grant on — HTTP 201, a real `blobId` — but never actually
//! stores it there: `Email/import` referencing that same `blobId` moments
//! later answers `blobNotFound`, and `Blob/copy` into that account answers
//! `forbidden: You are not an owner of account`. So on Stalwart, no grant at
//! all lets a second account author a brand-new message into a shared
//! mailbox; only an account's owner can put a new blob in it. This looks
//! like a deliberate scoping choice (Stalwart's blob store is keyed to
//! account ownership, sidestepping this crate's own ACL model entirely)
//! rather than a bug, and there is nothing to fix in this crate: Evolution's
//! `append_message_sync` has no wider blob-copy path to fall back to,
//! because it is handed raw bytes, not another account's existing email id.
//!
//! What *does* work under `mayAddItems` alone, confirmed the same way: an
//! account's own existing message can be filed into a mailbox it does not
//! yet belong to. `MailSync::file_message`/`Filing::copied_into` patches
//! only `mailboxIds` (`Email/set` update, no blob involved at all), which
//! Stalwart accepts from a grantee with `mayAddItems` on the destination
//! mailbox and nothing else. This is also the more faithful test of what a
//! real Evolution session does with a shared mailbox day to day — dragging
//! an already-received message from one folder into a shared one — so this
//! test exercises reading via `MailSync::messages` and writing via
//! `MailSync::file_message`, not `import_message`.
//!
//! Before any grant, the real server was observed answering both ways for
//! address books and calendars: a method-level `forbidden`, and (more often)
//! a successful but empty listing. This test tolerates either for `Email`,
//! the same way.
//!
//! ## Running it
//!
//! Needs both `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` and
//! `JMAP_LIVE_SERVER_RECIPIENT_USER`/`_PASSWORD` from
//! `docs/manual-test-live-server.md`, since this is inherently a two-account
//! test: the write-test account owns the shared mailbox, the recipient
//! account is granted access to it.
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync -- --ignored
//! ```
//!
//! Skipped, not failed, when either pair is unset.

use std::env;

use jmap_client::{Client, Credentials, Error as ClientError};
use jmap_mail_sync::{Filing, Keywords, MailSync, SyncError};
use jmap_proto::mail::Mailbox;
use jmap_proto::principals::PrincipalQueryFilter;
use jmap_proto::session::{CAPABILITY_MAIL, CAPABILITY_PRINCIPALS};
use serde_json::json;

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-book-sync/tests/live_server_shared_book.rs::connect_owner`.
fn connect_owner() -> Option<(Client, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user.clone(), password))
        .expect("could not fetch the session document for the write-test account");
    Some((client, user))
}

/// Mirrors `jmap-book-sync/tests/live_server_shared_book.rs::connect_recipient`.
fn connect_recipient() -> Option<(Client, String)> {
    let user = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_RECIPIENT_PASSWORD").expect(
        "JMAP_LIVE_SERVER_RECIPIENT_USER is set but JMAP_LIVE_SERVER_RECIPIENT_PASSWORD is not",
    );
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_RECIPIENT_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user.clone(), password))
        .expect("could not fetch the session document for the recipient account");
    Some((client, user))
}

/// A missing grant can surface two ways: a whole method refused outright
/// (no query/get rights at all, RFC 8620 §3.6.2), or an update let through
/// method dispatch but rejected per-record (query/get rights but no write,
/// RFC 8620 §5.3) — `file_message`'s `mailboxIds` patch hits the latter.
fn is_forbidden(error: &SyncError) -> bool {
    matches!(
        error,
        SyncError::Client(ClientError::Method(method_error))
            if method_error.error_type == "forbidden"
    ) || matches!(
        error,
        SyncError::Client(ClientError::Set(set_error))
            if set_error.error_type == "forbidden"
    )
}

/// Whether the recipient can see the owner's message through
/// `recipient_sync` (targeting the not-yet-or-no-longer-shared mailbox), the
/// same way `jmap-book-sync/tests/live_server_shared_book.rs`'s
/// `assert_owner_contact_not_visible` treats an absent grant: the real
/// server answers either shape (a method-level `forbidden`, or a successful
/// but empty listing), and both mean the same thing.
fn assert_owner_message_not_visible(
    recipient_sync: &MailSync,
    mailbox_id: &jmap_proto::Id,
    owner_uid: &jmap_proto::Id,
    when: &str,
) {
    match recipient_sync.messages(mailbox_id) {
        Err(error) => assert!(
            is_forbidden(&error),
            "expected forbidden {when}, got: {error:?}"
        ),
        Ok((_, messages)) => assert!(
            messages.iter().all(|summary| &summary.uid != owner_uid),
            "the owner's message should not be visible {when}: {:?}",
            messages.iter().map(|m| &m.uid).collect::<Vec<_>>()
        ),
    }
}

fn message(subject: &str) -> Vec<u8> {
    format!(
        "From: agent-mailsync-shared@example.invalid\r\n\
         To: agent-mailsync-shared@example.invalid\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         It arrived through a shared mailbox.\r\n"
    )
    .into_bytes()
}

/// Creates a fresh mailbox to share plus a second, unshared staging mailbox
/// on the owner account, and a message in each. Drives the recipient
/// account's own `MailSync` against the owner's account id through three
/// stages: forbidden before any grant, read-only once `mayReadItems` is
/// granted (sees the owner's message in the shared mailbox, but filing the
/// staging message into it is still refused), and read-write once the grant
/// widens to include `mayAddItems` (can file the staging message into the
/// shared mailbox, which the owner then sees too). Destroys both mailboxes
/// (and everything in them) at the end regardless of where the assertions
/// land.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_shared_mailboxs_messages_are_readable_then_writable_as_the_grant_widens() {
    let Some((owner, _owner_address)) = connect_owner() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the shared mailbox test"
        );
        return;
    };
    let Some((recipient, recipient_address)) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the shared mailbox test"
        );
        return;
    };
    if !recipient
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!(
            "server does not advertise {CAPABILITY_PRINCIPALS}; skipping the shared mailbox test"
        );
        return;
    }

    let owner_account_id = owner
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    let recipient_principal_id = owner
        .principal_query(
            &owner_account_id,
            PrincipalQueryFilter::email(&recipient_address),
        )
        .expect("Principal/query failed against the real server")
        .into_iter()
        .next()
        .expect("the owner cannot resolve the recipient's principal id by email");

    let suffix = unique_suffix();
    let mailbox = owner
        .mailbox_create(
            &owner_account_id,
            &Mailbox {
                name: format!("agent-mailsync-shared-{suffix}"),
                ..Mailbox::default()
            },
        )
        .expect("Mailbox/set create failed against the real server");
    let mailbox_id = mailbox
        .id
        .clone()
        .expect("the server named the new mailbox");

    let staging = owner
        .mailbox_create(
            &owner_account_id,
            &Mailbox {
                name: format!("agent-mailsync-shared-staging-{suffix}"),
                ..Mailbox::default()
            },
        )
        .expect("Mailbox/set create failed against the real server (staging)");
    let staging_id = staging
        .id
        .clone()
        .expect("the server named the new staging mailbox");

    let owner_sync = MailSync::new(owner, owner_account_id.clone());

    let owner_uid = owner_sync
        .import_message(
            &mailbox_id,
            message(&format!("agent-mailsync-shared-owner-{suffix}")),
            &Keywords::default(),
            None,
        )
        .expect("Email/import failed against the real server");

    let staged_uid = owner_sync
        .import_message(
            &staging_id,
            message(&format!("agent-mailsync-shared-staged-{suffix}")),
            &Keywords::default(),
            None,
        )
        .expect("Email/import failed against the real server (staging)");

    let recipient_sync = MailSync::new(recipient, owner_account_id.clone());

    assert_owner_message_not_visible(&recipient_sync, &mailbox_id, &owner_uid, "before any grant");

    owner_sync
        .client()
        .mailbox_update(
            &owner_account_id,
            &mailbox_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayReadItems": true}}}),
        )
        .expect("Mailbox/set shareWith failed against the real server");

    let (_, listed) = recipient_sync
        .messages(&mailbox_id)
        .expect("a mayReadItems grant should let the recipient list the shared mailbox");
    assert!(
        listed.iter().any(|summary| summary.uid == owner_uid),
        "the owner's message should be visible through the shared mailbox: {:?}",
        listed.iter().map(|m| &m.uid).collect::<Vec<_>>()
    );

    let file_without_grant = recipient_sync
        .file_message(&staged_uid, &Filing::copied_into(mailbox_id.clone()))
        .expect_err("a mayReadItems-only grant must not allow filing a message into the mailbox");
    assert!(
        is_forbidden(&file_without_grant),
        "expected forbidden for a write under a read-only grant, got: {file_without_grant:?}"
    );

    owner_sync
        .client()
        .mailbox_update(
            &owner_account_id,
            &mailbox_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayReadItems": true, "mayAddItems": true}}}),
        )
        .expect("Mailbox/set shareWith widen failed against the real server");

    recipient_sync
        .file_message(&staged_uid, &Filing::copied_into(mailbox_id.clone()))
        .expect(
            "a mayAddItems grant should let the recipient file a message into the shared mailbox",
        );

    let (_, listed_after_write) = owner_sync
        .messages(&mailbox_id)
        .expect("listing the mailbox as its owner failed after the recipient's write");
    assert!(
        listed_after_write
            .iter()
            .any(|summary| summary.uid == staged_uid),
        "the message the recipient filed should be visible to the owner too: {:?}",
        listed_after_write
            .iter()
            .map(|m| &m.uid)
            .collect::<Vec<_>>()
    );

    owner_sync.expunge_message(&staged_uid, &staging_id).expect(
        "expunging the staged message from staging failed against the real server (cleanup)",
    );
    owner_sync
        .expunge_message(&staged_uid, &mailbox_id)
        .expect("expunging the staged message from the shared mailbox failed against the real server (cleanup)");
    owner_sync
        .expunge_message(&owner_uid, &mailbox_id)
        .expect("expunging the owner's message failed against the real server (cleanup)");
    owner_sync
        .client()
        .mailbox_destroy(&owner_account_id, &staging_id)
        .expect("Mailbox/set destroy failed against the real server (cleanup, staging)");
    owner_sync
        .client()
        .mailbox_destroy(&owner_account_id, &mailbox_id)
        .expect("Mailbox/set destroy failed against the real server (cleanup)");
}
