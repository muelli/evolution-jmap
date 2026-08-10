// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The write side. The theme throughout is that saving a component must not
//! destroy what the component could not carry: the mapping keeps nine
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
fn a_new_event_that_states_its_end_rather_than_its_length_still_has_one() {
    // What Evolution's appointment editor actually writes: DTEND, never
    // DURATION. An event saved from it used to reach the server with no
    // duration at all — RFC 8984's P0D — so the meeting the user scheduled for
    // an hour and a half was shared as a zero-length one.
    let fixture = Fixture::start();
    let icalendar = NEW_EVENT.replace("DURATION:PT90M", "DTEND;TZID=Europe/Berlin:20260115T143000");

    let saved = fixture.sync().save_component(&icalendar, None).unwrap();

    let stored = fixture.event(&saved.uid.as_str().into());
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T13:00:00"));
    assert_eq!(stored.duration.as_deref(), Some("PT1H30M"));
    // And what EDS is handed back says the same, in the one spelling this
    // mapping writes.
    assert!(
        saved.icalendar.contains("\r\nDURATION:PT1H30M\r\n"),
        "{saved:?}"
    );
}

#[test]
fn a_new_all_day_event_reaches_the_server_as_one() {
    // What Evolution writes for an all-day appointment: DTSTART and DTEND as
    // DATE values. Without showWithoutTime the server — and every other client
    // reading from it — was told about a midnight appointment instead.
    let fixture = Fixture::start();
    let icalendar = NEW_EVENT
        .replace(
            "DTSTART;TZID=Europe/Berlin:20260115T130000",
            "DTSTART;VALUE=DATE:20260115",
        )
        .replace("DURATION:PT90M", "DTEND;VALUE=DATE:20260116");

    let saved = fixture.sync().save_component(&icalendar, None).unwrap();

    let stored = fixture.event(&saved.uid.as_str().into());
    assert_eq!(stored.show_without_time, Some(true));
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T00:00:00"));
    assert_eq!(stored.duration.as_deref(), Some("P1D"));
    // RFC 5545 §3.2.19 and RFC 8984 §4.1.5 agree that a day has no zone.
    assert_eq!(stored.time_zone, None);
    // And EDS gets the same event back, still without a time.
    assert!(
        saved
            .icalendar
            .contains("\r\nDTSTART;VALUE=DATE:20260115\r\n"),
        "{saved:?}"
    );
}

#[test]
fn giving_an_all_day_event_a_time_clears_the_flag_on_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Retreat", "2026-01-15T00:00:00");
    // A day has no zone (RFC 8984 §4.1.5), and `CalendarEvent::simple` seeds
    // one, so it goes as part of making this event all-day.
    fixture.patch(
        &id,
        json!({"showWithoutTime": true, "duration": "P1D", "timeZone": null}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("DTSTART;VALUE=DATE:20260115"),
        "{icalendar}"
    );
    let edited = icalendar
        .replace("DTSTART;VALUE=DATE:20260115", "DTSTART:20260115T090000")
        .replace("DURATION:P1D", "DURATION:PT2H");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.show_without_time, None,
        "the day the user turned into an appointment is still a day on the server"
    );
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T09:00:00"));
    assert_eq!(stored.duration.as_deref(), Some("PT2H"));
}

