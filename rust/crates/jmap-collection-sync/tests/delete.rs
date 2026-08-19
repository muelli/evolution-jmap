// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deleting a collection from a running server — `tests/create.rs`'s mirror,
//! and the one operation in this crate that destroys something the user cannot
//! get back.
//!
//! The create tests are about a *join*: that a created child is the child the
//! next discovery would write. A delete has no join to check, so what is tested
//! instead is the other property, which is the one with teeth — that the
//! collection that disappears is the one that was named and nothing else. Two
//! failures in particular have no symptom short of lost data: a destroy sent
//! through the wrong `/set` call, and a destroy sent to the wrong JMAP account.
//! The mock is seeded so that both would show up here.

use jmap_client::{Client, Credentials};
use jmap_collection_sync::{
    ChildKind, DeleteFailure, Doomed, Fanout, Parts, Requested, create_collection,
    delete_collection,
};
use jmap_mock::MockServer;
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

/// Every collection of both kinds the server holds, as a populate would list
/// them.
fn discovered(server: &MockServer) -> Fanout {
    Fanout::discover(&client(server), Parts::ALL)
        .expect("the mock answers every listing it is asked for")
}

fn address_book_names(fanout: &Fanout) -> Vec<String> {
    fanout
        .address_books
        .iter()
        .map(|book| book.name.clone())
        .collect()
}

fn calendar_names(fanout: &Fanout) -> Vec<String> {
    fanout
        .calendars
        .iter()
        .map(|calendar| calendar.name.clone())
        .collect()
}

#[test]
fn deleting_an_address_book_takes_it_off_the_server() {
    let server = MockServer::builder().start();
    let child = create_collection(&client(&server), &requested(ChildKind::AddressBook, "Work"))
        .expect("the mock creates address books");
    assert!(address_book_names(&discovered(&server)).contains(&"Work".to_owned()));

    delete_collection(
        &client(&server),
        &Doomed {
            kind: ChildKind::AddressBook,
            collection_id: child.collection_id.clone(),
        },
    )
    .expect("the mock destroys address books");

    assert!(
        !address_book_names(&discovered(&server)).contains(&"Work".to_owned()),
        "the address book is still on the server"
    );
}

#[test]
fn deleting_a_calendar_takes_it_off_the_server() {
    let server = MockServer::builder().start();
    let child = create_collection(&client(&server), &requested(ChildKind::Calendar, "Trips"))
        .expect("the mock creates calendars");

    delete_collection(
        &client(&server),
        &Doomed {
            kind: ChildKind::Calendar,
            collection_id: child.collection_id.clone(),
        },
    )
    .expect("the mock destroys calendars");

    assert!(
        !calendar_names(&discovered(&server)).contains(&"Trips".to_owned()),
        "the calendar is still on the server"
    );
}

#[test]
fn deleting_an_address_book_leaves_a_calendar_of_the_same_id_alone() {
    // The failure this file exists for. Ids are scoped per account *and per
    // object type* (RFC 8620 §1.2), so an address book and a calendar may both
    // be `X1` — and on a server that numbers its objects from one, they will be.
    // A delete that read the id and guessed the `/set` call would destroy the
    // user's calendar because they asked to remove an address book, and nothing
    // would report an error: the destroy succeeds, on the wrong object.
    let server = MockServer::builder().start();
    let client = client(&server);
    let book = create_collection(&client, &requested(ChildKind::AddressBook, "Work"))
        .expect("the mock creates address books");
    let calendar = create_collection(&client, &requested(ChildKind::Calendar, "Work"))
        .expect("the mock creates calendars");

    delete_collection(
        &client,
        &Doomed {
            kind: ChildKind::AddressBook,
            collection_id: book.collection_id.clone(),
        },
    )
    .expect("the mock destroys address books");

    let after = discovered(&server);
    assert!(
        !address_book_names(&after).contains(&"Work".to_owned()),
        "the address book that was named is still there"
    );
    assert!(
        after
            .calendars
            .iter()
            .any(|held| held.id == calendar.collection_id),
        "the calendar was destroyed by a delete that named an address book"
    );
}

#[test]
fn deleting_the_collection_leaves_every_other_one_of_its_kind_alone() {
    let server = MockServer::builder().start();
    let client = client(&server);
    let doomed = create_collection(&client, &requested(ChildKind::AddressBook, "Work"))
        .expect("the mock creates address books");
    let spared = create_collection(&client, &requested(ChildKind::AddressBook, "Home"))
        .expect("the mock creates address books");

    delete_collection(
        &client,
        &Doomed {
            kind: ChildKind::AddressBook,
            collection_id: doomed.collection_id.clone(),
        },
    )
    .expect("the mock destroys address books");

    let after = discovered(&server);
    assert!(
        after
            .address_books
            .iter()
            .any(|held| held.id == spared.collection_id),
        "an address book that was not named was destroyed too"
    );
}

#[test]
fn deleting_a_collection_the_server_does_not_hold_is_an_error() {
    // Reported rather than shrugged at: a destroy the server refused means the
    // collection is still there, and answering the vfunc `TRUE` would have EDS
    // remove the child source for a collection that goes on existing — which the
    // next populate then writes a *new* source for.
    let server = MockServer::builder().start();

    let failure = delete_collection(
        &client(&server),
        &Doomed {
            kind: ChildKind::AddressBook,
            collection_id: Id::new("no-such-address-book"),
        },
    )
    .expect_err("the mock holds no such address book");

    assert!(
        matches!(failure, DeleteFailure::Client(_)),
        "expected the server's own refusal, got {failure:?}"
    );
}

#[test]
fn a_login_whose_server_serves_no_contacts_refuses_the_delete() {
    // The same rule the create applies, and for a sharper reason: the account
    // that serves contacts is resolved from the session document, so a delete
    // that fell back to the primary account would send a destroy naming this id
    // to an account where that id means something else entirely.
    let server = MockServer::builder()
        .without_capability(CAPABILITY_CONTACTS)
        .start();

    let failure = delete_collection(
        &client(&server),
        &Doomed {
            kind: ChildKind::AddressBook,
            collection_id: Id::new("AB1"),
        },
    )
    .expect_err("the login serves no contacts");

    assert!(
        matches!(failure, DeleteFailure::Unserved(ChildKind::AddressBook)),
        "expected an Unserved(AddressBook), got {failure:?}"
    );
}

#[test]
fn a_login_whose_server_serves_no_calendars_refuses_the_delete() {
    let server = MockServer::builder()
        .without_capability(CAPABILITY_CALENDARS)
        .start();

    let failure = delete_collection(
        &client(&server),
        &Doomed {
            kind: ChildKind::Calendar,
            collection_id: Id::new("Cal1"),
        },
    )
    .expect_err("the login serves no calendars");

    assert!(
        matches!(failure, DeleteFailure::Unserved(ChildKind::Calendar)),
        "expected an Unserved(Calendar), got {failure:?}"
    );
}
