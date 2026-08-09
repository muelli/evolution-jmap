// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The child sources one login warrants, off a running server.
//!
//! `src/children.rs`'s unit tests cover the naming rules against hand-built
//! fan-outs. These cover the one thing a hand-built fan-out cannot: that the
//! ids a real server hands out survive the whole way — session document,
//! listing, resource id, and back out of the parse the `dup_resource_id` vfunc
//! will do. A rule that holds for `AB1` and not for the mock's actual ids is a
//! rule that holds for nothing.

use std::sync::{Arc, Mutex};

use jmap_client::{Client, Credentials};
use jmap_collection_sync::{
    ChildKind, Connection, Fanout, Parts, parse_resource_id, resource_id_for,
};
use jmap_mock::{AccountState, DEFAULT_ACCOUNT_ID, MockServer, ServerState};
use jmap_proto::Id;

fn fanout_of(server: &MockServer) -> Fanout {
    let client = Client::connect(server.origin(), Credentials::none())
        .expect("the mock serves a session document");
    Fanout::discover(&client, Parts::ALL).expect("the mock answers every listing it is asked for")
}

/// Runs `f` against the default account's state.
fn with_account(state: &Arc<Mutex<ServerState>>, f: impl FnOnce(&mut AccountState)) {
    let mut state = state.lock().expect("the mock server thread is alive");
    let id = Id::new(DEFAULT_ACCOUNT_ID);
    f(state.account_mut(&id).expect("the default account"));
}

#[test]
fn a_child_per_collection_the_server_lists() {
    let server = MockServer::builder().start();
    let state = server.state();
    with_account(&state, |account| {
        account.seed_address_book("Personal", true);
        account.seed_address_book("Shared", false);
        account.seed_calendar("Work", true);
    });

    let fanout = fanout_of(&server);
    let children = fanout.children();

    assert_eq!(
        children
            .iter()
            .map(|child| (child.kind, child.display_name.as_str()))
            .collect::<Vec<_>>(),
        [
            (ChildKind::AddressBook, "Personal"),
            (ChildKind::AddressBook, "Shared"),
            (ChildKind::Calendar, "Work"),
        ]
    );
    for child in &children {
        assert_eq!(
            child.account_id,
            Id::new(DEFAULT_ACCOUNT_ID),
            "every child talks to the account the layout resolved for its kind"
        );
        assert!(!child.read_only, "the mock's account is not read-only");
    }
    assert!(children[0].is_default);
    assert!(!children[1].is_default);
}

#[test]
fn every_resource_id_reads_back_as_the_collection_it_was_made_from() {
    // The pairing EDS relies on: the string `populate` hands
    // `e_collection_backend_new_child` is the string `dup_resource_id` has to
    // return for the child that came back, or the next populate creates a
    // second source for a collection that already has one.
    let server = MockServer::builder().start();
    let state = server.state();
    with_account(&state, |account| {
        account.seed_address_book("Personal", true);
        account.seed_calendar("Work", true);
        account.seed_calendar("Birthdays", false);
    });

    let fanout = fanout_of(&server);
    let children = fanout.children();
    assert_eq!(children.len(), 3, "two calendars and an address book");

    for child in &children {
        assert_eq!(
            parse_resource_id(&child.resource_id),
            Some((child.kind, child.collection_id.clone())),
            "{} did not read back as itself",
            child.resource_id
        );
    }

    let mut resource_ids: Vec<&str> = children
        .iter()
        .map(|child| child.resource_id.as_str())
        .collect();
    resource_ids.sort_unstable();
    let unique = resource_ids.len();
    resource_ids.dedup();
    assert_eq!(
        resource_ids.len(),
        unique,
        "two children under one resource id are one source, not two"
    );
}

#[test]
fn the_identity_written_on_a_child_is_an_id_the_server_answers_to() {
    // Two claims at once, and neither survives hand-written ids alone: the
    // `[Resource] Identity` a child is written with is the id the book or
    // calendar backend will put in an `AddressBook/get`, so it has to be one the
    // server issued and not the prefixed resource id; and the pair
    // (extension, identity) — the only two settings that outlive a restart — is
    // what `dup_resource_id` reconstructs the resource id from.
    let server = MockServer::builder().start();
    let state = server.state();
    with_account(&state, |account| {
        account.seed_address_book("Personal", true);
        account.seed_calendar("Work", true);
    });

    let fanout = fanout_of(&server);
    let served: Vec<&str> = fanout
        .address_books
        .iter()
        .chain(&fanout.calendars)
        .map(|resource| resource.id.as_str())
        .collect();
    assert_eq!(served.len(), 2, "the mock listed both collections");

    // The connection settings are the account's, not the collection's, and
    // nothing here turns on them.
    let connection = Connection {
        host: "127.0.0.1".to_owned(),
        port: None,
        user: None,
        auth_method: None,
        secure: false,
    };

    let children = fanout.children();
    assert_eq!(children.len(), 2);
    for child in &children {
        let settings = child.settings(&connection);
        let identity = settings
            .iter()
            .find(|setting| (setting.group, setting.key) == ("Resource", "Identity"))
            .expect("every child is written with an identity");

        assert!(
            served.contains(&identity.value.as_str()),
            "{} was written with an identity the server never issued",
            child.resource_id
        );
        assert_eq!(
            resource_id_for(child.kind.extension(), &identity.value),
            Some(child.resource_id.clone()),
            "{} did not come back out of the settings it was written with",
            child.resource_id
        );
    }
}

#[test]
fn a_login_that_serves_no_collections_has_no_children() {
    // A mail-only account authenticates, resolves, and warrants no address book
    // and no calendar. That is an account, not an error — and the fan-out has
    // to say so with an empty child list rather than a child that fails on use.
    let server = MockServer::builder()
        .without_capability(jmap_proto::session::CAPABILITY_CONTACTS)
        .without_capability(jmap_proto::session::CAPABILITY_CALENDARS)
        .start();

    let fanout = fanout_of(&server);

    assert!(fanout.layout.mail.is_some(), "the login still serves mail");
    assert!(fanout.children().is_empty());
}
