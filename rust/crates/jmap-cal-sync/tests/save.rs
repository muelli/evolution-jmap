// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The write side. The theme throughout is that saving a component must not
//! destroy what the component could not carry: the mapping keeps seven
//! properties of a JSCalendar event and drops the rest, so a save that
//! replaced properties wholesale would delete data the user never touched and
//! cannot even see.

mod common;

use common::Fixture;
use serde_json::json;

/// The component Evolution hands to `save_component_sync` for a brand new
/// appointment: the `UID` is a name the local cache invented, not a server
/// identifier.
const NEW_EVENT: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:20260808T101500Z-4711-1000-1-0@localhost\r\n\
SUMMARY:Planning\r\n\
DTSTART;TZID=Europe/Berlin:20260115T130000\r\n\
DURATION:PT90M\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

#[test]
fn saving_a_new_event_files_it_in_this_calendar_under_a_server_identifier() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let saved = sync.save_component(NEW_EVENT, None).unwrap();

    assert_ne!(
        saved.uid, "20260808T101500Z-4711-1000-1-0@localhost",
        "the locally invented UID must not be sent as the JMAP id"
    );
    let stored = fixture.event(&saved.uid.as_str().into());
    assert_eq!(stored.title.as_deref(), Some("Planning"));
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T13:00:00"));
    assert_eq!(stored.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(stored.duration.as_deref(), Some("PT90M"));
    assert!(
        stored
            .calendar_ids
            .as_ref()
            .unwrap()
            .contains_key(&fixture.ours),
        "filed in the calendar being synced"
    );
    // The listing agrees with what save reported.
    let (_, events) = sync.list_existing().unwrap();
    assert_eq!(events, vec![saved]);
}

#[test]
fn a_new_events_icalendar_uid_becomes_the_jscalendar_uid() {
    let fixture = Fixture::start();

    let saved = fixture.sync().save_component(NEW_EVENT, None).unwrap();

    assert_eq!(
        fixture.event(&saved.uid.as_str().into()).uid.as_deref(),
        Some("20260808T101500Z-4711-1000-1-0@localhost"),
        "the identity other iTIP clients know the event by must survive the create"
    );
}

#[test]
fn editing_an_event_leaves_unmapped_properties_alone() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    // Properties no component we produce can carry.
    fixture.patch(
        &id,
        json!({
            "locations": {"l1": {"@type": "Location", "name": "Room 3"}},
            "participants": {"p1": {"@type": "Participant", "email": "vera@example.com"}},
            "priority": 5,
        }),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
    assert_eq!(
        stored.extra.get("locations"),
        Some(&json!({"l1": {"@type": "Location", "name": "Room 3"}})),
        "an unmapped property was overwritten"
    );
    assert_eq!(stored.extra.get("priority"), Some(&json!(5)));
    assert!(stored.extra.contains_key("participants"));
}

#[test]
fn rescheduling_an_event_moves_the_start_and_keeps_its_zone() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"timeZone": "Europe/Berlin"}));
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(icalendar.contains("TZID=Europe/Berlin"), "{icalendar}");
    let edited = icalendar.replace("20260115T090000", "20260115T093000");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T09:30:00"));
    assert_eq!(
        stored.time_zone.as_deref(),
        Some("Europe/Berlin"),
        "moving the start within its zone must not restate the zone"
    );
}

#[test]
fn moving_an_event_to_another_zone_lengthening_it_and_unconfirming_it_all_arrive() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"timeZone": "Europe/Berlin"}));
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("TZID=Europe/Berlin", "TZID=America/New_York")
        .replace("DURATION:PT1H", "DURATION:PT2H")
        .replace("STATUS:CONFIRMED", "STATUS:TENTATIVE");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.time_zone.as_deref(), Some("America/New_York"));
    assert_eq!(stored.duration.as_deref(), Some("PT2H"));
    assert_eq!(stored.status.as_deref(), Some("tentative"));
    assert_eq!(
        stored.start.as_deref(),
        Some("2026-01-15T09:00:00"),
        "the wall-clock time was not what the user changed"
    );
}

