// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The read side of the calendar backend: what exists, what one event looks
//! like as iCalendar, and what changed — all against a live mock server
//! rather than a fixture.

mod common;

use common::Fixture;
use serde_json::json;

#[test]
fn list_existing_returns_only_the_events_in_this_calendar() {
    let fixture = Fixture::start();
    let mine = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.seed(&fixture.theirs, "Their offsite", "2026-01-15T10:00:00");

    let (state, events) = fixture.sync().list_existing().unwrap();

    assert_eq!(events.len(), 1, "the other calendar must not leak in");
    assert_eq!(events[0].uid, mine.to_string());
    assert!(events[0].icalendar.contains("SUMMARY:Standup"));
    assert!(events[0].icalendar.contains("BEGIN:VEVENT"));
    assert!(!events[0].revision.is_empty());
    assert!(!state.as_str().is_empty());
}

#[test]
fn list_existing_on_an_empty_calendar_still_yields_a_state() {
    let fixture = Fixture::start();
    fixture.seed(&fixture.theirs, "Their offsite", "2026-01-15T10:00:00");

    let (state, events) = fixture.sync().list_existing().unwrap();

    assert!(events.is_empty());
    assert!(!state.as_str().is_empty());
}

#[test]
fn the_revision_tracks_the_mapped_content_and_nothing_else() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();
    let before = sync.load_component(id.as_str()).unwrap().revision;

    // A property the iCalendar mapping drops: EDS cannot see it change, so
    // re-downloading every event because of it would be pure churn.
    // (`useDefaultAlerts` was the exemplar here until the reminders became
    // mapped — it now decides whether they are drawn.)
    fixture.patch(&id, json!({"sequence": 3}));
    assert_eq!(sync.load_component(id.as_str()).unwrap().revision, before);

    fixture.patch(&id, json!({"title": "Standup (short)"}));
    assert_ne!(sync.load_component(id.as_str()).unwrap().revision, before);

    // And one the mapping draws — the place the event happens at, which reaches
    // the component as its LOCATION — has to move the revision, or Evolution
    // keeps showing the room the meeting was moved out of.
    let before = sync.load_component(id.as_str()).unwrap().revision;
    fixture.patch(
        &id,
        json!({"locations": {"l1": {"@type": "Location", "name": "Room 3"}}}),
    );
    assert_ne!(sync.load_component(id.as_str()).unwrap().revision, before);

    // The same for the tags, which reach it as CATEGORIES: a tag added on the
    // server has to show up in Evolution's category list.
    let before = sync.load_component(id.as_str()).unwrap().revision;
    fixture.patch(&id, json!({"keywords": {"offsite": true}}));
    assert_ne!(sync.load_component(id.as_str()).unwrap().revision, before);

    // And the importance, which reaches it as PRIORITY. (This was the exemplar of
    // a *dropped* property above until the property became mapped.)
    let before = sync.load_component(id.as_str()).unwrap().revision;
    fixture.patch(&id, json!({"priority": 1}));
    assert_ne!(sync.load_component(id.as_str()).unwrap().revision, before);

    // And who may see it, which reaches it as CLASS — the one on this list where a
    // stale revision is a confidentiality problem and not just a stale display: an
    // event the server has since made private would keep being shown, and offered
    // for editing, as the public one Evolution cached.
    let before = sync.load_component(id.as_str()).unwrap().revision;
    fixture.patch(&id, json!({"privacy": "private"}));
    assert_ne!(sync.load_component(id.as_str()).unwrap().revision, before);

    // And the reminders, which reach it as VALARMs — the first mapped property
    // that is a child component, so a revision computed from the properties alone
    // would miss it and leave the user un-reminded of an event they were reminded
    // of on every other client.
    let before = sync.load_component(id.as_str()).unwrap().revision;
    fixture.patch(
        &id,
        json!({"alerts": {"k1": {
            "@type": "Alert",
            "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
            "action": "display",
        }}}),
    );
    assert_ne!(sync.load_component(id.as_str()).unwrap().revision, before);
}

#[test]
fn load_component_reports_an_unknown_identifier_as_not_found() {
    let fixture = Fixture::start();
    let error = fixture.sync().load_component("no-such-event").unwrap_err();

    assert!(
        matches!(&error, jmap_cal_sync::SyncError::NotFound(uid) if uid == "no-such-event"),
        "{error:?}"
    );
}

#[test]
fn get_changes_reports_creations_updates_and_destructions() {
    let fixture = Fixture::start();
    // Present before the window: an event created *and* destroyed inside one
    // window is correctly reported in neither list, so it cannot stand in for
    // a destruction.
    let doomed = fixture.seed(&fixture.ours, "Cancelled offsite", "2026-01-16T09:00:00");
    let edited = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();
    let (state, _) = sync.list_existing().unwrap();

    let created = fixture.seed(&fixture.ours, "Retro", "2026-01-17T15:00:00");
    fixture.patch(&edited, json!({"title": "Standup (short)"}));
    sync.remove_component(doomed.as_str()).unwrap();

    let changes = sync.get_changes(&state).unwrap();

    let mut changed: Vec<&str> = changes.changed.iter().map(|c| c.uid.as_str()).collect();
    changed.sort_unstable();
    let mut expected = vec![created.as_str(), edited.as_str()];
    expected.sort_unstable();
    assert_eq!(changed, expected);
    assert!(
        changes
            .changed
            .iter()
            .any(|c| c.icalendar.contains("SUMMARY:Standup (short)")),
        "the changed event is rendered, not just named"
    );
    assert_eq!(changes.removed, vec![doomed.to_string()]);
    assert_ne!(changes.new_state, state);

    // Nothing has happened since, so the follow-up delta is empty.
    let quiet = sync.get_changes(&changes.new_state).unwrap();
    assert!(quiet.changed.is_empty() && quiet.removed.is_empty());
}

#[test]
fn get_changes_ignores_events_in_another_calendar() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let (state, _) = sync.list_existing().unwrap();

    fixture.seed(&fixture.theirs, "Their offsite", "2026-01-15T10:00:00");

    let changes = sync.get_changes(&state).unwrap();
    assert!(changes.changed.is_empty(), "{:?}", changes.changed);
    assert!(changes.removed.is_empty(), "{:?}", changes.removed);
}

#[test]
fn an_event_moved_to_another_calendar_is_reported_as_removed() {
    let fixture = Fixture::start();
    let moved = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();
    let (state, _) = sync.list_existing().unwrap();

    fixture.patch(
        &moved,
        json!({"calendarIds": {fixture.theirs.to_string(): true}}),
    );

    // It comes back as an update, not a destruction; reporting it as changed
    // would leave the calendar showing an appointment it no longer contains.
    let changes = sync.get_changes(&state).unwrap();
    assert!(changes.changed.is_empty(), "{:?}", changes.changed);
    assert_eq!(changes.removed, vec![moved.to_string()]);
}

#[test]
fn remove_component_destroys_the_event() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();

    sync.remove_component(id.as_str()).unwrap();

    assert!(sync.list_existing().unwrap().1.is_empty());
    assert!(sync.remove_component(id.as_str()).is_err(), "already gone");
}