#[test]
fn an_all_day_event_the_component_could_not_say_is_all_day_keeps_its_flag() {
    // A server may set showWithoutTime on an event no DATE value can hold — one
    // that starts at 09:00, or, as here, one carrying a zone. The component
    // shows it as timed, which loses the flag; the save must not read that loss
    // back as the user having cleared it.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Retreat", "2026-01-15T00:00:00");
    fixture.patch(
        &id,
        json!({"showWithoutTime": true, "duration": "P1D", "timeZone": "Europe/Berlin"}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("DTSTART;TZID=Europe/Berlin:20260115T000000"),
        "{icalendar}"
    );
    let edited = icalendar.replace("SUMMARY:Retreat", "SUMMARY:Retreat (offsite)");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Retreat (offsite)"));
    assert_eq!(
        stored.show_without_time,
        Some(true),
        "a flag the component never showed cannot have been unset by the user"
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
fn a_length_the_component_could_not_state_survives_a_save() {
    // The server's own `duration` can be one iCalendar has no room for: RFC 5545
    // §3.3.6 spells a negative length and RFC 8984 §1.4.6 does not, so a value
    // this way round is a value the mapping refuses to write — libical refusing
    // the content line would cost the whole component, and the appointment with
    // it. What must not follow is a save *clearing* it. The baseline a save diffs
    // against is the server's event put through the same rendering, so the length
    // is absent on both sides and an edit elsewhere leaves the server's value
    // where it was.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"duration": "-PT1H"}));
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(!icalendar.contains("DURATION"), "{icalendar}");
    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Daily standup");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Daily standup"));
    assert_eq!(
        stored.duration.as_deref(),
        Some("-PT1H"),
        "a length the component could not state is not a length the user cleared"
    );
}

#[test]
fn an_occurrence_whose_length_the_component_could_not_state_is_left_alone() {
    // One level down, where the property is replaced whole rather than patched
    // key by key. An override the component cannot describe comes back as the
    // empty patch, so writing the map would delete the length the server holds —
    // the same reason a rule with a `byDay` leaves `recurrenceRules` untouched.
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    fixture.patch(
        &id,
        json!({"recurrenceOverrides": {"2026-01-20T09:00:00": {"duration": "-PT1H"}}}),
    );
    let sync = fixture.sync();

    // Placed by an RDATE, at the series' length: the occurrence is shown, and
    // the length it really has is not.
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(icalendar.contains("RDATE:20260120T090000Z"), "{icalendar}");
    let edited = with_line(&icalendar, "EXDATE:20260116T090000Z");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.event(&id).recurrence_overrides,
        Some(
            [(
                "2026-01-20T09:00:00".to_owned(),
                json!({"duration": "-PT1H"})
            )]
            .into()
        ),
        "the occurrence the mapping saw in part must not be rewritten from that view",
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

/// A daily event on the server, so that the tests below have a recurrence to
/// name single instances of.
fn seed_daily(fixture: &Fixture) -> jmap_proto::Id {
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRules": [
            {"@type": "RecurrenceRule", "frequency": "daily"},
        ]}),
    );
    id
}

/// The component with `line` inserted ahead of its `END:VEVENT`, which is what
/// deleting one occurrence in Evolution amounts to.
fn with_line(icalendar: &str, line: &str) -> String {
    icalendar.replace("END:VEVENT\r\n", &format!("{line}\r\nEND:VEVENT\r\n"))
}

#[test]
fn deleting_one_occurrence_reaches_the_server_as_an_excluded_override() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = with_line(&icalendar, "EXDATE:20260116T090000Z");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.recurrence_overrides,
        Some([("2026-01-16T09:00:00".to_owned(), json!({"excluded": true}))].into()),
        "the instance the user deleted is off, and only that one"
    );
    // The rule it is an exception to is untouched.
    assert_eq!(stored.recurrence_rules.unwrap()[0].frequency, "daily");
}

