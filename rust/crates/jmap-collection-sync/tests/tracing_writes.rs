// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `create_collection`/`delete_collection` trace their writes with
//! `account_id`/`kind`.

use jmap_client::{Client, Credentials};
use jmap_collection_sync::{ChildKind, Doomed, Requested, create_collection, delete_collection};
use jmap_mock::{DEFAULT_ACCOUNT_ID, MockServer};
use jmap_proto::Id;
use jmap_tracing_capture::capture;
use tracing::Level;

fn has(captured: &[(Level, String, String)], level: Level, name: &str, value: &str) -> bool {
    captured
        .iter()
        .any(|(l, n, v)| *l == level && n == name && v == value)
}

fn client(server: &MockServer) -> Client {
    Client::connect(server.origin(), Credentials::none())
        .expect("the mock serves a session document")
}

#[test]
fn creating_an_address_book_traces_the_account_and_kind_on_success() {
    let server = MockServer::builder().start();

    let (_, captured) = capture(|| {
        create_collection(
            &client(&server),
            &Requested {
                kind: ChildKind::AddressBook,
                display_name: "Work".to_owned(),
            },
        )
        .expect("the mock creates address books");
    });

    assert!(
        has(&captured, Level::DEBUG, "account_id", DEFAULT_ACCOUNT_ID),
        "expected a DEBUG account_id field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful create should not log an error field, got {captured:?}"
    );
}

#[test]
fn creating_a_collection_in_an_account_that_serves_none_traces_the_failure() {
    let server = MockServer::builder()
        .without_capability(jmap_proto::session::CAPABILITY_CONTACTS)
        .start();

    let (_, captured) = capture(|| {
        let _ = create_collection(
            &client(&server),
            &Requested {
                kind: ChildKind::AddressBook,
                display_name: "Work".to_owned(),
            },
        );
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}

#[test]
fn deleting_a_calendar_traces_the_account_and_collection_id_on_success() {
    let server = MockServer::builder().start();
    let created = capture(|| {
        create_collection(
            &client(&server),
            &Requested {
                kind: ChildKind::Calendar,
                display_name: "Trips".to_owned(),
            },
        )
        .expect("the mock creates calendars")
    })
    .0;

    let (_, captured) = capture(|| {
        delete_collection(
            &client(&server),
            &Doomed {
                kind: ChildKind::Calendar,
                collection_id: created.collection_id.clone(),
            },
        )
        .expect("the mock destroys calendars");
    });

    assert!(
        has(&captured, Level::DEBUG, "account_id", DEFAULT_ACCOUNT_ID),
        "expected a DEBUG account_id field, got {captured:?}"
    );
    assert!(
        has(
            &captured,
            Level::DEBUG,
            "collection_id",
            created.collection_id.as_ref()
        ),
        "expected a DEBUG collection_id field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful destroy should not log an error field, got {captured:?}"
    );
}

#[test]
fn deleting_a_collection_the_server_does_not_hold_traces_the_failure() {
    let server = MockServer::builder().start();

    let (_, captured) = capture(|| {
        let _ = delete_collection(
            &client(&server),
            &Doomed {
                kind: ChildKind::AddressBook,
                collection_id: Id::new("no-such-address-book"),
            },
        );
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}
