// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sending email: draft creation + submission in one request, envelope
//! derivation, onSuccessUpdateEmail, and rejection paths.

use jmap_client::{Client, Credentials, Error, limits};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::mail::{
    Email, EmailAddress, EmailBodyPart, EmailBodyValue, EmailImport, Envelope, EnvelopeAddress,
    Schedule, keyword, role,
};
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

/// RFC 8620 §5.3 lets a `created` entry omit properties the client already
/// sent — and a client always sends `identityId`/`emailId` when creating a
/// submission, so a spec-following server (Stalwart, in the finding this
/// regression-tests) may leave both out. `send_email`'s chained
/// `Email/set`+`EmailSubmission/set` form must still come back with a
/// complete `EmailSubmission`, backfilling both from what it sent rather
/// than failing to deserialize a response with neither.
#[test]
fn send_email_tolerates_a_server_that_omits_identity_and_email_id() {
    let server = MockServer::builder().terse_submission_create().start();
    let account_id = server.account_id();

    let (drafts, _sent) = {
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
    let identity_id = client.identities(&account_id).unwrap()[0]
        .id
        .clone()
        .unwrap();

    let (created_email, submission) = client
        .send_email(&account_id, &draft(&drafts), &identity_id, None)
        .unwrap();

    let email_id = created_email.id.expect("server assigned an email id");
    assert_eq!(submission.identity_id, identity_id);
    assert_eq!(submission.email_id, email_id);
}

/// The same tolerance for the split form of sending
/// (`sending_splits_when_the_server_takes_one_call` in `call_limits.rs`
/// covers the split itself; this covers a terse response on top of it).
#[test]
fn send_email_tolerates_a_terse_response_when_split_across_two_calls() {
    let server = MockServer::builder()
        .calls_in_request(1)
        .terse_submission_create()
        .start();
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

    let (created_email, submission) = client
        .send_email(&account_id, &draft(&drafts), &identity_id, None)
        .unwrap();

    let email_id = created_email.id.expect("server assigned an email id");
    assert_eq!(submission.identity_id, identity_id);
    assert_eq!(submission.email_id, email_id);
}

/// `submit_email` (the non-composing half, for a message that arrived via
/// `Email/import`) needs the same tolerance: it deserializes a created
/// `EmailSubmission` from its own single request, independently of
/// `send_email`'s two call sites.
#[test]
fn submit_email_tolerates_a_server_that_omits_identity_and_email_id() {
    let server = MockServer::builder().terse_submission_create().start();
    let account_id = server.account_id();

    let inbox = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_identity("Alice", "alice@example.com");
        account.seed_mailbox("Inbox", Some(role::INBOX))
    };

    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let identity_id = client.identities(&account_id).unwrap()[0]
        .id
        .clone()
        .unwrap();

    let message = b"From: alice@example.com\r\nTo: bob@example.com\r\nSubject: Ping\r\n\r\nHi\r\n";
    let upload = client
        .upload_blob(&account_id, "message/rfc822", message.to_vec())
        .unwrap();
    let imported = client
        .email_import(&account_id, &EmailImport::new(upload.blob_id, inbox))
        .unwrap();
    let email_id = imported.id.expect("server assigned an email id");

    let submission = client
        .submit_email(&account_id, &email_id, &identity_id, None, None)
        .unwrap();

    assert_eq!(submission.identity_id, identity_id);
    assert_eq!(submission.email_id, email_id);
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

/// A server advertising `maxDelayedSend` and FUTURERELEASE
/// (RFC 8621 §7.1) is detectable through the session document before a
/// caller offers scheduled send at all.
#[test]
fn the_session_names_the_accounts_delayed_send_limit() {
    let server = MockServer::builder().max_delayed_send(3600).start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let account = client.session().accounts.get(&account_id).unwrap();
    assert_eq!(account.max_delayed_send(), Some(3600));
    assert!(
        account
            .submission_capability()
            .is_some_and(|capability| capability.supports_future_release())
    );
}

/// A server that never advertised `maxDelayedSend` answers `None`, not an
/// invented number — the ordinary case, since most deployments have no SMTP
/// FUTURERELEASE.
#[test]
fn a_server_with_no_delayed_send_support_says_so() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let account = client.session().accounts.get(&account_id).unwrap();
    assert_eq!(account.max_delayed_send(), None);
    assert!(
        !account
            .submission_capability()
            .is_some_and(|capability| capability.supports_future_release())
    );
}

