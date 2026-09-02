// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `Email/query` with AND/OR/NOT nesting (RFC 8620 §5.5): the generic
//! `Filter<F>`/`FilterOperator` jmap-proto models but Email/query itself did
//! not yet accept.

use jmap_client::{Client, Credentials};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::mail::{EmailQueryFilter, role};
use jmap_proto::methods::Filter;

fn seed_three(server: &MockServer) -> (jmap_proto::Id, jmap_proto::Id) {
    let account_id = server.account_id();
    let state = server.state();
    let mut state = state.lock().unwrap();
    let account = state.account_mut(&account_id).unwrap();
    let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
    account.seed_email(EmailSeed::new(
        inbox.clone(),
        ("Bob", "bob@example.com"),
        "Hello Alice",
        "hi",
        "2026-08-01T10:00:00Z",
    ));
    account.seed_email(EmailSeed::new(
        inbox.clone(),
        ("Carol", "carol@example.com"),
        "Lunch plans",
        "hi",
        "2026-08-02T10:00:00Z",
    ));
    account.seed_email(EmailSeed::new(
        inbox.clone(),
        ("Bob", "bob@example.com"),
        "Meeting notes",
        "hi",
        "2026-08-03T10:00:00Z",
    ));
    (account_id, inbox)
}

#[test]
fn email_query_or_matches_either_condition() {
    let server = MockServer::builder().start();
    let (account_id, _inbox) = seed_three(&server);
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let filter = Filter::or([
        Filter::condition(EmailQueryFilter::default().subject("Hello")),
        Filter::condition(EmailQueryFilter::default().subject("Lunch")),
    ]);
    let response = client
        .email_query(&account_id, filter, None, None, 0)
        .unwrap();

    assert_eq!(response.ids.len(), 2);
}

#[test]
fn email_query_and_requires_every_condition() {
    let server = MockServer::builder().start();
    let (account_id, _inbox) = seed_three(&server);
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    // Both Bob messages match "from bob", only one of them also matches
    // "subject contains Meeting".
    let filter = Filter::and([
        Filter::condition(EmailQueryFilter::default().from("bob@example.com")),
        Filter::condition(EmailQueryFilter::default().subject("Meeting")),
    ]);
    let response = client
        .email_query(&account_id, filter, None, None, 0)
        .unwrap();

    assert_eq!(response.ids.len(), 1);
}

#[test]
fn email_query_not_negates_its_conditions() {
    let server = MockServer::builder().start();
    let (account_id, _inbox) = seed_three(&server);
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let filter = Filter::not([Filter::condition(
        EmailQueryFilter::default().from("bob@example.com"),
    )]);
    let response = client
        .email_query(&account_id, filter, None, None, 0)
        .unwrap();

    // Only Carol's message is not from Bob.
    assert_eq!(response.ids.len(), 1);
}

#[test]
fn email_query_still_accepts_a_flat_filter() {
    let server = MockServer::builder().start();
    let (account_id, _inbox) = seed_three(&server);
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let response = client
        .email_query(
            &account_id,
            EmailQueryFilter::default().subject("Lunch"),
            None,
            None,
            0,
        )
        .unwrap();

    assert_eq!(response.ids.len(), 1);
}
