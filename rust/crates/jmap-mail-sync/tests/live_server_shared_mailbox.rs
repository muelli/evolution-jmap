// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `MailSync` reads and writes of messages in a shared mailbox: a
//! mailbox of a different real account, with an ACL grant.
//!
//! `mailbox_shared_with_a_second_real_account_is_visible_only_after_the_grant`
//! in `jmap-client/tests/live_server.rs` shows that the grant changes the
//! `Mailbox/get` result (visibility and `myRights`). It does not read or write
//! the messages of the shared mailbox through the grantee. Evolution uses this
//! path when the user adds a shared mailbox. `live_server_shared_book.rs` in
//! `jmap-book-sync` and `live_server_shared_calendar.rs` in `jmap-cal-sync`
//! make the same checks for a shared address book and a shared calendar.
//!
//! ## A Stalwart limit (not a client bug)
//!
//! The address book and calendar tests write with `save_contact` and
//! `save_component`. The equivalent here is `MailSync::import_message`, which
//! `append_message_sync` calls. `import_message` uploads the blob to the
//! target account (RFC 8620 §6.1). A live check showed this result on
//! Stalwart, for a caller with only an item-level ACL grant:
//!
//! - The upload gives HTTP 201 and a `blobId`.
//! - `Email/import` with that `blobId` gives `blobNotFound`.
//! - `Blob/copy` into that account gives `forbidden: You are not an owner of
//!   account`.
//!
//! Thus on Stalwart, only the owner of an account can add a new blob, and no
//! grant lets a grantee import a new message into a shared mailbox. This crate
//! cannot correct this: `append_message_sync` gets raw bytes, not an existing
//! email id, thus no blob-copy fallback is possible.
//!
//! A live check showed that `mayAddItems` lets a grantee file an existing
//! message of the owner account into the shared mailbox.
//! `MailSync::file_message` with `Filing::copied_into` is an `Email/set`
//! update of `mailboxIds` only, with no blob. Thus this test reads with
//! `MailSync::messages` and writes with `MailSync::file_message`.
//!
//! Before a grant, the server gave two results for address books and
//! calendars: a method-level `forbidden`, or (more frequently) an empty
//! listing. This test accepts both results for `Email`.
//!
//! ## Running it
//!
//! See `docs/manual-test-live-server.md`. The write-test account owns the
//! shared mailbox, and the recipient account gets the grant. Set
//! `JMAP_LIVE_SERVER_URL`, `JMAP_LIVE_SERVER_WRITE_USER`,
//! `JMAP_LIVE_SERVER_WRITE_PASSWORD`, `JMAP_LIVE_SERVER_RECIPIENT_USER` and
//! `JMAP_LIVE_SERVER_RECIPIENT_PASSWORD`, then:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync -- --ignored
//! ```
//!
//! The test skips when `JMAP_LIVE_SERVER_WRITE_USER` or
//! `JMAP_LIVE_SERVER_RECIPIENT_USER` is unset, or when the server does not
//! advertise the principals capability. It panics when one of the two user
//! variables is set but its `_PASSWORD` variable or `JMAP_LIVE_SERVER_URL` is
//! unset.

use jmap_mail_sync::{Filing, Keywords, MailSync, SyncError};
use jmap_proto::mail::Mailbox;
use jmap_proto::principals::PrincipalQueryFilter;
use jmap_proto::session::{CAPABILITY_MAIL, CAPABILITY_PRINCIPALS};
use jmap_test_support::{connect_owner, connect_recipient_with_user, unique_suffix};
use serde_json::json;

/// Returns `true` for a `forbidden` method error or a `forbidden` set error.
///
/// With no read right, the server refuses the full method (RFC 8620 §3.6.2).
/// With a read right but no write right, the server refuses the record
/// (RFC 8620 §5.3). The `mailboxIds` patch of `file_message` gets the second
/// error.
fn is_forbidden(error: &SyncError) -> bool {
    matches!(error, SyncError::Client(error) if jmap_test_support::is_forbidden(error))
}

/// Asserts that `recipient_sync` cannot see the message of the owner in a
/// mailbox without a grant. A method-level `forbidden` and a listing without
/// the message both pass. `assert_owner_contact_not_visible` in
/// `jmap-book-sync/tests/live_server_shared_book.rs` does the same.
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

/// Creates a shared mailbox and an unshared staging mailbox on the owner
/// account, with one message in each. The `MailSync` of the recipient, with
/// the owner account id, then goes through three stages:
///
/// 1. Before a grant, it cannot see the message of the owner.
/// 2. With `mayReadItems`, it sees that message, but the server refuses to
///    file the staging message into the shared mailbox.
/// 3. With `mayAddItems` also, it files the staging message into the shared
///    mailbox, and the owner sees it there.
///
/// At the end, the test expunges the messages and destroys both mailboxes.
/// This cleanup runs only when all assertions pass.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_shared_mailboxs_messages_are_readable_then_writable_as_the_grant_widens() {
    let Some((owner, _owner_address)) = connect_owner() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the shared mailbox test"
        );
        return;
    };
    let Some((recipient, recipient_address)) = connect_recipient_with_user() else {
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
            &Mailbox::new(format!("agent-mailsync-shared-{suffix}")),
        )
        .expect("Mailbox/set create failed against the real server");
    let mailbox_id = mailbox
        .id
        .clone()
        .expect("the server named the new mailbox");

    let staging = owner
        .mailbox_create(
            &owner_account_id,
            &Mailbox::new(format!("agent-mailsync-shared-staging-{suffix}")),
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
