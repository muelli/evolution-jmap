// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The address books and calendars behind one login, listed off a running
//! server.
//!
//! `src/resources.rs`'s unit tests cover the collection objects a server *may*
//! send; these cover the ones it does, fetched over HTTP by the same client the
//! backends use — including the thing no hand-written object can show, which is
//! *which calls the discovery makes*. A listing sent for a capability this login
//! does not have is not a slightly wasteful request: RFC 8620 §3.3 has the
//! server answer a `using` it does not advertise with `unknownCapability`, which
//! fails the whole request, so the fan-out would come back empty rather than
//! short.

use std::sync::{Arc, Mutex};

use jmap_client::{Client, Credentials};
use jmap_collection_sync::Fanout;
use jmap_mock::{AccountState, DEFAULT_ACCOUNT_ID, MockServer, ServerState};
use jmap_proto::Id;
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_CONTACTS};

fn fanout_of(server: &MockServer) -> Fanout {
    let client = Client::connect(server.origin(), Credentials::none())
        .expect("the mock serves a session document");
    Fanout::discover(&client).expect("the mock answers every listing it is asked for")
}

/// Runs `f` against the default account's state.
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
    // The ids are what a child source names in `[Resource] Identity`, so they
    // have to be the server's own, not an index into the listing.
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

    // The capability is there, so the question was asked — and answered with
    // an empty list, which is a real state (a fresh account) and not an error.
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
