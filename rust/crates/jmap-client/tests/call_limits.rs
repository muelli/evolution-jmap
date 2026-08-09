// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `maxCallsInRequest` (RFC 8620 §2): the two places this client chains two
//! method calls into one request, against a server that takes only one call at
//! a time.
//!
//! Both chains are optimisations — `Email/query` + `Email/get` through a
//! `#ids` back-reference, and `Email/set` + `EmailSubmission/set` through a
//! `#draft` creation reference — and the point of these tests is that they stay
//! optimisations. A server too small for the chain must still be able to read
//! and send mail, at the cost of the round trip the chain saved.

use jmap_client::{Client, Credentials, Error};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::{
    Email, EmailAddress, EmailBodyPart, EmailBodyValue, EmailQueryFilter, keyword, role,
};
use jmap_proto::methods::Comparator;
use jmap_proto::request::Request;
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_MAIL};
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

/// Seed an inbox with two messages and answer its id.
fn seed_inbox(server: &MockServer) -> Id {
    let account_id = server.account_id();
    let state = server.state();
    let mut state = state.lock().unwrap();
    let account = state.account_mut(&account_id).unwrap();
    let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
    account.seed_email(EmailSeed::new(
        inbox.clone(),
        ("Bob", "bob@example.com"),
        "Hello Alice",
        "Hi Alice, how are you?",
        "2026-08-01T10:00:00Z",
    ));
    account.seed_email(EmailSeed::new(
        inbox.clone(),
        ("Carol", "carol@example.com"),
        "Meeting tomorrow",
        "Can we meet at 10?",
        "2026-08-02T09:30:00Z",
    ));
    inbox
}

/// Whether the server has answered a call by this name.
fn called(server: &MockServer, method: &str) -> bool {
    server.method_calls().iter().any(|name| name == method)
}

/// The mock enforces the limit it advertises: a two-call request to a server
/// that takes one is refused whole, and neither call runs. Without this the
/// tests below would pass against a permissive mock while proving nothing.
#[test]
fn a_server_that_takes_one_call_refuses_a_request_with_two() {
    let server = MockServer::builder().calls_in_request(1).start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let echo = json!({"hello": "world"});
    let request = Request::new([CAPABILITY_CORE, CAPABILITY_MAIL])
        .call("Core/echo", &echo, "c1")
        .unwrap()
        .call("Core/echo", &echo, "c2")
        .unwrap();
    let error = client.api_call(&request).unwrap_err();

    match error {
        Error::Http {
            status,
            problem: Some(problem),
        } => {
            assert_eq!(status, 400);
            assert_eq!(problem.error_type, "urn:ietf:params:jmap:error:limit");
        }
        other => panic!("expected a request-level limit error, got {other}"),
    }
    assert!(
        !called(&server, "Core/echo"),
        "a refused request runs none of its calls"
    );

    // The session says so, which is what a client is meant to read.
    assert_eq!(client.session().max_calls_in_request(), Some(1));
    // And the account is still usable one call at a time.
    assert!(client.mailbox_get(&account_id).is_ok());
}

/// The chain is what a server with room for it gets: one request, not two.
#[test]
fn query_and_get_travel_together_when_the_server_takes_two_calls() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let inbox = seed_inbox(&server);
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let before = server.api_requests();
    let emails = client
        .email_query_then_get(
            &account_id,
            EmailQueryFilter::in_mailbox(inbox),
            Some(vec![Comparator::descending("receivedAt")]),
            None,
        )
        .unwrap();

    assert_eq!(emails.len(), 2);
    assert_eq!(
        server.api_requests() - before,
        1,
        "the back-reference saves one"
    );
}

