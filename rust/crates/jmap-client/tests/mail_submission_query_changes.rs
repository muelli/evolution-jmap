// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `EmailSubmission/query` (RFC 8621 §7.3) and `EmailSubmission/changes`
//! (RFC 8620 §5.2): the two ways to discover submissions after the fact,
//! independent of `EmailSubmission/set`'s own create/cancel.

use jmap_client::{Client, Credentials};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::mail::{
    Email, EmailAddress, EmailBodyPart, EmailBodyValue, EmailSubmissionQueryFilter, Envelope,
    EnvelopeAddress, Schedule, keyword, role,
};
use jmap_proto::{Id, UtcDate};

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

/// Filtering by `identityIds`, `threadIds`, and `after`/`before` time range
/// (RFC 8621 §7.3) narrows submissions to matching criteria.
#[test]
fn submission_query_filters_by_identity_ids_thread_ids_and_time_range() {
    let server = MockServer::builder().max_delayed_send(3600).start();
    let account_id = server.account_id();
    let (drafts, inbox, alice_identity, bob_identity) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let alice_id = account.seed_identity("Alice", "alice@example.com");
        let bob_id = account.seed_identity("Bob", "bob@example.com");
        (
            account.seed_mailbox("Drafts", Some(role::DRAFTS)),
            account.seed_mailbox("Inbox", Some(role::INBOX)),
            alice_id,
            bob_id,
        )
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let (sent_email, first_submission) = client
        .send_email(
            &account_id,
            &draft(&drafts, "carol@example.com"),
            &alice_identity,
            None,
        )
        .unwrap();

    let held_email_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_email(EmailSeed::new(
                inbox,
                ("Bob", "bob@example.com"),
                "Held Message",
                "Hi Carol",
                "2026-08-01T10:00:00Z",
            ))
    };

    let second_submission = client
        .submit_email_at(
            &account_id,
            &held_email_id,
            &bob_identity,
            Envelope::new(
                EnvelopeAddress::new("bob@example.com"),
                [EnvelopeAddress::new("carol@example.com")],
            ),
            &Schedule::HoldFor(600),
            None,
        )
        .unwrap();

    let first_id = first_submission.id.unwrap();
    let second_id = second_submission.id.unwrap();

    // 1. Filter by identityIds:
    let alice_ids = client
        .email_submission_query(
            &account_id,
            EmailSubmissionQueryFilter::new().with_identity_ids([alice_identity.clone()]),
        )
        .unwrap();
    assert_eq!(alice_ids, vec![first_id.clone()]);

    let bob_ids = client
        .email_submission_query(
            &account_id,
            EmailSubmissionQueryFilter::new().with_identity_ids([bob_identity.clone()]),
        )
        .unwrap();
    assert_eq!(bob_ids, vec![second_id.clone()]);

    // 2. Filter by threadIds:
    if let Some(thread_id) = sent_email.thread_id {
        let thread_matches = client
            .email_submission_query(
                &account_id,
                EmailSubmissionQueryFilter::new().with_thread_ids([thread_id]),
            )
            .unwrap();
        assert_eq!(thread_matches, vec![first_id.clone()]);
    }

    // 3. Filter by time range (sendAt is 2026-01-01T00:10:00Z for the held submission):
    let in_window = client
        .email_submission_query(
            &account_id,
            EmailSubmissionQueryFilter::new()
                .after(UtcDate::new("2026-01-01T00:05:00Z"))
                .before(UtcDate::new("2026-01-01T00:15:00Z")),
        )
        .unwrap();
    assert_eq!(in_window, vec![second_id.clone()]);

    let outside_window = client
        .email_submission_query(
            &account_id,
            EmailSubmissionQueryFilter::new().after(UtcDate::new("2026-01-01T00:15:00Z")),
        )
        .unwrap();
    assert!(outside_window.is_empty());
}
