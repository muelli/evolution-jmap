// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for `Client::query_every_id` and `Client::email_query_every_id` pagination and sorting.

use jmap_client::{Client, Credentials, Error};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::{EmailQueryFilter, role};
use jmap_proto::methods::{Comparator, Filter};

const CAPABILITY_CORE: &str = "urn:ietf:params:jmap:core";
const CAPABILITY_MAIL: &str = "urn:ietf:params:jmap:mail";

fn seed_five_emails(server: &MockServer) -> (Id, Vec<Id>) {
    let account_id = server.account_id();
    let state = server.state();
    let mut state = state.lock().unwrap();
    let account = state.account_mut(&account_id).unwrap();
    let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));

    let dates = [
        "2026-08-01T10:00:00Z",
        "2026-08-02T10:00:00Z",
        "2026-08-03T10:00:00Z",
        "2026-08-04T10:00:00Z",
        "2026-08-05T10:00:00Z",
    ];
    let mut ids = Vec::new();
    for (index, date) in dates.iter().enumerate() {
        let subject = format!("Message {index}");
        let id = account.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Alice", "alice@example.com"),
            &subject,
            "body",
            date,
        ));
        ids.push(id);
    }
    (account_id, ids)
}

#[test]
fn query_every_id_with_sort_ascending_and_descending() {
    let server = MockServer::builder().query_page_size(2).start();
    let (account_id, seeded_ids) = seed_five_emails(&server);
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let asc_sort = Some(vec![Comparator::ascending("receivedAt")]);
    let asc_ids = client
        .query_every_id(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Email/query",
            &account_id,
            Filter::condition(EmailQueryFilter::default()),
            asc_sort,
        )
        .unwrap();

    assert_eq!(asc_ids.len(), 5);
    assert_eq!(asc_ids, seeded_ids);

    let desc_sort = Some(vec![Comparator::descending("receivedAt")]);
    let desc_ids = client
        .query_every_id(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Email/query",
            &account_id,
            Filter::condition(EmailQueryFilter::default()),
            desc_sort,
        )
        .unwrap();

    assert_eq!(desc_ids.len(), 5);
    let mut expected_desc = seeded_ids.clone();
    expected_desc.reverse();
    assert_eq!(desc_ids, expected_desc);
}

#[test]
fn query_every_id_without_sort_returns_all_pages() {
    let server = MockServer::builder().query_page_size(2).start();
    let (account_id, _) = seed_five_emails(&server);
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let ids = client
        .query_every_id(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Email/query",
            &account_id,
            Filter::condition(EmailQueryFilter::default()),
            None,
        )
        .unwrap();

    assert_eq!(ids.len(), 5);
}

#[test]
fn email_query_every_id_sorts_correctly() {
    let server = MockServer::builder().query_page_size(2).start();
    let (account_id, seeded_ids) = seed_five_emails(&server);
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let asc_ids = client
        .email_query_every_id(
            &account_id,
            EmailQueryFilter::default(),
            Some(vec![Comparator::ascending("receivedAt")]),
        )
        .unwrap();

    assert_eq!(asc_ids, seeded_ids);
}

#[test]
fn query_every_id_fails_when_page_hides_limit_and_count_disagrees_with_total() {
    let server = MockServer::builder()
        .query_page_size(2)
        .query_hides_limit()
        .start();
    let (account_id, _) = seed_five_emails(&server);
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let result = client.query_every_id(
        &[CAPABILITY_CORE, CAPABILITY_MAIL],
        "Email/query",
        &account_id,
        Filter::condition(EmailQueryFilter::default()),
        None,
    );

    match result {
        Err(Error::Protocol(message)) => {
            assert!(message.contains("Email/query"));
            assert!(message.contains("reports a total of 5"));
        }
        other => panic!("expected Error::Protocol for short total disagreement, got {other:?}"),
    }
}
