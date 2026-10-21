// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the address books and calendars of one login against a running mock server.
//!
//! These tests also examine which methods the discovery calls. A server rejects
//! a capability that it does not advertise with `unknownCapability` (RFC 8620
//! §3.6.1), and this error fails the whole request.

use std::sync::{Arc, Mutex};

use jmap_client::{Client, Credentials};
use jmap_collection_sync::{ChildKind, Fanout, Parts};
use jmap_mock::{AccountState, DEFAULT_ACCOUNT_ID, MockServer, ServerState};
use jmap_proto::Id;
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_CONTACTS};

fn fanout_of(server: &MockServer) -> Fanout {
    fanout_with(server, Parts::ALL)
}

/// The fan-out of a login with only the given `parts` switched on.
fn fanout_with(server: &MockServer, parts: Parts) -> Fanout {
    let client = Client::connect(server.origin(), Credentials::none())
        .expect("the mock serves a session document");
    Fanout::discover(&client, parts).expect("the mock answers every listing it is asked for")
}

/// Runs `f` on the state of the default account.
fn with_account(state: &Arc<Mutex<ServerState>>, f: impl FnOnce(&mut AccountState)) {
    let mut state = state.lock().expect("the mock server thread is alive");
    let id = Id::new(DEFAULT_ACCOUNT_ID);
    f(state.account_mut(&id).expect("the default account"));
}

#[test]
fn every_subscribed_collection_of_the_resolved_account_is_a_resource() {
    let server = MockServer::builder().start();
    let state = server.state();
    with_account(&state, |account| {
        account.seed_address_book("Personal", true);
        account.seed_address_book("Shared", false);
        account.seed_calendar("Work", true);
    });

    let fanout = fanout_of(&server);

    assert_eq!(
        fanout
            .address_books
            .iter()
            .map(|book| book.name.as_str())
            .collect::<Vec<_>>(),
        ["Personal", "Shared"]
    );
    assert!(fanout.address_books[0].is_default);
    assert!(!fanout.address_books[1].is_default);
    assert_eq!(
        fanout
            .calendars
            .iter()
            .map(|calendar| calendar.name.as_str())
            .collect::<Vec<_>>(),
        ["Work"]
    );
    // `[Resource] Identity` needs the ids of the server, not listing indexes.
    assert!(fanout.address_books[0].id != fanout.address_books[1].id);
}

#[test]
fn a_collection_the_user_is_not_subscribed_to_is_no_child() {
    let server = MockServer::builder().start();
    let state = server.state();
    with_account(&state, |account| {
        let hidden = account.seed_address_book("Left behind", false);
        account
            .address_books
            .get_mut(&hidden)
            .expect("just seeded")
            .is_subscribed = Some(false);
        account.seed_address_book("Personal", true);

        let hidden = account.seed_calendar("Someone else's", false);
        account
            .calendars
            .get_mut(&hidden)
            .expect("just seeded")
            .is_subscribed = Some(false);
    });

    let fanout = fanout_of(&server);

    assert_eq!(
        fanout
            .address_books
            .iter()
            .map(|book| book.name.as_str())
            .collect::<Vec<_>>(),
        ["Personal"],
        "isSubscribed=false is the user having said no to this collection"
    );
    assert!(fanout.calendars.is_empty());
}

#[test]
fn the_servers_sort_order_is_the_order_the_resources_come_back_in() {
    let server = MockServer::builder().start();
    let state = server.state();
    with_account(&state, |account| {
        let last = account.seed_address_book("Archive", false);
        account
            .address_books
            .get_mut(&last)
            .expect("just seeded")
            .sort_order = Some(30);
        let first = account.seed_address_book("Personal", true);
        account
            .address_books
            .get_mut(&first)
            .expect("just seeded")
            .sort_order = Some(10);
    });

    let fanout = fanout_of(&server);

    assert_eq!(
        fanout
            .address_books
            .iter()
            .map(|book| book.name.as_str())
            .collect::<Vec<_>>(),
        ["Personal", "Archive"],
        "sortOrder is the server's statement about how the user wants them \
         listed, and the child sources are created in that order"
    );
}

#[test]
fn a_login_without_contacts_is_never_asked_for_address_books() {
    let server = MockServer::builder()
        .without_capability(CAPABILITY_CONTACTS)
        .start();
    let state = server.state();
    with_account(&state, |account| {
        account.seed_address_book("Unreachable", true);
        account.seed_calendar("Work", true);
    });

    let fanout = fanout_of(&server);

    assert!(fanout.address_books.is_empty());
    assert!(
        !server
            .method_calls()
            .iter()
            .any(|call| call == "AddressBook/get"),
        "a request naming a capability the server does not advertise is \
         answered with unknownCapability, which fails the whole request: {:?}",
        server.method_calls()
    );
    assert_eq!(
        fanout
            .calendars
            .iter()
            .map(|calendar| calendar.name.as_str())
            .collect::<Vec<_>>(),
        ["Work"],
        "the calendars are untouched by contacts being absent"
    );
}

