// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `ParticipantIdentity/get`, `/changes` and `/set`
//! (draft-ietf-jmap-calendars-28 section 3): how an account tells the
//! server which calendar addresses are its own, for iTIP scheduling.

use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::State;
use jmap_proto::calendars::ParticipantIdentity;

const ALICE: &str = "mailto:alice@example.com";

fn connect() -> (MockServer, Client) {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    (server, client)
}

fn set_error(err: Error) -> String {
    match err {
        Error::Set(set_error) => set_error.error_type,
        other => panic!("expected Error::Set, got {other:?}"),
    }
}

#[test]
fn get_all_returns_the_seeded_identity() {
    let (server, client) = connect();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_participant_identity("Alice", ALICE, true);
    }

    let identities = client.participant_identities(&account_id).unwrap();
    assert_eq!(identities.len(), 1);
    assert_eq!(identities[0].name, "Alice");
    assert_eq!(identities[0].calendar_address.as_deref(), Some(ALICE));
    assert_eq!(identities[0].is_default, Some(true));
}

#[test]
fn get_reports_unknown_ids_as_not_found() {
    let (server, client) = connect();
    let account_id = server.account_id();

    let arguments = client
        .single_call(
            &[
                jmap_proto::session::CAPABILITY_CORE,
                jmap_proto::session::CAPABILITY_CALENDARS,
            ],
            "ParticipantIdentity/get",
            &jmap_proto::methods::GetRequest::ids(account_id.clone(), ["nonexistent"]),
        )
        .unwrap();
    let response: jmap_proto::methods::GetResponse<ParticipantIdentity> =
        serde_json::from_value(arguments).unwrap();
    assert!(response.list.is_empty());
    assert_eq!(
        response.not_found,
        vec![jmap_proto::Id::from("nonexistent")]
    );
}

#[test]
fn the_first_identity_an_account_creates_becomes_the_default() {
    let (server, client) = connect();
    let account_id = server.account_id();

    let created = client
        .participant_identity_create(&account_id, &ParticipantIdentity::new("Alice"))
        .unwrap();

    assert!(created.id.is_some());
    assert_eq!(created.is_default, Some(true));
}

#[test]
fn a_second_identity_is_not_default() {
    let (server, client) = connect();
    let account_id = server.account_id();
    client
        .participant_identity_create(&account_id, &ParticipantIdentity::new("Alice"))
        .unwrap();

    let second = client
        .participant_identity_create(&account_id, &ParticipantIdentity::new("Alice Work"))
        .unwrap();

    assert_eq!(second.is_default, Some(false));
}

#[test]
fn create_rejects_a_client_supplied_id() {
    let (server, client) = connect();
    let account_id = server.account_id();

    let err = client
        .participant_identity_create(
            &account_id,
            &ParticipantIdentity::new("Alice").with_id("pi_1"),
        )
        .unwrap_err();

    assert_eq!(set_error(err), jmap_proto::error::set::INVALID_PROPERTIES);
}

#[test]
fn create_rejects_a_client_supplied_is_default() {
    let (server, client) = connect();
    let account_id = server.account_id();

    let err = client
        .participant_identity_create(
            &account_id,
            &ParticipantIdentity::new("Alice").is_default(false),
        )
        .unwrap_err();

    assert_eq!(set_error(err), jmap_proto::error::set::INVALID_PROPERTIES);
}

#[test]
fn destroying_the_default_identity_is_refused() {
    let (server, client) = connect();
    let account_id = server.account_id();
    let identity = client
        .participant_identity_create(&account_id, &ParticipantIdentity::new("Alice"))
        .unwrap();

    let err = client
        .participant_identity_destroy(&account_id, identity.id.as_ref().unwrap())
        .unwrap_err();

    assert_eq!(
        set_error(err),
        jmap_proto::calendars::participant_identity_set_error::CANNOT_DESTROY_DEFAULT
    );
}

#[test]
fn destroying_a_non_default_identity_succeeds() {
    let (server, client) = connect();
    let account_id = server.account_id();
    client
        .participant_identity_create(&account_id, &ParticipantIdentity::new("Alice"))
        .unwrap();
    let second = client
        .participant_identity_create(&account_id, &ParticipantIdentity::new("Alice Work"))
        .unwrap();

    client
        .participant_identity_destroy(&account_id, second.id.as_ref().unwrap())
        .unwrap();

    let remaining = client.participant_identities(&account_id).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].name, "Alice");
}

#[test]
fn set_default_promotes_another_identity_and_demotes_the_old_one() {
    let (server, client) = connect();
    let account_id = server.account_id();
    let first = client
        .participant_identity_create(&account_id, &ParticipantIdentity::new("Alice"))
        .unwrap();
    let second = client
        .participant_identity_create(&account_id, &ParticipantIdentity::new("Alice Work"))
        .unwrap();

    client
        .participant_identity_set_default(&account_id, second.id.as_ref().unwrap())
        .unwrap();

    let identities = client.participant_identities(&account_id).unwrap();
    let by_id = |id: &jmap_proto::Id| identities.iter().find(|i| i.id.as_ref() == Some(id));
    assert_eq!(
        by_id(first.id.as_ref().unwrap()).unwrap().is_default,
        Some(false)
    );
    assert_eq!(
        by_id(second.id.as_ref().unwrap()).unwrap().is_default,
        Some(true)
    );

    // The old default is no longer the default, so it can now be destroyed.
    client
        .participant_identity_destroy(&account_id, first.id.as_ref().unwrap())
        .unwrap();
}

#[test]
fn set_default_with_an_unknown_id_is_silently_ignored() {
    let (server, client) = connect();
    let account_id = server.account_id();
    let first = client
        .participant_identity_create(&account_id, &ParticipantIdentity::new("Alice"))
        .unwrap();

    client
        .participant_identity_set_default(&account_id, &jmap_proto::Id::new("nonexistent"))
        .unwrap();

    let identities = client.participant_identities(&account_id).unwrap();
    assert_eq!(identities[0].id, first.id);
    assert_eq!(identities[0].is_default, Some(true));
}

#[test]
fn update_can_rename_but_not_change_is_default() {
    let (server, client) = connect();
    let account_id = server.account_id();
    let identity = client
        .participant_identity_create(&account_id, &ParticipantIdentity::new("Alice"))
        .unwrap();
    let id = identity.id.clone().unwrap();

    client
        .participant_identity_update(
            &account_id,
            &id,
            serde_json::json!({"name": "Alice Renamed"}),
        )
        .unwrap();
    let renamed = client.participant_identities(&account_id).unwrap();
    assert_eq!(renamed[0].name, "Alice Renamed");

    let err = client
        .participant_identity_update(&account_id, &id, serde_json::json!({"isDefault": false}))
        .unwrap_err();
    assert_eq!(set_error(err), jmap_proto::error::set::INVALID_PROPERTIES);
}

#[test]
fn changes_reports_a_newly_created_identity() {
    let (server, client) = connect();
    let account_id = server.account_id();

    let created = client
        .participant_identity_create(&account_id, &ParticipantIdentity::new("Alice"))
        .unwrap();

    let response = client
        .changes(&account_id, "ParticipantIdentity", &State::new("0"))
        .unwrap();
    assert_eq!(response.created, vec![created.id.unwrap()]);
    assert!(response.updated.is_empty());
    assert!(response.destroyed.is_empty());
}