#[test]
fn a_recurrence_the_mapping_can_carry_is_patched() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRules": [
            {"@type": "RecurrenceRule", "frequency": "daily", "count": 10},
        ]}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RRULE:FREQ=DAILY;COUNT=10"),
        "{icalendar}"
    );
    let edited = icalendar.replace("COUNT=10", "COUNT=5");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rules.unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].frequency, "daily");
    assert_eq!(rules[0].count, Some(5));
}

#[test]
fn a_recurrence_the_mapping_cannot_carry_is_left_alone() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    // `byDay` has no place in the RecurrenceRule this crate models, so the
    // RRULE the user edited is a narrower rule than the one on the server.
    fixture.patch(
        &id,
        json!({"recurrenceRules": [{
            "@type": "RecurrenceRule",
            "frequency": "weekly",
            "byDay": [{"@type": "NDay", "day": "mo"}, {"@type": "NDay", "day": "th"}],
        }]}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar.replace("RRULE:FREQ=WEEKLY", "RRULE:FREQ=WEEKLY;COUNT=4");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rules.unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(
        rules[0].extra.get("byDay"),
        Some(&json!([
            {"@type": "NDay", "day": "mo"},
            {"@type": "NDay", "day": "th"},
        ])),
        "a rule part the RRULE could not carry was dropped"
    );
    assert_eq!(
        rules[0].count, None,
        "narrowing a rule we cannot fully see is worse than ignoring the edit"
    );
}

#[test]
fn clearing_the_description_clears_it_on_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"description": "bring the numbers"}));
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited: String = icalendar
        .lines()
        .filter(|line| !line.starts_with("DESCRIPTION"))
        .map(|line| format!("{line}\r\n"))
        .collect();
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.event(&id).description, None);
}

#[test]
fn a_save_whose_start_cannot_be_read_leaves_the_servers_start_alone() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();

    // JSCalendar has no way to say "no start", so a component whose DTSTART
    // the mapping cannot read must not be turned into one that says it.
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("DTSTART:20260115T090000Z", "DTSTART:whenever")
        .replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T09:00:00"));
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
}

#[test]
fn a_save_that_changes_nothing_sends_no_patch() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    // Two things the component cannot say exactly: a time zone spelled the
    // other legal way, and a recurrence part the RRULE has to drop. Neither is
    // an edit, and neither may look like one.
    fixture.patch(
        &id,
        json!({
            "timeZone": "UTC",
            "recurrenceRules": [{
                "@type": "RecurrenceRule",
                "frequency": "weekly",
                "byDay": [{"@type": "NDay", "day": "mo"}],
            }],
        }),
    );
    let sync = fixture.sync();

    let before = sync.load_component(id.as_str()).unwrap();
    let (state_before, _) = sync.list_existing().unwrap();
    let after = sync
        .save_component(&before.icalendar, Some(id.as_str()))
        .unwrap();

    assert_eq!(after, before);
    let (state_after, _) = sync.list_existing().unwrap();
    assert_eq!(
        state_after, state_before,
        "a no-op save must not bump the server state and wake every other client"
    );
}

#[test]
fn saving_over_an_unknown_identifier_is_not_found() {
    let fixture = Fixture::start();
    let error = fixture
        .sync()
        .save_component(NEW_EVENT, Some("no-such-event"))
        .unwrap_err();

    assert!(
        matches!(&error, jmap_cal_sync::SyncError::NotFound(uid) if uid == "no-such-event"),
        "{error:?}"
    );
}

#[test]
fn saving_something_that_holds_no_event_fails_before_any_request() {
    let fixture = Fixture::start();
    let error = fixture
        .sync()
        .save_component("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n", None)
        .unwrap_err();

    assert!(
        matches!(error, jmap_cal_sync::SyncError::ICal(_)),
        "{error:?}"
    );
    assert!(fixture.sync().list_existing().unwrap().1.is_empty());
}
