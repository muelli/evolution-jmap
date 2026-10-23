// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the child sources of one login against a running mock server.
//!
//! The unit tests in `src/children.rs` use hand-built fan-outs. These tests
//! make sure that the ids that a server issues stay correct through the
//! listing, the resource id and the parse that `dup_resource_id` does.

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

/// Runs `f` on the state of the default account.
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
    // EDS needs `dup_resource_id` to return the resource id that
    // `e_collection_backend_new_child` got, or the next populate adds a second source.
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
    // `[Resource] Identity` must be a server-issued id, and `resource_id_for` must
    // rebuild the resource id from it (as `dup_resource_id` does after a restart).
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

    // The connection settings have no effect on this test.
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
    // A mail-only account is valid: it gives an empty child list, not an error.
    let server = MockServer::builder()
        .without_capability(jmap_proto::session::CAPABILITY_CONTACTS)
        .without_capability(jmap_proto::session::CAPABILITY_CALENDARS)
        .start();

    let fanout = fanout_of(&server);

    assert!(fanout.layout.mail.is_some(), "the login still serves mail");
    assert!(fanout.children().is_empty());
}

#[test]
fn a_shared_account_named_as_the_user_gives_children_apart_from_the_users_own() {
    // A hostile server names the shared account as the user. Both accounts hold
    // a "Team" book and calendar with the same ids.
    let server = MockServer::builder()
        .shared_account_name_copies_user()
        .start();
    let shared = Id::new("A2");
    {
        let state = server.state();
        let mut state = state.lock().expect("the mock server thread is alive");
        let own = state
            .account_mut(&Id::new(DEFAULT_ACCOUNT_ID))
            .expect("the default account");
        own.seed_address_book("Team", true);
        own.seed_calendar("Team", true);
        let sharer = state.add_account(shared.clone(), "bob@example.com");
        sharer.is_personal = false;
        sharer.seed_address_book("Team", true);
        sharer.seed_calendar("Team", true);
    }

    let client = Client::connect(server.origin(), Credentials::none())
        .expect("the mock serves a session document");
    let session = client.session();
    assert_eq!(
        session.accounts[&shared].name, session.username,
        "the mock did not copy the user's name onto the shared account"
    );
    let username = session.username.clone();
    let children = Fanout::discover(&client, Parts::ALL)
        .expect("the mock answers every listing it is asked for")
        .children();

    let shared_label = format!("{username}: Team");
    assert_eq!(
        children
            .iter()
            .map(|child| (
                child.kind,
                child.account_id.as_str(),
                child.display_name.as_str()
            ))
            .collect::<Vec<_>>(),
        [
            (ChildKind::AddressBook, DEFAULT_ACCOUNT_ID, "Team"),
            (ChildKind::Calendar, DEFAULT_ACCOUNT_ID, "Team"),
            (ChildKind::AddressBook, "A2", shared_label.as_str()),
            (ChildKind::Calendar, "A2", shared_label.as_str()),
        ],
        "the owner prefix still marks the shared children, and a name never picks the account"
    );
    for (own, other) in children[..2].iter().zip(&children[2..]) {
        assert_eq!(
            own.collection_id, other.collection_id,
            "the fixture must give both accounts the same collection ids"
        );
        assert_ne!(
            own.resource_id, other.resource_id,
            "the shared {:?} merged into the user's own source",
            own.kind
        );
        assert_eq!(other.identity(), format!("A2/{}", other.collection_id));
    }
}