#[test]
fn a_login_without_calendars_is_never_asked_for_them() {
    let server = MockServer::builder()
        .without_capability(CAPABILITY_CALENDARS)
        .start();
    let state = server.state();
    with_account(&state, |account| {
        account.seed_calendar("Unreachable", true);
    });

    let fanout = fanout_of(&server);

    assert!(fanout.calendars.is_empty());
    assert!(
        !server
            .method_calls()
            .iter()
            .any(|call| call == "Calendar/get"),
        "{:?}",
        server.method_calls()
    );
}

#[test]
fn an_account_offering_contacts_and_holding_none_warrants_no_address_book() {
    let server = MockServer::builder().start();

    let fanout = fanout_of(&server);

    // The discovery asks, and an empty list (a new account) is not an error.
    assert!(
        server
            .method_calls()
            .iter()
            .any(|call| call == "AddressBook/get")
    );
    assert!(fanout.address_books.is_empty());
    assert!(fanout.calendars.is_empty());
    assert!(
        fanout.layout.mail.is_some(),
        "an account with no books and no calendars is still a mail account"
    );
}

#[test]
fn a_part_the_user_switched_off_is_never_asked_about() {
    // The user switched off contacts: do not fetch them (and maybe ask for credentials).
    let server = MockServer::builder().start();
    let state = server.state();
    with_account(&state, |account| {
        account.seed_address_book("Not wanted here", true);
        account.seed_calendar("Work", true);
    });

    let fanout = fanout_with(
        &server,
        Parts {
            contacts: false,
            ..Parts::ALL
        },
    );

    assert!(
        !server
            .method_calls()
            .iter()
            .any(|call| call == "AddressBook/get"),
        "{:?}",
        server.method_calls()
    );
    assert!(fanout.address_books.is_empty());
    assert!(
        fanout
            .children()
            .iter()
            .all(|child| child.kind == ChildKind::Calendar),
        "a part that was never asked about warrants no child"
    );
    assert_eq!(
        fanout
            .calendars
            .iter()
            .map(|calendar| calendar.name.as_str())
            .collect::<Vec<_>>(),
        ["Work"],
        "the part that is on is populated as usual"
    );
}

#[test]
fn an_account_with_nothing_switched_on_is_asked_nothing_at_all() {
    // No part is on (`e_collection_backend_get_part_enabled()` is false), so no request.
    let server = MockServer::builder().start();
    let state = server.state();
    with_account(&state, |account| {
        account.seed_address_book("Personal", true);
        account.seed_calendar("Work", true);
    });

    let fanout = fanout_with(&server, Parts::NONE);

    assert!(
        !server
            .method_calls()
            .iter()
            .any(|call| call == "AddressBook/get" || call == "Calendar/get"),
        "{:?}",
        server.method_calls()
    );
    assert!(fanout.children().is_empty());
}

/// A mock with a shared account `zz-shared` (`isPersonal: false`) that holds
/// the address book `Team book` and the calendar `Team calendar`.
fn with_a_share() -> MockServer {
    let server = MockServer::builder().start();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.add_account(Id::new("zz-shared"), "owner@example.com");
        account.is_personal = false;
        account.seed_address_book("Team book", true);
        account.seed_calendar("Team calendar", true);
    }
    server
}

fn listed_names(listing: Option<&[jmap_collection_sync::Resource]>) -> Option<Vec<&str>> {
    listing.map(|resources| {
        resources
            .iter()
            .map(|resource| resource.name.as_str())
            .collect()
    })
}

#[test]
fn the_collections_of_a_shared_account_are_listed() {
    let server = with_a_share();

    let fanout = fanout_of(&server);

    assert_eq!(fanout.shared.len(), 1, "{:?}", fanout.shared);
    let listing = &fanout.shared[0];
    assert_eq!(listing.account.id, Id::new("zz-shared"));
    assert_eq!(
        listed_names(listing.listing(ChildKind::AddressBook)),
        Some(vec!["Team book"])
    );
    assert_eq!(
        listed_names(listing.listing(ChildKind::Calendar)),
        Some(vec!["Team calendar"])
    );
}

#[test]
fn a_part_the_user_switched_off_is_not_listed_in_a_shared_account() {
    // `None`, not an empty listing: an empty one makes the populate remove the
    // share's existing children.
    for kind in [ChildKind::AddressBook, ChildKind::Calendar] {
        let server = with_a_share();
        let parts = match kind {
            ChildKind::AddressBook => Parts {
                contacts: false,
                ..Parts::ALL
            },
            ChildKind::Calendar => Parts {
                calendars: false,
                ..Parts::ALL
            },
        };

        let fanout = fanout_with(&server, parts);

        assert_eq!(fanout.shared.len(), 1, "{:?}", fanout.shared);
        assert_eq!(
            fanout.shared[0].listing(kind).map(<[_]>::len),
            None,
            "the {kind:?} part is off, but the shared account was listed for it"
        );
    }
}
