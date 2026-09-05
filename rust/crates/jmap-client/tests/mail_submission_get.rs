// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `EmailSubmission/get` (RFC 8621 §7.4) tests.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::mail::{Email, EmailAddress, EmailBodyPart, EmailBodyValue, keyword, role};

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

#[test]
fn submission_get_returns_the_submission_named_by_its_own_id() {
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

    let (email, submission) = client
        .send_email(
            &account_id,
            &draft(&drafts, "bob@example.com"),
            &identity_id,
            None,
        )
        .unwrap();
    let submission_id = submission.id.clone().unwrap();

    let fetched = client
        .email_submission_get(&account_id, [submission_id.clone()])
        .unwrap();

    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].id.as_ref(), Some(&submission_id));
    assert_eq!(fetched[0].email_id, email.id.unwrap());
    assert_eq!(fetched[0].identity_id, identity_id);
}

#[test]
fn submission_get_of_an_unknown_id_is_silently_absent_like_thread_get() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let fetched = client
        .email_submission_get(&account_id, [Id::from("no-such-submission")])
        .unwrap();

    assert!(fetched.is_empty());
}

#[test]
fn submission_get_reports_two_ids_independently() {
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

    let (_, submission) = client
        .send_email(
            &account_id,
            &draft(&drafts, "bob@example.com"),
            &identity_id,
            None,
        )
        .unwrap();
    let submission_id = submission.id.clone().unwrap();

    let fetched = client
        .email_submission_get(
            &account_id,
            [submission_id.clone(), Id::from("no-such-submission")],
        )
        .unwrap();

    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].id.as_ref(), Some(&submission_id));
}
