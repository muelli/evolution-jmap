// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the read side of the calendar backend against a live mock server, not a
//! fixture: the list of events, one event as iCalendar, and the changes.

mod common;

use std::collections::BTreeMap;

use common::Fixture;
use jmap_cal_sync::SyncError;
use jmap_mock::MockServer;
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
fn a_capped_query_still_lists_every_event() {
    let fixture = Fixture::start_with(MockServer::builder().query_page_size(2));
    let mut seeded: Vec<String> = (1..=5)
        .map(|day| {
            let start = format!("2026-01-{day:02}T09:00:00");
            fixture.seed(&fixture.ours, "Standup", &start).to_string()
        })
        .collect();
    fixture.seed(&fixture.theirs, "Their offsite", "2026-01-15T10:00:00");

    let (_, events) = fixture.sync().list_existing().unwrap();

    // EDS removes from its cache each event that the list does not have.
    let mut listed: Vec<String> = events.into_iter().map(|event| event.uid).collect();
    listed.sort();
    seeded.sort();
    assert_eq!(listed, seeded);
}

#[test]
fn a_query_that_drops_events_without_a_limit_fails_the_list() {
    let fixture = Fixture::start_with(MockServer::builder().query_page_size(2).query_hides_limit());
    for day in 1..=3 {
        fixture.seed(
            &fixture.ours,
            "Standup",
            &format!("2026-01-{day:02}T09:00:00"),
        );
    }

    let error = fixture.sync().list_existing().unwrap_err();

    assert!(
        matches!(&error, SyncError::Client(jmap_client::Error::Protocol(_))),
        "{error:?}"
    );
}

#[test]
fn the_revision_tracks_the_mapped_content_and_nothing_else() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();
    let before = sync.load_component(id.as_str()).unwrap().revision;

    // The iCalendar mapping drops this property. EDS cannot see it change, so a
    // new download of each event would be unnecessary work. (`useDefaultAlerts` is
    // not an example: it controls if the reminders are shown.)
    fixture.patch(&id, json!({"sequence": 3}));
    assert_eq!(sync.load_component(id.as_str()).unwrap().revision, before);

    fixture.patch(&id, json!({"title": "Standup (short)"}));
    assert_ne!(sync.load_component(id.as_str()).unwrap().revision, before);

    // The place goes to the component as LOCATION, so it must change the revision.
    // If not, Evolution continues to show the old room.
    let before = sync.load_component(id.as_str()).unwrap().revision;
    fixture.patch(
        &id,
        json!({"locations": {"l1": {"@type": "Location", "name": "Room 3"}}}),
    );
    assert_ne!(sync.load_component(id.as_str()).unwrap().revision, before);

    // The same for the tags (CATEGORIES): a tag added on the server must show in
    // Evolution's category list.
    let before = sync.load_component(id.as_str()).unwrap().revision;
    fixture.patch(&id, json!({"keywords": {"offsite": true}}));
    assert_ne!(sync.load_component(id.as_str()).unwrap().revision, before);

    // The same for the priority (PRIORITY).
    let before = sync.load_component(id.as_str()).unwrap().revision;
    fixture.patch(&id, json!({"priority": 1}));
    assert_ne!(sync.load_component(id.as_str()).unwrap().revision, before);

    // The same for the privacy (CLASS). Here an old revision is a confidentiality
    // problem: Evolution would continue to show and offer for editing an event
    // that the server made private.
    let before = sync.load_component(id.as_str()).unwrap().revision;
    fixture.patch(&id, json!({"privacy": "private"}));
    assert_ne!(sync.load_component(id.as_str()).unwrap().revision, before);

    // The same for the reminders (VALARM). This is the first mapped child
    // component, so a revision from the properties only would not see it. The
    // user would then get no reminder, but other clients give one.
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
fn load_component_reports_a_matched_event_without_an_id_as_a_protocol_violation() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.strip_id(&id);

    let error = fixture.sync().load_component(id.as_str()).unwrap_err();

    assert!(
        matches!(&error, jmap_cal_sync::SyncError::Client(jmap_client::Error::Protocol(message)) if message.contains("without an id")),
        "{error:?}"
    );
}

#[test]
fn list_existing_reports_a_matched_event_without_an_id_as_a_protocol_violation() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.strip_id(&id);

    let error = fixture.sync().list_existing().unwrap_err();

    assert!(
        matches!(&error, jmap_cal_sync::SyncError::Client(jmap_client::Error::Protocol(message)) if message.contains("without an id")),
        "{error:?}"
    );
}

#[test]
fn get_changes_reports_creations_updates_and_destructions() {
    let fixture = Fixture::start();
    // Create this event before the window. An event that is created *and*
    // destroyed in one window is in neither list, so it cannot test a destruction.
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

    // Nothing changed since, so the next delta is empty.
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

    // The server reports an update, not a destruction. If the sync reported it as
    // changed, the calendar would show an appointment that it does not contain.
    let changes = sync.get_changes(&state).unwrap();
    assert!(changes.changed.is_empty(), "{:?}", changes.changed);
    assert_eq!(changes.removed, vec![moved.to_string()]);
}

/// A server that states non-membership with a `false` entry, and does not omit the
/// key, violates the set semantics of `calendarIds` (RFC 8620 §1.5). The
/// `messages_since` of `jmap-mail-sync` also refuses this incorrect shape. This
/// crate has no basis to read the entry as absence. Thus it must refuse it and not
/// report that the event left the calendar.
#[test]
fn get_changes_refuses_a_false_valued_calendar_entry() {
    let fixture = Fixture::start();
    let flagged = fixture.seed(
        &fixture.ours,
        "Still here, allegedly not",
        "2026-01-15T09:00:00",
    );
    let sync = fixture.sync();
    let (state, _) = sync.list_existing().unwrap();

    fixture.set_calendar_ids(&flagged, BTreeMap::from([(fixture.ours.clone(), false)]));

    let error = sync
        .get_changes(&state)
        .expect_err("a false-valued calendarIds entry must not be guessed at");
    match error {
        jmap_cal_sync::SyncError::Client(jmap_client::Error::Protocol(message)) => {
            assert!(
                message.contains("false-valued"),
                "expected a message naming the false-valued entry, got: {message}"
            );
        }
        other => panic!("expected a protocol error, got {other:?}"),
    }
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