#[test]
fn restoring_a_deleted_occurrence_removes_the_override() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    fixture.patch(
        &id,
        json!({"recurrenceOverrides": {"2026-01-16T09:00:00": {"excluded": true}}}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(icalendar.contains("EXDATE:20260116T090000Z"), "{icalendar}");
    let edited: String = icalendar
        .lines()
        .filter(|line| !line.starts_with("EXDATE"))
        .map(|line| format!("{line}\r\n"))
        .collect();
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    // Removing the property is how a PatchObject says "back to the default",
    // which for recurrenceOverrides is no named instances at all.
    assert_eq!(fixture.event(&id).recurrence_overrides, None);
}

#[test]
fn an_instance_edited_on_its_own_survives_an_edit_to_another_instance() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    // An override that changes the instance is a VEVENT of its own in the
    // component, so deleting a *different* occurrence has to leave it standing.
    fixture.patch(
        &id,
        json!({"recurrenceOverrides": {
            "2026-01-20T09:00:00": {"title": "Standup with the board"},
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = with_line(&icalendar, "EXDATE:20260116T090000Z");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.event(&id).recurrence_overrides,
        Some(
            [
                ("2026-01-16T09:00:00".to_owned(), json!({"excluded": true})),
                (
                    "2026-01-20T09:00:00".to_owned(),
                    json!({"title": "Standup with the board"}),
                )
            ]
            .into()
        ),
    );
}

/// The component with `vevent` appended inside its envelope, which is what
/// editing one occurrence in Evolution amounts to: a second instance of the
/// same uid, carrying the `RECURRENCE-ID` of the day it replaces.
fn with_instance(icalendar: &str, vevent: &str) -> String {
    icalendar.replace("END:VCALENDAR\r\n", &format!("{vevent}END:VCALENDAR\r\n"))
}

#[test]
fn editing_one_occurrence_reaches_the_server_as_a_patched_override() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    // A detached instance is a whole component, not a patch: Evolution clones
    // the series and edits that, so what it restates unchanged — here the
    // status, and the length it states as a DTEND — is not an edit, and only
    // the moved start and the new title reach the server.
    let edited = with_instance(
        &icalendar,
        &format!(
            "BEGIN:VEVENT\r\n\
             UID:{id}\r\n\
             RECURRENCE-ID:20260116T090000Z\r\n\
             DTSTART:20260116T100000Z\r\n\
             DTEND:20260116T110000Z\r\n\
             STATUS:CONFIRMED\r\n\
             SUMMARY:Standup with the board\r\n\
             END:VEVENT\r\n"
        ),
    );
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.event(&id).recurrence_overrides,
        Some(
            [(
                "2026-01-16T09:00:00".to_owned(),
                json!({
                    "start": "2026-01-16T10:00:00",
                    "title": "Standup with the board",
                }),
            )]
            .into()
        ),
    );
    // The series is what it was: one daily rule, and the title the other
    // occurrences still carry.
    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup"));
    assert_eq!(stored.recurrence_rules.unwrap()[0].frequency, "daily");
}

#[test]
fn undoing_an_edit_to_one_occurrence_removes_the_override() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    fixture.patch(
        &id,
        json!({"recurrenceOverrides": {
            "2026-01-20T09:00:00": {"title": "Standup with the board"},
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RECURRENCE-ID:20260120T090000Z"),
        "{icalendar}"
    );
    // Deleting the detached instance is how Evolution says "this occurrence is
    // like the others again".
    let detached = icalendar.rfind("BEGIN:VEVENT").expect("two instances");
    let series = format!("{}END:VCALENDAR\r\n", &icalendar[..detached]);
    sync.save_component(&series, Some(id.as_str())).unwrap();

    assert_eq!(fixture.event(&id).recurrence_overrides, None);
}

#[test]
fn an_instance_the_server_says_is_not_excluded_stays_spelled_that_way() {
    // RFC 8984 §4.3.4 defaults `excluded` to false, so an override that says so
    // out loud and one that says nothing are the same instance, and the
    // component has the same single RDATE for both. Re-saving it must not
    // rewrite the property merely because the spelling comes back shorter —
    // the baseline is the round trip, not the server's own event.
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    fixture.patch(
        &id,
        json!({"recurrenceOverrides": {"2026-01-20T09:00:00": {"excluded": false}}}),
    );
    let sync = fixture.sync();

    let before = sync.load_component(id.as_str()).unwrap();
    assert!(
        before.icalendar.contains("RDATE:20260120T090000Z"),
        "{before:?}"
    );
    let (state_before, _) = sync.list_existing().unwrap();
    sync.save_component(&before.icalendar, Some(id.as_str()))
        .unwrap();

    assert_eq!(
        fixture.event(&id).recurrence_overrides,
        Some([("2026-01-20T09:00:00".to_owned(), json!({"excluded": false}))].into()),
    );
    assert_eq!(
        sync.list_existing().unwrap().0,
        state_before,
        "a no-op save must not bump the server state and wake every other client"
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
    // Three things the component cannot say exactly: a time zone spelled the
    // other legal way, a recurrence part the RRULE has to drop, and an instance
    // edited on its own, which an RDATE can place but not describe. None of
    // them is an edit, and none may look like one.
    fixture.patch(
        &id,
        json!({
            "timeZone": "UTC",
            "recurrenceRules": [{
                "@type": "RecurrenceRule",
                "frequency": "weekly",
                "byDay": [{"@type": "NDay", "day": "mo"}],
            }],
            "recurrenceOverrides": {
                "2026-01-22T09:00:00": {"title": "Standup (long)"},
            },
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

/// The `TZID` Evolution puts on every zoned component it saves: libical names
/// its builtin zones with a solidus-prefixed identifier of its own, and the
/// appointment editor sets the start with the zone object, so this is what
/// comes back even for a component we handed out spelling the zone plainly.
const LIBICAL_TZID: &str = "/freeassociation.sourceforge.net/Europe/Berlin";

/// The `VTIMEZONE` libical writes beside it, trimmed to the two lines that
/// matter here; `X-LIC-LOCATION` is its record of which IANA zone this is.
///
/// The envelope the backend builds does carry it:
/// `marshal::icalendar_from_instances` copies in a definition for every zone the
/// instances refer to, which is the other half of the answer these tests are the
/// mapping's half of. `jmap-functional`'s calendar leg is what says the two
/// halves meet through real EDS — this crate can only supply the identifier by
/// hand.
const LIBICAL_VTIMEZONE: &str = "BEGIN:VTIMEZONE\r\n\
TZID:/freeassociation.sourceforge.net/Europe/Berlin\r\n\
X-LIC-LOCATION:Europe/Berlin\r\n\
BEGIN:STANDARD\r\nTZNAME:CET\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n\
DTSTART:19701025T030000\r\nEND:STANDARD\r\n\
END:VTIMEZONE\r\n";

/// Respelling a zone is not changing it. Every save of a zoned appointment
/// arrives with libical's own identifier in place of the name we wrote, and
/// reading that as an edit would put a string into `timeZone` that RFC 8984
/// §1.4.9 only admits beside a `timeZones` definition — a value a server is
/// entitled to reject, taking the user's real edits down with it.
#[test]
fn the_zone_evolution_respells_is_not_read_as_a_zone_change() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"timeZone": "Europe/Berlin"}));
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("TZID=Europe/Berlin", &format!("TZID={LIBICAL_TZID}"))
        .replace("BEGIN:VEVENT", &format!("{LIBICAL_VTIMEZONE}BEGIN:VEVENT"))
        .replace("SUMMARY:Standup", "SUMMARY:Standup (daily)");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (daily)"));
    assert_eq!(
        stored.time_zone.as_deref(),
        Some("Europe/Berlin"),
        "the zone was respelled, not changed"
    );
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T09:00:00"));
}

/// The same respelling on the other side of a real move: the zone the user
/// picked has to reach the server as the name JSCalendar spells it with.
#[test]
fn a_move_to_another_zone_arrives_under_its_iana_name() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"timeZone": "Europe/Berlin"}));
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace(
            "TZID=Europe/Berlin",
            "TZID=/freeassociation.sourceforge.net/America/New_York",
        )
        .replace(
            "BEGIN:VEVENT",
            "BEGIN:VTIMEZONE\r\n\
             TZID:/freeassociation.sourceforge.net/America/New_York\r\n\
             X-LIC-LOCATION:America/New_York\r\n\
             BEGIN:STANDARD\r\nTZNAME:EST\r\nTZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\n\
             DTSTART:19701101T020000\r\nEND:STANDARD\r\n\
             END:VTIMEZONE\r\nBEGIN:VEVENT",
        );
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.time_zone.as_deref(), Some("America/New_York"));
}

