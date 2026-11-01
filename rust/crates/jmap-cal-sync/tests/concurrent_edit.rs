// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A save with a base copy follows `OnConflict` for a property that both the
//! user and a different client changed.

mod common;

use std::sync::Arc;
use std::thread;

use common::Fixture;
use common::race::{DelayedDeliveryTransport, Gate};
use jmap_cal_sync::{Base, Itip, OnConflict, SyncError};
use jmap_client::{Client, Credentials};
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

/// A1-1's shape (`jmap-ui/src/filters/io.rs`), narrowed to the window A2-2
/// of the race/TOCTOU audit names: `save_component_since` reads `current`
/// once and sends the update with no `ifInState`, so a true conflict that
/// lands in the round trip between the fetch and the write goes undetected
/// and the local edit silently overwrites it, even under `OnConflict::Fail`.
#[test]
fn a_concurrent_edit_landing_between_the_fetch_and_the_write_is_not_silently_overwritten() {
    let fixture = Fixture::start();
    let (id, loaded) = loaded(&fixture);

    let gate = Arc::new(Gate::default());
    let transport = DelayedDeliveryTransport::new(Arc::clone(&gate), "CalendarEvent/get");
    let gated = Client::builder()
        .transport(transport)
        .connect(fixture.server.origin(), Credentials::none())
        .unwrap();
    let sync = fixture.sync_with(gated);

    let edited = described(&loaded).replace("SUMMARY:Standup\r\n", "SUMMARY:Standup local\r\n");
    let loaded_for_save = loaded.clone();
    let id_for_save = id.clone();
    let save = thread::spawn(move || {
        sync.save_component_since(
            Some(Base {
                icalendar: &loaded_for_save,
                on_conflict: OnConflict::Fail,
            }),
            &edited,
            Some(id_for_save.as_str()),
            Itip::Send,
        )
    });

    // The save's own fetch has already left the server and is parked on its
    // way back. This edit is the server's honest, later answer: the race
    // this test reproduces is the fetch's stale answer landing regardless.
    gate.wait_for_start();
    fixture.patch(&id, json!({"title": "Standup (moved)"}));
    gate.release();

    let result = save.join().unwrap();

    assert!(matches!(result, Err(SyncError::OutOfSync(_))), "{result:?}");
    assert_eq!(
        fixture.event(&id).title.as_deref(),
        Some("Standup (moved)"),
        "the concurrent edit must survive a save that started before it landed"
    );
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
