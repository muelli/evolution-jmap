// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A save with a base copy keeps the changes of a different client, and follows
//! `OnConflict` for a property that both the user and the other client changed.

mod common;

use common::Fixture;
use jmap_book_sync::{Base, OnConflict, SyncError};
use jmap_proto::Id;
use serde_json::json;

/// Seeds "Vera Oldenburg" and returns its id and the vCard that a load gives.
fn loaded(fixture: &Fixture) -> (Id, String) {
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let vcard = fixture.sync().load_contact(id.as_str()).unwrap().vcard;
    (id, vcard)
}

fn full_name(fixture: &Fixture, id: &Id) -> String {
    let card = fixture.card(id);
    card.name.unwrap().full.unwrap()
}

fn email(fixture: &Fixture, id: &Id) -> String {
    let card = fixture.card(id);
    card.emails.unwrap()["e0"].address.clone()
}

fn base(vcard: &str, on_conflict: OnConflict) -> Option<Base<'_>> {
    Some(Base { vcard, on_conflict })
}

#[test]
fn an_entry_that_a_remote_client_added_survives_a_local_edit() {
    let fixture = Fixture::start();
    let (id, loaded) = loaded(&fixture);
    fixture.patch(
        &id,
        json!({"phones": {"p1": {"number": "+41 44 000 00 00"}}}),
    );

    let edited = loaded.replace("vera@example.com", "vera@example.org");
    fixture
        .sync()
        .save_contact_since(
            base(&loaded, OnConflict::KeepLocal),
            &edited,
            Some(id.as_str()),
        )
        .unwrap();

    let card = fixture.card(&id);
    let phones = card.phones.expect("the remote phone was removed");
    assert_eq!(phones["p1"].number, "+41 44 000 00 00");
}

#[test]
fn keep_server_keeps_the_server_value_and_still_sends_the_other_edits() {
    let fixture = Fixture::start();
    let (id, loaded) = loaded(&fixture);
    fixture.patch(&id, json!({"emails/e0/address": "vera@remote.example"}));

    let edited = loaded
        .replace("vera@example.com", "vera@example.org")
        .replace("FN:Vera Oldenburg", "FN:Vera Local");
    let saved = fixture
        .sync()
        .save_contact_since(
            base(&loaded, OnConflict::KeepServer),
            &edited,
            Some(id.as_str()),
        )
        .unwrap();

    assert_eq!(email(&fixture, &id), "vera@remote.example");
    assert_eq!(full_name(&fixture, &id), "Vera Local");
    assert!(
        saved.vcard.contains("vera@remote.example"),
        "{}",
        saved.vcard
    );
}

#[test]
fn fail_refuses_a_conflict_and_writes_nothing() {
    let fixture = Fixture::start();
    let (id, loaded) = loaded(&fixture);
    fixture.patch(&id, json!({"emails/e0/address": "vera@remote.example"}));

    let edited = loaded
        .replace("vera@example.com", "vera@example.org")
        .replace("FN:Vera Oldenburg", "FN:Vera Local");
    let result = fixture.sync().save_contact_since(
        base(&loaded, OnConflict::Fail),
        &edited,
        Some(id.as_str()),
    );

    assert!(matches!(result, Err(SyncError::OutOfSync(_))), "{result:?}");
    assert_eq!(email(&fixture, &id), "vera@remote.example");
    assert_eq!(full_name(&fixture, &id), "Vera Oldenburg");
}

#[test]
fn fail_accepts_edits_of_different_properties() {
    let fixture = Fixture::start();
    let (id, loaded) = loaded(&fixture);
    fixture.patch(&id, json!({"name/full": "Vera Remote"}));

    let edited = loaded.replace("vera@example.com", "vera@example.org");
    fixture
        .sync()
        .save_contact_since(base(&loaded, OnConflict::Fail), &edited, Some(id.as_str()))
        .unwrap();

    assert_eq!(full_name(&fixture, &id), "Vera Remote");
    assert_eq!(email(&fixture, &id), "vera@example.org");
}

#[test]
fn a_base_that_does_not_parse_falls_back_to_the_server_copy() {
    let fixture = Fixture::start();
    let (id, loaded) = loaded(&fixture);

    let edited = loaded.replace("vera@example.com", "vera@example.org");
    fixture
        .sync()
        .save_contact_since(
            base("not a vCard", OnConflict::Fail),
            &edited,
            Some(id.as_str()),
        )
        .unwrap();

    assert_eq!(email(&fixture, &id), "vera@example.org");
}

#[test]
fn an_entry_that_a_remote_client_removed_stays_removed_after_a_local_edit() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"phones": {"p1": {"number": "+41 44 000 00 00"}}}),
    );
    let loaded = fixture.sync().load_contact(id.as_str()).unwrap().vcard;
    fixture.patch(&id, json!({"phones": null}));

    let edited = loaded.replace("FN:Vera Oldenburg", "FN:Vera Local");
    fixture
        .sync()
        .save_contact_since(
            base(&loaded, OnConflict::KeepLocal),
            &edited,
            Some(id.as_str()),
        )
        .unwrap();

    let card = fixture.card(&id);
    assert!(
        card.phones.unwrap_or_default().is_empty(),
        "the phone came back"
    );
    assert_eq!(card.name.unwrap().full.as_deref(), Some("Vera Local"));
}
