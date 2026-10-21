// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `AddressBook/set` and `Calendar/set` creates against a running mock
//! server over HTTP, and the [`Child`] made from the response.
//!
//! The main check: a created child must be equal to the child that the next
//! discovery finds. Otherwise the next populate adds a second source, or EDS
//! deletes the cache file of the source.

use jmap_client::{Client, Credentials, Error};
use jmap_collection_sync::{
    Child, ChildKind, CreateFailure, Fanout, Parts, Requested, create_collection,
};
use jmap_mock::{DEFAULT_ACCOUNT_ID, MockServer};
use jmap_proto::Id;
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_CONTACTS};

fn client(server: &MockServer) -> Client {
    Client::connect(server.origin(), Credentials::none())
        .expect("the mock serves a session document")
}

fn requested(kind: ChildKind, display_name: &str) -> Requested {
    Requested {
        kind,
        display_name: display_name.to_owned(),
    }
}

/// The child that a new discovery finds for `resource_id`.
fn discovered(server: &MockServer, resource_id: &str) -> Child {
    let fanout = Fanout::discover(&client(server), Parts::ALL)
        .expect("the mock answers every listing it is asked for");
    fanout
        .children()
        .into_iter()
        .find(|child| child.resource_id == resource_id)
        .unwrap_or_else(|| panic!("the discovery did not list {resource_id}"))
}

#[test]
fn creating_an_address_book_makes_one_on_the_server_and_answers_its_child() {
    let server = MockServer::builder().start();

    let child = create_collection(&client(&server), &requested(ChildKind::AddressBook, "Work"))
        .expect("the mock creates address books");

    assert_eq!(child.kind, ChildKind::AddressBook);
    assert_eq!(child.display_name, "Work");
    assert_eq!(child.account_id, Id::new(DEFAULT_ACCOUNT_ID));
    assert_eq!(
        child.resource_id,
        ChildKind::AddressBook.resource_id(&child.collection_id),
        "the resource id has to be derived from the id the server assigned"
    );

    // The populate must find a child equal to the created child.
    assert_eq!(
        discovered(&server, &child.resource_id),
        child,
        "a created child differs from the one the next discovery writes"
    );
}

#[test]
fn creating_a_calendar_makes_one_on_the_server_and_answers_its_child() {
    let server = MockServer::builder().start();

    let child = create_collection(&client(&server), &requested(ChildKind::Calendar, "Trips"))
        .expect("the mock creates calendars");

    assert_eq!(child.kind, ChildKind::Calendar);
    assert_eq!(child.display_name, "Trips");
    assert_eq!(
        child.resource_id,
        ChildKind::Calendar.resource_id(&child.collection_id)
    );
    assert_eq!(discovered(&server, &child.resource_id), child);
}

#[test]
fn an_address_book_and_a_calendar_created_together_stay_two_children() {
    // Ids are unique per account and type (RFC 8620 §1.2): both can get the same id.
    let server = MockServer::builder().start();
    let client = client(&server);

    let book = create_collection(&client, &requested(ChildKind::AddressBook, "Work"))
        .expect("the mock creates address books");
    let calendar = create_collection(&client, &requested(ChildKind::Calendar, "Work"))
        .expect("the mock creates calendars");

    assert_ne!(book.resource_id, calendar.resource_id);
    assert_eq!(
        discovered(&server, &book.resource_id).kind,
        ChildKind::AddressBook
    );
    assert_eq!(
        discovered(&server, &calendar.resource_id).kind,
        ChildKind::Calendar
    );
}

#[test]
fn a_created_collection_is_the_only_new_child_of_its_kind() {
    // Exactly one collection must appear, with the requested name.
    let server = MockServer::builder().start();
    let before = Fanout::discover(&client(&server), Parts::ALL).expect("the mock answers");

    create_collection(&client(&server), &requested(ChildKind::AddressBook, "Work"))
        .expect("the mock creates address books");

    let after = Fanout::discover(&client(&server), Parts::ALL).expect("the mock answers");
    assert_eq!(after.address_books.len(), before.address_books.len() + 1);
    assert_eq!(
        after.calendars.len(),
        before.calendars.len(),
        "creating an address book created a calendar too"
    );
    assert!(after.address_books.iter().any(|book| book.name == "Work"));
}

/// A `created` response can omit an unchanged `name` (RFC 8620 §5.3).
#[test]
fn a_create_whose_response_omits_the_name_shows_the_requested_name_not_the_id() {
    let server = MockServer::builder().terse_collection_create().start();

    for kind in [ChildKind::AddressBook, ChildKind::Calendar] {
        let child = create_collection(&client(&server), &requested(kind, "testcal"))
            .unwrap_or_else(|error| panic!("the mock creates {kind:?}s: {error}"));

        assert_eq!(
            child.display_name, "testcal",
            "a create whose response omits `name` must keep the requested name"
        );
    }
}

