// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A save with a base copy follows `OnConflict` for a property that both the
//! user and a different client changed.

mod common;

use common::Fixture;
use jmap_cal_sync::{Base, Itip, OnConflict, SyncError};
use jmap_proto::Id;
use serde_json::json;

/// Seeds "Standup" and returns its id and the iCalendar that a load gives.
fn loaded(fixture: &Fixture) -> (Id, String) {
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let icalendar = fixture
        .sync()
        .load_component(id.as_str())
        .unwrap()
        .icalendar;
    assert!(icalendar.contains("SUMMARY:Standup\r\n"), "{icalendar}");
    (id, icalendar)
}

fn described(icalendar: &str) -> String {
    icalendar.replacen("END:VEVENT", "DESCRIPTION:new\r\nEND:VEVENT", 1)
}

fn save(
    fixture: &Fixture,
    id: &Id,
    edited: &str,
    loaded: &str,
    on_conflict: OnConflict,
) -> Result<jmap_cal_sync::ComponentInfo, SyncError> {
    fixture.sync().save_component_since(
        Some(Base {
            icalendar: loaded,
            on_conflict,
        }),
        edited,
        Some(id.as_str()),
        Itip::Send,
    )
}

#[test]
fn keep_server_keeps_the_server_value_and_still_sends_the_other_edits() {
    let fixture = Fixture::start();
    let (id, loaded) = loaded(&fixture);
    fixture.patch(&id, json!({"title": "Standup (moved)"}));

    let edited = described(&loaded).replace("SUMMARY:Standup\r\n", "SUMMARY:Standup local\r\n");
    let saved = save(&fixture, &id, &edited, &loaded, OnConflict::KeepServer).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (moved)"));
    assert_eq!(stored.description.as_deref(), Some("new"));
    assert!(
        saved.icalendar.contains("SUMMARY:Standup (moved)"),
        "{}",
        saved.icalendar
    );
}

#[test]
fn fail_refuses_a_conflict_and_writes_nothing() {
    let fixture = Fixture::start();
    let (id, loaded) = loaded(&fixture);
    fixture.patch(&id, json!({"title": "Standup (moved)"}));

    let edited = described(&loaded).replace("SUMMARY:Standup\r\n", "SUMMARY:Standup local\r\n");
    let result = save(&fixture, &id, &edited, &loaded, OnConflict::Fail);

    assert!(matches!(result, Err(SyncError::OutOfSync(_))), "{result:?}");
    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (moved)"));
    assert_eq!(stored.description.as_deref().unwrap_or(""), "");
}

#[test]
fn fail_accepts_edits_of_different_properties() {
    let fixture = Fixture::start();
    let (id, loaded) = loaded(&fixture);
    fixture.patch(&id, json!({"title": "Standup (moved)"}));

    save(
        &fixture,
        &id,
        &described(&loaded),
        &loaded,
        OnConflict::Fail,
    )
    .unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (moved)"));
    assert_eq!(stored.description.as_deref(), Some("new"));
}

#[test]
fn a_base_that_does_not_parse_falls_back_to_the_server_copy() {
    let fixture = Fixture::start();
    let (id, loaded) = loaded(&fixture);

    save(
        &fixture,
        &id,
        &described(&loaded),
        "not iCalendar",
        OnConflict::Fail,
    )
    .unwrap();

    assert_eq!(fixture.event(&id).description.as_deref(), Some("new"));
}