/// The same read against a server that takes one call at a time: two requests,
/// and the same two messages in the same order. The order matters because it
/// is the query's, not the `/get`'s — RFC 8620 §5.1 lets a `/get` answer in any
/// order, and the split path has to restore the sort just as the chain does.
#[test]
fn query_and_get_split_when_the_server_takes_one_call() {
    let server = MockServer::builder().calls_in_request(1).start();
    let account_id = server.account_id();
    let inbox = seed_inbox(&server);
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let before = server.api_requests();
    let emails = client
        .email_query_then_get(
            &account_id,
            EmailQueryFilter::in_mailbox(inbox),
            Some(vec![Comparator::descending("receivedAt")]),
            None,
        )
        .unwrap();

    assert_eq!(emails.len(), 2);
    assert_eq!(emails[0].subject.as_deref(), Some("Meeting tomorrow"));
    assert_eq!(emails[1].subject.as_deref(), Some("Hello Alice"));
    assert_eq!(server.api_requests() - before, 2, "one call each");

    // The body is there, so the split path asked for the same properties.
    let email = &emails[1];
    let part_id = email.text_body.as_ref().unwrap()[0]
        .part_id
        .clone()
        .unwrap();
    assert_eq!(
        email.body_values.as_ref().unwrap()[&part_id].value,
        "Hi Alice, how are you?"
    );
}

/// A query that matches nothing costs one request, not two: there is nothing
/// to fetch, and an `Email/get` with no ids is a round trip for an empty list.
#[test]
fn an_empty_query_asks_for_nothing_when_the_server_takes_one_call() {
    let server = MockServer::builder().calls_in_request(1).start();
    let account_id = server.account_id();
    seed_inbox(&server);
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let before = server.api_requests();
    let emails = client
        .email_query_then_get(
            &account_id,
            EmailQueryFilter::in_mailbox(Id::new("M404")),
            None,
            None,
        )
        .unwrap();

    assert!(emails.is_empty());
    assert_eq!(server.api_requests() - before, 1);
    assert!(!called(&server, "Email/get"));
}

/// Sending is the other chain, and the harder one: the submission names the
/// draft as `#draft`, a creation reference that only resolves inside the
/// request that created it. Split, it has to name the real id instead.
#[test]
fn sending_splits_when_the_server_takes_one_call() {
    let server = MockServer::builder().calls_in_request(1).start();
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
    let identity_id = client.identities(&account_id).unwrap()[0]
        .id
        .clone()
        .unwrap();

    let on_success = json!({
        "mailboxIds": { sent.as_str(): true },
        format!("keywords/{}", keyword::SEEN): true,
    });
    let before = server.api_requests();
    let (created_email, submission) = client
        .send_email(&account_id, &draft(&drafts), &identity_id, Some(on_success))
        .unwrap();

    assert_eq!(server.api_requests() - before, 2, "one call each");
    let email_id = created_email.id.clone().expect("the server set an id");
    assert_eq!(submission.email_id, email_id);

    let state = server.state();
    let state = state.lock().unwrap();
    let account = state.account(&account_id).unwrap();

    // It reached the outbox, addressed to the same message.
    assert_eq!(account.outbox.len(), 1);
    assert_eq!(account.outbox[0].email_id, email_id);

    // And `onSuccessUpdateEmail` still applied: the draft moved to Sent, which
    // is the part that would silently stop happening if the split dropped the
    // patch along with the creation reference.
    let stored = account.emails.get(&email_id).expect("the draft is stored");
    assert_eq!(
        stored
            .mailbox_ids
            .as_ref()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec![&sent]
    );
    assert!(
        stored
            .keywords
            .as_ref()
            .unwrap()
            .contains_key(keyword::SEEN)
    );
}

/// And sending still travels together where there is room for it.
#[test]
fn sending_is_one_request_when_the_server_takes_two_calls() {
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

    let before = server.api_requests();
    client
        .send_email(&account_id, &draft(&drafts), &identity_id, None)
        .unwrap();

    assert_eq!(
        server.api_requests() - before,
        1,
        "the creation reference saves one"
    );
}

/// A server that names no limit at all is sent the chain. RFC 8620 §2 requires
/// the property, so this is a server out of spec — and splitting for it would
/// pay a round trip per read against a server that never asked for one.
#[test]
fn a_server_that_names_no_call_limit_still_gets_the_chain() {
    let server = MockServer::builder().no_calls_in_request().start();
    let account_id = server.account_id();
    let inbox = seed_inbox(&server);
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    assert_eq!(client.session().max_calls_in_request(), None);

    let before = server.api_requests();
    let emails = client
        .email_query_then_get(&account_id, EmailQueryFilter::in_mailbox(inbox), None, None)
        .unwrap();

    assert_eq!(emails.len(), 2);
    assert_eq!(server.api_requests() - before, 1);
}
