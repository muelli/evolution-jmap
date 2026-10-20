// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the delete of a collection on a running mock server.
//!
//! The delete must destroy only the named collection. The mock data makes these
//! two silent failures visible: a destroy through the wrong `/set` method, and a
//! destroy in the wrong JMAP account.

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

/// All address books and calendars on the server, as a populate lists them.
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
    // Ids are unique per account and type (RFC 8620 §1.2): both get the same id.
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
    // On `TRUE`, EDS removes the source, and the next populate adds it again.
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
    // Do not use the primary account: there, the id can name a different object.
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

/// A mock with a shared account `zz-shared` (`isPersonal: false`).
///
/// The own account stays primary. Both accounts hold a calendar and an address book
/// with the same ids.
fn with_a_share() -> (MockServer, Id, Id, Id) {
    let server = MockServer::builder().start();
    let own = server.account_id();
    let shared = Id::new("zz-shared");
    let (calendar, book) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let mine = state.account_mut(&own).expect("the mock's own account");
        mine.seed_calendar("Mine", false);
        mine.seed_address_book("Mine", false);
        let account = state.add_account(shared.clone(), "owner@example.com");
        account.is_personal = false;
        (
            account.seed_calendar("Team", true),
            account.seed_address_book("Team", true),
        )
    };
    (server, shared, calendar, book)
}

fn held_by(server: &MockServer, account: &Id, kind: ChildKind) -> Vec<Id> {
    let state = server.state();
    let state = state.lock().unwrap();
    let account = state.account(account).expect("the account is seeded");
    match kind {
        ChildKind::AddressBook => account
            .address_books
            .iter()
            .map(|(id, _)| id.clone())
            .collect(),
        ChildKind::Calendar => account.calendars.iter().map(|(id, _)| id.clone()).collect(),
    }
}

#[test]
fn deleting_a_shared_collection_destroys_it_in_the_sharing_account_only() {
    for kind in [ChildKind::Calendar, ChildKind::AddressBook] {
        let (server, shared, calendar, book) = with_a_share();
        let collection = match kind {
            ChildKind::Calendar => calendar,
            ChildKind::AddressBook => book,
        };
        let own = server.account_id();
        let own_before = held_by(&server, &own, kind);
        assert!(
            own_before.contains(&collection),
            "the fixture must give the user's own account a {kind:?} with the shared one's id"
        );

        delete_collection(
            &client(&server),
            &Doomed {
                kind,
                collection_id: Id::new(format!("{shared}/{collection}")),
            },
        )
        .expect("the mock destroys collections of a shared account");

        assert!(
            !held_by(&server, &shared, kind).contains(&collection),
            "the shared {kind:?} is still on the server"
        );
        assert_eq!(
            held_by(&server, &own, kind),
            own_before,
            "the delete of a shared {kind:?} touched the user's own account"
        );
    }
}

#[test]
fn deleting_from_an_account_that_is_no_longer_shared_is_refused() {
    // The share is gone: the primary account must not get the destroy.
    let (server, _, calendar, _) = with_a_share();
    let own = server.account_id();
    let own_before = held_by(&server, &own, ChildKind::Calendar);

    let failure = delete_collection(
        &client(&server),
        &Doomed {
            kind: ChildKind::Calendar,
            collection_id: Id::new(format!("withdrawn/{calendar}")),
        },
    )
    .expect_err("no account `withdrawn` is shared with this login");

    assert!(
        matches!(failure, DeleteFailure::Unshared(ChildKind::Calendar)),
        "expected an Unshared(Calendar), got {failure:?}"
    );
    assert_eq!(held_by(&server, &own, ChildKind::Calendar), own_before);
}

#[test]
fn deleting_through_a_personal_account_id_in_the_identity_is_refused() {
    // Only a shared account (`isPersonal: false`) can be `<accountId>` in an identity.
    let (server, _, calendar, _) = with_a_share();
    let own = server.account_id();

    let failure = delete_collection(
        &client(&server),
        &Doomed {
            kind: ChildKind::Calendar,
            collection_id: Id::new(format!("{own}/{calendar}")),
        },
    )
    .expect_err("the user's own account is not a share");

    assert!(
        matches!(failure, DeleteFailure::Unshared(ChildKind::Calendar)),
        "expected an Unshared(Calendar), got {failure:?}"
    );
    assert!(held_by(&server, &own, ChildKind::Calendar).contains(&calendar));
}
