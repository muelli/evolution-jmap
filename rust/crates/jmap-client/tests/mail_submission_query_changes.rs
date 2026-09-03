// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `EmailSubmission/query` (RFC 8621 §7.3) and `EmailSubmission/changes`
//! (RFC 8620 §5.2): the two ways to discover submissions after the fact,
//! independent of `EmailSubmission/set`'s own create/cancel.

use jmap_client::{Client, Credentials};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::{
    Email, EmailAddress, EmailBodyPart, EmailBodyValue, EmailSubmissionQueryFilter, Envelope,
    EnvelopeAddress, Schedule, keyword, role,
};

fn draft(mailbox: &Id, to: &str) -> Email {
    Email {
        mailbox_ids: Some([(mailbox.clone(), true)].into()),
        keywords: Some([(keyword::DRAFT.to_owned(), true)].into()),
        from: Some(vec![EmailAddress::new(Some("Alice"), "alice@example.com")]),
        to: Some(vec![EmailAddress::new(None, to)]),
        subject: Some("Ping".to_owned()),
        body_values: Some([("1".to_owned(), EmailBodyValue::new("Hello"))].into()),
        text_body: Some(vec![EmailBodyPart {
            part_id: Some("1".to_owned()),
            content_type: Some("text/plain".to_owned()),
            ..EmailBodyPart::default()
        }]),
        ..Email::default()
    }
}

/// `EmailSubmission/query` with no filter (RFC 8621 §7.3) returns every
/// submission's bare id, the same shape `SieveScript/query` already has.
#[test]
fn submission_query_with_no_filter_returns_every_submission() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let drafts = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_identity("Alice", "alice@example.com");
        account.seed_mailbox("Drafts", Some(role::DRAFTS))
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let identity_id = client.identities(&account_id).unwrap()[0]
        .id
        .clone()
        .unwrap();

    let (_, first) = client
        .send_email(
            &account_id,
            &draft(&drafts, "bob@example.com"),
            &identity_id,
            None,
        )
        .unwrap();
    let (_, second) = client
        .send_email(
            &account_id,
            &draft(&drafts, "carol@example.com"),
            &identity_id,
            None,
        )
        .unwrap();

    let mut ids = client
        .email_submission_query(&account_id, EmailSubmissionQueryFilter::new())
        .unwrap();
    ids.sort();
    let mut expected = vec![first.id.unwrap(), second.id.unwrap()];
    expected.sort();
    assert_eq!(ids, expected);
}

/// Filtering by `emailIds` (RFC 8621 §7.3) narrows to just the submission of
/// that one message.
#[test]
fn submission_query_filters_by_email_ids() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let drafts = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_identity("Alice", "alice@example.com");
        account.seed_mailbox("Drafts", Some(role::DRAFTS))
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let identity_id = client.identities(&account_id).unwrap()[0]
        .id
        .clone()
        .unwrap();

    client
        .send_email(
            &account_id,
            &draft(&drafts, "bob@example.com"),
            &identity_id,
            None,
        )
        .unwrap();
    let (wanted_email, wanted_submission) = client
        .send_email(
            &account_id,
            &draft(&drafts, "carol@example.com"),
            &identity_id,
            None,
        )
        .unwrap();

    let ids = client
        .email_submission_query(
            &account_id,
            EmailSubmissionQueryFilter::new().with_email_ids([wanted_email.id.clone().unwrap()]),
        )
        .unwrap();
    assert_eq!(ids, vec![wanted_submission.id.unwrap()]);
}