/// The boilerplate every scheduling test below shares: an identity, a message
/// imported into a seeded inbox, and a connected client. Answers
/// `(client, identity id, email id)`.
fn ready_to_submit(server: &MockServer) -> (Client, Id, Id) {
    let account_id = server.account_id();
    let inbox = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_identity("Alice", "alice@example.com");
        account.seed_mailbox("Inbox", Some(role::INBOX))
    };

    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let identity_id = client.identities(&account_id).unwrap()[0]
        .id
        .clone()
        .unwrap();

    let message = b"From: alice@example.com\r\nTo: bob@example.com\r\nSubject: Ping\r\n\r\nHi\r\n";
    let upload = client
        .upload_blob(&account_id, "message/rfc822", message.to_vec())
        .unwrap();
    let imported = client
        .email_import(&account_id, &EmailImport::new(upload.blob_id, inbox))
        .unwrap();
    let email_id = imported.id.expect("server assigned an email id");

    (client, identity_id, email_id)
}

/// The envelope the scheduling tests submit with; scheduling requires one,
/// since the FUTURERELEASE parameter has to sit on its `mailFrom`.
fn alice_to_bob() -> Envelope {
    Envelope {
        mail_from: EnvelopeAddress::new("alice@example.com"),
        rcpt_to: vec![EnvelopeAddress::new("bob@example.com")],
    }
}

/// `submit_email_at` with a `HOLDFOR`: the mock holds the message rather than
/// delivering it — `undoStatus: "pending"`, the release time answered in
/// `sendAt`, nothing in the outbox — proving the client's scheduled-send path
/// end to end even though nothing in Evolution calls it yet (item 29's "ready
/// and proven").
#[test]
fn a_held_submission_is_pending_with_the_release_time_in_send_at() {
    let server = MockServer::builder().max_delayed_send(3600).start();
    let account_id = server.account_id();
    let (client, identity_id, email_id) = ready_to_submit(&server);

    let submission = client
        .submit_email_at(
            &account_id,
            &email_id,
            &identity_id,
            alice_to_bob(),
            &Schedule::HoldFor(600),
            None,
        )
        .unwrap();

    // The mock's clock stands at 2026-01-01T00:00:00Z; the hold lands 600
    // seconds past it, computed by the server, not echoed from the client.
    assert_eq!(
        submission.send_at,
        Some(jmap_proto::UtcDate::new("2026-01-01T00:10:00Z"))
    );
    assert_eq!(submission.undo_status.as_deref(), Some("pending"));

    let state = server.state();
    let state = state.lock().unwrap();
    let account = state.account(&account_id).unwrap();
    assert!(
        account.outbox.is_empty(),
        "a pending submission must not have been delivered yet"
    );
}

/// The `HOLDUNTIL` form: a future date-time holds with `sendAt` echoing the
/// release time; one already in the past delivers immediately (RFC 4865
/// §4.1), it is not an error.
#[test]
fn hold_until_holds_for_the_future_and_releases_the_past() {
    let server = MockServer::builder().max_delayed_send(u64::MAX).start();
    let account_id = server.account_id();
    let (client, identity_id, email_id) = ready_to_submit(&server);

    let future = jmap_proto::UtcDate::new("2027-01-01T00:00:00Z");
    let submission = client
        .submit_email_at(
            &account_id,
            &email_id,
            &identity_id,
            alice_to_bob(),
            &Schedule::HoldUntil(future.clone()),
            None,
        )
        .unwrap();
    assert_eq!(submission.send_at, Some(future));
    assert_eq!(submission.undo_status.as_deref(), Some("pending"));

    let past = jmap_proto::UtcDate::new("2025-01-01T00:00:00Z");
    let submission = client
        .submit_email_at(
            &account_id,
            &email_id,
            &identity_id,
            alice_to_bob(),
            &Schedule::HoldUntil(past),
            None,
        )
        .unwrap();
    assert_eq!(submission.undo_status.as_deref(), Some("final"));

    let state = server.state();
    let state = state.lock().unwrap();
    let account = state.account(&account_id).unwrap();
    assert_eq!(
        account.outbox.len(),
        1,
        "only the expired hold may have been delivered"
    );
}

/// A hold the server never offered — no `maxDelayedSend` advertised — is
/// refused as `invalidProperties`, and one beyond the advertised limit
/// likewise: the gate a caller is supposed to read first, enforced.
#[test]
fn a_hold_the_server_does_not_offer_is_refused() {
    let unsupporting = MockServer::builder().start();
    let account_id = unsupporting.account_id();
    let (client, identity_id, email_id) = ready_to_submit(&unsupporting);
    match client.submit_email_at(
        &account_id,
        &email_id,
        &identity_id,
        alice_to_bob(),
        &Schedule::HoldFor(600),
        None,
    ) {
        Err(Error::Set(set_error)) => assert_eq!(set_error.error_type, "invalidProperties"),
        other => panic!("expected SetError, got {other:?}"),
    }

    let limited = MockServer::builder().max_delayed_send(3600).start();
    let account_id = limited.account_id();
    let (client, identity_id, email_id) = ready_to_submit(&limited);
    match client.submit_email_at(
        &account_id,
        &email_id,
        &identity_id,
        alice_to_bob(),
        &Schedule::HoldFor(7200),
        None,
    ) {
        Err(Error::Set(set_error)) => assert_eq!(set_error.error_type, "invalidProperties"),
        other => panic!("expected SetError, got {other:?}"),
    }
}

