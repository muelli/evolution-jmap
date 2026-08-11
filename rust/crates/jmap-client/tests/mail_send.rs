// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sending email: draft creation + submission in one request, envelope
//! derivation, onSuccessUpdateEmail, and rejection paths.

use jmap_client::{Client, Credentials, Error, limits};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::mail::{Email, EmailAddress, EmailBodyPart, EmailBodyValue, keyword, role};
use serde_json::json;

/// A minimal draft addressed from `alice` to `bob`.
fn draft(drafts_mailbox: &Id) -> Email {
    Email {
        mailbox_ids: Some([(drafts_mailbox.clone(), true)].into()),
        keywords: Some([(keyword::DRAFT.to_owned(), true)].into()),
        from: Some(vec![EmailAddress::new(Some("Alice"), "alice@example.com")]),
        to: Some(vec![EmailAddress::new(Some("Bob"), "bob@example.com")]),
        subject: Some("Ping".to_owned()),
        body_values: Some([("1".to_owned(), EmailBodyValue::new("Hello Bob"))].into()),
        text_body: Some(vec![EmailBodyPart {
            part_id: Some("1".to_owned()),
            content_type: Some("text/plain".to_owned()),
            ..EmailBodyPart::default()
        }]),
        ..Email::default()
    }
}

#[test]
fn send_email_full_flow() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();

    let (drafts, sent) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_identity("Alice", "alice@example.com");
        (
            account.seed_mailbox("Drafts", Some(role::DRAFTS)),
            account.seed_mailbox("Sent", Some(role::SENT)),
        )
    };

    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let identities = client.identities(&account_id).unwrap();
    assert_eq!(identities.len(), 1);
    assert_eq!(identities[0].email, "alice@example.com");
    let identity_id = identities[0].id.clone().unwrap();

    // Move to Sent and mark seen once the submission is accepted.
    let on_success = json!({
        "mailboxIds": { sent.as_str(): true },
        format!("keywords/{}", keyword::SEEN): true,
    });
    let (created_email, submission) = client
        .send_email(&account_id, &draft(&drafts), &identity_id, Some(on_success))
        .unwrap();

    let email_id = created_email.id.expect("server assigned an email id");
    assert!(submission.id.is_some());
    assert_eq!(submission.email_id, email_id);

    // White box: exactly one submission hit the outbox, with the envelope
    // derived from the message headers.
    {
        let state = server.state();
        let state = state.lock().unwrap();
        let account = state.account(&account_id).unwrap();
        assert_eq!(account.outbox.len(), 1);
        let recorded = &account.outbox[0];
        assert_eq!(recorded.email_id, email_id);
        assert_eq!(recorded.envelope.mail_from.email, "alice@example.com");
        assert_eq!(recorded.envelope.rcpt_to.len(), 1);
        assert_eq!(recorded.envelope.rcpt_to[0].email, "bob@example.com");
    }

    // onSuccessUpdateEmail took effect: now in Sent, marked seen.
    let emails = client
        .email_get(&account_id, &[email_id], Some(&["mailboxIds", "keywords"]))
        .unwrap();
    let email = &emails[0];
    let mailbox_ids = email.mailbox_ids.as_ref().unwrap();
    assert_eq!(mailbox_ids.get(&sent), Some(&true));
    let keywords = email.keywords.as_ref().unwrap();
    assert_eq!(keywords.get(keyword::SEEN), Some(&true));
    assert_eq!(keywords.get(keyword::DRAFT), Some(&true)); // patch left it alone
}

#[test]
fn send_email_invalid_identity_rejected() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();

    let drafts = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        // No identity seeded.
        account.seed_mailbox("Drafts", Some(role::DRAFTS))
    };

    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let bogus_identity = Id::new("I999");
    let result = client.send_email(&account_id, &draft(&drafts), &bogus_identity, None);

    match result {
        Err(Error::Set(set_error)) => {
            assert_eq!(set_error.error_type, "invalidProperties");
        }
        other => panic!("expected SetError, got {other:?}"),
    }

    let state = server.state();
    let state = state.lock().unwrap();
    let account = state.account(&account_id).unwrap();
    assert!(
        account.outbox.is_empty(),
        "rejected submission must not send"
    );
}

#[test]
fn upload_download_blob_roundtrip() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let data = b"\x89PNG\r\n binary-ish payload".to_vec();
    let uploaded = client
        .upload_blob(&account_id, "image/png", data.clone())
        .unwrap();
    assert_eq!(uploaded.content_type.as_deref(), Some("image/png"));
    assert_eq!(uploaded.size, data.len() as u64);

    let downloaded = client
        .download_blob(
            &account_id,
            &uploaded.blob_id,
            "img.png",
            limits::MAX_BLOB_BYTES,
        )
        .unwrap();
    assert_eq!(downloaded, data);
}