/// A zone nothing in the document explains: a Windows name from Exchange, and
/// libical's own identifier with no `VTIMEZONE` beside it to translate it. The
/// backend's envelope now defines the zones its components name, so the second
/// shape no longer reaches this crate from Evolution — but a document is not
/// only ever built there, and an identifier no zone database knows (the first
/// shape) still arrives undefined however careful the envelope is.
///
/// Neither is a value JSCalendar can carry, so neither is sent; the server
/// keeps the zone it had, which is the zone the component was showing. The cost
/// stays on the record for the case that remains: a zone the user really did
/// change to something unresolvable is not seen either.
#[test]
fn a_zone_the_document_could_not_name_leaves_the_servers_alone() {
    for tzid in ["W. Europe Standard Time", LIBICAL_TZID] {
        let fixture = Fixture::start();
        let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
        fixture.patch(&id, json!({"timeZone": "Europe/Berlin"}));
        let sync = fixture.sync();

        let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
        let edited = icalendar
            .replace("TZID=Europe/Berlin", &format!("TZID={tzid}"))
            .replace("SUMMARY:Standup", "SUMMARY:Standup (daily)");
        sync.save_component(&edited, Some(id.as_str())).unwrap();

        let stored = fixture.event(&id);
        assert_eq!(
            stored.title.as_deref(),
            Some("Standup (daily)"),
            "the edit the user made must still arrive, {tzid}"
        );
        assert_eq!(stored.time_zone.as_deref(), Some("Europe/Berlin"), "{tzid}");
    }
}

/// The same value on a create, where there is no server zone to keep. The
/// appointment is filed floating rather than not at all: a wall-clock time
/// with no zone shows correctly for the user who typed it, and an event the
/// server refused shows nothing.
#[test]
fn a_new_events_unnameable_zone_is_not_sent() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let icalendar = NEW_EVENT.replace("TZID=Europe/Berlin", "TZID=W. Europe Standard Time");

    let saved = sync.save_component(&icalendar, None).unwrap();

    let stored = fixture.event(&saved.uid.as_str().into());
    assert_eq!(stored.title.as_deref(), Some("Planning"));
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T13:00:00"));
    assert_eq!(stored.time_zone, None);
}

/// And a create carrying libical's spelling, which is what Evolution actually
/// hands the backend for a brand-new zoned appointment.
#[test]
fn a_new_events_zone_arrives_under_its_iana_name() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let icalendar = NEW_EVENT
        .replace("TZID=Europe/Berlin", &format!("TZID={LIBICAL_TZID}"))
        .replace("BEGIN:VEVENT", &format!("{LIBICAL_VTIMEZONE}BEGIN:VEVENT"));

    let saved = sync.save_component(&icalendar, None).unwrap();

    let stored = fixture.event(&saved.uid.as_str().into());
    assert_eq!(stored.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T13:00:00"));
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
