// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Creating a collection on a running server, and the child it becomes.
//!
//! `tests/resources.rs` covers the read direction — what a login holds. This is
//! the write one: `AddressBook/set` and `Calendar/set` against a real
//! `jmap-mockd` over HTTP, and the [`Child`] derived from what the server
//! answered.
//!
//! The thing worth testing here that no unit test can show is the *join*: that
//! the child a create produces is the child the next discovery would produce for
//! the same collection. A create that got the resource id, the account id or the
//! identity even slightly different would not fail — it would put a second source
//! in the sidebar for one server-side address book on the next populate, or a
//! source whose cache file EDS deletes.

use jmap_client::{Client, Credentials};
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

/// The child the discovery finds for `resource_id`, which is what a created
/// child has to be identical to.
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

    // The join: the same collection, found the way a populate finds it.
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
    // Ids are scoped per account *and per object type* (RFC 8620 §1.2), so a
    // server that numbers its objects from one gives both the same id — and the
    // resource id namespace is flat. A create that named children by the bare id
    // would have the second resolve to the first one's source.
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
    // The other half of "the create is what the discovery finds": exactly one
    // collection appears, under the name that was asked for. A create that sent
    // the object twice, or that created it in the wrong account, shows up here.
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

#[test]
fn a_login_whose_server_serves_no_contacts_refuses_the_create() {
    // Not "send it to the primary account and hope": on a server whose contacts
    // and calendars live in different accounts, guessing would put the new
    // address book in an account this backend never lists, and a collection that
    // exists on the server and never appears in Evolution is worse than one that
    // was refused.
    let server = MockServer::builder()
        .without_capability(CAPABILITY_CONTACTS)
        .start();

    let failure = create_collection(&client(&server), &requested(ChildKind::AddressBook, "Work"))
        .expect_err("the login serves no contacts");

    assert!(
        matches!(failure, CreateFailure::Unserved(ChildKind::AddressBook)),
        "expected an Unserved(AddressBook), got {failure:?}"
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
}