/// The server keeps the padding of a name, so the client must not trim it.
#[test]
fn a_create_whose_response_omits_the_name_keeps_whitespace_padding() {
    let server = MockServer::builder().terse_collection_create().start();

    for kind in [ChildKind::AddressBook, ChildKind::Calendar] {
        let child = create_collection(&client(&server), &requested(kind, "  Padded Name  "))
            .unwrap_or_else(|error| panic!("the mock creates {kind:?}s: {error}"));

        assert_eq!(
            child.display_name, "  Padded Name  ",
            "a create whose response omits `name` must keep the requested name \
             exactly, padding included, not just its trimmed content"
        );
    }
}

#[test]
fn a_login_whose_server_serves_no_contacts_refuses_the_create() {
    // Do not guess an account: Evolution does not show collections in unlisted accounts.
    let server = MockServer::builder()
        .without_capability(CAPABILITY_CONTACTS)
        .start();

    let failure = create_collection(&client(&server), &requested(ChildKind::AddressBook, "Work"))
        .expect_err("the login serves no contacts");

    assert!(
        matches!(failure, CreateFailure::Unserved(ChildKind::AddressBook)),
        "expected an Unserved(AddressBook), got {failure:?}"
    );
    assert_eq!(
        failure.to_string(),
        "this account's JMAP server offers no contacts to create one in"
    );
}

#[test]
fn a_login_whose_server_serves_no_calendars_refuses_the_create() {
    let server = MockServer::builder()
        .without_capability(CAPABILITY_CALENDARS)
        .start();

    let failure = create_collection(&client(&server), &requested(ChildKind::Calendar, "Trips"))
        .expect_err("the login serves no calendars");

    assert!(
        matches!(failure, CreateFailure::Unserved(ChildKind::Calendar)),
        "expected an Unserved(Calendar), got {failure:?}"
    );
    assert_eq!(
        failure.to_string(),
        "this account's JMAP server offers no calendars to create one in"
    );
}

/// Stalwart sets `isSubscribed: false` on new collections, and discovery drops them.
///
/// `create_collection` must request `isSubscribed: true`. The mock copies Stalwart.
#[test]
fn a_created_collection_is_discoverable_even_when_the_server_defaults_new_collections_to_unsubscribed()
 {
    let server = MockServer::builder()
        .new_collections_default_unsubscribed()
        .start();

    for kind in [ChildKind::AddressBook, ChildKind::Calendar] {
        let child = create_collection(&client(&server), &requested(kind, "Work"))
            .unwrap_or_else(|error| panic!("the mock creates {kind:?}s: {error}"));

        // `discovered` panics if the new discovery does not find the resource id.
        assert_eq!(
            discovered(&server, &child.resource_id),
            child,
            "a freshly created {kind:?} must be discoverable immediately, \
             not only once some other client subscribes it"
        );
    }
}

/// A `notCreated` entry (RFC 8620 §5.3) must become `CreateFailure::Client`, not a panic.
#[test]
fn a_create_the_server_refuses_is_a_client_failure_not_a_panic() {
    let server = MockServer::builder().reject_collection_create().start();

    for kind in [ChildKind::AddressBook, ChildKind::Calendar] {
        let failure = create_collection(&client(&server), &requested(kind, "Work"))
            .expect_err("the mock is configured to refuse every create");

        assert!(
            matches!(failure, CreateFailure::Client(Error::Set(_))),
            "expected a Client(Set(_)) failure for {kind:?}, got {failure:?}"
        );
    }
}

/// A `created` entry without an `id` breaks RFC 8620 §5.3 and must be a client failure.
#[test]
fn a_create_whose_response_omits_the_id_is_a_client_failure_not_a_panic() {
    let server = MockServer::builder()
        .id_omitting_collection_create()
        .start();

    for kind in [ChildKind::AddressBook, ChildKind::Calendar] {
        let failure = create_collection(&client(&server), &requested(kind, "Work"))
            .expect_err("the mock omits id from every create response");

        assert!(
            matches!(failure, CreateFailure::Client(Error::Protocol(_))),
            "expected a Client(Protocol(_)) failure for {kind:?}, got {failure:?}"
        );
        let CreateFailure::Client(error) = failure else {
            unreachable!()
        };
        assert!(
            error.to_string().contains("reported no id"),
            "expected the missing-id message, got: {error}"
        );
    }
}