/// Filtering by `undoStatus` (RFC 8621 §7.3) separates a still-pending
/// FUTURERELEASE hold from one the mock already delivered.
#[test]
fn submission_query_filters_by_undo_status() {
    let server = MockServer::builder().max_delayed_send(3600).start();
    let account_id = server.account_id();
    let (drafts, inbox) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_identity("Alice", "alice@example.com");
        (
            account.seed_mailbox("Drafts", Some(role::DRAFTS)),
            account.seed_mailbox("Inbox", Some(role::INBOX)),
        )
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let identity_id = client.identities(&account_id).unwrap()[0]
        .id
        .clone()
        .unwrap();

    let (_, sent_now) = client
        .send_email(
            &account_id,
            &draft(&drafts, "bob@example.com"),
            &identity_id,
            None,
        )
        .unwrap();
    assert_eq!(sent_now.undo_status.as_deref(), Some("final"));

    let held_email_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_email(EmailSeed::new(
                inbox,
                ("Alice", "alice@example.com"),
                "Held",
                "Hi Carol",
                "2026-08-01T10:00:00Z",
            ))
    };
    let held = client
        .submit_email_at(
            &account_id,
            &held_email_id,
            &identity_id,
            Envelope::new(
                EnvelopeAddress::new("alice@example.com"),
                [EnvelopeAddress::new("carol@example.com")],
            ),
            &Schedule::HoldFor(600),
            None,
        )
        .unwrap();
    assert_eq!(held.undo_status.as_deref(), Some("pending"));

    let ids = client
        .email_submission_query(
            &account_id,
            EmailSubmissionQueryFilter::new().with_undo_status("pending"),
        )
        .unwrap();
    assert_eq!(ids, vec![held.id.unwrap()]);
}

/// `EmailSubmission/changes` (RFC 8620 §5.2) reports a fresh submission as
/// created, through the generic `Client::changes` already wired for
/// `Mailbox`/`Email`/`Thread`/etc.
#[test]
fn submission_changes_reports_a_new_submission_as_created() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let (since, drafts) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_identity("Alice", "alice@example.com");
        let drafts = account.seed_mailbox("Drafts", Some(role::DRAFTS));
        (account.submissions.state(), drafts)
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let identity_id = client.identities(&account_id).unwrap()[0]
        .id
        .clone()
        .unwrap();

    let (_, submission) = client
        .send_email(
            &account_id,
            &draft(&drafts, "bob@example.com"),
            &identity_id,
            None,
        )
        .unwrap();

    let changes = client
        .changes(&account_id, "EmailSubmission", &since)
        .unwrap();
    assert_eq!(changes.created, vec![submission.id.unwrap()]);
    assert!(changes.updated.is_empty());
    assert!(changes.destroyed.is_empty());
}

/// Canceling a pending submission (`EmailSubmission/set`'s only allowed
/// update, RFC 8621 §7.4) reports as an update, not a second creation.
#[test]
fn submission_changes_reports_a_canceled_submission_as_updated() {
    let server = MockServer::builder().max_delayed_send(3600).start();
    let account_id = server.account_id();
    let (inbox, identity_id) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let identity_id = account.seed_identity("Alice", "alice@example.com");
        (
            account.seed_mailbox("Inbox", Some(role::INBOX)),
            identity_id,
        )
    };
    let held_email_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_email(EmailSeed::new(
                inbox,
                ("Alice", "alice@example.com"),
                "Held",
                "Hi Bob",
                "2026-08-01T10:00:00Z",
            ))
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let held = client
        .submit_email_at(
            &account_id,
            &held_email_id,
            &identity_id,
            Envelope::new(
                EnvelopeAddress::new("alice@example.com"),
                [EnvelopeAddress::new("bob@example.com")],
            ),
            &Schedule::HoldFor(600),
            None,
        )
        .unwrap();
    let submission_id = held.id.unwrap();

    let since = {
        let state = server.state();
        let state = state.lock().unwrap();
        state.account(&account_id).unwrap().submissions.state()
    };

    client
        .cancel_email_submission(&account_id, &submission_id)
        .unwrap();

    let changes = client
        .changes(&account_id, "EmailSubmission", &since)
        .unwrap();
    assert!(changes.created.is_empty());
    assert_eq!(changes.updated, vec![submission_id]);
    assert!(changes.destroyed.is_empty());
}