/// `HOLDFOR` and `HOLDUNTIL` on the same envelope are refused (RFC 4865 §3
/// forbids naming both).
#[test]
fn hold_for_and_hold_until_together_are_refused() {
    let server = MockServer::builder().max_delayed_send(3600).start();
    let account_id = server.account_id();
    let (client, identity_id, email_id) = ready_to_submit(&server);

    let envelope = alice_to_bob()
        .with_schedule(&Schedule::HoldFor(600))
        .with_schedule(&Schedule::HoldUntil(jmap_proto::UtcDate::new(
            "2027-01-01T00:00:00Z",
        )));
    match client.submit_email(&account_id, &email_id, &identity_id, Some(envelope), None) {
        Err(Error::Set(set_error)) => assert_eq!(set_error.error_type, "invalidProperties"),
        other => panic!("expected SetError, got {other:?}"),
    }
}

/// A pending submission can be canceled (RFC 8621 §7.4): `undoStatus` moves
/// to `"canceled"` and the message never reaches the outbox.
#[test]
fn a_pending_submission_can_be_canceled() {
    let server = MockServer::builder().max_delayed_send(3600).start();
    let account_id = server.account_id();
    let (client, identity_id, email_id) = ready_to_submit(&server);

    let submission = client
        .submit_email_at(
            &account_id,
            &email_id,
            &identity_id,
            alice_to_bob(),
            &Schedule::HoldFor(600),
            None,
        )
        .unwrap();
    let submission_id = submission.id.expect("server assigned a submission id");

    client
        .cancel_email_submission(&account_id, &submission_id)
        .unwrap();

    let state = server.state();
    let state = state.lock().unwrap();
    let account = state.account(&account_id).unwrap();
    assert!(
        account.outbox.is_empty(),
        "a canceled submission must not send"
    );
}

/// A client-set `sendAt` is refused: the property is server-set (RFC 8621
/// §7.1) and this crate's own client used to send it, so the mock's refusal
/// is the regression trap. Raw request, since the client no longer can.
#[test]
fn a_client_set_send_at_is_refused_as_server_set() {
    use jmap_client::transport::{HttpMethod, HttpRequest, Transport, UreqTransport};

    let server = MockServer::builder().max_delayed_send(3600).start();
    let account_id = server.account_id();
    let (client, identity_id, email_id) = ready_to_submit(&server);

    let body = serde_json::to_vec(&json!({
        "using": [
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:mail",
            "urn:ietf:params:jmap:submission",
        ],
        "methodCalls": [["EmailSubmission/set", {
            "accountId": account_id,
            "create": {"s": {
                "identityId": identity_id,
                "emailId": email_id,
                "sendAt": "2027-01-01T00:00:00Z",
            }},
        }, "c0"]],
    }))
    .unwrap();

    let headers = [("Content-Type".to_owned(), "application/json".to_owned())];
    let response = UreqTransport::new(std::time::Duration::from_secs(10))
        .execute(HttpRequest {
            method: HttpMethod::Post,
            url: &client.session().api_url,
            headers: &headers,
            body: Some(&body),
            cancel: None,
            max_response_bytes: 1024 * 1024,
        })
        .unwrap();

    assert_eq!(response.status, 200);
    let response: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
    let set_error = &response["methodResponses"][0][1]["notCreated"]["s"];
    assert_eq!(set_error["type"], "invalidProperties");
}

/// Canceling a submission the mock already delivered (never held, so it went
/// out immediately with `undoStatus: "final"`) is refused — undoing an
/// already-sent message is not on offer.
#[test]
fn canceling_an_already_final_submission_is_refused() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let (client, identity_id, email_id) = ready_to_submit(&server);

    let submission = client
        .submit_email(&account_id, &email_id, &identity_id, None, None)
        .unwrap();
    let submission_id = submission.id.expect("server assigned a submission id");

    match client.cancel_email_submission(&account_id, &submission_id) {
        Err(Error::Set(set_error)) => assert_eq!(set_error.error_type, "forbidden"),
        other => panic!("expected SetError, got {other:?}"),
    }
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
