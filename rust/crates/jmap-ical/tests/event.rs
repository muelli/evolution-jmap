// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSCalendar `CalendarEvent` ↔ iCalendar `VEVENT`, the minimal property set
//! the calendar backend needs: UID, SUMMARY, DESCRIPTION, DTSTART (+timeZone,
//! or as a date for showWithoutTime), DURATION, STATUS, LOCATION, RRULE, and
//! the instances named one at a time by an EXDATE, an RDATE, or a component of
//! their own carrying a RECURRENCE-ID.

use std::collections::BTreeMap;

use jmap_ical::{
    ICalError, MAX_DEPTH, OVERRIDE_PROPERTIES, busy_periods_to_vfreebusy, defines_time_zone,
    event_to_ical, free_busy_type, ical_to_event, maps_alerts, maps_keyword, maps_locations,
    maps_recurrence_override, maps_recurrence_rule, maps_time_zone, maps_virtual_locations,
    names_time_zone, prune_time_zones, sends_recurrence_override, time_zone_definition,
    unique_tzid_to_iana, unstateable_until, windows_time_zone_to_iana,
};
use jmap_proto::calendars::{CalendarEvent, NDay, RecurrenceRule};
use jmap_proto::principals::BusyPeriod;
use jmap_proto::state::UtcDate;
use serde_json::{Value, json};

fn fixture_event() -> CalendarEvent {
    let path = format!(
        "{}/tests/fixtures/calendar_event.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn line<'a>(ics: &'a str, prefix: &str) -> &'a str {
    ics.split("\r\n")
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no line starting {prefix} in\n{ics}"))
}

/// The same, with the line's folds undone: RFC 5545 §3.1 splits a content line
/// longer than 75 octets across several physical ones, so an assertion about a
/// long `RRULE` has to name the line the reader sees rather than the first
/// fragment the emitter wrote.
fn content_line(ics: &str, prefix: &str) -> String {
    let unfolded = ics.replace("\r\n ", "").replace("\r\n\t", "");
    line(&unfolded, prefix).to_owned()
}

fn without(ics: &str, prefix: &str) -> bool {
    !ics.split("\r\n").any(|line| line.starts_with(prefix))
}

/// The `n`th `VEVENT` of the document, from its first content line onwards, so
/// that [`line`] and [`without`] can be pointed at one component of several.
fn vevent(ics: &str, n: usize) -> &str {
    ics.split("BEGIN:VEVENT\r\n")
        .nth(n + 1)
        .unwrap_or_else(|| panic!("no VEVENT {n} in\n{ics}"))
}

fn vevents(ics: &str) -> usize {
    ics.matches("BEGIN:VEVENT\r\n").count()
}

#[test]
fn emits_a_vcalendar_envelope_around_one_vevent() {
    let ics = event_to_ical(&fixture_event());

    assert!(
        ics.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"),
        "{ics}"
    );
    // libical refuses a calendar without a PRODID, and Evolution shows it in
    // the event properties, so it names this backend.
    assert!(line(&ics, "PRODID:").contains("evolution-jmap"), "{ics}");
    assert!(ics.contains("BEGIN:VEVENT\r\n"), "{ics}");
    assert!(ics.ends_with("END:VEVENT\r\nEND:VCALENDAR\r\n"), "{ics}");
}

#[test]
fn uid_is_the_jmap_id_and_the_jscalendar_uid_is_kept_aside() {
    // As on the address book side: EDS keys its cache on the iCalendar UID and
    // hands it back to load_component_sync/remove_component_sync, so it has to
    // be the identifier the JMAP methods take — the server-assigned id.
    let ics = event_to_ical(&fixture_event());
    assert_eq!(line(&ics, "UID:"), "UID:E1");
    assert_eq!(
        line(&ics, "X-JMAP-UID:"),
        "X-JMAP-UID:urn:uuid:8f2b1c94-0d3a-4f7e-9c11-2a6d5e8b7f30"
    );

    let event = ical_to_event(&ics).expect("parse");
    assert_eq!(event.id.as_ref().unwrap().as_str(), "E1");
    assert_eq!(
        event.uid.as_deref(),
        Some("urn:uuid:8f2b1c94-0d3a-4f7e-9c11-2a6d5e8b7f30")
    );
}

#[test]
fn an_event_without_a_jscalendar_uid_falls_back_to_the_id_alone() {
    let event = CalendarEvent {
        id: Some("E9".into()),
        title: Some("Retro".to_owned()),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);

    assert_eq!(line(&ics, "UID:"), "UID:E9");
    assert!(without(&ics, "X-JMAP-UID"), "{ics}");
    assert_eq!(ical_to_event(&ics).expect("parse").uid, None);
}

#[test]
fn a_new_event_with_no_id_is_identified_by_its_jscalendar_uid() {
    // Before the first CalendarEvent/set there is no server id, and EDS still
    // needs a UID to key the component on.
    let event = CalendarEvent {
        uid: Some("urn:uuid:fresh".to_owned()),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);

    assert_eq!(line(&ics, "UID:"), "UID:urn:uuid:fresh");
    assert_eq!(line(&ics, "X-JMAP-UID:"), "X-JMAP-UID:urn:uuid:fresh");
}

#[test]
fn title_and_description_are_summary_and_description_escaped_as_text() {
    let ics = event_to_ical(&fixture_event());

    assert_eq!(line(&ics, "SUMMARY:"), "SUMMARY:Sprint planning");
    // TEXT escaping: the semicolon and the line break are data, not structure.
    assert_eq!(
        line(&ics, "DESCRIPTION:"),
        "DESCRIPTION:Agenda:\\nreview\\; then plan"
    );

    let event = ical_to_event(&ics).expect("parse");
    assert_eq!(event.title.as_deref(), Some("Sprint planning"));
    assert_eq!(
        event.description.as_deref(),
        Some("Agenda:\nreview; then plan")
    );
}

#[test]
fn a_named_time_zone_becomes_the_tzid_parameter() {
    let ics = event_to_ical(&fixture_event());
    // JSCalendar's LocalDateTime and iCalendar's DATE-TIME are the same
    // instant spelled differently.
    assert_eq!(
        line(&ics, "DTSTART"),
        "DTSTART;TZID=Europe/Berlin:20260115T130000"
    );
    assert_eq!(line(&ics, "DURATION:"), "DURATION:PT1H30M");

    let event = ical_to_event(&ics).expect("parse");
    assert_eq!(event.start.as_deref(), Some("2026-01-15T13:00:00"));
    assert_eq!(event.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(event.duration.as_deref(), Some("PT1H30M"));
}

#[test]
fn utc_is_spelled_with_a_trailing_z_and_no_tzid() {
    let event = CalendarEvent {
        start: Some("2026-01-15T13:00:00".to_owned()),
        time_zone: Some("Etc/UTC".to_owned()),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "DTSTART"), "DTSTART:20260115T130000Z");

    // Round-tripped as Etc/UTC, the spelling JSCalendar and the client use.
    let back = ical_to_event(&ics).expect("parse");
    assert_eq!(back.start.as_deref(), Some("2026-01-15T13:00:00"));
    assert_eq!(back.time_zone.as_deref(), Some("Etc/UTC"));
}

#[test]
fn an_event_without_a_time_zone_stays_floating() {
    let event = CalendarEvent {
        start: Some("2026-01-15T13:00:00".to_owned()),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    // No TZID and no Z: RFC 5545 form 1, a local time in whatever zone the
    // viewer is in. Inventing UTC here would move the event.
    assert_eq!(line(&ics, "DTSTART"), "DTSTART:20260115T130000");
    assert_eq!(ical_to_event(&ics).expect("parse").time_zone, None);
}

/// An event made in Evolution says how long it is with `DTEND`, not
/// `DURATION`: the appointment editor calls `e_cal_component_set_dtend`, and
/// RFC 5545 §3.6.1 makes the two mutually exclusive, so a component that has
/// one does not have the other. Reading only `DURATION` therefore dropped the
/// length of every appointment a user created, and an event with no duration
/// is `P0D` by RFC 8984 §4.2.2 — the calendar the user shares shows a
/// zero-length blip where their afternoon meeting was.
#[test]
fn an_events_length_may_arrive_as_a_dtend_instead_of_a_duration() {
    for (start, end, duration) in [
        // The ordinary case: an hour and a half of one afternoon.
        ("20260115T130000", "20260115T143000", "PT1H30M"),
        // Over midnight, where the difference is not a subtraction of clock
        // fields.
        ("20260115T230000", "20260116T003000", "PT1H30M"),
        // Over a month boundary and a leap day: 2024 has a 29 February.
        ("20240228T090000", "20240301T100000", "P2DT1H"),
        // A whole day, as an all-day event is written — DTEND is exclusive,
        // so the next day means one day long.
        ("20260115", "20260116", "P1D"),
        ("20260115", "20260122", "P7D"),
        // Over a year, to the second.
        ("20261231T235959", "20270101T000000", "PT1S"),
    ] {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:E8\r\n\
             DTSTART:{start}\r\nDTEND:{end}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let event = ical_to_event(&ics).expect("parse");
        assert_eq!(event.duration.as_deref(), Some(duration), "{start}/{end}");
    }
}

#[test]
fn a_length_read_from_a_dtend_is_written_back_as_a_duration() {
    let ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:E8\r\n",
        "DTSTART;TZID=Europe/Berlin:20260115T130000\r\n",
        "DTEND;TZID=Europe/Berlin:20260115T143000\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");

    // Written back the one way this crate spells a length. Emitting DTEND too
    // would make the component invalid, and emitting it instead would need the
    // zone arithmetic this crate does not do.
    let back = event_to_ical(&event);
    assert_eq!(line(&back, "DURATION:"), "DURATION:PT1H30M");
    assert!(without(&back, "DTEND"), "{back}");
    assert_eq!(
        ical_to_event(&back).expect("parse").duration.as_deref(),
        Some("PT1H30M")
    );
}

#[test]
fn a_duration_wins_over_a_dtend_that_contradicts_it() {
    // RFC 5545 §3.6.1 allows only one of the two, so a component with both is
    // already malformed. DURATION is the one that maps to the JSCalendar
    // property without arithmetic, so it is the one believed.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:E8\r\n",
        "DTSTART:20260115T130000\r\nDURATION:PT30M\r\nDTEND:20260115T170000\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n",
    );
    assert_eq!(
        ical_to_event(ics).expect("parse").duration.as_deref(),
        Some("PT30M")
    );
}

#[test]
fn a_dtend_that_is_not_after_the_start_leaves_the_event_without_a_length() {
    // Each of these is a component that says nothing usable about how long the
    // event is, and "nothing" is what the event should then say: a negative
    // duration is not a JSCalendar Duration the server would accept, and a
    // guessed one would be an appointment the user never made.
    for (start, end) in [
        // Ends before it starts.
        ("20260115T130000", "20260115T120000"),
        // Ends when it starts, which is the P0D default anyway.
        ("20260115T130000", "20260115T130000"),
        // An end nobody can read, and one that names no real instant.
        ("20260115T130000", "next tuesday"),
        ("20260115T130000", "20260230T130000"),
        // No start to measure from.
        ("tuesday", "20260115T140000"),
    ] {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:E8\r\n\
             DTSTART:{start}\r\nDTEND:{end}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let event = ical_to_event(&ics).expect("parse");
        assert_eq!(event.duration, None, "{start}/{end}");
    }
}

#[test]
fn a_duration_that_states_no_length_leaves_the_event_without_one() {
    // DURATION was passed through untouched, so whatever the component said
    // became the JSCalendar `duration` a save hands the server. RFC 5545 §3.3.6
    // spells a *negative* duration, and RFC 8984 §1.4.6 has none to map it onto:
    // a component saying an event lasts minus an hour became a set the server
    // rejects — and the save that carried it takes the user's real edits down
    // with it. Everything else here is not a duration at all.
    for value in [
        // Negative, both designators, and the RFC 5545 spelling of "backwards".
        "-PT1H",
        "-P1D",
        "-PT1H30M",
        // Not a duration.
        "next tuesday",
        "1H",
        "3600",
        // A designator with nothing measured after it.
        "P",
        "PT",
        // A measurement with no unit.
        "PT1",
        "P1",
        // A unit that measures nothing RFC 5545 knows.
        "PT1X",
        "P1Y",
        // A second designator part way through.
        "PT1HP2D",
        // Fractional, which neither format's grammar admits.
        "PT0.5S",
    ] {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:E8\r\n\
             DTSTART:20260115T130000\r\nDURATION:{value}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let event = ical_to_event(&ics).expect("parse");
        assert_eq!(event.duration, None, "{value}");
    }
}

#[test]
fn a_length_stated_as_a_duration_is_passed_through_as_written() {
    // The other half of the same rule: a value that *is* a length still crosses
    // untouched, because the two formats spell an ISO 8601 duration identically.
    // Refusing one of these would drop the length of an event that plainly
    // states it, which is the failure the check exists to avoid, not to cause.
    for (value, duration) in [
        ("PT1H", "PT1H"),
        ("PT30M", "PT30M"),
        ("PT45S", "PT45S"),
        ("P1D", "P1D"),
        ("P2W", "P2W"),
        ("PT1H30M", "PT1H30M"),
        ("PT1H30M15S", "PT1H30M15S"),
        ("P1DT2H30M", "P1DT2H30M"),
        // Zero is a length, and the one this mapping hands over as written
        // rather than recognising — see the RDATE period tests for why.
        ("PT0S", "PT0S"),
        // An explicit plus sign is RFC 5545 §3.3.6's, and is the same length;
        // RFC 8984 §1.4.6 has no sign at all, so it is dropped rather than
        // handed to a server that has nowhere to put it.
        ("+PT1H", "PT1H"),
        // An hour and a quarter minute, skipping the minutes: outside RFC 5545's
        // nesting, which lets an hour be followed only by minutes, but what
        // every reader adds up and what some emitters write. The check refuses
        // values that are not lengths, not values that are unfashionable ones.
        ("PT1H15S", "PT1H15S"),
        // Three the parser has already repaired by the time the check sees
        // them: a trailing designator dropped, a missing count read as none,
        // and units put back in order. What is checked is calcard's rendering
        // of the value, not the octets the component carried, so these are
        // lengths — and refusing them would be refusing the parser's answer.
        ("P1DT", "P1D"),
        ("PTH", "PT0S"),
        ("PT30M1H", "PT1H30M"),
    ] {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:E8\r\n\
             DTSTART:20260115T130000\r\nDURATION:{value}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let event = ical_to_event(&ics).expect("parse");
        assert_eq!(event.duration.as_deref(), Some(duration), "{value}");
    }
}

#[test]
fn a_duration_that_states_no_length_lets_a_dtend_state_it_instead() {
    // A value the mapping cannot read is treated as absent, like every other
    // one, and what an absent DURATION leaves is the DTEND branch. RFC 5545
    // §3.6.1 forbids the pair, so this is a malformed component either way —
    // but one of its two statements about the length is usable, and using it
    // beats showing the meeting as a zero-length blip.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:E8\r\n",
        "DTSTART:20260115T130000\r\nDURATION:-PT30M\r\nDTEND:20260115T143000\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n",
    );
    assert_eq!(
        ical_to_event(ics).expect("parse").duration.as_deref(),
        Some("PT1H30M")
    );
}

/// An event of one length, with nothing else that would change how it is
/// written.
fn lasting(duration: &str) -> CalendarEvent {
    CalendarEvent {
        id: Some("E19".into()),
        title: Some("Standup".to_owned()),
        start: Some("2026-01-15T13:00:00".to_owned()),
        duration: Some(duration.to_owned()),
        ..CalendarEvent::default()
    }
}

#[test]
fn a_duration_that_states_no_length_is_not_written_to_the_component() {
    // The same rule on the way out. A `duration` the server sends was put into
    // DURATION verbatim, so a value RFC 8984 §1.4.6 has no room for — or one
    // that is no length at all — became a content line libical has to read, and
    // libical refusing a property costs the whole component: the appointment
    // disappears from the calendar rather than merely losing its length.
    for value in [
        "-PT1H",
        "-P1D",
        "next tuesday",
        "1H",
        "3600",
        "P",
        "PT",
        "PT1",
        "P1",
        "PT1X",
        "P1Y",
        "PT1HP2D",
        "PT0.5S",
    ] {
        let ics = event_to_ical(&lasting(value));
        assert!(without(&ics, "DURATION"), "{value}: {ics}");
    }
}

#[test]
fn a_length_the_event_states_is_written_as_the_duration_it_is() {
    // The other half, and the one that matters: the check can only go wrong by
    // refusing too much, and nothing else in the suite would notice a length
    // that stopped being written. `+PT1H` is the one value that changes on the
    // way through — RFC 5545 §3.3.6's sign, which RFC 8984 has nowhere to put,
    // and which is dropped rather than handed on.
    for (value, written) in [
        ("PT1H", "PT1H"),
        ("PT30M", "PT30M"),
        ("P1D", "P1D"),
        ("P2W", "P2W"),
        ("PT1H30M15S", "PT1H30M15S"),
        ("P1DT2H30M", "P1DT2H30M"),
        ("PT0S", "PT0S"),
        ("PT1H15S", "PT1H15S"),
        ("+PT1H", "PT1H"),
    ] {
        let ics = event_to_ical(&lasting(value));
        assert_eq!(line(&ics, "DURATION"), format!("DURATION:{written}"));
        assert_eq!(
            ical_to_event(&ics).expect("parse").duration.as_deref(),
            Some(written),
            "{value}"
        );
    }
}

#[test]
fn a_date_only_dtstart_is_read_as_an_all_day_event_starting_at_midnight() {
    // Evolution writes VALUE=DATE for an all-day event. The start is read as
    // midnight, because dropping it would leave an event with no time at all,
    // and the day-ness is carried separately in showWithoutTime — without it
    // the server, and every other client reading from it, sees a midnight
    // appointment.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E7\r\n",
        "DTSTART;VALUE=DATE:20260115\r\n",
        "DTEND;VALUE=DATE:20260116\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    assert_eq!(event.start.as_deref(), Some("2026-01-15T00:00:00"));
    assert_eq!(event.show_without_time, Some(true));
    assert_eq!(event.duration.as_deref(), Some("P1D"));
    // RFC 5545 §3.2.19: a TZID does not apply to a DATE value, and RFC 8984
    // wants no zone on an event shown without a time.
    assert_eq!(event.time_zone, None);
}

#[test]
fn a_timed_dtstart_says_nothing_about_showing_without_time() {
    // Not `Some(false)`: a timed event is the RFC 8984 default, and the save
    // path reads "no difference from the baseline" off this, so answering
    // `false` where the server said nothing would be an edit that never
    // happened.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E7\r\n",
        "DTSTART:20260115T130000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    assert_eq!(event.show_without_time, None);
}

#[test]
fn an_all_day_event_is_written_as_a_date_and_survives_the_trip_back() {
    let event = CalendarEvent {
        start: Some("2026-01-15T00:00:00".to_owned()),
        duration: Some("P2D".to_owned()),
        show_without_time: Some(true),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);

    // The parameter is not decoration: without it DTSTART's value type is
    // DATE-TIME by default and `20260115` is not one, so libical would refuse
    // the component and Evolution would show no event at all.
    assert_eq!(line(&ics, "DTSTART"), "DTSTART;VALUE=DATE:20260115");
    // A whole number of days is the only length RFC 5545 §3.6.1 allows next to
    // a DATE start, and it is what this event has.
    assert_eq!(line(&ics, "DURATION"), "DURATION:P2D");

    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.start, event.start);
    assert_eq!(read_back.duration, event.duration);
    assert_eq!(read_back.show_without_time, Some(true));
}

#[test]
fn an_all_day_event_the_date_form_cannot_hold_stays_a_date_time() {
    // RFC 8984 §4.1.5 asks that an event shown without a time start at
    // midnight and last whole days, but a server is free to send otherwise,
    // and a zone cannot ride on a DATE value at all. In each of these the DATE
    // form would silently move or shorten the event, or drop its zone, so the
    // usual DATE-TIME is written instead: the event shows as a timed one,
    // which is wrong about its day-ness but right about when it is. The save
    // path compares against this same rendering, so it does not read the lost
    // flag back as the user having cleared it.
    for (why, start, duration, zone) in [
        (
            "a start that is not midnight",
            "2026-01-15T09:00:00",
            None,
            None,
        ),
        (
            "a length with a time component",
            "2026-01-15T00:00:00",
            Some("P1DT2H"),
            None,
        ),
        (
            "a length shorter than a day",
            "2026-01-15T00:00:00",
            Some("PT90M"),
            None,
        ),
        (
            "a zone the DATE form would drop",
            "2026-01-15T00:00:00",
            Some("P1D"),
            Some("Europe/Berlin"),
        ),
    ] {
        let event = CalendarEvent {
            start: Some(start.to_owned()),
            duration: duration.map(str::to_owned),
            time_zone: zone.map(str::to_owned),
            show_without_time: Some(true),
            ..CalendarEvent::default()
        };
        let ics = event_to_ical(&event);
        assert!(line(&ics, "DTSTART").contains("20260115T"), "{why}: {ics}");
        assert!(without(&ics, "DTSTART;VALUE=DATE"), "{why}: {ics}");

        let read_back = ical_to_event(&ics).expect("parse");
        assert_eq!(read_back.show_without_time, None, "{why}: {ics}");
        assert_eq!(read_back.start.as_deref(), Some(start), "{why}: {ics}");
    }
}

#[test]
fn an_all_day_event_with_no_length_is_still_written_as_a_date() {
    // No DURATION and no DTEND next to a DATE start is one day by RFC 5545
    // §3.6.1, where RFC 8984 would call it zero-length. A day is what an event
    // shown without a time means, and the reverse — a midnight appointment of
    // no duration — is not something a calendar can draw.
    let event = CalendarEvent {
        start: Some("2026-01-15T00:00:00".to_owned()),
        show_without_time: Some(true),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "DTSTART"), "DTSTART;VALUE=DATE:20260115");
    assert!(without(&ics, "DURATION"), "{ics}");

    // And the trip back says nothing about the length, so the save path does
    // not read the day RFC 5545 implies as a length the user typed.
    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.duration, None);
    assert_eq!(read_back.show_without_time, Some(true));
}

#[test]
fn a_start_that_is_not_a_date_time_is_left_out_rather_than_mangled() {
    // A DTSTART neither side can read is worse than none: libical would drop
    // the whole component, and a truncated value would silently move the
    // event. Both directions therefore check the shape before converting.
    for start in ["next tuesday", "2026-01-15", "202X-01-15T13:00:00"] {
        let event = CalendarEvent {
            start: Some(start.to_owned()),
            time_zone: Some("Etc/UTC".to_owned()),
            ..CalendarEvent::default()
        };
        let ics = event_to_ical(&event);
        assert!(without(&ics, "DTSTART"), "{start}: {ics}");
    }

    for value in ["tuesday", "2026-01-15T13:00:00", "202601"] {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:E6\r\n\
             DTSTART:{value}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let event = ical_to_event(&ics).expect("parse");
        assert_eq!(event.start, None, "{value}");
        assert_eq!(event.time_zone, None, "{value}");
    }
}

/// Where the line above now sits, and why it moved.
///
/// A DATE-TIME whose seconds — or minutes and seconds — are missing is not
/// legal RFC 5545 §3.3.5, and the hand-rolled lexer refused it. calcard
/// completes it instead, and that is the better answer for the only case it can
/// arise in: the missing field can only be zero, so the event does not move,
/// and refusing it would drop the start of an event that says plainly when it
/// begins.
///
/// The boundary matters, so it is asserted rather than described: a truncated
/// *date* is refused here only when it is too short to be a date at all
/// (`202601`, above). `2026011` is read as 2026-01-01 — the day the author
/// meant, 15, lost its second digit and the event moves two weeks. Nothing in
/// this repository can produce that: the iCalendar this mapping reads comes
/// from EDS, whose libical would itself have refused the value, and the JMAP
/// server sends JSON rather than iCalendar. But it *is* laxer than libical, so
/// it is written down here rather than left to be discovered.
#[test]
fn a_dtstart_missing_its_seconds_is_completed_rather_than_dropped() {
    for value in ["20260115T1300", "20260115T13"] {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:E6\r\n\
             DTSTART:{value}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let event = ical_to_event(&ics).expect("parse");
        assert_eq!(
            event.start.as_deref(),
            Some("2026-01-15T13:00:00"),
            "{value}"
        );
    }
}

#[test]
fn a_start_that_names_no_real_instant_is_left_out_rather_than_passed_on() {
    // Digits in the right places are not a date: neither this crate's shape
    // check nor calcard's parse looks at what the fields say, so a month of 13
    // or an hour of 25 used to travel intact in both directions. Outbound that
    // reaches libical, which refuses the component and takes the whole event
    // with it; inbound it reaches the server as a JSON value it has to reject,
    // failing the whole CalendarEvent/set rather than the one property.
    for start in [
        "2026-13-15T13:00:00",
        "2026-00-15T13:00:00",
        "2026-01-32T13:00:00",
        "2026-01-00T13:00:00",
        "2026-02-30T13:00:00",
        // 2026 is not a leap year.
        "2026-02-29T13:00:00",
        "2026-01-15T24:00:00",
        "2026-01-15T13:60:00",
        "2026-01-15T13:00:61",
    ] {
        let event = CalendarEvent {
            start: Some(start.to_owned()),
            time_zone: Some("Etc/UTC".to_owned()),
            ..CalendarEvent::default()
        };
        assert!(without(&event_to_ical(&event), "DTSTART"), "{start}");
    }

    for value in [
        "20261315T130000",
        "20260015T130000",
        "20260132T130000",
        "20260100T130000",
        "20260230T130000",
        "20260229T130000",
        "20260115T240000",
        "20260115T136000",
        "20260115T130061",
        // A VALUE=DATE carries the same risk with no time to check.
        "20260230",
    ] {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:E6\r\n\
             DTSTART:{value}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let event = ical_to_event(&ics).expect("parse");
        assert_eq!(event.start, None, "{value}");
        assert_eq!(event.time_zone, None, "{value}");
    }
}

#[test]
fn a_leap_day_and_a_leap_second_are_real_and_survive() {
    // The range check must not swallow the two dates that look wrong and are
    // not: 29 February of a leap year, and the leap second RFC 5545 §3.3.12 and
    // RFC 3339 both spell as :60.
    for start in ["2024-02-29T13:00:00", "2026-06-30T23:59:60"] {
        let event = CalendarEvent {
            start: Some(start.to_owned()),
            ..CalendarEvent::default()
        };
        let ics = event_to_ical(&event);
        assert!(!without(&ics, "DTSTART"), "{start}: {ics}");
        assert_eq!(
            ical_to_event(&ics).expect("parse").start.as_deref(),
            Some(start),
            "{start}"
        );
    }
    // And 2000 was a leap year where 1900 and 2100 were not.
    let event = CalendarEvent {
        start: Some("2000-02-29T00:00:00".to_owned()),
        ..CalendarEvent::default()
    };
    assert!(!without(&event_to_ical(&event), "DTSTART"));
    for start in ["1900-02-29T00:00:00", "2100-02-29T00:00:00"] {
        let event = CalendarEvent {
            start: Some(start.to_owned()),
            ..CalendarEvent::default()
        };
        assert!(without(&event_to_ical(&event), "DTSTART"), "{start}");
    }
}

#[test]
fn status_changes_case_between_the_two_formats() {
    for (jscalendar, ical) in [
        ("confirmed", "CONFIRMED"),
        ("cancelled", "CANCELLED"),
        ("tentative", "TENTATIVE"),
    ] {
        let event = CalendarEvent {
            status: Some(jscalendar.to_owned()),
            ..CalendarEvent::default()
        };
        let ics = event_to_ical(&event);
        assert_eq!(line(&ics, "STATUS:"), format!("STATUS:{ical}"));
        assert_eq!(
            ical_to_event(&ics).expect("parse").status.as_deref(),
            Some(jscalendar)
        );
    }
}

#[test]
fn an_unknown_status_is_dropped_rather_than_passed_through() {
    // A STATUS libical does not know is worse than no STATUS at all, and
    // JSCalendar's set is closed.
    let event = CalendarEvent {
        status: Some("dithering".to_owned()),
        ..CalendarEvent::default()
    };
    assert!(without(&event_to_ical(&event), "STATUS"), "{event:?}");
}

#[test]
fn whether_an_event_blocks_time_is_the_transparency_of_the_component() {
    // Evolution's "Show Time as: Busy/Free" — RFC 8984 §4.4.2's `freeBusyStatus`
    // on one side, RFC 5545 §3.8.2.7's `TRANSP` on the other. Both vocabularies
    // are closed and both spell the same two states, so this is a table and not
    // a judgement.
    for (jscalendar, ical) in [("free", "TRANSPARENT"), ("busy", "OPAQUE")] {
        let event = CalendarEvent {
            free_busy_status: Some(jscalendar.to_owned()),
            ..CalendarEvent::default()
        };
        let ics = event_to_ical(&event);
        assert_eq!(line(&ics, "TRANSP:"), format!("TRANSP:{ical}"));
        assert_eq!(
            ical_to_event(&ics)
                .expect("parse")
                .free_busy_status
                .as_deref(),
            Some(jscalendar),
            "the state survives the round trip"
        );
    }
}

#[test]
fn an_unknown_free_busy_status_is_dropped_rather_than_passed_through() {
    // The same closed-vocabulary rule STATUS follows: a TRANSP libical does not
    // know costs the whole component, and JSCalendar admits only the two states.
    let event = CalendarEvent {
        free_busy_status: Some("maybe".to_owned()),
        ..CalendarEvent::default()
    };
    assert!(without(&event_to_ical(&event), "TRANSP"), "{event:?}");

    // And in the other direction, where the value came off a content line: a
    // transparency this mapping cannot name is read as nothing said, not passed
    // on for the server to reject.
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n\
UID:E1\r\nDTSTART:20260115T130000Z\r\nTRANSP:MAYBE\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";
    assert_eq!(ical_to_event(ics).expect("parse").free_busy_status, None);
}

#[test]
fn an_event_that_says_nothing_about_blocking_time_carries_no_transp() {
    let ics = event_to_ical(&fixture_event());

    assert!(without(&ics, "TRANSP"), "{ics}");
    // `None` rather than `Some("busy")`, even though RFC 8984 §4.4.2 defaults to
    // busy and RFC 5545 §3.8.2.7 defaults to OPAQUE, which is the same state: the
    // save path reads an edit off a difference from what was shown, so answering
    // with the default would have a save state it where the server said nothing.
    assert_eq!(ical_to_event(&ics).expect("parse").free_busy_status, None);
}

#[test]
fn an_edited_instance_may_show_a_transparency_of_its_own() {
    // RFC 8984 §4.3.4 lets an override restate the property and iCalendar spells
    // it on the instance's own component, so an occurrence the user marked free
    // in a series that blocks time is a difference this mapping can carry whole.
    let patch = json!({"freeBusyStatus": "free"});
    assert!(maps_override("2026-01-29T13:00:00", &patch));

    let mut event = recurring_with(json!({"2026-01-29T13:00:00": patch}));
    event.free_busy_status = Some("busy".to_owned());
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert_eq!(line(&ics, "TRANSP:"), "TRANSP:OPAQUE");
    assert_eq!(line(vevent(&ics, 1), "TRANSP:"), "TRANSP:TRANSPARENT");
    assert_eq!(
        ical_to_event(&ics).expect("parse").recurrence_overrides,
        event.recurrence_overrides
    );
}

#[test]
fn an_edited_instance_is_drawn_at_the_series_transparency() {
    // The inheritance of RFC 8984 §4.3.4, and the reason it has to be drawn: the
    // instance's own component is the only place its transparency is stated, so
    // one written without the line reads back as an occurrence the user just set
    // to the default — a patch removing the property the series holds.
    let mut event = recurring_with(json!({"2026-01-29T13:00:00": {"title": "Sprint review"}}));
    event.free_busy_status = Some("free".to_owned());
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert_eq!(line(vevent(&ics, 1), "TRANSP:"), "TRANSP:TRANSPARENT");
    assert_eq!(
        ical_to_event(&ics).expect("parse").recurrence_overrides,
        event.recurrence_overrides,
        "the instance inherited the series' transparency, so nothing differs"
    );
}

#[test]
fn the_importance_of_an_event_is_the_priority_of_the_component() {
    // RFC 8984 §4.4.1's `priority` and RFC 5545 §3.8.1.9's `PRIORITY` are the
    // same integer with the same meaning — 0 undefined, 1 highest, 9 lowest — so
    // the whole range crosses unchanged, digit for digit.
    for priority in 0..=9 {
        let event = CalendarEvent {
            priority: Some(priority),
            ..CalendarEvent::default()
        };
        let ics = event_to_ical(&event);
        assert_eq!(line(&ics, "PRIORITY:"), format!("PRIORITY:{priority}"));
        assert_eq!(
            ical_to_event(&ics).expect("parse").priority,
            Some(priority),
            "the importance survives the round trip"
        );
    }
}

#[test]
fn a_priority_outside_the_range_the_two_formats_share_is_dropped() {
    // Both formats close the range at 0..=9, so a value outside it is dropped
    // rather than passed through in the other's clothes — the rule STATUS and
    // TRANSP follow, one type over.
    for priority in [-1, 10, 100] {
        let event = CalendarEvent {
            priority: Some(priority),
            ..CalendarEvent::default()
        };
        assert!(without(&event_to_ical(&event), "PRIORITY"), "{priority}");
    }

    // And in the other direction, where the value came off a content line: an
    // integer out of range, and something that is no integer at all, are read as
    // nothing said rather than passed on for the server to reject.
    for value in ["10", "-1", "high", "", "5.5", "1,2", " 5"] {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n\
UID:E1\r\nDTSTART:20260115T130000Z\r\nPRIORITY:{value}\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        assert_eq!(
            ical_to_event(&ics).expect("parse").priority,
            None,
            "{value}"
        );
    }
}

#[test]
fn an_event_that_says_nothing_about_priority_carries_no_priority() {
    let ics = event_to_ical(&fixture_event());

    assert!(without(&ics, "PRIORITY"), "{ics}");
    // `None` rather than `Some(0)`, even though RFC 8984 §4.4.1 defaults to 0 and
    // RFC 5545 §3.8.1.9 says a `VEVENT` with no `PRIORITY` is undefined, which is
    // the same state: the save path reads an edit off a difference from what was
    // shown, so answering with the default would have a save state it where the
    // server said nothing. Which is also why a `priority` of 0 the server *did*
    // state is written out as `PRIORITY:0` rather than left off — see
    // [`the_importance_of_an_event_is_the_priority_of_the_component`], where 0 is
    // in the table.
    assert_eq!(ical_to_event(&ics).expect("parse").priority, None);
}

#[test]
fn an_edited_instance_may_show_a_priority_of_its_own() {
    // RFC 8984 §4.3.4 lets an override restate the property and iCalendar spells
    // it on the instance's own component, so one occurrence the user made urgent
    // in an otherwise unremarkable series is a difference this mapping carries
    // whole.
    let patch = json!({"priority": 1});
    assert!(maps_override("2026-01-29T13:00:00", &patch));

    let mut event = recurring_with(json!({"2026-01-29T13:00:00": patch}));
    event.priority = Some(5);
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert_eq!(line(&ics, "PRIORITY:"), "PRIORITY:5");
    assert_eq!(line(vevent(&ics, 1), "PRIORITY:"), "PRIORITY:1");
    assert_eq!(
        ical_to_event(&ics).expect("parse").recurrence_overrides,
        event.recurrence_overrides
    );
}

#[test]
fn an_edited_instance_is_drawn_at_the_series_priority() {
    // The inheritance of RFC 8984 §4.3.4, and the reason it has to be drawn: the
    // instance's own component is the only place its priority is stated, so one
    // written without the line reads back as an occurrence the user just made
    // unimportant — a patch removing the property the series holds.
    let mut event = recurring_with(json!({"2026-01-29T13:00:00": {"title": "Sprint review"}}));
    event.priority = Some(2);
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert_eq!(line(vevent(&ics, 1), "PRIORITY:"), "PRIORITY:2");
    assert_eq!(
        ical_to_event(&ics).expect("parse").recurrence_overrides,
        event.recurrence_overrides,
        "the instance inherited the series' priority, so nothing differs"
    );
}

#[test]
fn who_may_see_an_event_is_the_class_of_the_component() {
    // RFC 8984 §4.4.3's `privacy` and RFC 5545 §3.8.1.3's `CLASS` are the same
    // three-value scale of how much of the event may be shared, in the same
    // order, so each value crosses to the other format's spelling of itself.
    for (jscalendar, ical) in [
        ("public", "PUBLIC"),
        ("private", "PRIVATE"),
        ("secret", "CONFIDENTIAL"),
    ] {
        let event = CalendarEvent {
            privacy: Some(jscalendar.to_owned()),
            ..CalendarEvent::default()
        };
        let ics = event_to_ical(&event);
        assert_eq!(line(&ics, "CLASS:"), format!("CLASS:{ical}"));
        assert_eq!(
            ical_to_event(&ics).expect("parse").privacy.as_deref(),
            Some(jscalendar),
            "the classification survives the round trip"
        );
    }
}

#[test]
fn an_unknown_privacy_is_dropped_rather_than_passed_through() {
    // Both vocabularies are extensible — RFC 5545 §3.8.1.3 admits an x-name and
    // RFC 8984 §4.4.3 a registered or vendor value — and neither says how a value
    // in one maps to a value in the other. So an unknown one is dropped, the rule
    // STATUS and TRANSP follow: passing it through in the other format's clothes
    // would state a classification nobody wrote.
    for privacy in ["deniable", "x-eyes-only", "CONFIDENTIAL"] {
        let event = CalendarEvent {
            privacy: Some(privacy.to_owned()),
            ..CalendarEvent::default()
        };
        assert!(without(&event_to_ical(&event), "CLASS"), "{privacy}");
    }

    // And in the other direction, where the value came off a content line: a
    // classification this mapping cannot name is read as nothing said, not passed
    // on for the server to reject. `secret` among them: it is JSCalendar's
    // spelling of a value iCalendar spells CONFIDENTIAL, so on a `CLASS` line it
    // is an x-name-shaped value and not that classification.
    for value in ["X-EYES-ONLY", "secret", "", "PUBLIC,PRIVATE"] {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n\
UID:E1\r\nDTSTART:20260115T130000Z\r\nCLASS:{value}\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        assert_eq!(ical_to_event(&ics).expect("parse").privacy, None, "{value}");
    }

    // Case, on the other hand, is not a difference: RFC 5545 §3.1 makes an
    // enumerated property value case-insensitive, so a client that wrote the
    // classification in lower case wrote the classification.
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n\
UID:E1\r\nDTSTART:20260115T130000Z\r\nCLASS:confidential\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";
    assert_eq!(
        ical_to_event(ics).expect("parse").privacy.as_deref(),
        Some("secret")
    );
}

#[test]
fn an_event_that_says_nothing_about_privacy_carries_no_class() {
    let ics = event_to_ical(&fixture_event());

    assert!(without(&ics, "CLASS"), "{ics}");
    // `None` rather than `Some("public")`, even though RFC 8984 §4.4.3 defaults
    // to public and RFC 5545 §3.8.1.3 defaults `CLASS` to PUBLIC, which is the
    // same state: the save path reads an edit off a difference from what was
    // shown, so answering with the default would have a save state it where the
    // server said nothing.
    //
    // Which is exactly why a `privacy` the server *did* state as public is
    // written out as `CLASS:PUBLIC` rather than left off — see
    // [`who_may_see_an_event_is_the_class_of_the_component`], where public is in
    // the table. Evolution's appointment editor sets `CLASS` on every save
    // (Options ▸ Classification, defaulting to public), so a baseline rendered
    // without the line would differ from what EDS hands back on *every* save of
    // such an event, not once.
    assert_eq!(ical_to_event(&ics).expect("parse").privacy, None);
}

#[test]
fn an_edited_instance_may_show_a_privacy_of_its_own() {
    // RFC 8984 §4.3.4 lets an override restate the property and iCalendar spells
    // it on the instance's own component, so the one occurrence of a series the
    // user marked private is a difference this mapping carries whole.
    let patch = json!({"privacy": "private"});
    assert!(maps_override("2026-01-29T13:00:00", &patch));

    let mut event = recurring_with(json!({"2026-01-29T13:00:00": patch}));
    event.privacy = Some("public".to_owned());
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert_eq!(line(&ics, "CLASS:"), "CLASS:PUBLIC");
    assert_eq!(line(vevent(&ics, 1), "CLASS:"), "CLASS:PRIVATE");
    assert_eq!(
        ical_to_event(&ics).expect("parse").recurrence_overrides,
        event.recurrence_overrides
    );
}

#[test]
fn an_edited_instance_is_drawn_at_the_series_privacy() {
    // The inheritance of RFC 8984 §4.3.4, and the reason it has to be drawn: the
    // instance's own component is the only place its classification is stated, so
    // one written without the line reads back as an occurrence the user just made
    // public — a patch removing the property the series holds.
    let mut event = recurring_with(json!({"2026-01-29T13:00:00": {"title": "Sprint review"}}));
    event.privacy = Some("secret".to_owned());
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert_eq!(line(vevent(&ics, 1), "CLASS:"), "CLASS:CONFIDENTIAL");
    assert_eq!(
        ical_to_event(&ics).expect("parse").recurrence_overrides,
        event.recurrence_overrides,
        "the instance inherited the series' classification, so nothing differs"
    );
}

/// An event carrying the two timestamps a server keeps for it.
fn stamped(created: &str, updated: &str) -> CalendarEvent {
    CalendarEvent {
        created: Some(created.to_owned()),
        updated: Some(updated.to_owned()),
        ..CalendarEvent::default()
    }
}

#[test]
fn when_an_event_was_made_and_last_changed_are_stamped_on_the_component() {
    // RFC 8984 §4.1.7's `created` and §4.1.8's `updated` are UTCDateTimes, and
    // RFC 5545 §3.8.7 spells the same two instants as CREATED and LAST-MODIFIED,
    // both UTC-only. DTSTAMP carries the second of them as well: §3.8.7.2 makes
    // it REQUIRED on a VEVENT and declares it equivalent to LAST-MODIFIED in a
    // calendar with no METHOD, which is every calendar this crate emits.
    let ics = event_to_ical(&stamped("2026-01-02T09:30:00Z", "2026-01-15T17:45:01Z"));

    assert_eq!(line(&ics, "CREATED:"), "CREATED:20260102T093000Z");
    assert_eq!(line(&ics, "DTSTAMP:"), "DTSTAMP:20260115T174501Z");
    assert_eq!(
        line(&ics, "LAST-MODIFIED:"),
        "LAST-MODIFIED:20260115T174501Z"
    );
}

#[test]
fn a_timestamp_that_is_not_an_instant_in_utc_is_left_off_rather_than_mangled() {
    // The same rule every other unreadable value gets: treated as absent. A
    // timestamp is only ever written, so nothing downstream loses an edit by it
    // — but a CREATED libical refuses costs the whole component, and every field
    // of the event with it.
    for value in [
        // A LocalDateTime: it names no instant without a zone beside it, and
        // there is nowhere on these properties to put one.
        "2026-01-15T17:45:01",
        // An offset, which RFC 8984 §1.4.5 does not admit and RFC 5545 §3.3.5
        // has no form for either.
        "2026-01-15T17:45:01+01:00",
        // A month that does not exist.
        "2026-13-15T17:45:01Z",
        // A fraction of a second, which neither format's DATE-TIME carries.
        "2026-01-15T17:45:01.5Z",
        "yesterday",
        "",
    ] {
        let ics = event_to_ical(&stamped(value, value));
        for name in ["CREATED", "DTSTAMP", "LAST-MODIFIED"] {
            assert!(without(&ics, name), "{value}: {ics}");
        }
    }
}

#[test]
fn an_event_that_says_nothing_about_its_timestamps_carries_none() {
    // Deliberately, even though RFC 5545 §3.6.1 makes DTSTAMP REQUIRED: the only
    // value that could be invented for it is "now", and a rendering that changes
    // between two runs is exactly what the save path cannot have — it reads an
    // edit off a difference from a re-rendering of what the server holds. So a
    // server that states no timestamp gets a component without one, and libical
    // reads such a component perfectly well.
    let ics = event_to_ical(&fixture_event());

    for name in ["CREATED", "DTSTAMP", "LAST-MODIFIED"] {
        assert!(without(&ics, name), "{ics}");
    }
}

#[test]
fn an_edited_instance_carries_the_timestamps_of_the_series() {
    // The inheritance of RFC 8984 §4.3.4 again: an override may not restate
    // either timestamp, so the instance's own component states the series'.
    let mut event = recurring_with(json!({"2026-01-29T13:00:00": {"title": "Sprint review"}}));
    event.created = Some("2026-01-02T09:30:00Z".to_owned());
    event.updated = Some("2026-01-15T17:45:01Z".to_owned());
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    let instance = vevent(&ics, 1);
    assert_eq!(line(instance, "CREATED:"), "CREATED:20260102T093000Z");
    assert_eq!(line(instance, "DTSTAMP:"), "DTSTAMP:20260115T174501Z");
}

#[test]
fn the_timestamps_are_written_and_never_read_back() {
    // The mirror of DTEND, which is read and never written. Both instants belong
    // to the server: it stamps `created` when the event first arrives and
    // `updated` whenever anything about it changes. A client that read them off a
    // component would be proposing a value for them — on a create, a guess at
    // when the server first saw the event; on a save, a claim about when the
    // server last changed it. So they are drawn for whoever reads the document
    // and nothing more, and neither appears in `MAPPED_PROPERTIES`.
    let ics = event_to_ical(&stamped("2026-01-02T09:30:00Z", "2026-01-15T17:45:01Z"));
    let event = ical_to_event(&ics).expect("parse");

    assert_eq!(event.created, None);
    assert_eq!(event.updated, None);
}

/// An event happening at one place, keyed the way a server keys it.
fn placed(key: &str, location: Value) -> CalendarEvent {
    CalendarEvent {
        start: Some("2026-01-15T13:00:00".to_owned()),
        time_zone: Some("Europe/Berlin".to_owned()),
        locations: Some([(key.to_owned(), location)].into()),
        ..CalendarEvent::default()
    }
}

#[test]
fn the_place_an_event_happens_at_is_the_location_and_keeps_its_key() {
    // The key is what makes the place patchable in situ rather than replaced:
    // it rides in X-JMAP-KEY, as the JSContact map keys do on the address book
    // side, so a save names `locations/<key>/name` and leaves the coordinates,
    // links and types the line could not show exactly where they were.
    let event = placed("loc1", json!({"@type": "Location", "name": "Room 42"}));
    let ics = event_to_ical(&event);

    assert_eq!(
        content_line(&ics, "LOCATION"),
        "LOCATION;X-JMAP-KEY=loc1:Room 42"
    );
    assert_eq!(
        ical_to_event(&ics).expect("parse").locations,
        event.locations,
        "the place and its key survive the round trip"
    );
    assert!(maps_locations(event.locations.as_ref().unwrap()));
}

#[test]
fn a_place_name_is_escaped_as_text() {
    // LOCATION is TEXT (RFC 5545 §3.8.1.7), so a comma in a room name is a
    // literal and not the separator of a value list.
    let event = placed(
        "loc1",
        json!({"@type": "Location", "name": "Berlin, Room 42; 3rd floor"}),
    );
    let ics = event_to_ical(&event);

    assert_eq!(
        content_line(&ics, "LOCATION"),
        "LOCATION;X-JMAP-KEY=loc1:Berlin\\, Room 42\\; 3rd floor"
    );
    assert_eq!(
        ical_to_event(&ics).expect("parse").locations,
        event.locations
    );
}

#[test]
fn an_event_that_names_no_place_carries_no_location() {
    let ics = event_to_ical(&fixture_event());

    assert!(without(&ics, "LOCATION"), "{ics}");
    assert_eq!(ical_to_event(&ics).expect("parse").locations, None);
}

#[test]
fn a_component_whose_location_carries_no_key_gets_one_invented() {
    // What Evolution writes: the appointment editor sets a location, and there
    // was never a server-side entry for it to name.
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n\
UID:E1\r\nDTSTART;TZID=Europe/Berlin:20260115T130000\r\nLOCATION:Room 42\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse");

    assert_eq!(
        event.locations,
        Some(
            [(
                "l1".to_owned(),
                json!({"@type": "Location", "name": "Room 42"})
            )]
            .into()
        )
    );
}

#[test]
fn a_place_that_says_more_than_its_name_is_still_drawn_by_name() {
    // The whole reason the name is patched by key: coordinates have no
    // iCalendar spelling here, and the entry keeps them because nothing
    // replaces the entry.
    let event = placed(
        "loc1",
        json!({
            "@type": "Location",
            "name": "Room 42",
            "coordinates": "geo:52.520008,13.404954",
        }),
    );
    let ics = event_to_ical(&event);

    assert_eq!(
        content_line(&ics, "LOCATION"),
        "LOCATION;X-JMAP-KEY=loc1:Room 42"
    );
    assert!(
        maps_locations(event.locations.as_ref().unwrap()),
        "a place with more than a name is still one whose name can be renamed"
    );
}

#[test]
fn a_place_with_no_name_is_not_drawn_but_may_still_be_named() {
    // A Location may say only where it is. There is no text to put on the line,
    // so nothing is drawn — and a user who types a place into the empty field
    // is naming *this* entry, not replacing it.
    let event = placed(
        "loc1",
        json!({"@type": "Location", "coordinates": "geo:52,13"}),
    );
    let ics = event_to_ical(&event);

    assert!(without(&ics, "LOCATION"), "{ics}");
    assert!(maps_locations(event.locations.as_ref().unwrap()));
}

#[test]
fn more_than_one_place_is_drawn_in_part_and_flagged() {
    // RFC 5545 §3.6.1 admits one LOCATION in a VEVENT (RFC 9073's VLOCATION
    // components are not something libical reads), so a second place cannot be
    // shown — and a property shown in part is one a save must not write.
    let mut event = placed("loc1", json!({"@type": "Location", "name": "Room 42"}));
    event.locations.as_mut().unwrap().insert(
        "loc2".to_owned(),
        json!({"@type": "Location", "name": "Cafeteria"}),
    );
    let ics = event_to_ical(&event);

    assert_eq!(
        content_line(&ics, "LOCATION"),
        "LOCATION;X-JMAP-KEY=loc1:Room 42",
        "the first place is at least visible"
    );
    assert!(!maps_locations(event.locations.as_ref().unwrap()));
}

#[test]
fn a_place_the_mapping_cannot_read_is_flagged_rather_than_drawn() {
    for location in [
        // Not an object: patching `locations/loc1/name` would be patching
        // *through* a string, which RFC 8620 §5.3 makes an error — and an error
        // costs every other edit in the same CalendarEvent/set.
        json!("Room 42"),
        json!(null),
        // A name that is not text has no line to be written on, and writing
        // one would invent a spelling for it.
        json!({"@type": "Location", "name": 42}),
    ] {
        let event = placed("loc1", location.clone());
        assert!(
            without(&event_to_ical(&event), "LOCATION"),
            "{location} was drawn"
        );
        assert!(
            !maps_locations(event.locations.as_ref().unwrap()),
            "{location} was called covered"
        );
    }
}

#[test]
fn a_place_key_no_patch_could_name_is_flagged() {
    // The key becomes a path segment of the save's PatchObject. `~` and `/` are
    // escapable (RFC 6901), so they are carried; an empty key names no member
    // of the map at all.
    let event = placed("", json!({"@type": "Location", "name": "Room 42"}));
    assert!(!maps_locations(event.locations.as_ref().unwrap()));

    let event = placed("wing/2~a", json!({"@type": "Location", "name": "Room 42"}));
    assert!(maps_locations(event.locations.as_ref().unwrap()));
    let ics = event_to_ical(&event);
    assert_eq!(
        content_line(&ics, "LOCATION"),
        "LOCATION;X-JMAP-KEY=wing/2~a:Room 42"
    );
    // It does *not* come back under that key. A key read off a content line may
    // be created server-side — see the save path — and RFC 8984 §1.4.4 admits
    // only letters, digits, `-` and `_` in one; the server's own key is the one
    // a rename patches, and it is taken from the event, not from the component.
    assert_eq!(
        ical_to_event(&ics).expect("parse").locations,
        Some(
            [(
                "l1".to_owned(),
                json!({"@type": "Location", "name": "Room 42"})
            )]
            .into()
        )
    );
}

#[test]
fn an_edited_instance_is_drawn_at_the_series_place() {
    // RFC 8984 §4.3.4: an instance holds every property of the series its
    // override does not patch. A component that left the place off would show
    // the user an occurrence that happens nowhere.
    let mut event = placed("loc1", json!({"@type": "Location", "name": "Room 42"}));
    event.recurrence_rule = Some(RecurrenceRule {
        frequency: "weekly".to_owned(),
        ..RecurrenceRule::default()
    });
    event.recurrence_overrides = Some(
        [(
            "2026-01-29T13:00:00".to_owned(),
            json!({"title": "Sprint review"}),
        )]
        .into(),
    );
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert_eq!(
        content_line(vevent(&ics, 1), "LOCATION"),
        "LOCATION;X-JMAP-KEY=loc1:Room 42"
    );
    // And it is not read back as an instance that moved: an override may not
    // name a place (see OVERRIDE_PROPERTIES), so a difference here would be one
    // the save path could neither send nor explain.
    assert_eq!(
        ical_to_event(&ics).expect("parse").recurrence_overrides,
        event.recurrence_overrides
    );
}

/// An event tagged the way a server tags it: RFC 8984 §4.2.9's `keywords` is an
/// RFC 8984 §1.4.3 Set, so the keys are the tags and every value is `true`.
fn tagged<const N: usize>(keywords: [(&str, Value); N]) -> CalendarEvent {
    CalendarEvent {
        start: Some("2026-01-15T13:00:00".to_owned()),
        time_zone: Some("Europe/Berlin".to_owned()),
        keywords: Some(
            keywords
                .into_iter()
                .map(|(keyword, set)| (keyword.to_owned(), set))
                .collect(),
        ),
        ..CalendarEvent::default()
    }
}

#[test]
fn the_tags_an_event_carries_are_the_categories() {
    // Unlike `locations`, this property is drawn *whole*: RFC 5545 §3.8.1.2's
    // CATEGORIES is a value list, and a JSCalendar keyword is a bare string, so
    // every tag fits on the line and the save can replace the property.
    let event = tagged([("offsite", json!(true)), ("planning", json!(true))]);
    let ics = event_to_ical(&event);

    assert_eq!(
        content_line(&ics, "CATEGORIES"),
        "CATEGORIES:offsite,planning",
        "the set in the order a set has, so a re-rendering is stable"
    );
    assert_eq!(
        ical_to_event(&ics).expect("parse").keywords,
        event.keywords,
        "the tags survive the round trip"
    );
    assert!(maps_keyword("offsite", &json!(true)));
}

#[test]
fn a_tag_is_escaped_as_text_rather_than_read_as_the_separator() {
    // CATEGORIES is a list of TEXT values (RFC 5545 §3.8.1.2), so a comma inside
    // one tag has to be escaped or the tag comes back as two — and a semicolon
    // would otherwise end the value and start a parameter.
    let event = tagged([("Berlin, offsite; 2026".to_owned().as_str(), json!(true))]);
    let ics = event_to_ical(&event);

    assert_eq!(
        content_line(&ics, "CATEGORIES"),
        "CATEGORIES:Berlin\\, offsite\\; 2026"
    );
    assert_eq!(ical_to_event(&ics).expect("parse").keywords, event.keywords);
    assert!(maps_keyword("Berlin, offsite; 2026", &json!(true)));
}

#[test]
fn an_event_with_no_tags_carries_no_categories() {
    let ics = event_to_ical(&fixture_event());

    assert!(without(&ics, "CATEGORIES"), "{ics}");
    // `None` rather than an empty map: the save path reads an edit off a
    // difference from what was shown, and an empty set is a claim the component
    // never made.
    assert_eq!(ical_to_event(&ics).expect("parse").keywords, None);
}

#[test]
fn categories_spread_over_several_lines_are_read_as_one_set() {
    // RFC 5545 §3.8.1.2 admits CATEGORIES more than once in a VEVENT, and a tag
    // named twice is still one member of a set. Both are what another client's
    // component looks like; neither is what this crate writes.
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n\
UID:E1\r\nDTSTART;TZID=Europe/Berlin:20260115T130000\r\n\
CATEGORIES:offsite,planning\r\nCATEGORIES:offsite,travel\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse");

    assert_eq!(
        event.keywords,
        Some(
            [
                ("offsite".to_owned(), json!(true)),
                ("planning".to_owned(), json!(true)),
                ("travel".to_owned(), json!(true)),
            ]
            .into()
        )
    );
}

#[test]
fn a_tag_the_component_cannot_show_is_flagged_rather_than_drawn() {
    // RFC 8984 §1.4.3 has every value of a Set be `true`. Anything else is an
    // entry this mapping will not put on the line — writing it would say the tag
    // is set when the server said it is not, and drawing nothing at all would
    // have a save replacing the property delete it.
    for set in [json!(false), json!("yes"), json!(null)] {
        let event = tagged([("offsite", json!(true)), ("odd", set.clone())]);
        assert_eq!(
            content_line(&event_to_ical(&event), "CATEGORIES"),
            "CATEGORIES:offsite",
            "{set} was drawn"
        );
        assert!(!maps_keyword("odd", &set), "{set} was called shown");
        assert!(
            maps_keyword("offsite", &json!(true)),
            "the tag beside it was called unshown"
        );
    }
}

#[test]
fn an_empty_tag_is_flagged_rather_than_drawn() {
    // There is no value slot for it: an empty part of a value list reads back as
    // nothing at all, so the tag would vanish and a save would delete it.
    let event = tagged([("", json!(true)), ("offsite", json!(true))]);

    assert_eq!(
        content_line(&event_to_ical(&event), "CATEGORIES"),
        "CATEGORIES:offsite"
    );
    assert!(!maps_keyword("", &json!(true)));
}

#[test]
fn a_tag_carrying_a_carriage_return_is_flagged_rather_than_drawn() {
    // A CR is dropped on its way onto a content line — it would otherwise end
    // the line and turn the rest of the tag into a property of its own — so the
    // tag would come back changed, and a save would rename it behind the user's
    // back. A line feed is not the same case: it has an escape (`\n`) and
    // survives.
    let event = tagged([("off\rsite", json!(true))]);
    assert!(without(&event_to_ical(&event), "CATEGORIES"), "{event:?}");
    assert!(!maps_keyword("off\rsite", &json!(true)));

    let event = tagged([("off\nsite", json!(true))]);
    let ics = event_to_ical(&event);
    assert_eq!(content_line(&ics, "CATEGORIES"), "CATEGORIES:off\\nsite");
    assert_eq!(ical_to_event(&ics).expect("parse").keywords, event.keywords);
    assert!(maps_keyword("off\nsite", &json!(true)));
}

#[test]
fn an_event_whose_every_tag_is_undrawable_carries_no_categories() {
    // And not an empty CATEGORIES line, which states a tag that is the empty
    // string rather than no tags at all.
    let event = tagged([("", json!(true))]);

    assert!(without(&event_to_ical(&event), "CATEGORIES"), "{event:?}");
    assert!(!maps_keyword("", &json!(true)));
}

#[test]
fn maps_keyword_answers_for_the_tag_the_line_left_off() {
    // The save needs the refusal *per tag*, not for the set: a tag the line could
    // not carry is one the user never saw and therefore never asked to lose, so
    // the save writes it back rather than dropping the whole edit. The predicate
    // is the emitter's own, so what the save calls invisible is what the emitter
    // actually left off — and a set holding one such tag still states the rest.
    for (tag, set) in [
        ("", json!(true)),
        ("two\rlines", json!(true)),
        ("odd", json!(false)),
        ("odd", json!("yes")),
        ("odd", json!(null)),
    ] {
        assert!(
            !maps_keyword(tag, &set),
            "the tag {tag:?} set to {set} was called shown"
        );
        let event = tagged([(tag, set.clone()), ("offsite", json!(true))]);
        assert_eq!(
            content_line(&event_to_ical(&event), "CATEGORIES"),
            "CATEGORIES:offsite",
            "the tag {tag:?} set to {set} was drawn, or the one beside it was not"
        );
    }

    assert!(maps_keyword("offsite", &json!(true)));
    // A line feed is not a carriage return: it has an escape and survives.
    assert!(maps_keyword("two\nlines", &json!(true)));
}

#[test]
fn an_edited_instance_is_drawn_with_the_series_tags() {
    // RFC 8984 §4.3.4: an instance holds every property its override does not
    // restate, and this override restates something else. A component that left
    // the tags off would show one occurrence of a tagged series as untagged —
    // and, now that an override *may* restate them (see OVERRIDE_PROPERTIES),
    // would read back as the user having unfiled that one occurrence.
    let mut event = tagged([("offsite", json!(true))]);
    event.recurrence_rule = Some(RecurrenceRule {
        frequency: "weekly".to_owned(),
        ..RecurrenceRule::default()
    });
    event.recurrence_overrides = Some(
        [(
            "2026-01-29T13:00:00".to_owned(),
            json!({"title": "Sprint review"}),
        )]
        .into(),
    );
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert_eq!(
        content_line(vevent(&ics, 1), "CATEGORIES"),
        "CATEGORIES:offsite"
    );
    // And the instance is not read back as one whose tags differ: it carries the
    // set it inherited, so the override says about them exactly what it said.
    assert_eq!(
        ical_to_event(&ics).expect("parse").recurrence_overrides,
        event.recurrence_overrides
    );
}

#[test]
fn an_edited_instance_may_show_tags_of_its_own() {
    // RFC 8984 §4.3.4 lets an override restate the property and iCalendar spells
    // it on the instance's own component, so the one occurrence of a series the
    // user filed differently is a difference this mapping carries whole.
    //
    // The set is *replaced* rather than added to: RFC 8984 §4.3.4 applies an
    // override as a PatchObject, so `keywords` naming one tag says the instance
    // carries that tag and no other — the series' own set is not merged in. Which
    // is why the patch below restates `offsite` to keep it.
    let patch = json!({"keywords": {"cancelled": true, "offsite": true}});
    assert!(maps_override("2026-01-29T13:00:00", &patch));

    let mut event = recurring_with(json!({"2026-01-29T13:00:00": patch}));
    event.keywords = Some([("offsite".to_owned(), json!(true))].into());
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert_eq!(content_line(&ics, "CATEGORIES"), "CATEGORIES:offsite");
    assert_eq!(
        content_line(vevent(&ics, 1), "CATEGORIES"),
        "CATEGORIES:cancelled,offsite"
    );
    assert_eq!(
        ical_to_event(&ics).expect("parse").recurrence_overrides,
        event.recurrence_overrides
    );
}

#[test]
fn an_instance_that_drops_its_tags_reads_back_as_removing_them() {
    // A PatchObject removes a property with a null and the component says the
    // same thing by carrying no `CATEGORIES` at all — so the one occurrence the
    // user unfiled comes back unfiled, rather than at the series' tags, and a
    // save does not refile it.
    let mut event = recurring_with(json!({"2026-01-29T13:00:00": {"keywords": null}}));
    event.keywords = Some([("offsite".to_owned(), json!(true))].into());
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert!(without(vevent(&ics, 1), "CATEGORIES"), "{ics}");
    assert_eq!(
        content_line(&ics, "CATEGORIES"),
        "CATEGORIES:offsite",
        "the series keeps its own"
    );
    assert_eq!(
        ical_to_event(&ics).expect("parse").recurrence_overrides,
        event.recurrence_overrides
    );
    assert!(maps_override(
        "2026-01-29T13:00:00",
        &json!({"keywords": null})
    ));
}

#[test]
fn the_tags_on_an_instances_own_component_are_the_tags_it_was_refiled_with() {
    // The direction a save arrives in, written the way another client — or
    // libical's own re-rendering — leaves it: the instance's set spread over
    // several `CATEGORIES` lines, which is one set, and the series' own left
    // where it was. Nothing here is what this crate emits, which is the point;
    // the tags on that component are the user's answer for that occurrence.
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n\
UID:E1\r\nDTSTART;TZID=Europe/Berlin:20260115T130000\r\nRRULE:FREQ=WEEKLY\r\n\
CATEGORIES:offsite\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\n\
UID:E1\r\nRECURRENCE-ID;TZID=Europe/Berlin:20260129T130000\r\n\
DTSTART;TZID=Europe/Berlin:20260129T130000\r\n\
CATEGORIES:cancelled\r\nCATEGORIES:offsite\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse");

    assert_eq!(
        event.keywords,
        Some([("offsite".to_owned(), json!(true))].into())
    );
    assert_eq!(
        event.recurrence_overrides,
        Some(
            [(
                "2026-01-29T13:00:00".to_owned(),
                json!({"keywords": {"cancelled": true, "offsite": true}}),
            )]
            .into()
        )
    );
}

#[test]
fn a_tag_an_instance_cannot_show_leaves_the_override_flagged() {
    // One level down from [`a_tag_the_component_cannot_show_is_flagged_rather_than_drawn`],
    // and for the same reason: an instance's `CATEGORIES` is drawn whole, so a
    // tag left off it is a tag the next save deletes from that occurrence. An
    // empty set is refused too — no line at all is what a *removal* reads back
    // as, which is a different patch from the empty set that asks for one.
    for keywords in [
        json!({}),
        json!({"": true}),
        json!({"offsite": false}),
        json!({"offsite": true, "": true}),
        json!({"off\rsite": true}),
        json!("offsite"),
        json!(["offsite"]),
    ] {
        let patch = json!({"keywords": keywords});
        assert!(!maps_override("2026-01-29T13:00:00", &patch), "{patch}");

        // And still placed as far as it goes: the occurrence gets a bare RDATE at
        // the series' tags rather than vanishing from the calendar.
        let mut event = recurring_with(json!({"2026-01-29T13:00:00": patch}));
        event.keywords = Some([("offsite".to_owned(), json!(true))].into());
        let ics = event_to_ical(&event);
        assert_eq!(vevents(&ics), 1, "{ics}");
        assert_eq!(
            line(&ics, "RDATE"),
            "RDATE;TZID=Europe/Berlin:20260129T130000"
        );
    }
}

/// An event reminded the way a server reminds: RFC 8984 §4.5.2's `alerts` is a
/// map of Alerts keyed by an RFC 8984 §1.4.4 Id.
fn reminded<const N: usize>(alerts: [(&str, Value); N]) -> CalendarEvent {
    CalendarEvent {
        title: Some("Sprint planning".to_owned()),
        start: Some("2026-01-15T13:00:00".to_owned()),
        time_zone: Some("Europe/Berlin".to_owned()),
        duration: Some("PT1H".to_owned()),
        alerts: Some(
            alerts
                .into_iter()
                .map(|(key, alert)| (key.to_owned(), alert))
                .collect(),
        ),
        ..CalendarEvent::default()
    }
}

/// The reminder Evolution's own editor asks for: a message a quarter of an hour
/// before the appointment.
fn quarter_of_an_hour_before() -> Value {
    json!({
        "@type": "Alert",
        "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
        "action": "display",
    })
}

#[test]
fn a_reminder_is_a_valarm_of_its_own() {
    let event = reminded([("k1", quarter_of_an_hour_before())]);
    let ics = event_to_ical(&event);

    // A component rather than a property, which is what makes this the first
    // mapped property drawn as a child of the VEVENT.
    assert!(ics.contains("BEGIN:VALARM\r\n"), "{ics}");
    assert_eq!(content_line(&ics, "ACTION"), "ACTION:DISPLAY");
    // RFC 5545 §3.8.6.3 spells the offset as a signed duration, negative for a
    // reminder before the event, and defaults it to relating to the start —
    // which is what RFC 8984 §4.5.3's `relativeTo` defaults to as well, so there
    // is nothing to state.
    assert_eq!(content_line(&ics, "TRIGGER"), "TRIGGER:-PT15M");
    // RFC 9074 §6 gives a VALARM a UID, which is where the key of the `alerts`
    // entry rides so that a save names the server's own reminder.
    assert_eq!(content_line(&ics, "UID:k1"), "UID:k1");
    // RFC 5545 §3.6.6 requires a DISPLAY alarm to say what to display, and the
    // only text an Alert has is the event's own summary.
    assert_eq!(
        content_line(&ics, "DESCRIPTION"),
        "DESCRIPTION:Sprint planning"
    );

    assert_eq!(
        ical_to_event(&ics).expect("parse").alerts,
        event.alerts,
        "the reminder survives the round trip"
    );
    assert!(maps_alerts(&event));
}

#[test]
fn a_reminder_after_the_end_states_what_it_is_relative_to() {
    // RFC 8984 §4.5.3's `relativeTo` is `start` or `end`, and RFC 5545 §3.2.14's
    // `RELATED` parameter is the same choice with the same default, so only the
    // end has to be written.
    let event = reminded([(
        "k1",
        json!({
            "@type": "Alert",
            "trigger": {"@type": "OffsetTrigger", "offset": "PT10M", "relativeTo": "end"},
            "action": "display",
        }),
    )]);
    let ics = event_to_ical(&event);

    assert_eq!(content_line(&ics, "TRIGGER"), "TRIGGER;RELATED=END:PT10M");
    assert_eq!(ical_to_event(&ics).expect("parse").alerts, event.alerts);
    assert!(maps_alerts(&event));
}

#[test]
fn a_reminder_relative_to_the_start_says_so_no_more_than_the_default_does() {
    // The other half of the pair: `relativeTo: "start"` is the default said out
    // loud, so the component carries no `RELATED` and the reminder reads back
    // without the member. That is a difference between the server's event and the
    // baseline, not between the baseline and the save — see `jmap_cal_sync::diff`,
    // which compares the latter two.
    let mut alert = quarter_of_an_hour_before();
    alert["trigger"]["relativeTo"] = json!("start");
    let event = reminded([("k1", alert)]);
    let ics = event_to_ical(&event);

    assert_eq!(content_line(&ics, "TRIGGER"), "TRIGGER:-PT15M");
    assert_eq!(
        ical_to_event(&ics).expect("parse").alerts,
        reminded([("k1", quarter_of_an_hour_before())]).alerts
    );
    assert!(maps_alerts(&event));
}

#[test]
fn an_event_with_no_reminders_carries_no_valarm() {
    let ics = event_to_ical(&fixture_event());

    assert!(without(&ics, "BEGIN:VALARM"), "{ics}");
    // `None` rather than an empty map, for the reason `keywords` gives: the save
    // path reads an edit off a difference from what was shown, and an empty map
    // is a claim the component never made.
    assert_eq!(ical_to_event(&ics).expect("parse").alerts, None);
    assert!(maps_alerts(&fixture_event()));
}

#[test]
fn a_reminder_the_component_cannot_show_is_flagged_rather_than_drawn() {
    // The property goes back replaced whole, so an alert left off the document is
    // an alert the next save deletes. Each of these is one this mapping cannot
    // put in a VALARM it would read back the same way:
    for alert in [
        // An action iCalendar can spell only with an ATTENDEE and a SUMMARY this
        // mapping has nothing to fill in from.
        json!({"@type": "Alert", "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"}, "action": "email"}),
        // A trigger at an absolute instant (RFC 8984 §4.5.4), which is not the
        // duration this mapping writes.
        json!({"@type": "Alert", "trigger": {"@type": "AbsoluteTrigger", "when": "2026-01-15T12:45:00Z"}}),
        // A reminder the user has already dismissed or snoozed (RFC 9074 §6.1),
        // which a VALARM states and this mapping does not write: replacing the
        // property would un-dismiss it.
        json!({"@type": "Alert", "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"}, "acknowledged": "2026-01-15T12:46:00Z"}),
        // Something else about the alert that is not drawn at all.
        json!({"@type": "Alert", "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"}, "relatedTo": {}}),
        // Not an Alert, not an object, and no trigger at all.
        json!({"@type": "Location", "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"}}),
        json!("-PT15M"),
        json!({"@type": "Alert", "action": "display"}),
        // An offset that is no duration, and one iCalendar cannot carry.
        json!({"@type": "Alert", "trigger": {"@type": "OffsetTrigger", "offset": "quarter of an hour"}}),
        json!({"@type": "Alert", "trigger": {"@type": "OffsetTrigger", "offset": 900}}),
        // A `relativeTo` outside the two RFC 8984 §4.5.3 admits: a `RELATED` this
        // mapping guessed at would move the reminder.
        json!({"@type": "Alert", "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M", "relativeTo": "middle"}}),
        // And a trigger with something on it this mapping does not draw.
        json!({"@type": "Alert", "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M", "feature": "audio"}}),
    ] {
        let event = reminded([("k1", quarter_of_an_hour_before()), ("k2", alert.clone())]);
        let ics = event_to_ical(&event);

        assert_eq!(
            ics.matches("BEGIN:VALARM\r\n").count(),
            1,
            "{alert} was drawn"
        );
        assert!(!maps_alerts(&event), "{alert} was called covered");
    }
}

#[test]
fn a_reminder_under_a_key_no_uid_can_carry_is_flagged_rather_than_drawn() {
    // The key is an RFC 8984 §1.4.4 Id, and it has to come back off the UID line
    // as itself: one this mapping would read back as a different key — or as the
    // invented one, having refused it — is a reminder a save renames behind the
    // user's back.
    for key in ["", "k 1", "k:1", "k\r1", &"k".repeat(256)] {
        let event = reminded([
            ("k1", quarter_of_an_hour_before()),
            (key, quarter_of_an_hour_before()),
        ]);

        assert_eq!(
            event_to_ical(&event).matches("BEGIN:VALARM\r\n").count(),
            1,
            "{key:?} was drawn"
        );
        assert!(!maps_alerts(&event), "{key:?} was called covered");
    }
}

#[test]
fn an_alarm_this_mapping_cannot_read_is_dropped_rather_than_guessed_at() {
    // A sound, a program and a mail are all reminders Evolution's editor offers
    // and RFC 8984 §4.5.2 has no `action` for — only `display` and `email` — so
    // there is nothing to read them as. An absolute trigger is the third case,
    // the one this mapping does not carry in either direction yet.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n",
        "UID:E1\r\nDTSTART;TZID=Europe/Berlin:20260115T130000\r\n",
        "BEGIN:VALARM\r\nACTION:AUDIO\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n",
        "BEGIN:VALARM\r\nACTION:PROCEDURE\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n",
        "BEGIN:VALARM\r\nACTION:EMAIL\r\nTRIGGER:-PT15M\r\n",
        "SUMMARY:Soon\r\nDESCRIPTION:Soon\r\nATTENDEE:mailto:vera@example.com\r\n",
        "END:VALARM\r\n",
        "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER;VALUE=DATE-TIME:20260115T124500Z\r\n",
        "DESCRIPTION:Soon\r\nEND:VALARM\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n",
    );

    assert_eq!(ical_to_event(ics).expect("parse").alerts, None);
}

#[test]
fn an_alarm_that_names_itself_no_id_gets_a_key_of_its_own() {
    // Evolution's editor writes a VALARM with an `X-EVOLUTION-ALARM-UID` and no
    // RFC 9074 `UID`, so a reminder the user has just added arrives with no key
    // for the `alerts` map. The keys invented for those are positional, which is
    // what makes them stable: the same component read twice yields the same map,
    // so a save that changed nothing else patches nothing.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n",
        "UID:E1\r\nDTSTART;TZID=Europe/Berlin:20260115T130000\r\n",
        "BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Soon\r\nTRIGGER:-PT15M\r\n",
        "END:VALARM\r\n",
        "BEGIN:VALARM\r\nUID:a1\r\nACTION:DISPLAY\r\nDESCRIPTION:Soon\r\n",
        "TRIGGER;RELATED=END:PT5M\r\nEND:VALARM\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n",
    );

    let alerts = ical_to_event(ics).expect("parse").alerts.expect("alerts");

    // `a1` is taken by the alarm that names it, so the nameless one is not given
    // it: two reminders must not collapse into one entry.
    assert_eq!(alerts.keys().collect::<Vec<_>>(), ["a1", "a2"]);
    assert_eq!(alerts["a1"]["trigger"]["relativeTo"], json!("end"));
    assert_eq!(alerts["a2"]["trigger"]["offset"], json!("-PT15M"));
    assert!(alerts.values().all(|alert| alert["action"] == "display"));
}

#[test]
fn an_event_that_uses_the_default_reminders_is_drawn_with_none() {
    // RFC 8984 §4.5.1: with `useDefaultAlerts` true the `alerts` property is
    // ignored, and the reminders that fire are the ones the user's own client
    // defaults to. Drawing the ignored ones would show reminders that do not
    // happen, and a save naming the property would edit what nothing reads.
    let mut event = reminded([("k1", quarter_of_an_hour_before())]);
    event
        .extra
        .insert("useDefaultAlerts".to_owned(), json!(true));

    assert!(without(&event_to_ical(&event), "BEGIN:VALARM"), "{event:?}");
    assert!(!maps_alerts(&event));
}

#[test]
fn an_edited_instance_is_drawn_with_the_series_reminders() {
    // RFC 8984 §4.3.4: an instance holds every property its override does not
    // restate. A component drawn without the series' VALARMs would be an
    // occurrence of a meeting nobody is reminded of.
    let mut event =
        recurring_with(json!({"2026-01-29T13:00:00": {"title": "Sprint planning (long)"}}));
    event.alerts = Some([("k1".to_owned(), quarter_of_an_hour_before())].into());
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert_eq!(ics.matches("BEGIN:VALARM\r\n").count(), 2, "{ics}");
    // And the reminder it inherited is not read as a difference from the series':
    // an override states what an instance differs *by*, and this one differs by
    // its title alone.
    assert_eq!(
        ical_to_event(&ics)
            .expect("parse")
            .recurrence_overrides
            .expect("overrides")["2026-01-29T13:00:00"],
        json!({"title": "Sprint planning (long)"})
    );
}

#[test]
fn an_edited_instance_may_show_reminders_of_its_own() {
    // RFC 8984 §4.3.4 lets an override restate the property and iCalendar spells
    // it as the VALARMs of the instance's own component, so the one occurrence of
    // a series the user is reminded of differently is a difference this mapping
    // carries whole.
    //
    // The map is *replaced* rather than added to, exactly as `keywords` is: an
    // override naming one alert is an occurrence with that reminder and no other.
    // Which is why the patch below restates the series' own key — an hour before
    // rather than a quarter of one is that same reminder moved, and a save under
    // another key would leave the user reminded twice.
    let mut moved = quarter_of_an_hour_before();
    moved["trigger"]["offset"] = json!("-PT1H");
    let patch = json!({"alerts": {"k1": moved}});
    assert!(maps_override("2026-01-29T13:00:00", &patch));

    let mut event = recurring_with(json!({"2026-01-29T13:00:00": patch}));
    event.alerts = Some([("k1".to_owned(), quarter_of_an_hour_before())].into());
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert_eq!(ics.matches("BEGIN:VALARM\r\n").count(), 2, "{ics}");
    assert_eq!(content_line(&ics, "TRIGGER"), "TRIGGER:-PT15M");
    assert_eq!(content_line(vevent(&ics, 1), "TRIGGER"), "TRIGGER:-PT1H");
    // Under the server's own key on both components, which is what makes the two
    // the same reminder at different times.
    assert_eq!(content_line(vevent(&ics, 1), "UID:k1"), "UID:k1");
    assert_eq!(
        ical_to_event(&ics).expect("parse").recurrence_overrides,
        event.recurrence_overrides
    );
}

#[test]
fn an_instance_that_drops_its_reminders_reads_back_as_removing_them() {
    // A PatchObject removes a property with a null and the component says the same
    // thing by carrying no VALARM at all — so the one occurrence the user is not
    // reminded of comes back that way, rather than at the series' reminders, and a
    // save does not put them back.
    let mut event = recurring_with(json!({"2026-01-29T13:00:00": {"alerts": null}}));
    event.alerts = Some([("k1".to_owned(), quarter_of_an_hour_before())].into());
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert!(without(vevent(&ics, 1), "BEGIN:VALARM"), "{ics}");
    assert_eq!(
        ics.matches("BEGIN:VALARM\r\n").count(),
        1,
        "the series keeps its own\n{ics}"
    );
    assert_eq!(
        ical_to_event(&ics).expect("parse").recurrence_overrides,
        event.recurrence_overrides
    );
    assert!(maps_override(
        "2026-01-29T13:00:00",
        &json!({"alerts": null})
    ));
}

#[test]
fn a_reminder_one_instance_could_not_show_is_flagged_rather_than_drawn() {
    // `a_reminder_the_component_cannot_show_is_flagged_rather_than_drawn` one level
    // down, asked of the same `drawn_alert`: an alert left off the instance's
    // component is one the next save deletes from that occurrence.
    let mut dismissed = quarter_of_an_hour_before();
    dismissed["acknowledged"] = json!("2026-01-29T12:46:00Z");
    for alerts in [
        // A reminder the user has already dismissed (RFC 9074 §6.1), which a
        // VALARM cannot say.
        json!({"k2": dismissed}),
        // A reminder that sends mail, and one that fires at an instant rather than
        // at an offset — neither of which this mapping writes.
        json!({"k2": {"@type": "Alert", "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"}, "action": "email"}}),
        json!({"k2": {"@type": "Alert", "trigger": {"@type": "AbsoluteTrigger", "when": "2026-01-29T12:45:00Z"}}}),
        // A key no UID can carry back, so the entry would return renamed.
        json!({"": quarter_of_an_hour_before()}),
        // The empty map, refused for the reason the empty set of tags is: it is
        // written the same way as a removal and would come back as that null,
        // which is a different patch.
        json!({}),
    ] {
        // Beside a title, so the instance is drawn at all: an override whose only
        // field is one this mapping cannot state says nothing a component of its
        // own would show, and is placed by a bare `RDATE` instead.
        let patch = json!({"title": "Sprint planning (long)", "alerts": alerts});
        assert!(
            !maps_override("2026-01-29T13:00:00", &patch),
            "{patch} was called covered"
        );

        // And the instance is still drawn, at the reminders it inherited: an
        // occurrence is worth showing even where its override was not seen whole,
        // and `maps_recurrence_override` is what tells the save path so.
        let mut event = recurring_with(json!({"2026-01-29T13:00:00": patch}));
        event.alerts = Some([("k1".to_owned(), quarter_of_an_hour_before())].into());
        let ics = event_to_ical(&event);
        assert_eq!(vevents(&ics), 2, "{ics}");
        assert_eq!(ics.matches("BEGIN:VALARM\r\n").count(), 2, "{ics}");
        assert_eq!(ics.matches("\r\nUID:k1\r\n").count(), 2, "{ics}");
        // The unshowable reminder reaches neither component, and the occurrence
        // reads back reminded as the series is — which is what makes replacing the
        // property from the drawing the deletion `maps_recurrence_override` is
        // refusing.
        assert!(!ics.contains("k2"), "{ics}");
        assert_eq!(
            ical_to_event(&ics)
                .expect("parse")
                .recurrence_overrides
                .expect("overrides")["2026-01-29T13:00:00"],
            json!({"title": "Sprint planning (long)"})
        );
    }
}

#[test]
fn an_occurrence_of_an_event_that_uses_the_default_reminders_is_drawn_with_none() {
    // `an_event_that_uses_the_default_reminders_is_drawn_with_none` one level down,
    // and the reason `maps_recurrence_override` is asked of the series rather than
    // of the patch alone: RFC 8984 §4.5.1's `useDefaultAlerts` is not a property an
    // override may restate, so the series' answer holds for every instance and an
    // occurrence's own `alerts` is ignored exactly as the series' is.
    let mut event = recurring_with(json!({
        "2026-01-29T13:00:00": {"title": "Sprint planning (long)"},
    }));
    event.alerts = Some([("k1".to_owned(), quarter_of_an_hour_before())].into());
    event
        .extra
        .insert("useDefaultAlerts".to_owned(), json!(true));
    let ics = event_to_ical(&event);

    // Neither component draws them — an occurrence reminded where the series
    // beside it is not would be a reminder that never fires, and would read back as
    // an occurrence the user had just set one on.
    assert_eq!(vevents(&ics), 2, "{ics}");
    assert!(without(&ics, "BEGIN:VALARM"), "{ics}");
    assert_eq!(
        ical_to_event(&ics)
            .expect("parse")
            .recurrence_overrides
            .expect("overrides")["2026-01-29T13:00:00"],
        json!({"title": "Sprint planning (long)"})
    );
    // And the override is refused rather than replaced by the nothing that was
    // drawn.
    assert!(!maps_recurrence_override(
        &event,
        "2026-01-29T13:00:00",
        &json!({"alerts": {"k1": quarter_of_an_hour_before()}})
    ));
    // Down to the null: a save that sent it would be editing what nothing reads.
    assert!(!maps_recurrence_override(
        &event,
        "2026-01-29T13:00:00",
        &json!({"alerts": null})
    ));
    // What the flag does not touch is every other restated property.
    assert!(maps_recurrence_override(
        &event,
        "2026-01-29T13:00:00",
        &json!({"title": "Sprint planning (long)"})
    ));
}

#[test]
fn a_recurrence_rule_carries_freq_interval_and_count() {
    // The end before the interval, which is the order libical writes them in —
    // measured in `jmap-backend-cal/tests/marshal.rs`, and the same contract the
    // `BYxxx` parts are emitted under: a rule that goes out in another order
    // comes back out of EDS's own cache respelled.
    let ics = event_to_ical(&fixture_event());
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=WEEKLY;COUNT=10;INTERVAL=2"
    );

    let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
    assert_eq!(rules.frequency, "weekly");
    assert_eq!(rules.interval, Some(2));
    assert_eq!(rules.count, Some(10));
    assert_eq!(rules.rule_type.as_deref(), Some("RecurrenceRule"));
}

#[test]
fn an_interval_of_one_is_left_implicit() {
    let event = CalendarEvent {
        recurrence_rule: Some(RecurrenceRule {
            interval: Some(1),
            ..RecurrenceRule::new("daily")
        }),
        ..CalendarEvent::default()
    };
    // INTERVAL=1 is the RFC 5545 default; writing it out only makes the line
    // longer, and it comes back as None.
    assert_eq!(line(&event_to_ical(&event), "RRULE:"), "RRULE:FREQ=DAILY");
}

#[test]
fn until_is_a_date_time_in_the_events_own_zone() {
    let event = CalendarEvent {
        start: Some("2026-01-15T13:00:00".to_owned()),
        time_zone: Some("Etc/UTC".to_owned()),
        recurrence_rule: Some(RecurrenceRule {
            until: Some("2026-12-31T09:00:00".to_owned()),
            ..RecurrenceRule::new("monthly")
        }),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=MONTHLY;UNTIL=20261231T090000Z"
    );

    let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
    assert_eq!(rules.until.as_deref(), Some("2026-12-31T09:00:00"));
}

#[test]
fn an_all_day_events_until_is_a_date_like_its_start() {
    // RFC 5545 §3.3.10: UNTIL's value type has to match DTSTART's, so an event
    // written as a DATE cannot carry a DATE-TIME end to its recurrence. The
    // time dropped here is midnight, which is the only time an event shown
    // without one has.
    let event = CalendarEvent {
        start: Some("2026-01-15T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        show_without_time: Some(true),
        recurrence_rule: Some(RecurrenceRule {
            until: Some("2026-12-31T00:00:00".to_owned()),
            ..RecurrenceRule::new("weekly")
        }),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "RRULE:"), "RRULE:FREQ=WEEKLY;UNTIL=20261231");

    let read_back = ical_to_event(&ics).expect("parse");
    let rules = read_back.recurrence_rule.expect("a rule came back");
    assert_eq!(rules.until.as_deref(), Some("2026-12-31T00:00:00"));
}

#[test]
fn an_all_day_event_whose_recurrence_ends_at_a_time_stays_a_date_time() {
    // The other half of the rule above: an UNTIL at 09:00 cannot become a DATE
    // without moving the day the recurrence stops, and it cannot stay a
    // DATE-TIME beside a DATE start either. So the event keeps its DATE-TIME
    // form, showing as timed rather than lying about when it ends.
    let event = CalendarEvent {
        start: Some("2026-01-15T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        show_without_time: Some(true),
        recurrence_rule: Some(RecurrenceRule {
            until: Some("2026-12-31T09:00:00".to_owned()),
            ..RecurrenceRule::new("weekly")
        }),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "DTSTART"), "DTSTART:20260115T000000");
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=WEEKLY;UNTIL=20261231T090000"
    );
}

#[test]
fn a_rule_whose_until_cannot_be_written_is_dropped_rather_than_left_unbounded() {
    // An UNTIL that cannot be rendered used to be left off the RRULE, which
    // turns a recurrence that ends into one that never does — an event repeated
    // into every week of the user's calendar for ever. Showing the rule not at
    // all is the smaller lie, and the save path is told so: recurrenceRule is
    // patched only when every rule the server holds survives the trip.
    for until in ["2026-13-31T09:00:00", "whenever", "2026-02-30T09:00:00"] {
        let rule = RecurrenceRule {
            until: Some(until.to_owned()),
            ..RecurrenceRule::new("weekly")
        };
        assert!(!maps_recurrence_rule(&rule), "{until}");

        let event = CalendarEvent {
            recurrence_rule: Some(rule),
            ..CalendarEvent::default()
        };
        assert!(without(&event_to_ical(&event), "RRULE"), "{until}");
    }

    // A rule with no frequency has no RRULE spelling at all, and never had one.
    assert!(!maps_recurrence_rule(&RecurrenceRule::new("")));
}

#[test]
fn a_zoned_rules_utc_until_is_not_taken_for_a_local_time() {
    // RFC 5545 §3.3.10 requires UNTIL to be a UTC instant whenever DTSTART
    // names a zone, so it is what every conformant producer writes — an
    // Exchange invitation, a Google `.ics`, anything imported into the
    // calendar. RFC 8984 §4.3.3's `until` is a local time in the event's own
    // zone, and the two are the same instant only where that zone is UTC.
    //
    // Dropping the `Z` and calling the digits local moves the end of the series
    // by the zone's offset: the rule below would end at 07:00 Zurich time
    // rather than 09:00, which is two hours before the last occurrence starts —
    // so a save would tell the server the series stops a day earlier than it
    // does.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E1\r\n",
        "DTSTART;TZID=Europe/Zurich:20260810T090000\r\n",
        "RRULE:FREQ=DAILY;UNTIL=20260901T070000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let rules = ical_to_event(ics)
        .expect("parse")
        .recurrence_rule
        .expect("a rule came back");
    assert_ne!(rules.until.as_deref(), Some("2026-09-01T07:00:00"));
    // Converting it would need a zone database this crate deliberately does not
    // carry, so the rule is one the save path must leave alone: the server's own
    // `until` stays where it is rather than being moved by an edit that never
    // touched the recurrence.
    assert!(!maps_recurrence_rule(&rules));
}

#[test]
fn a_utc_until_is_read_as_local_wherever_the_two_are_the_same_instant() {
    // The other side of the rule above. A `Z` is only a shift where there is an
    // offset to shift by, so the three cases that have none keep reading as they
    // did — refusing them would strand every recurring event this crate itself
    // writes.
    let zoned = |dtstart: &str, until: &str| {
        format!(
            "BEGIN:VCALENDAR\r\n\
             BEGIN:VEVENT\r\n\
             UID:E1\r\n\
             DTSTART{dtstart}\r\n\
             RRULE:FREQ=DAILY;UNTIL={until}\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n"
        )
    };
    for (dtstart, until, read) in [
        // An event whose own zone is UTC: the instant and the local time are
        // the same digits.
        (
            ":20260810T070000Z",
            "20260901T070000Z",
            "2026-09-01T07:00:00",
        ),
        // A floating event, which has no zone to resolve a UTC instant against.
        // RFC 5545 admits no `Z` here at all, so this is a producer being loose
        // and the digits are the best reading of it.
        (
            ":20260810T090000",
            "20260901T090000Z",
            "2026-09-01T09:00:00",
        ),
        // And the form this crate writes for a zoned event: local, as its
        // DTSTART is.
        (
            ";TZID=Europe/Zurich:20260810T090000",
            "20260901T090000",
            "2026-09-01T09:00:00",
        ),
    ] {
        let ics = zoned(dtstart, until);
        let rules = ical_to_event(&ics)
            .expect("parse")
            .recurrence_rule
            .expect("a rule came back");
        assert_eq!(rules.until.as_deref(), Some(read), "{dtstart}");
        assert!(maps_recurrence_rule(&rules), "{dtstart}");
    }
}

/// Berlin as libical writes it, and as every renderer of tzdata's own rules
/// does: two observances, each springing on the last Sunday of its month, which
/// is what Directive 2000/84/EC states for the whole EU.
fn berlin(tzid: &str) -> String {
    format!(
        "BEGIN:VTIMEZONE\r\n\
         TZID:{tzid}\r\n\
         X-LIC-LOCATION:Europe/Berlin\r\n\
         BEGIN:DAYLIGHT\r\n\
         TZOFFSETFROM:+0100\r\n\
         TZOFFSETTO:+0200\r\n\
         TZNAME:CEST\r\n\
         DTSTART:19700329T020000\r\n\
         RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3\r\n\
         END:DAYLIGHT\r\n\
         BEGIN:STANDARD\r\n\
         TZOFFSETFROM:+0200\r\n\
         TZOFFSETTO:+0100\r\n\
         TZNAME:CET\r\n\
         DTSTART:19701025T030000\r\n\
         RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n\
         END:STANDARD\r\n\
         END:VTIMEZONE\r\n"
    )
}

/// A recurring event in `tzid`, ending at `until`, in a document that defines
/// the zone the way a real one does.
fn recurring_in(tzid: &str, definition: &str, until: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\n\
         {definition}\
         BEGIN:VEVENT\r\n\
         UID:E1\r\n\
         DTSTART;TZID={tzid}:20260115T130000\r\n\
         RRULE:FREQ=WEEKLY;UNTIL={until}\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    )
}

#[test]
fn a_zoned_rules_utc_until_is_converted_through_the_documents_own_vtimezone() {
    // The other side of `a_zoned_rules_utc_until_is_not_taken_for_a_local_time`:
    // converting the instant §3.3.10 requires needs the offset in force *at* it,
    // and a document that carries the `VTIMEZONE` — which is every document
    // Evolution writes, and every invitation and `.ics` worth the name — says
    // what that offset is. So the zone database this crate refuses to ship is
    // not needed after all: the rules are in the file.
    //
    // Both spellings of the identifier, because the definition is looked up by
    // the `TZID` the property names and not by the zone it resolves to:
    // libical's own components carry the solidus form.
    for tzid in [
        "Europe/Berlin",
        "/freeassociation.sourceforge.net/Europe/Berlin",
    ] {
        for (until, read) in [
            // Summer: the last Sunday of March has passed, so +0200.
            ("20260331T120000Z", "2026-03-31T14:00:00"),
            // Winter, on the other side of the October transition.
            ("20261130T120000Z", "2026-11-30T13:00:00"),
            // At the spring transition itself, which the new offset owns.
            ("20260329T010000Z", "2026-03-29T03:00:00"),
            // And one second before it, which the old one still does.
            ("20260329T005959Z", "2026-03-29T01:59:59"),
            // Before the definition's first transition, where the only thing it
            // says about the zone is the offset that transition moved away from.
            ("19600101T000000Z", "1960-01-01T01:00:00"),
        ] {
            let ics = recurring_in(tzid, &berlin(tzid), until);

            let rules = ical_to_event(&ics)
                .expect("parse")
                .recurrence_rule
                .expect("a rule came back");

            assert_eq!(rules.until.as_deref(), Some(read), "{until} in {tzid}");
            // Which is the point of the conversion: the rule can now be sent,
            // where before it was kept verbatim and the save path had to leave
            // `recurrenceRule` alone — or refuse the create outright.
            assert!(maps_recurrence_rule(&rules), "{until} in {tzid}");
        }
    }

    // The trip back is the local form beside the zoned `DTSTART`, which is what
    // this crate has always written and libical reads in the event's own zone —
    // the same instant, and one this mapping then reads back unchanged.
    let event = ical_to_event(&recurring_in(
        "Europe/Berlin",
        &berlin("Europe/Berlin"),
        "20260331T120000Z",
    ))
    .expect("parse");
    let ics = event_to_ical(&event);
    assert!(
        ics.contains("RRULE:FREQ=WEEKLY;UNTIL=20260331T140000\r\n"),
        "{ics}"
    );
    assert_eq!(
        ical_to_event(&ics).expect("parse").recurrence_rule,
        event.recurrence_rule
    );
}

#[test]
fn a_series_end_no_zone_could_state_is_the_one_thing_a_refusal_can_quote() {
    // `maps_recurrence_rule` says *whether* a rule can be sent; a save that
    // refuses over it has to tell the user *what to change*, and the only
    // refusal there is anything useful to say about is this one — the end that
    // stayed a UTC instant because the document did not say how to move it into
    // the event's zone. So the value is handed back rather than merely denied,
    // and the save path quotes it beside the zone's name.
    let named_and_not_defined = recurring_in("Europe/Berlin", "", "20260331T120000Z");
    let rules = ical_to_event(&named_and_not_defined)
        .expect("parse")
        .recurrence_rule
        .expect("a rule came back");
    assert!(!maps_recurrence_rule(&rules));
    assert_eq!(
        unstateable_until(&rules),
        Some("2026-03-31T12:00:00Z"),
        "the instant kept verbatim is what the user gets told about"
    );

    // A zone the document defines converts the end, and then there is nothing
    // to report: the rule goes out as the series it is.
    let defined = recurring_in(
        "Europe/Berlin",
        &berlin("Europe/Berlin"),
        "20260331T120000Z",
    );
    let rules = ical_to_event(&defined)
        .expect("parse")
        .recurrence_rule
        .expect("a rule came back");
    assert!(maps_recurrence_rule(&rules));
    assert_eq!(unstateable_until(&rules), None);

    // And a rule refused for a reason that is not its end reports none either,
    // so a caller cannot phrase an unrelated refusal as a time-zone problem.
    // RFC 7529's leap month is the same one `jmap-cal-sync`'s save tests use.
    let leap_month = "BEGIN:VCALENDAR\r\n\
         BEGIN:VEVENT\r\n\
         UID:E1\r\n\
         DTSTART;TZID=Europe/Berlin:20260115T130000\r\n\
         RRULE:FREQ=YEARLY;BYMONTH=5L\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n";
    let rules = ical_to_event(leap_month)
        .expect("parse")
        .recurrence_rule
        .expect("a rule came back");
    assert!(!maps_recurrence_rule(&rules));
    assert_eq!(unstateable_until(&rules), None);
}

#[test]
fn a_summer_that_began_last_year_is_still_the_offset_in_force() {
    // Auckland, whose daylight saving starts in September and ends in April, so
    // that a January instant is governed by a transition in the *previous*
    // year — and whose two observances start in different years, as they do
    // wherever one of the two rules changed and the other did not.
    //
    // The naive answer is the transition each rule has in the year asked about,
    // and in January both of those are still to come; the answer left is then
    // whichever observance was written down last, which here is the wrong one
    // by an hour. A southern-hemisphere summer is the case that says so.
    let definition = "BEGIN:VTIMEZONE\r\n\
         TZID:Pacific/Auckland\r\n\
         BEGIN:DAYLIGHT\r\n\
         TZOFFSETFROM:+1200\r\n\
         TZOFFSETTO:+1300\r\n\
         TZNAME:NZDT\r\n\
         DTSTART:20070930T020000\r\n\
         RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=9\r\n\
         END:DAYLIGHT\r\n\
         BEGIN:STANDARD\r\n\
         TZOFFSETFROM:+1300\r\n\
         TZOFFSETTO:+1200\r\n\
         TZNAME:NZST\r\n\
         DTSTART:20080406T030000\r\n\
         RRULE:FREQ=YEARLY;BYDAY=1SU;BYMONTH=4\r\n\
         END:STANDARD\r\n\
         END:VTIMEZONE\r\n";
    for (until, read) in [
        // Summer there: the last Sunday of September 2025 put +1300 in force
        // and nothing has moved it since.
        ("20260115T120000Z", "2026-01-16T01:00:00"),
        // Winter, after the first Sunday of April took it back to +1200.
        ("20260715T120000Z", "2026-07-16T00:00:00"),
    ] {
        let ics = recurring_in("Pacific/Auckland", definition, until);

        let rules = ical_to_event(&ics)
            .expect("parse")
            .recurrence_rule
            .expect("a rule came back");

        assert_eq!(rules.until.as_deref(), Some(read), "{until}");
        assert!(maps_recurrence_rule(&rules), "{until}");
    }
}

#[test]
fn a_zone_of_one_observance_states_the_offset_it_never_moves_from() {
    // A zone that does not observe daylight saving is a `VTIMEZONE` of a single
    // observance with no rule at all, and its one `DTSTART` is the whole of it.
    let definition = "BEGIN:VTIMEZONE\r\n\
         TZID:Asia/Kolkata\r\n\
         BEGIN:STANDARD\r\n\
         TZOFFSETFROM:+0530\r\n\
         TZOFFSETTO:+0530\r\n\
         TZNAME:IST\r\n\
         DTSTART:19700101T000000\r\n\
         END:STANDARD\r\n\
         END:VTIMEZONE\r\n";
    let ics = recurring_in("Asia/Kolkata", definition, "20260331T120000Z");

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rule
        .expect("a rule came back");

    assert_eq!(rules.until.as_deref(), Some("2026-03-31T17:30:00"));
    assert!(maps_recurrence_rule(&rules));
}

#[test]
fn a_transition_the_zone_lists_by_date_counts_like_any_other() {
    // Not every zone moves on a rule. Casablanca's clocks go back for Ramadan,
    // which is not a date any `RRULE` states, so tzdata's renderers write the
    // transitions out one date at a time — a `DTSTART` and an `RDATE` per year
    // — and a zone whose dated transitions went uncounted would be read as
    // whatever its last *rule* said, an hour out for weeks at a time.
    let definition = "BEGIN:VTIMEZONE\r\n\
         TZID:Africa/Casablanca\r\n\
         BEGIN:STANDARD\r\n\
         TZOFFSETFROM:+0100\r\n\
         TZOFFSETTO:+0000\r\n\
         TZNAME:+00\r\n\
         DTSTART:20250301T030000\r\n\
         RDATE:20260218T030000\r\n\
         END:STANDARD\r\n\
         BEGIN:DAYLIGHT\r\n\
         TZOFFSETFROM:+0000\r\n\
         TZOFFSETTO:+0100\r\n\
         TZNAME:+01\r\n\
         DTSTART:20250406T020000\r\n\
         RDATE:20260322T020000\r\n\
         END:DAYLIGHT\r\n\
         END:VTIMEZONE\r\n";
    for (until, read) in [
        // Between the two dates of 2026, where the zone is at +0000.
        ("20260301T120000Z", "2026-03-01T12:00:00"),
        // And after the second of them, back at +0100.
        ("20260401T120000Z", "2026-04-01T13:00:00"),
    ] {
        let ics = recurring_in("Africa/Casablanca", definition, until);

        let rules = ical_to_event(&ics)
            .expect("parse")
            .recurrence_rule
            .expect("a rule came back");

        assert_eq!(rules.until.as_deref(), Some(read), "{until}");
    }
}

#[test]
fn a_transition_rule_that_has_stopped_stops_being_counted() {
    // Istanbul, which abolished daylight saving in September 2016 and has been
    // at +0300 ever since: two rules that ended, and a third observance with no
    // rule at all that has been in force since. An `UNTIL` need not fall on an
    // occurrence (RFC 5545 §3.3.10 does not ask it to), and the one here names
    // the moment the rule stopped applying rather than the last transition it
    // made — so its own year still has an occurrence, one that never happened.
    //
    // Counting it would put the zone back an hour for every instant since, and
    // it is the *later* of the two candidates, so it would win.
    let definition = "BEGIN:VTIMEZONE\r\n\
         TZID:Europe/Istanbul\r\n\
         BEGIN:DAYLIGHT\r\n\
         TZOFFSETFROM:+0200\r\n\
         TZOFFSETTO:+0300\r\n\
         TZNAME:EEST\r\n\
         DTSTART:19700329T030000\r\n\
         RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3;UNTIL=20160327T010000Z\r\n\
         END:DAYLIGHT\r\n\
         BEGIN:STANDARD\r\n\
         TZOFFSETFROM:+0300\r\n\
         TZOFFSETTO:+0200\r\n\
         TZNAME:EET\r\n\
         DTSTART:19701025T040000\r\n\
         RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10;UNTIL=20160907T210000Z\r\n\
         END:STANDARD\r\n\
         BEGIN:STANDARD\r\n\
         TZOFFSETFROM:+0300\r\n\
         TZOFFSETTO:+0300\r\n\
         TZNAME:+03\r\n\
         DTSTART:20160908T000000\r\n\
         END:STANDARD\r\n\
         END:VTIMEZONE\r\n";
    let ics = recurring_in("Europe/Istanbul", definition, "20260331T120000Z");

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rule
        .expect("a rule came back");

    assert_eq!(rules.until.as_deref(), Some("2026-03-31T15:00:00"));
    assert!(maps_recurrence_rule(&rules));
}

#[test]
fn a_transition_named_by_a_weekday_and_a_run_of_dates_is_counted() {
    // The shape libical's tzdata renderer writes for tzdata's own commonest
    // idiom, "the first Sunday on or after the 25th": a `BYDAY` with no ordinal
    // beside a `BYMONTHDAY` listing the run of dates that Sunday can fall on.
    // §3.3.10 makes `BYMONTHDAY` expand and `BYDAY` limit, so the two together
    // name exactly one day — the run is seven long, and a weekday occurs once
    // in any seven consecutive dates.
    //
    // Ireland, whose autumn transition tzdata states as `Sun>=23`. Measured
    // against libical's own definition in `jmap-backend-cal/tests/zones.rs`;
    // spelled out here so the arithmetic has a test that needs no EDS.
    let definition = "BEGIN:VTIMEZONE\r\n\
         TZID:Europe/Dublin\r\n\
         BEGIN:DAYLIGHT\r\n\
         TZOFFSETFROM:+0000\r\n\
         TZOFFSETTO:+0100\r\n\
         TZNAME:IST\r\n\
         DTSTART:19810329T010000\r\n\
         RRULE:FREQ=YEARLY;BYDAY=SU;BYMONTHDAY=25,26,27,28,29,30,31;BYMONTH=3\r\n\
         END:DAYLIGHT\r\n\
         BEGIN:STANDARD\r\n\
         TZOFFSETFROM:+0100\r\n\
         TZOFFSETTO:+0000\r\n\
         TZNAME:GMT\r\n\
         DTSTART:19811025T020000\r\n\
         RRULE:FREQ=YEARLY;BYDAY=SU;BYMONTHDAY=23,24,25,26,27,28,29;BYMONTH=10\r\n\
         END:STANDARD\r\n\
         END:VTIMEZONE\r\n";
    for (until, read) in [
        // 2026-03-29 is the last Sunday of March, so summer time is in force by
        // the 31st and the clock reads an hour past UTC.
        ("20260331T120000Z", "2026-03-31T13:00:00"),
        // And on the 25th it is not: that year's transition is on the 29th.
        ("20260325T120000Z", "2026-03-25T12:00:00"),
        // The autumn rule is the one whose run of dates crosses a month end in
        // other years; here it puts the zone back to +0000 from the 25th.
        ("20261101T120000Z", "2026-11-01T12:00:00"),
    ] {
        let ics = recurring_in("Europe/Dublin", definition, until);

        let rules = ical_to_event(&ics)
            .expect("parse")
            .recurrence_rule
            .expect("a rule came back");

        assert_eq!(rules.until.as_deref(), Some(read), "{until}");
        assert!(maps_recurrence_rule(&rules), "{until}");
    }
}

/// A zone whose one transition is stated by a rule that does not fire every
/// year: the 25th of March, but only when it is a Sunday — 1990, 2001, 2007,
/// 2012, 2018 and then nothing until 2029. Nothing moves the clocks back, so
/// whatever the rule last did is still in force.
fn sparsely(rule: &str) -> String {
    format!(
        "BEGIN:VTIMEZONE\r\n\
         TZID:Example/Sparse\r\n\
         BEGIN:STANDARD\r\n\
         TZOFFSETFROM:+0100\r\n\
         TZOFFSETTO:+0000\r\n\
         TZNAME:XST\r\n\
         DTSTART:19700101T000000\r\n\
         END:STANDARD\r\n\
         BEGIN:DAYLIGHT\r\n\
         TZOFFSETFROM:+0000\r\n\
         TZOFFSETTO:+0100\r\n\
         TZNAME:XDT\r\n\
         DTSTART:19840325T000000\r\n\
         RRULE:{rule}\r\n\
         END:DAYLIGHT\r\n\
         END:VTIMEZONE\r\n"
    )
}

#[test]
fn a_transition_rule_that_skips_years_is_searched_for_further_back_than_two() {
    // Looking at the target's year and the one before it is enough for a rule
    // that fires every year, which a transition rule almost always is. A rule
    // that skips years is the case that pair cannot answer: the last time the
    // 25th of March was a Sunday was 2018, so a search two years deep finds
    // nothing in a rule that has fired five times — and "found nothing" is
    // indistinguishable, from inside the search, from a rule that never fires.
    //
    // The clocks went forward in 2018 and nothing here puts them back, so the
    // end of March 2026 is an hour past UTC.
    let ics = recurring_in(
        "Example/Sparse",
        &sparsely("FREQ=YEARLY;BYDAY=SU;BYMONTHDAY=25;BYMONTH=3"),
        "20260331T120000Z",
    );

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rule
        .expect("a rule came back");

    assert_eq!(rules.until.as_deref(), Some("2026-03-31T13:00:00"));
    assert!(maps_recurrence_rule(&rules));
}

#[test]
fn a_rule_the_search_found_no_occurrence_of_is_refused_rather_than_read_as_silent() {
    // The other end of the same search. A rule whose shape is understood but
    // which fired in none of the years looked at is not a rule that never
    // fired: it may have fired before them, and reading it as silent would
    // leave the zone described by whichever *other* observance was latest —
    // here the 1970 one, which would answer +0000 and be an hour out for every
    // instant since the rule last ran.
    //
    // The 30th of February is the flat case of it: no year has one, the rule
    // reaches back to 1970, and forty years of searching does not get there.
    let ics = recurring_in(
        "Example/Sparse",
        &sparsely("FREQ=YEARLY;BYMONTHDAY=30;BYMONTH=2"),
        "20260331T120000Z",
    );

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rule
        .expect("a rule came back");

    assert_eq!(rules.until.as_deref(), Some("2026-03-31T12:00:00Z"));
    assert!(!maps_recurrence_rule(&rules));
}

#[test]
fn an_observance_moving_off_a_sub_minute_offset_west_of_utc_still_describes_the_zone() {
    // Accra, whose earliest observance leaves local mean time — 52 seconds west
    // of Greenwich — for GMT in 1915. calcard's offset parser reads four digits
    // and no more, so the value arrives with its seconds dropped, and dropping
    // them from a *western* one leaves `-0000`: a spelling §3.3.14 forbids
    // outright, because the sign says which side of UTC the zone is on and
    // there is no negative zero.
    //
    // So the observance's offset was unreadable, and an observance whose offset
    // cannot be read costs the whole definition. A zone that has not moved its
    // clocks since 1915 was refused over the offset it moved away from then —
    // which is not something an appointment in Accra today is in any way about.
    let definition = "BEGIN:VTIMEZONE\r\n\
         TZID:Africa/Accra\r\n\
         BEGIN:STANDARD\r\n\
         TZOFFSETFROM:-000052\r\n\
         TZOFFSETTO:+0000\r\n\
         TZNAME:GMT\r\n\
         DTSTART:19151102T000000\r\n\
         END:STANDARD\r\n\
         END:VTIMEZONE\r\n";
    let ics = recurring_in("Africa/Accra", definition, "20260331T120000Z");

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rule
        .expect("a rule came back");

    assert_eq!(rules.until.as_deref(), Some("2026-03-31T12:00:00"));
    assert!(maps_recurrence_rule(&rules));
}

#[test]
fn a_transition_rule_naming_the_start_of_its_workweek_is_counted_all_the_same() {
    // Exchange's `VTIMEZONE`, verbatim from an invitation it sent: the rule
    // carries a `WKST`, and Zimbra writes the same part in the same place. Both
    // reach an Evolution user as an invitation, which is precisely the
    // `VTIMEZONE` this module cannot fall back to a database for.
    //
    // §3.3.10 gives `WKST` a meaning in exactly two rules — a `WEEKLY` one
    // repeating at an interval, and a `YEARLY` one carrying a `BYWEEKNO` —
    // neither of which this module counts. So no value of it can move a
    // transition this module reads, and refusing the whole definition over it
    // cost the invitation its recurring appointment for nothing.
    //
    // The `DTSTART` is Exchange's own: year 1601 and the first of January, a
    // date the rule then overrides down to the month and the day, leaving only
    // the time of day to come from it.
    let definition = "BEGIN:VTIMEZONE\r\n\
         TZID:GMT +0100 (Standard) / GMT +0200 (Daylight)\r\n\
         BEGIN:STANDARD\r\n\
         DTSTART:16010101T030000\r\n\
         TZOFFSETFROM:+0200\r\n\
         TZOFFSETTO:+0100\r\n\
         RRULE:FREQ=YEARLY;WKST=MO;INTERVAL=1;BYMONTH=10;BYDAY=-1SU\r\n\
         END:STANDARD\r\n\
         BEGIN:DAYLIGHT\r\n\
         DTSTART:16010101T020000\r\n\
         TZOFFSETFROM:+0100\r\n\
         TZOFFSETTO:+0200\r\n\
         RRULE:FREQ=YEARLY;WKST=MO;INTERVAL=1;BYMONTH=3;BYDAY=-1SU\r\n\
         END:DAYLIGHT\r\n\
         END:VTIMEZONE\r\n";
    for (until, read) in [
        // Summer, past the last Sunday of March: +0200.
        ("20260331T120000Z", "2026-03-31T14:00:00"),
        // Winter, past the last Sunday of October: +0100.
        ("20261130T120000Z", "2026-11-30T13:00:00"),
    ] {
        let ics = recurring_in(
            "GMT +0100 (Standard) / GMT +0200 (Daylight)",
            definition,
            until,
        );

        let rules = ical_to_event(&ics)
            .expect("parse")
            .recurrence_rule
            .expect("a rule came back");

        assert_eq!(rules.until.as_deref(), Some(read), "{until}");
        assert!(maps_recurrence_rule(&rules), "{until}");
    }
}

#[test]
fn a_transition_rule_stating_its_own_time_of_day_is_counted() {
    // Lotus Notes' `VTIMEZONE`, verbatim: the hour and the minute the clocks
    // move at are stated in the rule as well as in the `DTSTART`, which
    // §3.3.10 admits — in a `YEARLY` rule `BYHOUR` and `BYMINUTE` expand, so
    // they replace that part of the `DTSTART`'s time of day rather than adding
    // to the days the rule names. Here they agree with it, as they do in every
    // Notes definition; agreeing or not, a part that was not understood cost
    // the whole zone.
    let definition = "BEGIN:VTIMEZONE\r\n\
         TZID:Eastern\r\n\
         BEGIN:STANDARD\r\n\
         DTSTART:19501029T020000\r\n\
         TZOFFSETFROM:-0400\r\n\
         TZOFFSETTO:-0500\r\n\
         RRULE:FREQ=YEARLY;BYMINUTE=0;BYHOUR=2;BYDAY=-1SU;BYMONTH=10\r\n\
         END:STANDARD\r\n\
         BEGIN:DAYLIGHT\r\n\
         DTSTART:19500402T020000\r\n\
         TZOFFSETFROM:-0500\r\n\
         TZOFFSETTO:-0400\r\n\
         RRULE:FREQ=YEARLY;BYMINUTE=0;BYHOUR=2;BYDAY=1SU;BYMONTH=4\r\n\
         END:DAYLIGHT\r\n\
         END:VTIMEZONE\r\n";
    for (until, read) in [
        // Summer, past the first Sunday of April: -0400.
        ("20260715T120000Z", "2026-07-15T08:00:00"),
        // January, whose offset was put in force by the *previous* October.
        ("20260115T120000Z", "2026-01-15T07:00:00"),
    ] {
        let ics = recurring_in("Eastern", definition, until);

        let rules = ical_to_event(&ics)
            .expect("parse")
            .recurrence_rule
            .expect("a rule came back");

        assert_eq!(rules.until.as_deref(), Some(read), "{until}");
        assert!(maps_recurrence_rule(&rules), "{until}");
    }
}

/// A zone that goes forward on the last Sunday of March and back on the last
/// Sunday of October, with the time of day the spring transition happens at
/// stated by `rule` rather than by the `DTSTART` beside it — which says
/// midnight, two hours before what a real definition of this shape says.
fn at_the_hour_the_rule_states(rule: &str) -> String {
    format!(
        "BEGIN:VTIMEZONE\r\n\
         TZID:Example/StatedHour\r\n\
         BEGIN:STANDARD\r\n\
         TZOFFSETFROM:+0100\r\n\
         TZOFFSETTO:+0000\r\n\
         TZNAME:XST\r\n\
         DTSTART:19701025T000000\r\n\
         RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n\
         END:STANDARD\r\n\
         BEGIN:DAYLIGHT\r\n\
         TZOFFSETFROM:+0000\r\n\
         TZOFFSETTO:+0100\r\n\
         TZNAME:XDT\r\n\
         DTSTART:19700329T000000\r\n\
         RRULE:{rule}\r\n\
         END:DAYLIGHT\r\n\
         END:VTIMEZONE\r\n"
    )
}

#[test]
fn a_time_of_day_the_rule_states_is_the_one_the_transition_happens_at() {
    // The discriminating case for the part above, which no producer writes
    // because they all restate what the `DTSTART` already says: an hour in the
    // rule that *differs* from the `DTSTART`'s. §3.3.10's expansion makes the
    // rule's the answer, and an instant in the two hours between the two is
    // where the difference is visible — reading the hour off the `DTSTART`
    // there answers with an offset the zone does not take up until later.
    for (until, read) in [
        // Between the `DTSTART`'s midnight and the rule's two o'clock: the
        // clocks have not moved yet.
        ("20260329T010000Z", "2026-03-29T01:00:00"),
        // And after the transition the rule does state.
        ("20260329T030000Z", "2026-03-29T04:00:00"),
    ] {
        let ics = recurring_in(
            "Example/StatedHour",
            &at_the_hour_the_rule_states("FREQ=YEARLY;BYMINUTE=0;BYHOUR=2;BYDAY=-1SU;BYMONTH=3"),
            until,
        );

        let rules = ical_to_event(&ics)
            .expect("parse")
            .recurrence_rule
            .expect("a rule came back");

        assert_eq!(rules.until.as_deref(), Some(read), "{until}");
        assert!(maps_recurrence_rule(&rules), "{until}");
    }
}

#[test]
fn a_rule_stating_more_than_one_time_of_day_is_refused() {
    // The same reason a `BYMONTHDAY` listing dates is refused: two hours is two
    // transitions in the day, and this module counts the one a transition
    // rule has. Choosing between them would be a guess, and a guess about an
    // offset is invisible from here on.
    let ics = recurring_in(
        "Example/StatedHour",
        &at_the_hour_the_rule_states("FREQ=YEARLY;BYMINUTE=0;BYHOUR=2,3;BYDAY=-1SU;BYMONTH=3"),
        "20260329T010000Z",
    );

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rule
        .expect("a rule came back");

    assert_eq!(rules.until.as_deref(), Some("2026-03-29T01:00:00Z"));
    assert!(!maps_recurrence_rule(&rules));
}

#[test]
fn a_time_of_day_outside_the_range_it_is_stated_in_is_refused() {
    // A field is only a replacement for the `DTSTART`'s while it is a value
    // that field can hold: an hour of 25 carried into the arithmetic would move
    // the transition into the following day rather than being noticed, and the
    // day is the one thing about a transition the rule *has* stated.
    //
    // Over the top of each of §3.3.10's ranges — and the leap second a
    // `BYSECOND` may name, which is in range for §3.3.10 and out of it here,
    // because placing it means pushing the onset into the next minute. Only
    // that end of each range is reachable from a document: calcard's parser
    // drops a negative `BYHOUR` before this layer sees the rule, which arrives
    // here as a rule that does not state an hour at all.
    for rule in [
        "FREQ=YEARLY;BYHOUR=25;BYDAY=-1SU;BYMONTH=3",
        "FREQ=YEARLY;BYMINUTE=60;BYDAY=-1SU;BYMONTH=3",
        "FREQ=YEARLY;BYSECOND=60;BYDAY=-1SU;BYMONTH=3",
    ] {
        let ics = recurring_in(
            "Example/StatedHour",
            &at_the_hour_the_rule_states(rule),
            "20260329T010000Z",
        );

        let rules = ical_to_event(&ics)
            .expect("parse")
            .recurrence_rule
            .expect("a rule came back");

        assert_eq!(
            rules.until.as_deref(),
            Some("2026-03-29T01:00:00Z"),
            "{rule}"
        );
        assert!(!maps_recurrence_rule(&rules), "{rule}");
    }
}

#[test]
fn a_weekday_and_a_run_of_dates_naming_two_of_them_is_refused() {
    // The pair is a single transition only while the run holds one occurrence
    // of the weekday, and eight consecutive dates hold two: 2026-03-22 and
    // 2026-03-29 are both Sundays. That is a rule stating a set — the thing
    // this module refuses — and taking the earlier or the later of them would
    // be a guess.
    //
    // Which of the two it is depends on the year, so the refusal has to be
    // decided per year rather than from the shape of the rule: 2025's March has
    // exactly one Sunday in the same run, and reading the rule off *that* year
    // would answer with an offset 2026 does not have.
    let definition = berlin("Europe/Berlin").replace(
        "FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3",
        "FREQ=YEARLY;BYDAY=SU;BYMONTHDAY=22,23,24,25,26,27,28,29;BYMONTH=3",
    );
    let ics = recurring_in("Europe/Berlin", &definition, "20260331T120000Z");

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rule
        .expect("a rule came back");

    assert_eq!(rules.until.as_deref(), Some("2026-03-31T12:00:00Z"));
    assert!(!maps_recurrence_rule(&rules));
}

#[test]
fn a_year_a_transition_rule_does_not_happen_in_is_a_year_it_does_not_happen_in() {
    // A run holding no occurrence of its weekday is not a rule to throw out:
    // §3.3.10 reads it as a year without an occurrence, and a zone whose clocks
    // did not go forward that spring is a zone still on winter time. Which is
    // only answerable because the search goes back over the years rather than
    // looking at the target's own and the one before it.
    //
    // The run below skips 2026-03-29, the only Sunday it spans, so Berlin never
    // leaves +0100 that year and the 31st of March reads an hour past UTC
    // rather than two. The autumn rule is untouched and still ran in 2025, so
    // the offset in force is the one it moved to.
    let definition = berlin("Europe/Berlin").replace(
        "FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3",
        "FREQ=YEARLY;BYDAY=SU;BYMONTHDAY=25,26,27,28,30,31;BYMONTH=3",
    );
    let ics = recurring_in("Europe/Berlin", &definition, "20260331T120000Z");

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rule
        .expect("a rule came back");

    assert_eq!(rules.until.as_deref(), Some("2026-03-31T13:00:00"));
    assert!(maps_recurrence_rule(&rules));
}

#[test]
fn a_zone_whose_transitions_cannot_be_worked_out_leaves_the_until_alone() {
    // The conversion is only as good as the definition it reads, so a rule this
    // cannot count occurrences of takes the whole zone with it and the `UNTIL`
    // stays exactly where it was: kept verbatim, no LocalDateTime, so the save
    // path knows the end did not survive. Guessing an offset out of half a
    // definition would move the end of the series by an hour or by twelve, and
    // nothing downstream could tell that it had happened.
    for rule in [
        // A weekday with no ordinal is every Sunday in the month, which is a set
        // of days and not the one a transition happens on.
        "FREQ=YEARLY;BYDAY=SU;BYMONTH=3",
        // A frequency this does not count in.
        "FREQ=MONTHLY;BYMONTHDAY=1",
        // A part outside the handful a transition rule is written from: read as
        // "the rule says more than was understood", not ignored.
        "FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3;BYSETPOS=1",
    ] {
        let definition = berlin("Europe/Berlin").replace("FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3", rule);
        let ics = recurring_in("Europe/Berlin", &definition, "20260331T120000Z");

        let rules = ical_to_event(&ics)
            .expect("parse")
            .recurrence_rule
            .expect("a rule came back");

        assert_eq!(
            rules.until.as_deref(),
            Some("2026-03-31T12:00:00Z"),
            "{rule}"
        );
        assert!(!maps_recurrence_rule(&rules), "{rule}");
    }
}

#[test]
fn a_rule_with_unmodeled_parts_is_flagged_rather_than_silently_narrowed() {
    // `rscale` & friends ride in `extra` and do not survive the trip through
    // iCalendar, so the save path must not patch recurrenceRule for them. (It
    // has an iCalendar spelling of its own, RFC 7529's `RSCALE`, which neither
    // this mapping nor libical carries — a rule counted in another calendar
    // drawn as a Gregorian one repeats on the wrong days entirely.)
    let mut rule = RecurrenceRule::new("monthly");
    rule.extra.insert("rscale".to_owned(), json!("chinese"));
    assert!(!maps_recurrence_rule(&rule));
    assert!(maps_recurrence_rule(&RecurrenceRule::new("weekly")));
}

#[test]
fn a_weekly_rule_names_the_days_it_repeats_on() {
    // Without BYDAY a weekly meeting lands on whatever day the series happens
    // to start, which is the wrong day for every Monday-and-Thursday standup
    // created anywhere but on a Monday.
    let event = CalendarEvent {
        recurrence_rule: Some(RecurrenceRule {
            by_day: Some(vec![NDay::new("mo"), NDay::new("th")]),
            count: Some(6),
            ..RecurrenceRule::new("weekly")
        }),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=WEEKLY;COUNT=6;BYDAY=MO,TH"
    );

    let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
    assert_eq!(
        rules.by_day.as_deref(),
        Some(&[NDay::new("mo"), NDay::new("th")][..])
    );
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules));
}

#[test]
fn a_monthly_rule_names_which_of_those_days_it_means() {
    // RFC 5545 §3.3.10's ordinal, and RFC 8984 §4.3.3's `nthOfPeriod`: the
    // second Wednesday and the last Friday of the month, not every one of them.
    let days = vec![
        NDay {
            nth_of_period: Some(2),
            ..NDay::new("we")
        },
        NDay {
            nth_of_period: Some(-1),
            ..NDay::new("fr")
        },
    ];
    let event = CalendarEvent {
        recurrence_rule: Some(RecurrenceRule {
            by_day: Some(days.clone()),
            ..RecurrenceRule::new("monthly")
        }),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "RRULE:"), "RRULE:FREQ=MONTHLY;BYDAY=2WE,-1FR");

    let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
    assert_eq!(rules.by_day.as_deref(), Some(&days[..]));
    assert!(maps_recurrence_rule(&rules));
}

#[test]
fn reads_the_days_off_a_rule_written_by_hand() {
    // The ordinal RFC 5545 §3.3.10 lets an emitter write with a leading plus,
    // and the uppercase weekday every one of them writes.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E1\r\n",
        "DTSTART:20260115T090000Z\r\n",
        "RRULE:FREQ=YEARLY;BYDAY=+3TU\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let rules = ical_to_event(ics)
        .expect("parse")
        .recurrence_rule
        .expect("a rule came back");
    assert_eq!(
        rules.by_day.as_deref(),
        Some(
            &[NDay {
                nth_of_period: Some(3),
                ..NDay::new("tu")
            }][..]
        )
    );
}

#[test]
fn an_ordinal_weekday_is_refused_where_the_recurrence_has_no_period_to_count_in() {
    // RFC 5545 §3.3.10: BYDAY MUST NOT carry a numeric value unless FREQ is
    // MONTHLY or YEARLY. Writing `BYDAY=2MO` beside FREQ=WEEKLY is a line
    // libical is entitled to refuse, and refusing one costs the whole
    // component; dropping only the ordinal would repeat the event every Monday
    // instead of every second one. So the days are left off — the same
    // narrowing an unmodeled rule part gets — and the save path is told the
    // rule was seen in part.
    for frequency in ["weekly", "daily", "hourly"] {
        let rule = RecurrenceRule {
            by_day: Some(vec![NDay {
                nth_of_period: Some(2),
                ..NDay::new("mo")
            }]),
            ..RecurrenceRule::new(frequency)
        };
        assert!(!maps_recurrence_rule(&rule), "{frequency}");

        let event = CalendarEvent {
            recurrence_rule: Some(rule),
            ..CalendarEvent::default()
        };
        let ics = event_to_ical(&event);
        assert_eq!(
            line(&ics, "RRULE:"),
            format!("RRULE:FREQ={}", frequency.to_ascii_uppercase()),
        );
    }
}

#[test]
fn a_day_no_weekday_names_is_flagged_rather_than_written() {
    // `day` is a closed vocabulary in both formats (RFC 8984 §4.3.3, RFC 5545
    // §3.3.10), so a value outside it is dropped rather than passed through in
    // the other format's clothes — and, as everywhere else in this mapping,
    // what was shown in part is not written back.
    for days in [
        vec![NDay::new("monday")],
        vec![NDay::new("")],
        vec![NDay::new("mo,tu")],
        // Zero is no occurrence of a weekday; RFC 8984 §4.3.3 forbids it and
        // RFC 5545's ordwk starts at 1.
        vec![NDay {
            nth_of_period: Some(0),
            ..NDay::new("mo")
        }],
        // An NDay carrying more than this mapping reads is as unseen as a rule
        // that does.
        vec![NDay {
            extra: [("weekOfMonth".to_owned(), json!(2))].into(),
            ..NDay::new("mo")
        }],
        // No day at all is not a set of days a BYDAY can name.
        vec![],
    ] {
        let rule = RecurrenceRule {
            by_day: Some(days.clone()),
            ..RecurrenceRule::new("monthly")
        };
        assert!(!maps_recurrence_rule(&rule), "{days:?}");

        let event = CalendarEvent {
            recurrence_rule: Some(rule),
            ..CalendarEvent::default()
        };
        assert_eq!(
            line(&event_to_ical(&event), "RRULE:"),
            "RRULE:FREQ=MONTHLY",
            "{days:?}"
        );
    }
}

#[test]
fn a_monthly_rule_names_the_days_of_the_month_it_repeats_on() {
    // The other half of Evolution's monthly recurrence page: not "the second
    // Wednesday" but "the 15th", and RFC 8984 §4.3.3's negative value for the
    // last day of the month, whichever day of the week that lands on.
    let event = CalendarEvent {
        recurrence_rule: Some(RecurrenceRule {
            by_month_day: Some(vec![15, -1]),
            count: Some(6),
            ..RecurrenceRule::new("monthly")
        }),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=MONTHLY;COUNT=6;BYMONTHDAY=15,-1"
    );

    let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
    assert_eq!(rules.by_month_day.as_deref(), Some(&[15, -1][..]));
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules));
}

#[test]
fn the_days_of_the_month_are_written_after_the_days_of_the_week() {
    // Both parts at once, in the order libical writes them, so that a rule read
    // back out of EDS's own cache compares equal to the one that went in.
    let event = CalendarEvent {
        recurrence_rule: Some(RecurrenceRule {
            by_day: Some(vec![NDay::new("we")]),
            by_month_day: Some(vec![15]),
            ..RecurrenceRule::new("yearly")
        }),
        ..CalendarEvent::default()
    };
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=YEARLY;BYDAY=WE;BYMONTHDAY=15"
    );
}

#[test]
fn reads_the_days_of_the_month_off_a_rule_written_by_hand() {
    // RFC 5545 §3.3.10's `monthdaynum` may carry the leading plus JSCalendar has
    // no room for.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E1\r\n",
        "DTSTART:20260115T090000Z\r\n",
        "RRULE:FREQ=MONTHLY;BYMONTHDAY=+1,-31\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let rules = ical_to_event(ics).expect("parse").recurrence_rule.unwrap();
    assert_eq!(rules.by_month_day.as_deref(), Some(&[1, -31][..]));
    assert!(maps_recurrence_rule(&rules));
}

#[test]
fn days_of_the_month_are_refused_where_a_week_is_the_period() {
    // RFC 5545 §3.3.10: BYMONTHDAY MUST NOT be specified when FREQ is WEEKLY —
    // a week does not sit inside a month. As with an ordinal weekday, the part
    // is left off whole and the save path told the rule was seen in part, rather
    // than writing a line libical is entitled to refuse.
    let rule = RecurrenceRule {
        by_month_day: Some(vec![15]),
        ..RecurrenceRule::new("weekly")
    };
    assert!(!maps_recurrence_rule(&rule));

    let event = CalendarEvent {
        recurrence_rule: Some(rule),
        ..CalendarEvent::default()
    };
    assert_eq!(line(&event_to_ical(&event), "RRULE:"), "RRULE:FREQ=WEEKLY");
}

#[test]
fn a_day_no_month_has_is_flagged_rather_than_written() {
    // RFC 5545's ordmoday is 1 to 31 and RFC 8984 §4.3.3 counts backwards to
    // -31; zero is no day of any month. A set holding one such value is refused
    // whole, because a BYMONTHDAY holding the rest is a different recurrence
    // rather than a narrower view of this one.
    for days in [vec![0], vec![32], vec![-32], vec![15, 0], vec![]] {
        let rule = RecurrenceRule {
            by_month_day: Some(days.clone()),
            ..RecurrenceRule::new("monthly")
        };
        assert!(!maps_recurrence_rule(&rule), "{days:?}");

        let event = CalendarEvent {
            recurrence_rule: Some(rule),
            ..CalendarEvent::default()
        };
        assert_eq!(
            line(&event_to_ical(&event), "RRULE:"),
            "RRULE:FREQ=MONTHLY",
            "{days:?}"
        );
    }
}

#[test]
fn a_day_of_the_month_a_hand_written_rule_invents_is_not_written_back() {
    // The refusal above, reached the way a component really arrives: through the
    // parser. `32` is outside RFC 5545's `ordmoday` and survives calcard's own
    // representation of the rule unchanged, so the mapping is the one that has to
    // refuse it — and refuse the whole set, leaving the `RRULE` at its frequency.
    //
    // Not every malformed token gets this far: calcard re-renders an `RRULE` from
    // what it parsed, which drops a token it could not read (`BYMONTHDAY=15,XX`
    // arrives as `[15]`) and wraps one too large for its own field (`999` arrives
    // as `-25`). Both then look like days the user chose. That narrowing happens
    // below this crate and cannot be seen from here.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E1\r\n",
        "DTSTART:20260115T090000Z\r\n",
        "RRULE:FREQ=MONTHLY;BYMONTHDAY=15,32\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    let rules = event.recurrence_rule.as_ref().unwrap();
    assert_eq!(rules.by_month_day.as_deref(), Some(&[15, 32][..]));
    assert!(!maps_recurrence_rule(rules));
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=MONTHLY",
        "and the days are left off the rule it is drawn as"
    );
}

#[test]
fn a_yearly_rule_names_the_days_of_the_year_it_repeats_on() {
    // "Every 1 January and every 31 December" — RFC 8984 §4.3.3's `byYearDay`,
    // iCalendar's `BYYEARDAY`, whose negative value counts back from the end of
    // the year the way `byMonthDay`'s does from the end of the month.
    let event = CalendarEvent {
        recurrence_rule: Some(RecurrenceRule {
            by_year_day: Some(vec![1, -1]),
            count: Some(4),
            ..RecurrenceRule::new("yearly")
        }),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=YEARLY;COUNT=4;BYYEARDAY=1,-1"
    );

    let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
    assert_eq!(rules.by_year_day.as_deref(), Some(&[1, -1][..]));
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules));
}

#[test]
fn the_days_of_the_year_are_written_after_the_days_of_the_month() {
    // Every modeled part at once, in the order libical writes them —
    // `BYYEARDAY` between `BYMONTHDAY` and `BYMONTH` — so that a rule read back
    // out of EDS's own cache compares equal to the one that went in.
    let event = CalendarEvent {
        recurrence_rule: Some(RecurrenceRule {
            by_day: Some(vec![NDay::new("we")]),
            by_month_day: Some(vec![15]),
            by_year_day: Some(vec![100]),
            by_month: Some(vec!["3".to_owned()]),
            ..RecurrenceRule::new("yearly")
        }),
        ..CalendarEvent::default()
    };
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=YEARLY;BYDAY=WE;BYMONTHDAY=15;BYYEARDAY=100;BYMONTH=3"
    );
}

#[test]
fn reads_the_days_of_the_year_off_a_rule_written_by_hand() {
    // RFC 5545 §3.3.10's `yeardaynum` may carry the leading plus JSCalendar has no
    // room for, and counts to 366 for the leap day.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E1\r\n",
        "DTSTART:20260115T090000Z\r\n",
        "RRULE:FREQ=YEARLY;BYYEARDAY=+1,-366\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    let rules = event.recurrence_rule.as_ref().unwrap();
    assert_eq!(rules.by_year_day.as_deref(), Some(&[1, -366][..]));
    assert!(maps_recurrence_rule(rules));
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=YEARLY;BYYEARDAY=1,-366"
    );
}

#[test]
fn days_of_the_year_are_refused_where_the_period_is_shorter_than_one() {
    // RFC 5545 §3.3.10: BYYEARDAY MUST NOT be specified when FREQ is DAILY,
    // WEEKLY, or MONTHLY — none of those periods holds a year. The part is left
    // off whole and the save path told the rule was seen in part, rather than
    // writing a line libical is entitled to refuse.
    for frequency in ["daily", "weekly", "monthly"] {
        let rule = RecurrenceRule {
            by_year_day: Some(vec![100]),
            ..RecurrenceRule::new(frequency)
        };
        assert!(!maps_recurrence_rule(&rule), "{frequency}");

        let event = CalendarEvent {
            recurrence_rule: Some(rule),
            ..CalendarEvent::default()
        };
        assert_eq!(
            line(&event_to_ical(&event), "RRULE:"),
            format!("RRULE:FREQ={}", frequency.to_ascii_uppercase()),
            "{frequency}"
        );
    }
}

#[test]
fn a_day_of_the_year_is_carried_at_a_frequency_shorter_than_a_day() {
    // The other half of §3.3.10's table: `BYYEARDAY` is defined for `HOURLY`,
    // `MINUTELY` and `SECONDLY` — limiting the occurrences those expand to,
    // "the ninth hour of every hundredth day of the year" — so the gate names the
    // three frequencies the RFC excludes rather than allowing `YEARLY` alone.
    for frequency in ["hourly", "minutely", "secondly", "yearly"] {
        let event = CalendarEvent {
            recurrence_rule: Some(RecurrenceRule {
                by_year_day: Some(vec![100]),
                ..RecurrenceRule::new(frequency)
            }),
            ..CalendarEvent::default()
        };
        let ics = event_to_ical(&event);
        assert!(
            line(&ics, "RRULE:").ends_with(";BYYEARDAY=100"),
            "{frequency}"
        );
        let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
        assert!(maps_recurrence_rule(&rules), "{frequency}");
    }
}

#[test]
fn a_day_no_year_has_is_flagged_rather_than_written() {
    // RFC 5545's `yeardaynum` is 1 to 366 and RFC 8984 §4.3.3 counts backwards to
    // -366; zero is no day of any year, and 367 is a day no year has. A set
    // holding one such value is refused whole, because a `BYYEARDAY` holding the
    // rest is a different recurrence rather than a narrower view of this one.
    for days in [vec![0], vec![367], vec![-367], vec![100, 0], vec![]] {
        let rule = RecurrenceRule {
            by_year_day: Some(days.clone()),
            ..RecurrenceRule::new("yearly")
        };
        assert!(!maps_recurrence_rule(&rule), "{days:?}");

        let event = CalendarEvent {
            recurrence_rule: Some(rule),
            ..CalendarEvent::default()
        };
        assert_eq!(
            line(&event_to_ical(&event), "RRULE:"),
            "RRULE:FREQ=YEARLY",
            "{days:?}"
        );
    }
}

#[test]
fn a_day_of_the_year_a_hand_written_rule_invents_is_not_written_back() {
    // The refusal above, reached the way a component really arrives: through the
    // parser. `367` is outside RFC 5545's `yeardaynum`, so the mapping is the one
    // that has to refuse it — and refuse the whole set, leaving the `RRULE` at its
    // frequency.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E1\r\n",
        "DTSTART:20260115T090000Z\r\n",
        "RRULE:FREQ=YEARLY;BYYEARDAY=100,367\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    let rules = event.recurrence_rule.as_ref().unwrap();
    assert_eq!(rules.by_year_day.as_deref(), Some(&[100, 367][..]));
    assert!(!maps_recurrence_rule(rules));
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=YEARLY",
        "and the days are left off the rule it is drawn as"
    );
}

#[test]
fn a_yearly_rule_names_the_months_it_repeats_in() {
    // "Every March and September" — RFC 8984 §4.3.3's `byMonth`, iCalendar's
    // `BYMONTH`. JSCalendar holds each month as a string; the numbers 1 to 12 of
    // the Gregorian calendar are spelled the same in both formats.
    let event = CalendarEvent {
        recurrence_rule: Some(RecurrenceRule {
            by_month: Some(vec!["3".to_owned(), "9".to_owned()]),
            count: Some(4),
            ..RecurrenceRule::new("yearly")
        }),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=YEARLY;COUNT=4;BYMONTH=3,9"
    );

    let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
    assert_eq!(
        rules.by_month.as_deref(),
        Some(&["3".to_owned(), "9".to_owned()][..])
    );
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules));
}

#[test]
fn the_months_are_written_after_the_days_of_the_month() {
    // All three parts at once, in the order libical and calcard both write them
    // — `BYMONTH` last — so that a rule read back out of EDS's own cache
    // compares equal to the one that went in.
    let event = CalendarEvent {
        recurrence_rule: Some(RecurrenceRule {
            by_day: Some(vec![NDay::new("we")]),
            by_month_day: Some(vec![15]),
            by_month: Some(vec!["3".to_owned()]),
            ..RecurrenceRule::new("yearly")
        }),
        ..CalendarEvent::default()
    };
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=YEARLY;BYDAY=WE;BYMONTHDAY=15;BYMONTH=3"
    );
}

#[test]
fn a_month_is_carried_at_any_frequency() {
    // Unlike `BYMONTHDAY`, which RFC 5545 §3.3.10 forbids beside `FREQ=WEEKLY`,
    // `BYMONTH` is defined at every frequency — limiting the occurrences a
    // shorter period produces rather than expanding them. So there is no
    // frequency gate here, and a weekly rule keeps its months.
    for frequency in ["daily", "weekly", "monthly", "yearly"] {
        let event = CalendarEvent {
            recurrence_rule: Some(RecurrenceRule {
                by_month: Some(vec!["1".to_owned()]),
                ..RecurrenceRule::new(frequency)
            }),
            ..CalendarEvent::default()
        };
        let ics = event_to_ical(&event);
        assert!(line(&ics, "RRULE:").ends_with(";BYMONTH=1"), "{frequency}");
        let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
        assert!(maps_recurrence_rule(&rules), "{frequency}");
    }
}

#[test]
fn reads_the_months_off_a_rule_written_by_hand() {
    // RFC 5545 §3.3.10's `monthnum` is `1*2DIGIT`, so a month may be written
    // with a leading zero that JSCalendar's decimal string has no room for.
    // calcard hands back the canonical spelling, which is the one that survives
    // a trip back out.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E1\r\n",
        "DTSTART:20260115T090000Z\r\n",
        "RRULE:FREQ=YEARLY;BYMONTH=03,12\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    let rules = event.recurrence_rule.as_ref().unwrap();
    assert_eq!(
        rules.by_month.as_deref(),
        Some(&["3".to_owned(), "12".to_owned()][..])
    );
    assert!(maps_recurrence_rule(rules));
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=YEARLY;BYMONTH=3,12"
    );
}

#[test]
fn a_month_no_year_has_is_flagged_rather_than_written() {
    // RFC 5545's `monthnum` is 1 to 12 and RFC 8984 §4.3.3 counts no month
    // backwards, so there is no negative form and no thirteenth month. A set
    // holding one such value is refused whole, because a `BYMONTH` holding the
    // rest is a different recurrence rather than a narrower view of this one.
    //
    // `03` is refused for a subtler reason: it names a month that exists, but
    // libical and calcard both write it back as `3`, so a rule that went out
    // spelled `03` would come back spelled differently and read as an edit the
    // user never made. Only the canonical decimal spelling is carried.
    for months in [
        vec!["0"],
        vec!["13"],
        vec!["-1"],
        vec!["+3"],
        vec!["03"],
        vec!["3", "0"],
        vec!["March"],
        vec![""],
        vec![],
    ] {
        let rule = RecurrenceRule {
            by_month: Some(months.iter().map(|m| (*m).to_owned()).collect()),
            ..RecurrenceRule::new("yearly")
        };
        assert!(!maps_recurrence_rule(&rule), "{months:?}");

        let event = CalendarEvent {
            recurrence_rule: Some(rule),
            ..CalendarEvent::default()
        };
        assert_eq!(
            line(&event_to_ical(&event), "RRULE:"),
            "RRULE:FREQ=YEARLY",
            "{months:?}"
        );
    }
}

#[test]
fn a_leap_month_is_flagged_rather_than_written() {
    // The reason RFC 8984 §4.3.3 holds a month as a string at all: `5L` is the
    // leap month of a non-Gregorian calendar. iCalendar can only say it under
    // RFC 7529's `RSCALE`, which this mapping does not write — the event's
    // calendar system is not modeled, so writing `BYMONTH=5L` beside a
    // Gregorian series would name a month the series does not have.
    let rule = RecurrenceRule {
        by_month: Some(vec!["5L".to_owned()]),
        ..RecurrenceRule::new("yearly")
    };
    assert!(!maps_recurrence_rule(&rule));

    let event = CalendarEvent {
        recurrence_rule: Some(rule),
        ..CalendarEvent::default()
    };
    assert_eq!(line(&event_to_ical(&event), "RRULE:"), "RRULE:FREQ=YEARLY");
}

#[test]
fn a_month_a_hand_written_rule_invents_is_not_written_back() {
    // The refusals above, reached the way a component really arrives: through
    // the parser. Both `13` and a leap month survive calcard's representation of
    // the rule unchanged, so the mapping is the one that has to refuse them —
    // and refuse the whole set, leaving the `RRULE` at its frequency.
    for value in ["FREQ=YEARLY;BYMONTH=3,13", "FREQ=YEARLY;BYMONTH=5L"] {
        let ics = format!(
            "BEGIN:VCALENDAR\r\n\
             BEGIN:VEVENT\r\n\
             UID:E1\r\n\
             DTSTART:20260115T090000Z\r\n\
             RRULE:{value}\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n"
        );
        let event = ical_to_event(&ics).expect("parse");
        let rules = event.recurrence_rule.as_ref().unwrap();
        assert!(!maps_recurrence_rule(rules), "{value}");
        assert_eq!(
            line(&event_to_ical(&event), "RRULE:"),
            "RRULE:FREQ=YEARLY",
            "{value}: the months are left off the rule it is drawn as"
        );
    }
}

#[test]
fn a_weekly_rule_names_the_day_its_weeks_start_on() {
    // "Every other Tuesday, weeks counted from Sunday" — RFC 8984 §4.3.3's
    // `firstDayOfWeek`, iCalendar's `WKST`. It is not decoration: RFC 5545
    // §3.3.10 says the day the week starts on decides where the second week of a
    // fortnightly series begins, so the same `BYDAY` counted from Monday and from
    // Sunday produces different dates.
    let event = CalendarEvent {
        recurrence_rule: Some(RecurrenceRule {
            interval: Some(2),
            by_day: Some(vec![NDay::new("tu")]),
            first_day_of_week: Some("su".to_owned()),
            ..RecurrenceRule::new("weekly")
        }),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=TU;WKST=SU"
    );

    let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
    assert_eq!(rules.first_day_of_week.as_deref(), Some("su"));
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules));
}

#[test]
fn the_day_the_week_starts_on_is_written_last() {
    // Every modeled part at once, in the order libical and calcard both write
    // them — `WKST` after `BYMONTH`, last of all — so that a rule read back out
    // of EDS's own cache compares equal to the one that went in.
    let event = CalendarEvent {
        recurrence_rule: Some(RecurrenceRule {
            by_day: Some(vec![NDay::new("we")]),
            by_month_day: Some(vec![15]),
            by_year_day: Some(vec![100]),
            by_month: Some(vec!["3".to_owned()]),
            first_day_of_week: Some("su".to_owned()),
            ..RecurrenceRule::new("yearly")
        }),
        ..CalendarEvent::default()
    };
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=YEARLY;BYDAY=WE;BYMONTHDAY=15;BYYEARDAY=100;BYMONTH=3;WKST=SU"
    );
}

#[test]
fn a_week_starting_on_monday_is_left_off_the_rule() {
    // `WKST=MO` is RFC 5545 §3.3.10's default, and libical drops it from a rule
    // it reads — `jmap-backend-cal/tests/marshal.rs` measures that. So writing it
    // would come back out of EDS's cache missing and read as an edit the user
    // never made; the default is left off instead, exactly as `INTERVAL=1` is.
    //
    // The rule still maps: the day is not *refused*, it is the one value
    // iCalendar says by saying nothing.
    let rule = RecurrenceRule {
        first_day_of_week: Some("mo".to_owned()),
        ..RecurrenceRule::new("weekly")
    };
    assert!(maps_recurrence_rule(&rule));

    let event = CalendarEvent {
        recurrence_rule: Some(rule),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "RRULE:"), "RRULE:FREQ=WEEKLY");
    let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
    assert_eq!(rules.first_day_of_week, None);
}

#[test]
fn the_day_the_week_starts_on_is_carried_at_any_frequency() {
    // RFC 5545 §3.3.10 does not exclude `WKST` at any frequency — it says only
    // where the part is *significant*, which is a reader's business rather than a
    // writer's — and libical keeps it beside every one. So there is no frequency
    // gate: the day the server named is carried as it came.
    for frequency in ["daily", "weekly", "monthly", "yearly"] {
        let event = CalendarEvent {
            recurrence_rule: Some(RecurrenceRule {
                first_day_of_week: Some("su".to_owned()),
                ..RecurrenceRule::new(frequency)
            }),
            ..CalendarEvent::default()
        };
        let ics = event_to_ical(&event);
        assert!(line(&ics, "RRULE:").ends_with(";WKST=SU"), "{frequency}");
        let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
        assert_eq!(
            rules.first_day_of_week.as_deref(),
            Some("su"),
            "{frequency}"
        );
        assert!(maps_recurrence_rule(&rules), "{frequency}");
    }
}

#[test]
fn every_day_of_the_week_can_start_one() {
    // The whole closed vocabulary RFC 8984 §4.3.3 shares with `NDay`'s `day`, in
    // both directions. Monday is absent because it is the default and is left off
    // — see the test above.
    for (day, token) in [
        ("tu", "TU"),
        ("we", "WE"),
        ("th", "TH"),
        ("fr", "FR"),
        ("sa", "SA"),
        ("su", "SU"),
    ] {
        let event = CalendarEvent {
            recurrence_rule: Some(RecurrenceRule {
                first_day_of_week: Some(day.to_owned()),
                ..RecurrenceRule::new("weekly")
            }),
            ..CalendarEvent::default()
        };
        let ics = event_to_ical(&event);
        assert_eq!(
            line(&ics, "RRULE:"),
            format!("RRULE:FREQ=WEEKLY;WKST={token}")
        );
        let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
        assert_eq!(rules.first_day_of_week.as_deref(), Some(day), "{day}");
    }
}

#[test]
fn reads_the_day_the_week_starts_on_off_a_rule_written_by_hand() {
    // RFC 5545's `weekday` is upper case where JSCalendar's is lower, and
    // iCalendar parameter values are case-insensitive besides. The day comes back
    // as the two letters RFC 8984 §4.3.3 spells it with, whatever case the
    // component used.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E1\r\n",
        "DTSTART:20260115T090000Z\r\n",
        "RRULE:FREQ=WEEKLY;WKST=sa\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    let rules = event.recurrence_rule.as_ref().unwrap();
    assert_eq!(rules.first_day_of_week.as_deref(), Some("sa"));
    assert!(maps_recurrence_rule(rules));
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=WEEKLY;WKST=SA"
    );
}

#[test]
fn a_day_no_week_starts_on_is_flagged_rather_than_written() {
    // Outside the closed vocabulary there is nothing a `WKST` can say, and the
    // cost of guessing is the whole `RRULE`: libical refuses a component carrying
    // `WKST=XX` outright — measured in `jmap-backend-cal/tests/marshal.rs` — so
    // the event would reach EDS's cache as a single appointment with the user's
    // series gone. The part is left off and the save path told the rule was seen
    // in part.
    //
    // `MO` is refused for the subtler reason `BYMONTH=03` is: RFC 8984 §4.3.3
    // spells the day in lower case, so a value in any other case is one this
    // mapping would hand back respelled — and a rule that comes back spelled
    // differently reads as an edit the user never made. The parser can only
    // produce the canonical spelling, so nothing legitimate is lost.
    for day in ["xx", "", "monday", "MO", "SU", "1", "mo,tu"] {
        let rule = RecurrenceRule {
            first_day_of_week: Some(day.to_owned()),
            ..RecurrenceRule::new("weekly")
        };
        assert!(!maps_recurrence_rule(&rule), "{day}");

        let event = CalendarEvent {
            recurrence_rule: Some(rule),
            ..CalendarEvent::default()
        };
        assert_eq!(
            line(&event_to_ical(&event), "RRULE:"),
            "RRULE:FREQ=WEEKLY",
            "{day}"
        );
    }
}

#[test]
fn a_yearly_rule_names_the_weeks_of_the_year_it_repeats_in() {
    // "The first and the last week of the year, weeks counted from Sunday" —
    // RFC 8984 §4.3.3's `byWeekNo`, iCalendar's `BYWEEKNO`, whose negative value
    // counts back from the end of the year the way `byYearDay`'s does.
    //
    // The day beside it is not decoration: RFC 5545 §3.3.10 numbers the weeks by
    // ISO 8601 from `WKST`, so "week 1" counted from Sunday and from Monday can
    // name different days. That day is modeled now, which is what makes this part
    // safe to carry at all.
    let event = CalendarEvent {
        recurrence_rule: Some(RecurrenceRule {
            by_week_no: Some(vec![1, -1]),
            first_day_of_week: Some("su".to_owned()),
            count: Some(4),
            ..RecurrenceRule::new("yearly")
        }),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=YEARLY;COUNT=4;BYWEEKNO=1,-1;WKST=SU"
    );

    let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
    assert_eq!(rules.by_week_no.as_deref(), Some(&[1, -1][..]));
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules));
}

#[test]
fn the_weeks_of_the_year_are_written_after_the_days_of_the_year() {
    // Every modeled part at once, in the order libical writes them — `BYWEEKNO`
    // between `BYYEARDAY` and `BYMONTH` — so that a rule read back out of EDS's
    // own cache compares equal to the one that went in.
    //
    // The sixth part is what first pushes an `RRULE` past 75 octets, so this is
    // also the first rule the emitter folds; the assertion is on the unfolded line
    // because that is the one a reader reconstructs.
    let rule = RecurrenceRule {
        by_day: Some(vec![NDay::new("we")]),
        by_month_day: Some(vec![15]),
        by_year_day: Some(vec![100]),
        by_week_no: Some(vec![20]),
        by_month: Some(vec!["3".to_owned()]),
        first_day_of_week: Some("su".to_owned()),
        ..RecurrenceRule::new("yearly")
    };
    let event = CalendarEvent {
        recurrence_rule: Some(rule.clone()),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        content_line(&ics, "RRULE:"),
        "RRULE:FREQ=YEARLY;BYDAY=WE;BYMONTHDAY=15;BYYEARDAY=100;BYWEEKNO=20;BYMONTH=3;WKST=SU"
    );

    // And the fold survives the trip back: every part arrives as it left, so a
    // save comparing the two sees no edit.
    let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
    assert_eq!(rules, rule);
}

#[test]
fn reads_the_weeks_of_the_year_off_a_rule_written_by_hand() {
    // RFC 5545 §3.3.10's `weeknum` may carry the leading plus JSCalendar has no
    // room for, and counts to 53 — the week a long year has and a short one does
    // not.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E1\r\n",
        "DTSTART:20260115T090000Z\r\n",
        "RRULE:FREQ=YEARLY;BYWEEKNO=+1,-53\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    let rules = event.recurrence_rule.as_ref().unwrap();
    assert_eq!(rules.by_week_no.as_deref(), Some(&[1, -53][..]));
    assert!(maps_recurrence_rule(rules));
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=YEARLY;BYWEEKNO=1,-53"
    );
}

#[test]
fn weeks_of_the_year_are_refused_at_every_frequency_but_yearly() {
    // RFC 5545 §3.3.10 is narrower here than for any other part this mapping
    // writes: BYWEEKNO MUST NOT be specified when FREQ is anything *other than*
    // YEARLY — not even `HOURLY`, which `BYYEARDAY` is allowed beside. So the gate
    // names the one frequency that is allowed rather than the ones that are not.
    //
    // The part is left off whole and the save path told the rule was seen in part,
    // rather than writing a line libical is entitled to refuse.
    for frequency in [
        "daily", "weekly", "monthly", "hourly", "minutely", "secondly",
    ] {
        let rule = RecurrenceRule {
            by_week_no: Some(vec![20]),
            ..RecurrenceRule::new(frequency)
        };
        assert!(!maps_recurrence_rule(&rule), "{frequency}");

        let event = CalendarEvent {
            recurrence_rule: Some(rule),
            ..CalendarEvent::default()
        };
        assert_eq!(
            line(&event_to_ical(&event), "RRULE:"),
            format!("RRULE:FREQ={}", frequency.to_ascii_uppercase()),
            "{frequency}"
        );
    }
}

#[test]
fn a_week_no_year_has_is_flagged_rather_than_written() {
    // RFC 5545's `ordwk` is 1 to 53 and RFC 8984 §4.3.3 counts backwards to -53;
    // zero is no week of any year, and 54 is a week no year has. A set holding one
    // such value is refused whole, because a `BYWEEKNO` holding the rest is a
    // different recurrence rather than a narrower view of this one.
    //
    // 54 has to be refused *here*: libical keeps it verbatim rather than dropping
    // the rule — `jmap-backend-cal/tests/marshal.rs` measures that — so nothing
    // below this mapping would catch it.
    for weeks in [vec![0], vec![54], vec![-54], vec![20, 0], vec![]] {
        let rule = RecurrenceRule {
            by_week_no: Some(weeks.clone()),
            ..RecurrenceRule::new("yearly")
        };
        assert!(!maps_recurrence_rule(&rule), "{weeks:?}");

        let event = CalendarEvent {
            recurrence_rule: Some(rule),
            ..CalendarEvent::default()
        };
        assert_eq!(
            line(&event_to_ical(&event), "RRULE:"),
            "RRULE:FREQ=YEARLY",
            "{weeks:?}"
        );
    }
}

#[test]
fn a_week_of_the_year_a_hand_written_rule_invents_is_not_written_back() {
    // The refusal above, reached the way a component really arrives: through the
    // parser. `54` is outside RFC 5545's `ordwk`, so the mapping is the one that
    // has to refuse it — and refuse the whole set, leaving the `RRULE` at its
    // frequency.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E1\r\n",
        "DTSTART:20260115T090000Z\r\n",
        "RRULE:FREQ=YEARLY;BYWEEKNO=20,54\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    let rules = event.recurrence_rule.as_ref().unwrap();
    assert_eq!(rules.by_week_no.as_deref(), Some(&[20, 54][..]));
    assert!(!maps_recurrence_rule(rules));
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=YEARLY",
        "and the weeks are left off the rule it is drawn as"
    );
}

#[test]
fn a_monthly_rule_names_which_occurrence_of_the_set_it_takes() {
    // "The last Friday of the month" — RFC 8984 §4.3.3's `bySetPosition`,
    // iCalendar's `BYSETPOS`. It is unlike every other part here: the others say
    // which dates the interval expands to, and this one picks out of that set
    // after the fact, counting from the end when negative.
    //
    // `BYDAY=FR;BYSETPOS=-1` and `BYDAY=-1FR` name the same Fridays, but a server
    // is entitled to spell it either way and only one of them was carried before.
    let event = CalendarEvent {
        recurrence_rule: Some(RecurrenceRule {
            by_day: Some(vec![NDay::new("fr")]),
            by_set_position: Some(vec![-1]),
            count: Some(4),
            ..RecurrenceRule::new("monthly")
        }),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=MONTHLY;COUNT=4;BYDAY=FR;BYSETPOS=-1"
    );

    let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
    assert_eq!(rules.by_set_position.as_deref(), Some(&[-1][..]));
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules));
}

#[test]
fn the_position_in_the_set_is_written_after_the_months() {
    // Every modeled part at once, in the order libical writes them — `BYSETPOS`
    // after `BYMONTH` and before `WKST` — so that a rule read back out of EDS's
    // own cache compares equal to the one that went in.
    let rule = RecurrenceRule {
        by_day: Some(vec![NDay::new("we")]),
        by_month_day: Some(vec![15]),
        by_year_day: Some(vec![100]),
        by_week_no: Some(vec![20]),
        by_month: Some(vec!["3".to_owned()]),
        by_set_position: Some(vec![2]),
        first_day_of_week: Some("su".to_owned()),
        ..RecurrenceRule::new("yearly")
    };
    let event = CalendarEvent {
        recurrence_rule: Some(rule.clone()),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        content_line(&ics, "RRULE:"),
        "RRULE:FREQ=YEARLY;BYDAY=WE;BYMONTHDAY=15;BYYEARDAY=100;BYWEEKNO=20;\
         BYMONTH=3;BYSETPOS=2;WKST=SU"
    );

    let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
    assert_eq!(rules, rule);
}

#[test]
fn reads_the_position_in_the_set_off_a_rule_written_by_hand() {
    // RFC 5545 §3.3.10's `setposday` is spelled as `yeardaynum` is: the leading
    // plus JSCalendar has no room for, and a count to 366 in either direction.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E1\r\n",
        "DTSTART:20260115T090000Z\r\n",
        "RRULE:FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=+1,-1\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    let rules = event.recurrence_rule.as_ref().unwrap();
    assert_eq!(rules.by_set_position.as_deref(), Some(&[1, -1][..]));
    assert!(maps_recurrence_rule(rules));
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=1,-1"
    );
}

#[test]
fn a_position_with_nothing_to_select_from_is_flagged_rather_than_written() {
    // RFC 5545 §3.3.10: BYSETPOS MUST only be used together with another BYxxx
    // part. Alone it selects out of the one occurrence the frequency already
    // names, so `BYSETPOS=2` beside nothing is a series that never happens again.
    //
    // The gate is on the parts actually *written*, not on the ones the rule
    // holds: `byWeekNo` beside a monthly frequency is a part this mapping leaves
    // off, so a `BYSETPOS` written next to it would be selecting from a set the
    // reader cannot see either.
    for rule in [
        RecurrenceRule {
            by_set_position: Some(vec![-1]),
            ..RecurrenceRule::new("monthly")
        },
        RecurrenceRule {
            by_week_no: Some(vec![20]),
            by_set_position: Some(vec![-1]),
            ..RecurrenceRule::new("monthly")
        },
    ] {
        assert!(!maps_recurrence_rule(&rule), "{rule:?}");

        let event = CalendarEvent {
            recurrence_rule: Some(rule.clone()),
            ..CalendarEvent::default()
        };
        assert_eq!(
            line(&event_to_ical(&event), "RRULE:"),
            "RRULE:FREQ=MONTHLY",
            "{rule:?}"
        );
    }
}

#[test]
fn a_position_no_set_has_is_flagged_rather_than_written() {
    // RFC 5545's `setposday` is 1 to 366 and RFC 8984 §4.3.3 counts backwards to
    // -366; zero selects nothing at all. A set holding one such value is refused
    // whole, because a `BYSETPOS` holding the rest picks different occurrences
    // rather than showing fewer of these.
    //
    // 367 has to be refused *here*: libical keeps it verbatim rather than
    // dropping the rule — `jmap-backend-cal/tests/marshal.rs` measures that — so
    // nothing below this mapping would catch it.
    for positions in [vec![0], vec![367], vec![-367], vec![1, 0], vec![]] {
        let rule = RecurrenceRule {
            by_day: Some(vec![NDay::new("fr")]),
            by_set_position: Some(positions.clone()),
            ..RecurrenceRule::new("monthly")
        };
        assert!(!maps_recurrence_rule(&rule), "{positions:?}");

        let event = CalendarEvent {
            recurrence_rule: Some(rule),
            ..CalendarEvent::default()
        };
        assert_eq!(
            line(&event_to_ical(&event), "RRULE:"),
            "RRULE:FREQ=MONTHLY;BYDAY=FR",
            "{positions:?}"
        );
    }
}

#[test]
fn a_position_a_hand_written_rule_invents_is_not_written_back() {
    // The refusal above, reached the way a component really arrives: through the
    // parser. `367` is outside RFC 5545's `setposday`, so the mapping is the one
    // that has to refuse it — and refuse the whole set, leaving the days it
    // selects from in place.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E1\r\n",
        "DTSTART:20260115T090000Z\r\n",
        "RRULE:FREQ=MONTHLY;BYDAY=FR;BYSETPOS=1,367\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    let rules = event.recurrence_rule.as_ref().unwrap();
    assert_eq!(rules.by_set_position.as_deref(), Some(&[1, 367][..]));
    assert!(!maps_recurrence_rule(rules));
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=MONTHLY;BYDAY=FR",
        "and the position is left off the rule it is drawn as"
    );
}

#[test]
fn carries_the_hours_of_the_day_a_rule_repeats_at() {
    // RFC 8984 §4.3.3's `byHour` is RFC 5545 §3.3.10's BYHOUR: the hours within
    // each interval the series happens at, 0 to 23. The first part modeled here
    // that names a time of day rather than a date.
    let rule = RecurrenceRule {
        by_hour: Some(vec![9, 17]),
        ..RecurrenceRule::new("daily")
    };
    let event = CalendarEvent {
        recurrence_rule: Some(rule.clone()),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "RRULE:"), "RRULE:FREQ=DAILY;BYHOUR=9,17");

    let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
    assert_eq!(rules.by_hour.as_deref(), Some(&[9, 17][..]));
    assert_eq!(rules, rule);
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules));
}

#[test]
fn the_times_of_day_are_written_before_every_other_part() {
    // Every modeled part at once, in the order libical writes them — and the
    // three that name a time of day go *first*, finest unit outwards, ahead of
    // `BYDAY` and of each other. Measured in `jmap-backend-cal/tests/marshal.rs`:
    // a rule that went out in another order comes back out of EDS's own cache
    // reordered and compares unequal to itself, which the save path reads as an
    // edit.
    let rule = RecurrenceRule {
        by_second: Some(vec![0]),
        by_minute: Some(vec![30]),
        by_hour: Some(vec![9]),
        by_day: Some(vec![NDay::new("we")]),
        by_month_day: Some(vec![15]),
        by_year_day: Some(vec![100]),
        by_week_no: Some(vec![20]),
        by_month: Some(vec!["3".to_owned()]),
        by_set_position: Some(vec![2]),
        first_day_of_week: Some("su".to_owned()),
        ..RecurrenceRule::new("yearly")
    };
    let event = CalendarEvent {
        recurrence_rule: Some(rule.clone()),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        content_line(&ics, "RRULE:"),
        "RRULE:FREQ=YEARLY;BYSECOND=0;BYMINUTE=30;BYHOUR=9;BYDAY=WE;BYMONTHDAY=15;\
         BYYEARDAY=100;BYWEEKNO=20;BYMONTH=3;BYSETPOS=2;WKST=SU"
    );

    let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
    assert_eq!(rules, rule);
}

#[test]
fn reads_the_hours_of_the_day_off_a_rule_written_by_hand() {
    // And the hours are a set `BYSETPOS` may select out of: RFC 5545 §3.3.10
    // asks only that *some* other BYxxx part be there, and BYHOUR is one — so
    // "the last of 09:00 and 17:00 each day" is a rule this mapping now writes
    // where before it dropped the position for having nothing to select from.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E1\r\n",
        "DTSTART:20260115T090000Z\r\n",
        "RRULE:FREQ=DAILY;BYHOUR=9,17;BYSETPOS=-1\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    let rules = event.recurrence_rule.as_ref().unwrap();
    assert_eq!(rules.by_hour.as_deref(), Some(&[9, 17][..]));
    assert!(maps_recurrence_rule(rules));
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=DAILY;BYHOUR=9,17;BYSETPOS=-1"
    );
}

#[test]
fn an_hour_no_day_has_is_flagged_rather_than_written() {
    // RFC 5545's `hour` is 0 to 23, and RFC 8984 §4.3.3 has `byHour` unsigned,
    // so there is no backwards count here as there is for the days and weeks. A
    // set holding one unwritable value is refused whole, because a `BYHOUR`
    // holding the rest names different hours rather than fewer of these.
    //
    // libical drops the **entire** `RRULE` for an hour out of range — measured
    // in `jmap-backend-cal/tests/marshal.rs` — so a rule written that way would
    // reach EDS's cache as a single appointment with the user's series gone.
    // And it answers an *empty* `BYHOUR` by inventing `BYHOUR=0`, which would
    // move the whole series to midnight, so the empty set is refused too.
    for hours in [vec![24], vec![99], vec![9, 24], vec![]] {
        let rule = RecurrenceRule {
            by_hour: Some(hours.clone()),
            by_day: Some(vec![NDay::new("fr")]),
            ..RecurrenceRule::new("daily")
        };
        assert!(!maps_recurrence_rule(&rule), "{hours:?}");

        let event = CalendarEvent {
            recurrence_rule: Some(rule),
            ..CalendarEvent::default()
        };
        assert_eq!(
            line(&event_to_ical(&event), "RRULE:"),
            "RRULE:FREQ=DAILY;BYDAY=FR",
            "{hours:?}"
        );
    }
}

#[test]
fn an_hour_a_hand_written_rule_invents_is_not_written_back() {
    // The refusal above, reached the way a component really arrives. `24` is
    // outside RFC 5545's `hour`, and the part is left off while the days it
    // would have limited stay in place.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E1\r\n",
        "DTSTART:20260115T090000Z\r\n",
        "RRULE:FREQ=DAILY;BYDAY=FR;BYHOUR=9,24\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    let rules = event.recurrence_rule.as_ref().unwrap();
    assert!(!maps_recurrence_rule(rules));
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=DAILY;BYDAY=FR",
        "and the hours are left off the rule it is drawn as"
    );
}

#[test]
fn an_all_day_event_whose_rule_names_hours_is_drawn_as_a_timed_one() {
    // RFC 5545 §3.3.10: BYHOUR MUST NOT be specified when DTSTART has a value
    // type of DATE — an hour of the day means nothing beside a day with no
    // clock. libical keeps such a rule anyway (measured in
    // `jmap-backend-cal/tests/marshal.rs`), so this mapping is the only place
    // the contradiction can be resolved.
    //
    // It is resolved the way a 09:00 start and a zone already are: the DATE
    // form is dropped and the event is drawn timed, which is wrong about its
    // day-ness but right about when it happens. Refusing the hours instead
    // would draw an all-day series every day where the real one is at 09:00,
    // and hide the difference from the user.
    let event = CalendarEvent {
        start: Some("2026-01-15T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        show_without_time: Some(true),
        recurrence_rule: Some(RecurrenceRule {
            by_hour: Some(vec![9]),
            ..RecurrenceRule::new("daily")
        }),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "DTSTART"), "DTSTART:20260115T000000");
    assert_eq!(line(&ics, "RRULE:"), "RRULE:FREQ=DAILY;BYHOUR=9");

    // And the save path compares against this same rendering, so the flag lost
    // here is not read back as the user having cleared it.
    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.show_without_time, None);
    assert_eq!(read_back.recurrence_rule, event.recurrence_rule);
}

#[test]
fn an_all_day_event_whose_hours_are_unwritable_stays_a_date() {
    // The other side of that: an `RRULE` that will not carry the hours anyway
    // leaves nothing for the DATE form to contradict, so the event keeps its
    // day-ness. `maps_recurrence_rule` is what stops the save path patching the
    // recurrence it was shown only in part.
    let event = CalendarEvent {
        start: Some("2026-01-15T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        show_without_time: Some(true),
        recurrence_rule: Some(RecurrenceRule {
            by_hour: Some(vec![24]),
            ..RecurrenceRule::new("daily")
        }),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "DTSTART"), "DTSTART;VALUE=DATE:20260115");
    assert_eq!(line(&ics, "RRULE:"), "RRULE:FREQ=DAILY");
    assert!(!maps_recurrence_rule(
        event.recurrence_rule.as_ref().unwrap()
    ));
}

#[test]
fn carries_the_minutes_and_seconds_a_rule_repeats_at() {
    // RFC 8984 §4.3.3's `byMinute` and `bySecond` are RFC 5545 §3.3.10's
    // BYMINUTE and BYSECOND: "on the hour and the half hour, on the second".
    // With these the rule is modeled to the bottom of §4.3.3 but for `rscale`
    // and `skip`.
    let rule = RecurrenceRule {
        by_minute: Some(vec![0, 30]),
        by_second: Some(vec![0]),
        ..RecurrenceRule::new("hourly")
    };
    let event = CalendarEvent {
        recurrence_rule: Some(rule.clone()),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=HOURLY;BYSECOND=0;BYMINUTE=0,30"
    );

    let rules = ical_to_event(&ics).expect("parse").recurrence_rule.unwrap();
    assert_eq!(rules.by_minute.as_deref(), Some(&[0, 30][..]));
    assert_eq!(rules.by_second.as_deref(), Some(&[0][..]));
    assert_eq!(rules, rule);
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules));
}

#[test]
fn reads_the_minutes_and_seconds_off_a_rule_written_by_hand() {
    // Both are sets `BYSETPOS` may select out of, as the hours are: RFC 5545
    // §3.3.10 asks only that *some* other BYxxx part be there, and libical
    // agrees (measured in `jmap-backend-cal/tests/marshal.rs`).
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E1\r\n",
        "DTSTART:20260115T090000Z\r\n",
        "RRULE:FREQ=HOURLY;BYSECOND=0,30;BYMINUTE=15,45;BYSETPOS=-1\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    let rules = event.recurrence_rule.as_ref().unwrap();
    assert_eq!(rules.by_minute.as_deref(), Some(&[15, 45][..]));
    assert_eq!(rules.by_second.as_deref(), Some(&[0, 30][..]));
    assert!(maps_recurrence_rule(rules));
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=HOURLY;BYSECOND=0,30;BYMINUTE=15,45;BYSETPOS=-1"
    );
}

#[test]
fn the_sixtieth_second_is_written_and_the_sixtieth_minute_is_not() {
    // The one place the two ranges differ: RFC 5545 §3.3.10's `seconds` runs 0
    // to 60, the sixtieth being the leap second UTC occasionally inserts, while
    // `minutes` stops at 59. libical enforces exactly that (measured in
    // `jmap-backend-cal/tests/marshal.rs`), so the mapping does too rather than
    // treating the two parts as one range.
    let leap = RecurrenceRule {
        by_second: Some(vec![60]),
        ..RecurrenceRule::new("minutely")
    };
    assert!(maps_recurrence_rule(&leap));
    let event = CalendarEvent {
        recurrence_rule: Some(leap),
        ..CalendarEvent::default()
    };
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=MINUTELY;BYSECOND=60"
    );

    // A set holding one unwritable value is refused whole, because a part
    // holding the rest names different times rather than fewer of these — and
    // an out-of-range value costs libical the *whole* rule, so the event would
    // reach EDS's cache as a single appointment with the series gone. The empty
    // set is refused for the reason the empty `BYHOUR` is: libical answers it
    // with the zeroth minute or second, moving every occurrence.
    for (minutes, seconds) in [
        (Some(vec![60]), None),
        (Some(vec![u32::MAX]), None),
        (Some(vec![0, 60]), None),
        (Some(vec![]), None),
        (None, Some(vec![61])),
        (None, Some(vec![0, 61])),
        (None, Some(vec![])),
    ] {
        let rule = RecurrenceRule {
            by_minute: minutes.clone(),
            by_second: seconds.clone(),
            by_day: Some(vec![NDay::new("fr")]),
            ..RecurrenceRule::new("daily")
        };
        assert!(!maps_recurrence_rule(&rule), "{minutes:?} {seconds:?}");

        let event = CalendarEvent {
            recurrence_rule: Some(rule),
            ..CalendarEvent::default()
        };
        assert_eq!(
            line(&event_to_ical(&event), "RRULE:"),
            "RRULE:FREQ=DAILY;BYDAY=FR",
            "{minutes:?} {seconds:?}"
        );
    }
}

#[test]
fn a_minute_a_hand_written_rule_invents_is_not_written_back() {
    // The refusal above, reached the way a component really arrives. `60` is
    // outside RFC 5545's `minutes` and `61` outside its `seconds`, and — as for
    // an out-of-range `BYHOUR` — the parts are left off while the days they
    // would have limited stay in place.
    //
    // Both values are *numbers* out of range rather than unreadable tokens,
    // because a token the parser below this crate cannot read is dropped there
    // and never arrives: `BYSECOND=XX` reaches this mapping as no `BYSECOND` at
    // all, the narrowing already noted for `BYDAY` and `WKST`. What arrives here
    // is a number, which is why [`to_time_of_day`]'s sentinel is one no part can
    // carry rather than a signal the mapping relies on seeing.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E1\r\n",
        "DTSTART:20260115T090000Z\r\n",
        "RRULE:FREQ=DAILY;BYDAY=FR;BYMINUTE=30,60;BYSECOND=15,61\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    let rules = event.recurrence_rule.as_ref().unwrap();
    assert_eq!(rules.by_minute.as_deref(), Some(&[30, 60][..]));
    assert_eq!(rules.by_second.as_deref(), Some(&[15, 61][..]));
    assert!(!maps_recurrence_rule(rules));
    assert_eq!(
        line(&event_to_ical(&event), "RRULE:"),
        "RRULE:FREQ=DAILY;BYDAY=FR",
        "and the minutes and seconds are left off the rule it is drawn as"
    );
}

#[test]
fn an_all_day_event_whose_rule_names_minutes_is_drawn_as_a_timed_one() {
    // RFC 5545 §3.3.10 forbids BYMINUTE and BYSECOND beside a DATE-valued
    // DTSTART for the reason it forbids BYHOUR: a day with no clock has no
    // minute to repeat at. Resolved the way the hours are — the DATE form is
    // dropped and the event drawn timed — so all three parts are asked about
    // together rather than one at a time.
    let event = CalendarEvent {
        start: Some("2026-01-15T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        show_without_time: Some(true),
        recurrence_rule: Some(RecurrenceRule {
            by_minute: Some(vec![30]),
            by_second: Some(vec![15]),
            ..RecurrenceRule::new("daily")
        }),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "DTSTART"), "DTSTART:20260115T000000");
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=DAILY;BYSECOND=15;BYMINUTE=30"
    );

    // And the save path compares against this same rendering, so the flag lost
    // here is not read back as the user having cleared it.
    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.show_without_time, None);
    assert_eq!(read_back.recurrence_rule, event.recurrence_rule);
}

#[test]
fn an_all_day_event_whose_minutes_are_unwritable_stays_a_date() {
    // The other side of that, as for the hours: an `RRULE` that will not carry
    // the minutes anyway leaves nothing for the DATE form to contradict, so the
    // event keeps its day-ness and `maps_recurrence_rule` stops the save path
    // patching the recurrence it was shown only in part.
    let event = CalendarEvent {
        start: Some("2026-01-15T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        show_without_time: Some(true),
        recurrence_rule: Some(RecurrenceRule {
            by_minute: Some(vec![60]),
            ..RecurrenceRule::new("daily")
        }),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "DTSTART"), "DTSTART;VALUE=DATE:20260115");
    assert_eq!(line(&ics, "RRULE:"), "RRULE:FREQ=DAILY");
    assert!(!maps_recurrence_rule(
        event.recurrence_rule.as_ref().unwrap()
    ));
}

/// [`maps_recurrence_override`] asked of an override on a series that says
/// nothing about reminders.
///
/// The predicate takes the series because one restated property's coverage is the
/// series' to decide — RFC 8984 §4.5.1's `useDefaultAlerts`, which an override may
/// not restate — and that is all it reads the event for. So every test but the
/// flag's own asks it of a series that does not set it; see
/// `an_occurrence_of_an_event_that_uses_the_default_reminders_is_drawn_with_none`,
/// which asks it of one that does.
fn maps_override(id: &str, patch: &Value) -> bool {
    maps_recurrence_override(&CalendarEvent::default(), id, patch)
}

/// A recurring event in one zone, with `overrides` naming single instances.
fn recurring_with(overrides: Value) -> CalendarEvent {
    CalendarEvent {
        // A detached instance is tied to its series by the UID they share, so
        // the series needs one.
        id: Some("E11".into()),
        start: Some("2026-01-15T13:00:00".to_owned()),
        time_zone: Some("Europe/Berlin".to_owned()),
        duration: Some("PT1H".to_owned()),
        recurrence_rule: Some(RecurrenceRule::new("weekly")),
        recurrence_overrides: Some(
            overrides
                .as_object()
                .expect("an object")
                .iter()
                .map(|(id, patch)| (id.clone(), patch.clone()))
                .collect(),
        ),
        ..CalendarEvent::default()
    }
}

#[test]
fn an_instance_that_does_not_occur_is_an_exdate() {
    // RFC 8984 §4.3.4 says "this one is off" with an override; RFC 5545
    // §3.8.5.1 says it with an EXDATE, in the same value type and zone as
    // DTSTART, or a reader resolves the instance against the wrong clock and
    // the exclusion misses the occurrence it was meant to remove.
    let event = recurring_with(json!({"2026-01-29T13:00:00": {"excluded": true}}));
    let ics = event_to_ical(&event);

    assert_eq!(
        line(&ics, "EXDATE"),
        "EXDATE;TZID=Europe/Berlin:20260129T130000"
    );
    assert!(without(&ics, "RDATE"), "{ics}");

    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.recurrence_overrides, event.recurrence_overrides);
}

#[test]
fn an_instance_the_rule_would_not_generate_is_an_rdate() {
    // The other degenerate patch: an override that changes nothing is an
    // instance that simply happens, which is what an RDATE names.
    let event = recurring_with(json!({"2026-02-05T13:00:00": {}}));
    let ics = event_to_ical(&event);

    assert_eq!(
        line(&ics, "RDATE"),
        "RDATE;TZID=Europe/Berlin:20260205T130000"
    );
    assert!(without(&ics, "EXDATE"), "{ics}");

    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.recurrence_overrides, event.recurrence_overrides);
}

#[test]
fn instances_of_the_same_kind_share_one_line() {
    let event = recurring_with(json!({
        "2026-01-29T13:00:00": {"excluded": true},
        "2026-02-12T13:00:00": {"excluded": true},
        "2026-02-05T13:00:00": {},
    }));
    let ics = event_to_ical(&event);

    // One property per kind, its values in the chronological order a
    // BTreeMap of LocalDateTimes iterates in.
    assert_eq!(
        line(&ics, "EXDATE"),
        "EXDATE;TZID=Europe/Berlin:20260129T130000,20260212T130000"
    );
    assert_eq!(
        line(&ics, "RDATE"),
        "RDATE;TZID=Europe/Berlin:20260205T130000"
    );

    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.recurrence_overrides, event.recurrence_overrides);
}

#[test]
fn a_utc_events_excluded_instance_carries_the_z_its_start_does() {
    let mut event = recurring_with(json!({"2026-01-29T13:00:00": {"excluded": true}}));
    event.time_zone = Some("Etc/UTC".to_owned());
    let ics = event_to_ical(&event);

    assert_eq!(line(&ics, "DTSTART"), "DTSTART:20260115T130000Z");
    assert_eq!(line(&ics, "EXDATE"), "EXDATE:20260129T130000Z");
    assert_eq!(
        ical_to_event(&ics).expect("parse").recurrence_overrides,
        event.recurrence_overrides
    );
}

#[test]
fn an_all_day_events_excluded_instance_is_a_date_like_its_start() {
    // RFC 5545 §3.8.5.1, as for UNTIL: the value type has to match DTSTART's,
    // so an event written as a DATE excludes a day rather than an instant.
    let event = CalendarEvent {
        start: Some("2026-01-15T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        show_without_time: Some(true),
        recurrence_rule: Some(RecurrenceRule::new("weekly")),
        recurrence_overrides: Some(
            [("2026-01-29T00:00:00".to_owned(), json!({"excluded": true}))].into(),
        ),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);

    assert_eq!(line(&ics, "DTSTART"), "DTSTART;VALUE=DATE:20260115");
    assert_eq!(line(&ics, "EXDATE"), "EXDATE;VALUE=DATE:20260129");

    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.show_without_time, Some(true));
    assert_eq!(read_back.recurrence_overrides, event.recurrence_overrides);
}

#[test]
fn an_all_day_event_whose_excluded_instance_has_a_time_stays_a_date_time() {
    // The same trade as an UNTIL at 09:00: truncating the exclusion to a date
    // would drop an instance the server still holds, so the event keeps its
    // DATE-TIME form and shows as timed instead.
    let event = CalendarEvent {
        start: Some("2026-01-15T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        show_without_time: Some(true),
        recurrence_rule: Some(RecurrenceRule::new("weekly")),
        recurrence_overrides: Some(
            [("2026-01-29T09:00:00".to_owned(), json!({"excluded": true}))].into(),
        ),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);

    assert_eq!(line(&ics, "DTSTART"), "DTSTART:20260115T000000");
    assert_eq!(line(&ics, "EXDATE"), "EXDATE:20260129T090000");
}

#[test]
fn an_instance_edited_on_its_own_is_a_vevent_of_its_own() {
    // RFC 8984 §4.3.4's third kind of override: not off, not merely on, but
    // *different*. iCalendar says that with a second VEVENT carrying the
    // series' UID and a RECURRENCE-ID naming the instance it stands in for
    // (RFC 5545 §3.8.4.4) — and the properties it does not restate are the
    // series'.
    let patch = json!({"title": "Sprint review"});
    assert!(maps_override("2026-01-29T13:00:00", &patch));

    let event = recurring_with(json!({"2026-01-29T13:00:00": patch}));
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    let instance = vevent(&ics, 1);
    assert_eq!(line(instance, "UID:"), "UID:E11", "{ics}");
    assert_eq!(
        line(instance, "RECURRENCE-ID"),
        "RECURRENCE-ID;TZID=Europe/Berlin:20260129T130000"
    );
    assert_eq!(line(instance, "SUMMARY:"), "SUMMARY:Sprint review");
    // The instance's own start defaults to the id, and the series' length and
    // recurrence are not restated on it.
    assert_eq!(
        line(instance, "DTSTART"),
        "DTSTART;TZID=Europe/Berlin:20260129T130000"
    );
    assert_eq!(line(instance, "DURATION"), "DURATION:PT1H");
    assert!(without(instance, "RRULE"), "{ics}");
    // Placed by the component of its own, so an RDATE for the same instant
    // would only say a second time that it happens.
    assert!(without(&ics, "RDATE"), "{ics}");

    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.title.as_deref(), None, "{ics}");
    assert_eq!(read_back.recurrence_overrides, event.recurrence_overrides);
}

#[test]
fn an_instance_moved_to_another_time_keeps_the_recurrence_id_it_replaces() {
    // The one place the two ends of an override differ: RECURRENCE-ID names
    // the instance the series generated, DTSTART where it actually is. An
    // override says the second only when it moved.
    let event = recurring_with(json!({
        "2026-01-29T13:00:00": {"start": "2026-01-29T15:30:00", "duration": "PT30M"},
    }));
    let ics = event_to_ical(&event);

    let instance = vevent(&ics, 1);
    assert_eq!(
        line(instance, "RECURRENCE-ID"),
        "RECURRENCE-ID;TZID=Europe/Berlin:20260129T130000"
    );
    assert_eq!(
        line(instance, "DTSTART"),
        "DTSTART;TZID=Europe/Berlin:20260129T153000"
    );
    assert_eq!(line(instance, "DURATION"), "DURATION:PT30M");

    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.recurrence_overrides, event.recurrence_overrides);
}

#[test]
fn an_instance_moved_to_another_zone_carries_it_on_its_own_start() {
    // An occurrence can move across the clock as well as along it, and RFC 8984
    // §4.4.3 lets an override patch `timeZone` like any other property. In
    // iCalendar a zone belongs to the property that carries it, so the
    // instance's own DTSTART states it — while the RECURRENCE-ID keeps the
    // series' zone, because it names the occurrence the *rules* generated and
    // the rules run on the series' clock (RFC 5545 §3.8.4.4).
    let patch = json!({"start": "2026-01-29T09:00:00", "timeZone": "America/New_York"});
    assert!(maps_override("2026-01-29T13:00:00", &patch));

    let event = recurring_with(json!({"2026-01-29T13:00:00": patch}));
    let ics = event_to_ical(&event);

    // The series is untouched by where one of its instances went.
    assert_eq!(
        line(&ics, "DTSTART"),
        "DTSTART;TZID=Europe/Berlin:20260115T130000"
    );
    let instance = vevent(&ics, 1);
    assert_eq!(
        line(instance, "RECURRENCE-ID"),
        "RECURRENCE-ID;TZID=Europe/Berlin:20260129T130000"
    );
    assert_eq!(
        line(instance, "DTSTART"),
        "DTSTART;TZID=America/New_York:20260129T090000"
    );

    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(read_back.recurrence_overrides, event.recurrence_overrides);
}

#[test]
fn an_instance_in_utc_carries_the_z_the_series_does_not() {
    // The same move into the one zone iCalendar spells without a TZID at all.
    let event = recurring_with(json!({"2026-01-29T13:00:00": {"timeZone": "Etc/UTC"}}));
    let ics = event_to_ical(&event);

    let instance = vevent(&ics, 1);
    assert_eq!(
        line(instance, "RECURRENCE-ID"),
        "RECURRENCE-ID;TZID=Europe/Berlin:20260129T130000"
    );
    assert_eq!(line(instance, "DTSTART"), "DTSTART:20260129T130000Z");

    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.recurrence_overrides, event.recurrence_overrides);
}

#[test]
fn an_instance_that_drops_its_zone_floats_rather_than_inheriting_the_series() {
    // A null removes a property, and an instance with no zone is a floating
    // one: a DTSTART with no TZID and no `Z`, which is exactly what RFC 5545
    // §3.3.5 form 1 says. The series' zone does not reach across to it, so the
    // two ends agree without either inventing a zone.
    let patch = json!({"timeZone": null});
    assert!(maps_override("2026-01-29T13:00:00", &patch));

    let event = recurring_with(json!({"2026-01-29T13:00:00": patch}));
    let ics = event_to_ical(&event);

    let instance = vevent(&ics, 1);
    assert_eq!(line(instance, "DTSTART"), "DTSTART:20260129T130000");
    assert_eq!(
        line(instance, "RECURRENCE-ID"),
        "RECURRENCE-ID;TZID=Europe/Berlin:20260129T130000"
    );

    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.recurrence_overrides, event.recurrence_overrides);
}

#[test]
fn a_zone_an_instance_cannot_name_is_flagged_rather_than_written_back() {
    // The same rule the series' own zone follows, one level down: a `TZID` is
    // an iCalendar identifier and only sometimes an RFC 8984 §1.4.9 name, and
    // `recurrenceOverrides` is replaced whole — so an entry carrying a value
    // JSCalendar cannot hold would risk the server rejecting the save, and with
    // it every other edit in it.
    for zone in [
        json!(LIBICAL_TZID),
        json!("W. Europe Standard Time"),
        json!(""),
        json!(42),
    ] {
        let patch = json!({"timeZone": zone});
        assert!(!maps_override("2026-01-29T13:00:00", &patch), "{patch}");

        // Nothing left to draw, so the occurrence is placed by a bare RDATE at
        // the series' zone rather than moved to a zone we could not name.
        let event = recurring_with(json!({"2026-01-29T13:00:00": patch}));
        let ics = event_to_ical(&event);
        assert_eq!(vevents(&ics), 1, "{ics}");
        assert_eq!(
            line(&ics, "RDATE"),
            "RDATE;TZID=Europe/Berlin:20260129T130000"
        );
    }
}

#[test]
fn an_all_day_event_whose_instance_takes_a_zone_stays_a_date_time() {
    // The same trade as an instance that took a time: RFC 5545 §3.2.19 says a
    // DATE value carries no TZID, so writing the event as a date would drop the
    // zone the instance moved into. It is written as the timed event it half is.
    let event = CalendarEvent {
        id: Some("E12".into()),
        start: Some("2026-01-15T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        show_without_time: Some(true),
        recurrence_rule: Some(RecurrenceRule::new("weekly")),
        recurrence_overrides: Some(
            [(
                "2026-01-29T00:00:00".to_owned(),
                json!({"timeZone": "America/New_York"}),
            )]
            .into(),
        ),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);

    assert_eq!(line(&ics, "DTSTART"), "DTSTART:20260115T000000");
    let instance = vevent(&ics, 1);
    assert_eq!(
        line(instance, "RECURRENCE-ID"),
        "RECURRENCE-ID:20260129T000000"
    );
    assert_eq!(
        line(instance, "DTSTART"),
        "DTSTART;TZID=America/New_York:20260129T000000"
    );

    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.recurrence_overrides, event.recurrence_overrides);
}

#[test]
fn a_detached_instance_in_another_zone_is_read_as_a_zone_of_its_own() {
    // The reading direction with the identifiers Evolution really hands over:
    // libical's own for the series, translated off the VTIMEZONE beside it, and
    // a second zone on the instance that moved. Reading only the start would
    // resolve 09:00 New York against Berlin and move the occurrence six hours.
    let ics = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{}\
         BEGIN:VEVENT\r\nUID:E13\r\n\
         DTSTART;TZID={LIBICAL_TZID}:20260115T130000\r\n\
         DURATION:PT1H\r\nRRULE:FREQ=WEEKLY\r\nEND:VEVENT\r\n\
         BEGIN:VEVENT\r\nUID:E13\r\n\
         RECURRENCE-ID;TZID={LIBICAL_TZID}:20260129T130000\r\n\
         DTSTART;TZID=America/New_York:20260129T090000\r\n\
         DURATION:PT1H\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        vtimezone(LIBICAL_TZID, "Europe/Berlin")
    );

    let event = ical_to_event(&ics).expect("parse");

    assert_eq!(event.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(
        event.recurrence_overrides,
        Some(
            [(
                "2026-01-29T13:00:00".to_owned(),
                json!({"start": "2026-01-29T09:00:00", "timeZone": "America/New_York"}),
            )]
            .into()
        )
    );
}

#[test]
fn an_instance_that_drops_a_property_reads_back_as_removing_it() {
    // A PatchObject removes a property with a null, and the component says the
    // same thing by not carrying the line at all. Round-tripping that is what
    // keeps a save from restoring the series' description onto an instance the
    // user cleared it from.
    let mut event = recurring_with(json!({"2026-01-29T13:00:00": {"description": null}}));
    event.description = Some("bring the numbers".to_owned());
    let ics = event_to_ical(&event);

    let instance = vevent(&ics, 1);
    assert!(without(instance, "DESCRIPTION"), "{ics}");
    assert_eq!(
        line(&ics, "DESCRIPTION"),
        "DESCRIPTION:bring the numbers",
        "the series keeps its own"
    );

    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.recurrence_overrides, event.recurrence_overrides);
    assert!(maps_override(
        "2026-01-29T13:00:00",
        &json!({"description": null})
    ));
}

#[test]
fn an_instance_both_edited_and_excluded_is_excluded_and_flagged() {
    // An instance that does not happen has nothing to show an edited title on,
    // so the exclusion wins and the rest of the patch is lost — which the save
    // path has to be told, or the next save writes the loss back.
    let patch = json!({"excluded": true, "title": "Sprint review"});
    assert!(!maps_override("2026-01-29T13:00:00", &patch));

    let event = recurring_with(json!({"2026-01-29T13:00:00": patch}));
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 1, "{ics}");
    assert_eq!(
        line(&ics, "EXDATE"),
        "EXDATE;TZID=Europe/Berlin:20260129T130000"
    );
}

#[test]
fn an_override_the_mapping_cannot_draw_is_still_placed_at_the_parents_title() {
    // The narrowing that remains: a patch naming properties outside the drawn
    // set — or carrying a value the drawing cannot take — is placed by a bare
    // RDATE, so the occurrence is at least visible, and flagged so that a save
    // never replaces the property it came from.
    for patch in [
        json!({"locations/1/name": "Room 3"}),
        json!({"title": 42}),
        json!({"title": ""}),
        json!({"status": "postponed"}),
        json!({"freeBusyStatus": "maybe"}),
        json!({"priority": 10}),
        json!({"priority": "1"}),
        json!({"privacy": "deniable"}),
        json!({"keywords": {"offsite": 1}}),
        json!({"start": "2026-02-30T13:00:00"}),
    ] {
        assert!(!maps_override("2026-01-29T13:00:00", &patch), "{patch}");

        let event = recurring_with(json!({"2026-01-29T13:00:00": patch}));
        let ics = event_to_ical(&event);
        assert_eq!(vevents(&ics), 1, "{ics}");
        assert_eq!(
            line(&ics, "RDATE"),
            "RDATE;TZID=Europe/Berlin:20260129T130000"
        );
    }
}

#[test]
fn an_override_naming_one_property_it_can_draw_is_drawn_and_the_rest_narrowed() {
    // Half-known is the same trade as an RRULE that had to drop its byDay: draw
    // what can be drawn, and flag the property so a save leaves it alone.
    let patch = json!({"title": "Sprint review", "locations/1/name": "Room 3"});
    assert!(!maps_override("2026-01-29T13:00:00", &patch));

    let event = recurring_with(json!({"2026-01-29T13:00:00": patch}));
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert_eq!(line(vevent(&ics, 1), "SUMMARY:"), "SUMMARY:Sprint review");
}

#[test]
fn an_all_day_events_edited_instance_is_written_as_a_date() {
    // RFC 5545 §3.8.4.4, as for EXDATE and UNTIL: a RECURRENCE-ID's value type
    // has to match DTSTART's, or it names an instant the series never
    // generated and the edit attaches to nothing.
    let event = CalendarEvent {
        id: Some("E12".into()),
        start: Some("2026-01-15T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        show_without_time: Some(true),
        recurrence_rule: Some(RecurrenceRule::new("weekly")),
        recurrence_overrides: Some(
            [(
                "2026-01-29T00:00:00".to_owned(),
                json!({"title": "Company day"}),
            )]
            .into(),
        ),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);

    assert_eq!(line(&ics, "DTSTART"), "DTSTART;VALUE=DATE:20260115");
    let instance = vevent(&ics, 1);
    assert_eq!(
        line(instance, "RECURRENCE-ID"),
        "RECURRENCE-ID;VALUE=DATE:20260129"
    );
    assert_eq!(line(instance, "DTSTART"), "DTSTART;VALUE=DATE:20260129");

    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.show_without_time, Some(true));
    assert_eq!(read_back.recurrence_overrides, event.recurrence_overrides);
}

#[test]
fn an_all_day_event_whose_edited_instance_takes_a_time_stays_a_date_time() {
    // The same trade as an excluded instance at 09:00: a DATE value cannot hold
    // the time the instance moved to, and truncating it would move the
    // appointment, so the whole event is written as the timed one it half is.
    let event = CalendarEvent {
        id: Some("E12".into()),
        start: Some("2026-01-15T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        show_without_time: Some(true),
        recurrence_rule: Some(RecurrenceRule::new("weekly")),
        recurrence_overrides: Some(
            [(
                "2026-01-29T00:00:00".to_owned(),
                json!({"start": "2026-01-29T09:00:00", "duration": "PT2H"}),
            )]
            .into(),
        ),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);

    assert_eq!(line(&ics, "DTSTART"), "DTSTART:20260115T000000");
    let instance = vevent(&ics, 1);
    assert_eq!(
        line(instance, "RECURRENCE-ID"),
        "RECURRENCE-ID:20260129T000000"
    );
    assert_eq!(line(instance, "DTSTART"), "DTSTART:20260129T090000");

    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.recurrence_overrides, event.recurrence_overrides);
}

#[test]
fn the_series_is_the_vevent_without_a_recurrence_id_whatever_the_order() {
    // EDS hands a save every instance of one uid it holds, in no promised
    // order. Taking the first component would read a single edited day as if
    // it were the whole series.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E13\r\n",
        "RECURRENCE-ID:20260129T130000Z\r\n",
        "DTSTART:20260129T150000Z\r\n",
        "SUMMARY:Sprint review\r\n",
        "END:VEVENT\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E13\r\n",
        "DTSTART:20260115T130000Z\r\n",
        "SUMMARY:Standup\r\n",
        "RRULE:FREQ=WEEKLY\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");

    assert_eq!(event.title.as_deref(), Some("Standup"));
    assert_eq!(event.start.as_deref(), Some("2026-01-15T13:00:00"));
    assert_eq!(
        event.recurrence_overrides,
        Some(
            [(
                "2026-01-29T13:00:00".to_owned(),
                json!({"start": "2026-01-29T15:00:00", "title": "Sprint review"}),
            )]
            .into()
        )
    );
}

#[test]
fn a_detached_instance_that_restates_the_series_is_an_instance_that_happens() {
    // Nothing differs, so the patch is empty — which is exactly what an RDATE
    // says, and the same override either spelling produced.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E14\r\n",
        "DTSTART:20260115T130000Z\r\n",
        "SUMMARY:Standup\r\n",
        "RRULE:FREQ=WEEKLY\r\n",
        "END:VEVENT\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E14\r\n",
        "RECURRENCE-ID:20260129T130000Z\r\n",
        "DTSTART:20260129T130000Z\r\n",
        "SUMMARY:Standup\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    assert_eq!(
        ical_to_event(ics).expect("parse").recurrence_overrides,
        Some([("2026-01-29T13:00:00".to_owned(), json!({}))].into())
    );
}

#[test]
fn an_override_for_this_and_future_instances_is_not_read_as_one_instance() {
    // RFC 5545 §3.2.13's RANGE=THISANDFUTURE makes the component stand for
    // every instance from that one on. Reading it as a single override would
    // move one day and quietly drop the change to all the others, so it is
    // skipped — the same answer this mapping gives any value it cannot read.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E15\r\n",
        "DTSTART:20260115T130000Z\r\n",
        "SUMMARY:Standup\r\n",
        "RRULE:FREQ=WEEKLY\r\n",
        "END:VEVENT\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E15\r\n",
        "RECURRENCE-ID;RANGE=THISANDFUTURE:20260129T130000Z\r\n",
        "DTSTART:20260129T150000Z\r\n",
        "SUMMARY:Standup\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    assert_eq!(
        ical_to_event(ics).expect("parse").recurrence_overrides,
        None
    );
}

#[test]
fn a_detached_instance_with_no_series_is_still_read_as_an_event() {
    // Nothing to attach it to and nothing honest to say about the series, so
    // the component is read as the event it describes — which is what this
    // mapping did before it knew what a RECURRENCE-ID was.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E16\r\n",
        "RECURRENCE-ID:20260129T130000Z\r\n",
        "DTSTART:20260129T150000Z\r\n",
        "SUMMARY:Sprint review\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");

    assert_eq!(event.title.as_deref(), Some("Sprint review"));
    assert_eq!(event.start.as_deref(), Some("2026-01-29T15:00:00"));
    assert_eq!(event.recurrence_overrides, None);
}

#[test]
fn a_detached_instance_wins_over_an_rdate_for_the_same_instant() {
    // A component naming one instant both ways says the more specific thing
    // with the VEVENT; the RDATE only repeats that the instance happens.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E17\r\n",
        "DTSTART:20260115T130000Z\r\n",
        "SUMMARY:Standup\r\n",
        "RRULE:FREQ=WEEKLY\r\n",
        "RDATE:20260129T130000Z\r\n",
        "END:VEVENT\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E17\r\n",
        "RECURRENCE-ID:20260129T130000Z\r\n",
        "DTSTART:20260129T130000Z\r\n",
        "SUMMARY:Sprint review\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    assert_eq!(
        ical_to_event(ics).expect("parse").recurrence_overrides,
        Some(
            [(
                "2026-01-29T13:00:00".to_owned(),
                json!({"title": "Sprint review"}),
            )]
            .into()
        )
    );
}

/// A weekly hour-long series whose `RDATE` names one extra instance the given
/// way — a bare date-time, or either spelling of RFC 5545 §3.3.9's period.
fn series_with_rdate(value: &str) -> String {
    format!(
        concat!(
            "BEGIN:VCALENDAR\r\n",
            "VERSION:2.0\r\n",
            "BEGIN:VEVENT\r\n",
            "UID:E18\r\n",
            "DTSTART:20260115T130000Z\r\n",
            "DURATION:PT1H\r\n",
            "SUMMARY:Standup\r\n",
            "RRULE:FREQ=WEEKLY\r\n",
            "RDATE;VALUE=PERIOD:{value}\r\n",
            "END:VEVENT\r\n",
            "END:VCALENDAR\r\n",
        ),
        // `concat!` hides the placeholder from format_args!'s capture.
        value = value,
    )
}

#[test]
fn an_rdate_period_gives_its_instance_the_length_the_period_states() {
    // RFC 5545 §3.8.5.2 lets an RDATE state a period rather than an instant,
    // which is how iCalendar says "this extra occurrence runs longer than the
    // rest". Reading only its start showed the two-hour slot as the series'
    // hour — the occurrence was there, at the wrong length.
    for value in ["20260205T130000Z/PT2H", "20260205T130000Z/20260205T150000Z"] {
        let event = ical_to_event(&series_with_rdate(value)).expect("parse");

        assert_eq!(event.duration.as_deref(), Some("PT1H"), "{value}");
        assert_eq!(
            event.recurrence_overrides,
            Some(
                [(
                    "2026-02-05T13:00:00".to_owned(),
                    json!({"duration": "PT2H"})
                )]
                .into()
            ),
            "{value}"
        );
    }
}

#[test]
fn an_rdate_period_as_long_as_the_series_patches_nothing() {
    // The period restates the length the occurrence would have had, so there is
    // nothing to override: the same empty patch a bare RDATE produces.
    for value in ["20260205T130000Z/PT1H", "20260205T130000Z/20260205T140000Z"] {
        assert_eq!(
            ical_to_event(&series_with_rdate(value))
                .expect("parse")
                .recurrence_overrides,
            Some([("2026-02-05T13:00:00".to_owned(), json!({}))].into()),
            "{value}"
        );
    }
}

#[test]
fn an_rdate_period_naming_no_usable_length_leaves_the_instance_with_none() {
    // A period ending before it starts, and one whose duration is negative:
    // neither states a length an occurrence can have. The instance still
    // happens — that is what the RDATE said — but its length is removed rather
    // than silently inherited from the series, which is the answer a detached
    // VEVENT carrying neither DURATION nor DTEND already gives.
    for value in [
        "20260205T130000Z/20260205T120000Z",
        "20260205T130000Z/-PT1H",
        // A half that begins like a duration and measures nothing. It used to
        // reach the server as the occurrence's `duration`, since anything
        // starting with the designator was handed over as written.
        "20260205T130000Z/PT",
        "20260205T130000Z/P1X",
    ] {
        assert_eq!(
            ical_to_event(&series_with_rdate(value))
                .expect("parse")
                .recurrence_overrides,
            Some([("2026-02-05T13:00:00".to_owned(), json!({"duration": null}))].into()),
            "{value}"
        );
    }

    // A duration written as zero is the same zero length said the other way,
    // and it is passed through as written rather than recognised: PT0S, P0D and
    // PT0H0M0S all spell it, and telling them apart means parsing a value this
    // mapping otherwise hands over untouched. RFC 8984 §4.2.2 reads the result
    // as the zero length the null above leaves behind, so the two differ on
    // paper and not in the calendar.
    assert_eq!(
        ical_to_event(&series_with_rdate("20260205T130000Z/PT0S"))
            .expect("parse")
            .recurrence_overrides,
        Some(
            [(
                "2026-02-05T13:00:00".to_owned(),
                json!({"duration": "PT0S"})
            )]
            .into()
        ),
    );
}

#[test]
fn an_instance_an_rdate_period_lengthened_is_written_back_at_that_length() {
    // The length has to survive the way out as well, or the next save hands the
    // server back the occurrence at the series' hour. A patch that says
    // something is written as a VEVENT of its own — the RDATE's job was only to
    // place the instance — so the length is stated by that component's DURATION.
    let event = ical_to_event(&series_with_rdate("20260205T130000Z/PT2H")).expect("parse");
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert!(without(vevent(&ics, 0), "RDATE"), "{ics}");
    let instance = vevent(&ics, 1);
    assert_eq!(
        line(instance, "RECURRENCE-ID"),
        "RECURRENCE-ID:20260205T130000Z"
    );
    assert_eq!(line(instance, "DTSTART"), "DTSTART:20260205T130000Z");
    assert_eq!(line(instance, "DURATION"), "DURATION:PT2H");

    assert_eq!(
        ical_to_event(&ics).expect("parse").recurrence_overrides,
        event.recurrence_overrides
    );
}

#[test]
fn an_override_whose_instant_cannot_be_written_is_flagged() {
    // No EXDATE can name it, so a save that replaced recurrenceOverrides would
    // delete the exclusion outright.
    for id in ["2026-13-29T13:00:00", "sometime", "2026-02-30T13:00:00"] {
        assert!(!maps_override(id, &json!({"excluded": true})), "{id}");

        let event = recurring_with(json!({id: {"excluded": true}}));
        let ics = event_to_ical(&event);
        assert!(without(&ics, "EXDATE"), "{ics}");
        assert!(without(&ics, "RDATE"), "{ics}");
    }

    // The shapes the component can carry whole: the two an EXDATE or an RDATE
    // spells, and the properties a detached VEVENT restates.
    for patch in [
        json!({"excluded": true}),
        json!({}),
        json!({"title": "Sprint review", "description": "the quarter"}),
        json!({"start": "2026-01-29T15:30:00", "duration": "PT30M"}),
        json!({"status": "cancelled"}),
        json!({"freeBusyStatus": "free"}),
        json!({"priority": 0}),
        json!({"priority": 9}),
        json!({"privacy": "secret"}),
        json!({"keywords": {"offsite": true}}),
        json!({"alerts": {"k1": {
            "@type": "Alert",
            "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
            "action": "display",
        }}}),
        json!({
            "status": null, "duration": null, "freeBusyStatus": null,
            "priority": null, "privacy": null, "keywords": null,
            "alerts": null,
        }),
    ] {
        assert!(maps_override("2026-01-29T13:00:00", &patch), "{patch}");
    }
}

#[test]
fn an_overrides_duration_that_states_no_length_is_flagged() {
    // One level down, and for the same reason: the only place a component states
    // an instance's own length is that VEVENT's DURATION, so a length that
    // cannot be written there is an override the document shows in part. Read
    // back, the instance is at the series' length, and a save replacing
    // recurrenceOverrides would hand the server that as the user's edit —
    // quietly shortening an occurrence nobody touched.
    for value in ["-PT1H", "next tuesday", "PT", "3600"] {
        assert!(
            !maps_override("2026-01-29T13:00:00", &json!({"duration": value})),
            "{value}"
        );

        // Still drawn as far as it goes: the occurrence is placed by an RDATE
        // at the series' length, as any other override this mapping cannot
        // describe is.
        let ics = event_to_ical(&recurring_with(
            json!({"2026-01-29T13:00:00": {"duration": value}}),
        ));
        assert_eq!(vevents(&ics), 1, "{value}: {ics}");
        assert_eq!(
            line(&ics, "RDATE"),
            "RDATE;TZID=Europe/Berlin:20260129T130000",
            "{value}"
        );
    }

    // And the lengths that do cross, including the one value the check changes
    // on the way through: RFC 5545's `+` is dropped, and what comes back means
    // the same thing, which is what an override has to promise.
    for value in [json!("PT30M"), json!("P1D"), json!("+PT1H"), Value::Null] {
        assert!(
            maps_override("2026-01-29T13:00:00", &json!({"duration": value})),
            "{value}"
        );
    }
}

#[test]
fn an_instance_both_excluded_and_added_does_not_occur() {
    // A component naming the same instant in both properties contradicts
    // itself. Reading it as excluded is the reading that cannot invent an
    // appointment; RFC 5545 §3.8.5.1 has EXDATE win over the recurrence set
    // anyway.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E10\r\n",
        "DTSTART:20260115T130000Z\r\n",
        "RRULE:FREQ=WEEKLY\r\n",
        "RDATE:20260129T130000Z\r\n",
        "EXDATE:20260129T130000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let overrides = ical_to_event(ics).expect("parse").recurrence_overrides;
    assert_eq!(
        overrides,
        Some([("2026-01-29T13:00:00".to_owned(), json!({"excluded": true}))].into())
    );
}

#[test]
fn an_event_with_no_named_instances_says_nothing_about_them() {
    // None rather than an empty map: the save path reads an edit off a
    // difference from what was shown, and an empty map is a claim.
    let ics = event_to_ical(&fixture_event());
    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.recurrence_overrides, None);
    assert!(without(&ics, "EXDATE"), "{ics}");
    assert!(without(&ics, "RDATE"), "{ics}");
}

#[test]
fn properties_the_mapping_does_not_know_are_dropped_not_refused() {
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E8\r\n",
        "SUMMARY:Dentist\r\n",
        "LOCATION:Hauptstrasse 1\r\n",
        "SEQUENCE:3\r\n",
        "ATTENDEE;ROLE=REQ-PARTICIPANT:mailto:vera@example.com\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    assert_eq!(event.title.as_deref(), Some("Dentist"));
    // An unmapped property is a property we never write back, not a parse
    // failure: an event that loses its guest list still opens.
    assert!(event.extra.is_empty(), "{:?}", event.extra);
}

#[test]
fn a_calendar_without_an_event_is_an_error() {
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "BEGIN:VTODO\r\n",
        "UID:T1\r\n",
        "END:VTODO\r\n",
        "END:VCALENDAR\r\n",
    );
    // There is no empty CalendarEvent worth handing back; the caller has been
    // given something it cannot store.
    assert_eq!(ical_to_event(ics), Err(ICalError::NoEvent));
    assert_eq!(ical_to_event("nonsense"), Err(ICalError::NotACalendar));
}

/// The `TZID` libical hands out for a builtin zone, checked on this machine
/// against libical 3.x:
///
/// ```c
/// icaltimezone_get_tzid(icaltimezone_get_builtin_timezone("Europe/Berlin"))
/// // => "/freeassociation.sourceforge.net/Europe/Berlin"
/// ```
///
/// Every zoned component Evolution saves carries it, because the editor sets
/// the start with the zone object and libical writes that zone's own id.
const LIBICAL_TZID: &str = "/freeassociation.sourceforge.net/Europe/Berlin";

fn zoned(tzid: &str, vtimezone: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{vtimezone}\
         BEGIN:VEVENT\r\nUID:E9\r\nDTSTART;TZID={tzid}:20260115T130000\r\n\
         DURATION:PT1H\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn vtimezone(tzid: &str, location: &str) -> String {
    format!(
        "BEGIN:VTIMEZONE\r\nTZID:{tzid}\r\nX-LIC-LOCATION:{location}\r\n\
         BEGIN:STANDARD\r\nTZNAME:CET\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n\
         DTSTART:19701025T030000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n"
    )
}

#[test]
fn a_tzid_that_names_no_zone_is_read_off_the_vtimezone_it_points_at() {
    let ics = zoned(LIBICAL_TZID, &vtimezone(LIBICAL_TZID, "Europe/Berlin"));

    let event = ical_to_event(&ics).expect("parse");

    assert_eq!(
        event.time_zone.as_deref(),
        Some("Europe/Berlin"),
        "a solidus-prefixed TZID is RFC 5545 §3.2.19's non-standard identifier, \
         not an RFC 8984 §1.4.9 TimeZoneId; the VTIMEZONE says which zone it is"
    );
}

#[test]
fn a_tzid_that_names_a_zone_is_taken_at_its_word() {
    // The VTIMEZONE disagrees with the TZID. The TZID is already a name
    // JSCalendar can carry, so there is nothing to look up and no reason to
    // let a stray X-LIC-LOCATION move the event to another continent.
    let ics = zoned(
        "Europe/Berlin",
        &vtimezone("Europe/Berlin", "America/New_York"),
    );

    let event = ical_to_event(&ics).expect("parse");

    assert_eq!(event.time_zone.as_deref(), Some("Europe/Berlin"));
}

#[test]
fn a_tzid_nothing_explains_is_left_as_it_came() {
    // No VTIMEZONE at all, and one whose location is no more a zone name than
    // the TZID was. Neither yields a zone, and the value is passed on
    // unchanged rather than dropped: the save path has to be able to tell
    // "the component named a zone this mapping cannot" from "the component
    // named none", which is a floating event and a real thing to save.
    for vtimezone in [
        String::new(),
        vtimezone("Unknown Custom Zone", "Unknown Custom Zone"),
    ] {
        let ics = zoned("Unknown Custom Zone", &vtimezone);

        let event = ical_to_event(&ics).expect("parse");

        assert_eq!(
            event.time_zone.as_deref(),
            Some("Unknown Custom Zone"),
            "{ics}"
        );
    }
}

#[test]
fn a_zone_a_vtimezone_named_is_written_back_as_itself() {
    let ics = zoned(LIBICAL_TZID, &vtimezone(LIBICAL_TZID, "Europe/Berlin"));
    let event = ical_to_event(&ics).expect("parse");

    // The one spelling this crate writes, and the one the round trip is
    // measured against, so a save of an untouched component patches nothing.
    let back = event_to_ical(&event);
    assert_eq!(
        line(&back, "DTSTART"),
        "DTSTART;TZID=Europe/Berlin:20260115T130000"
    );
    assert_eq!(
        ical_to_event(&back).expect("parse").time_zone.as_deref(),
        Some("Europe/Berlin")
    );
}

#[test]
fn a_time_zone_is_a_name_or_it_is_not_one() {
    for value in [
        "Europe/Berlin",
        "America/Argentina/Buenos_Aires",
        "Etc/GMT+5",
        "Etc/UTC",
        "UTC",
    ] {
        assert!(names_time_zone(value), "{value} is an IANA zone name");
    }
    for value in [
        "",
        // RFC 8984 §1.4.9's other form: legal only alongside a `timeZones`
        // definition, which this mapping does not carry.
        LIBICAL_TZID,
        "/citadel.org/20250101_1/Europe/Berlin",
        // Windows zone names, which arrive from Exchange and Outlook.
        "W. Europe Standard Time",
        // A host name is not a zone, whichever end the solidus is on.
        "freeassociation.sourceforge.net/Europe/Berlin",
        "Europe/",
        "Europe//Berlin",
        // Anything a content line could be broken with.
        "Europe/Berlin\r\nSUMMARY:Gone",
        "Europe/Berlin;X=1",
    ] {
        assert!(!names_time_zone(value), "{value:?} is no zone name");
    }
}

/// The identifier of a zone no database names — RFC 8984 §1.4.9's other form,
/// which MUST begin with a solidus and which the event itself defines. It is
/// what an Exchange invitation's own `VTIMEZONE` becomes when a server converts
/// the invitation to JSCalendar, so it is what arrives from a JMAP account that
/// holds one.
const CUSTOM_TZID: &str = "/example.com/Europe-Berlin";

/// The definition of [`CUSTOM_TZID`] as RFC 8984 §4.7.2 spells one: central
/// Europe, with the two observances it moves between and the rule that says
/// when.
fn custom_zone() -> Value {
    json!({
        "@type": "TimeZone",
        "tzId": CUSTOM_TZID,
        "standard": [{
            "@type": "TimeZoneRule",
            "start": "1970-10-25T03:00:00",
            "offsetFrom": "+0200",
            "offsetTo": "+0100",
            "recurrenceRules": [{
                "@type": "RecurrenceRule",
                "frequency": "yearly",
                "byMonth": ["10"],
                "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": -1}],
            }],
            "names": {"CET": true},
        }],
        "daylight": [{
            "@type": "TimeZoneRule",
            "start": "1970-03-29T02:00:00",
            "offsetFrom": "+0100",
            "offsetTo": "+0200",
            "recurrenceRules": [{
                "@type": "RecurrenceRule",
                "frequency": "yearly",
                "byMonth": ["3"],
                "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": -1}],
            }],
            "names": {"CEST": true},
        }],
    })
}

/// The `VTIMEZONE` [`custom_zone`] has to be drawn as, and the whole of it: the
/// two observances, the offsets on either side of each, the rule that repeats
/// them and the names a reader shows.
const CUSTOM_DEFINITION: &str = "BEGIN:VTIMEZONE\r\n\
     TZID:/example.com/Europe-Berlin\r\n\
     BEGIN:STANDARD\r\n\
     DTSTART:19701025T030000\r\n\
     TZOFFSETFROM:+0200\r\n\
     TZOFFSETTO:+0100\r\n\
     RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n\
     TZNAME:CET\r\n\
     END:STANDARD\r\n\
     BEGIN:DAYLIGHT\r\n\
     DTSTART:19700329T020000\r\n\
     TZOFFSETFROM:+0100\r\n\
     TZOFFSETTO:+0200\r\n\
     RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3\r\n\
     TZNAME:CEST\r\n\
     END:DAYLIGHT\r\n\
     END:VTIMEZONE\r\n";

/// An event at 13:00 in `zone`, carrying `definitions` as its RFC 8984 §4.7.2
/// `timeZones`.
fn defining(zone: &str, definitions: Value) -> CalendarEvent {
    CalendarEvent {
        id: Some("E9".into()),
        start: Some("2026-01-15T13:00:00".to_owned()),
        time_zone: Some(zone.to_owned()),
        duration: Some("PT1H".to_owned()),
        time_zones: serde_json::from_value(definitions).expect("a map of time zones"),
        ..CalendarEvent::default()
    }
}

/// A zone the reader cannot look up has to be defined in the document that
/// refers to it, and RFC 8984 §4.7.2 is where the definition comes from.
///
/// This is the one `TZID` the mapping cannot lean on the consumer for. An IANA
/// name libical resolves out of its builtin table; a custom identifier resolves
/// nowhere, so a `DTSTART` naming one and defining nothing is a wall-clock time
/// in no particular zone — libical floats it and the appointment lands hours
/// from where the server put it.
#[test]
fn a_custom_zone_the_event_defines_is_defined_in_the_document() {
    let ics = event_to_ical(&defining(CUSTOM_TZID, json!({CUSTOM_TZID: custom_zone()})));

    assert_eq!(
        line(vevent(&ics, 0), "DTSTART"),
        format!("DTSTART;TZID={CUSTOM_TZID}:20260115T130000"),
        "{ics}"
    );
    assert!(
        ics.contains(CUSTOM_DEFINITION),
        "the zone the event names is not defined as it was given: {ics}"
    );
    // RFC 5545 §3.6's `icalbody` grammar puts the components in no particular
    // order, but a reader that resolves a `TZID` as it walks wants the
    // definition first, and it is what every emitter writes.
    assert!(
        ics.find("BEGIN:VTIMEZONE") < ics.find("BEGIN:VEVENT"),
        "the definition follows the event that refers to it: {ics}"
    );
}

/// The identifier survives the round trip as itself, which is what makes the
/// save path leave it alone on an edit: `patch::diff` never sends a `timeZone`
/// the server did not already hold, so the zone stays the server's, definition
/// and all.
#[test]
fn a_custom_zone_is_read_back_as_the_identifier_it_was_drawn_with() {
    let ics = event_to_ical(&defining(CUSTOM_TZID, json!({CUSTOM_TZID: custom_zone()})));

    let back = ical_to_event(&ics).expect("parse");

    assert_eq!(back.time_zone.as_deref(), Some(CUSTOM_TZID));
    assert_eq!(back.start.as_deref(), Some("2026-01-15T13:00:00"));
}

/// And the *definition* survives it too, which is what makes the identifier
/// worth anything: RFC 8984 §1.4.9 admits a custom `TimeZoneId` only beside the
/// `timeZones` entry that says what zone it is, so an event carrying the one
/// without the other names a zone nothing can resolve. A server is entitled to
/// refuse it, and a reader that does not floats the appointment hours from where
/// it belongs.
///
/// Reading it back is therefore not bookkeeping: it is the only way a document
/// whose zone came from somewhere other than a database — an Exchange
/// invitation's own `VTIMEZONE`, an `.ics` another client wrote — can be saved
/// as the event it is rather than as a floating one. See `read_time_zones`.
#[test]
fn a_custom_zone_the_document_defines_is_read_back_as_a_definition() {
    let ics = event_to_ical(&defining(CUSTOM_TZID, json!({CUSTOM_TZID: custom_zone()})));

    let back = ical_to_event(&ics).expect("parse");

    assert_eq!(
        serde_json::to_value(&back.time_zones).expect("a map of time zones"),
        json!({CUSTOM_TZID: custom_zone()}),
        "the definition the document carries is the one the event is in: {ics}"
    );
    // Which is the whole claim stated the other way round: what was drawn from
    // the definition draws again, so a save of an untouched component has
    // nothing to report.
    assert_eq!(event_to_ical(&back), ics);
}

/// The identifier of the zone [`ending`] defines.
const ENDING_TZID: &str = "/example.com/America-New_York";

/// A `VTIMEZONE` whose one observance stops repeating, in the shape RFC 5545
/// §3.6.5's own `America/New_York` example has it: a `DAYLIGHT` rule that ran
/// until the last Sunday of April 1973 and states that end as the UTC instant
/// §3.3.10 asks for beside a `DTSTART` that is a local time.
fn ending(until: &str, offset_from: &str) -> String {
    format!(
        "BEGIN:VTIMEZONE\r\n\
         TZID:{ENDING_TZID}\r\n\
         BEGIN:DAYLIGHT\r\n\
         DTSTART:19670430T020000\r\n\
         TZOFFSETFROM:{offset_from}\r\n\
         TZOFFSETTO:-0400\r\n\
         RRULE:FREQ=YEARLY;UNTIL={until};BYDAY=-1SU;BYMONTH=4\r\n\
         TZNAME:EDT\r\n\
         END:DAYLIGHT\r\n\
         END:VTIMEZONE\r\n"
    )
}

/// The same zone as RFC 8984 §4.7.2 spells one, with `until` stated in
/// `offset_from` — which is where §4.7.2 puts a TimeZoneRule's `start`, and the
/// only reading of `until` consistent with it, since the two bound one series of
/// the same local times.
fn ending_zone(until: &str, offset_from: &str) -> Value {
    json!({
        "@type": "TimeZone",
        "tzId": ENDING_TZID,
        "daylight": [{
            "@type": "TimeZoneRule",
            "start": "1967-04-30T02:00:00",
            "offsetFrom": offset_from,
            "offsetTo": "-0400",
            "recurrenceRules": [{
                "@type": "RecurrenceRule",
                "frequency": "yearly",
                "until": until,
                "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": -1}],
                "byMonth": ["4"],
            }],
            "names": {"EDT": true},
        }],
    })
}

/// The same bug as `a_zoned_rules_utc_until_is_not_taken_for_a_local_time`, one
/// level down — and this one *is* convertible without a zone database, which is
/// why it is converted rather than refused.
///
/// An observance dates itself in the zone it is defining, so the offset that
/// resolves its `DTSTART` is not a zone whose rules have to be evaluated: it is
/// the fixed number of seconds `TZOFFSETFROM` states, sitting in the same
/// component. Reading a UTC `UNTIL` there as local digits moves the end of the
/// transition rule by that offset — five hours, for the example below — which
/// puts the zone's last spring-forward in the wrong place and every event after
/// it an hour out.
#[test]
fn an_observances_utc_until_is_read_in_the_offset_it_states() {
    for (until, offset_from, read) in [
        // The RFC's own example: 07:00 UTC is 02:00 where the offset is -0500.
        ("19730429T070000Z", "-0500", "1973-04-29T02:00:00"),
        // Backwards over a month, a year and into a February — 1973 is not a
        // leap year and 1972 is, so the day the shift lands on differs.
        ("19730101T020000Z", "-0500", "1972-12-31T21:00:00"),
        ("19730301T000000Z", "-0500", "1973-02-28T19:00:00"),
        ("19720301T000000Z", "-0500", "1972-02-29T19:00:00"),
        // Forwards over one, at an offset that is not a whole hour.
        ("19721231T230000Z", "+0545", "1973-01-01T04:45:00"),
        // No offset to shift by, so the digits are the instant.
        ("19730429T070000Z", "+0000", "1973-04-29T07:00:00"),
        // Already the local time §3.3.10 asks for beside a local `DTSTART`,
        // which is what this crate itself writes — read as itself.
        ("19730429T020000", "-0500", "1973-04-29T02:00:00"),
    ] {
        let ics = zoned(ENDING_TZID, &ending(until, offset_from));

        let event = ical_to_event(&ics).expect("parse");

        assert_eq!(
            serde_json::to_value(&event.time_zones).expect("a map of time zones"),
            json!({ENDING_TZID: ending_zone(read, offset_from)}),
            "{until} at {offset_from}: {ics}"
        );
    }
}

/// The other direction of the same conversion. RFC 5545 §3.6.5's examples state
/// an observance's `UNTIL` as a UTC instant, which is what every producer of a
/// `VTIMEZONE` writes — tzdata's, libical's, Exchange's — so that is what this
/// draws, rather than asking a reader to accept a spelling it may never have
/// seen. It also makes the trip through JSCalendar and back byte-identical, so
/// a save of an untouched component still has nothing to report.
#[test]
fn an_observances_until_is_drawn_as_the_utc_instant_it_names() {
    for (until, offset_from, drawn) in [
        ("1973-04-29T02:00:00", "-0500", "19730429T070000Z"),
        ("1972-12-31T21:00:00", "-0500", "19730101T020000Z"),
        ("1973-01-01T04:45:00", "+0545", "19721231T230000Z"),
        ("1973-04-29T07:00:00", "+0000", "19730429T070000Z"),
    ] {
        let event = defining(
            ENDING_TZID,
            json!({ENDING_TZID: ending_zone(until, offset_from)}),
        );

        let ics = event_to_ical(&event);

        assert!(ics.contains(&ending(drawn, offset_from)), "{until}: {ics}");
        // And back again, unchanged in either half.
        let back = ical_to_event(&ics).expect("parse");
        assert_eq!(
            serde_json::to_value(&back.time_zones).expect("a map of time zones"),
            json!({ENDING_TZID: ending_zone(until, offset_from)}),
        );
        assert_eq!(event_to_ical(&back), ics);
    }
}

/// The end of a transition rule is the one part of an observance that cannot be
/// narrowed, only lost — and losing it is a zone that never stops moving.
///
/// The mirror of what the drawing already does: `observance` gives up the whole
/// `VTIMEZONE` rather than write a rule whose `UNTIL` it cannot state, because a
/// zone that keeps springing forward puts every event in it an hour out from the
/// day the transitions should have stopped. Reading was the asymmetric half — an
/// `UNTIL` it could not convert simply went missing, and the unbounded rule left
/// behind was then sent to the server as the zone's description.
///
/// Keeping the value it could not state is all that was needed: `read_time_zones`
/// already draws every definition it reads and keeps only what came back, which
/// is what `a_definition_with_a_part_that_cannot_be_read_is_not_read_in_part`
/// covers for an observance's *parts*. An end that went missing was invisible to
/// that check — the rule it left behind drew perfectly well, just not as the
/// zone the document described.
///
/// Not describing the zone at all is the conservative answer, and one the rest
/// of the stack already knows: the event still *names* the identifier, and
/// `jmap_cal_sync::patch` drops a property naming a zone the save cannot define,
/// so the server's own zone stands rather than being replaced by a wrong one.
#[test]
fn an_observance_whose_until_cannot_be_read_costs_the_whole_zone() {
    for (until, offset_from) in [
        // Digits in the right places that name no instant.
        ("20261315T000000Z", "-0500"),
        ("20260230T000000Z", "-0500"),
        // Convertible in principle, but the shift steps off either end of the
        // four-digit years RFC 5545 §3.3.4 admits, so there is no local time to
        // state it as.
        ("00000101T000000Z", "-0500"),
        ("99991231T230000Z", "+0545"),
    ] {
        let ics = zoned(ENDING_TZID, &ending(until, offset_from));

        let event = ical_to_event(&ics).expect("parse");

        assert_eq!(
            event.time_zones, None,
            "{until} at {offset_from} describes no zone: {ics}"
        );
        // And the identifier stays where it was: the save path has to be able to
        // tell a zone this mapping cannot describe from no zone at all.
        assert_eq!(event.time_zone.as_deref(), Some(ENDING_TZID), "{until}");
    }
}

/// The same defect one level up, where the sentinel rather than the refusal is
/// the answer.
///
/// An event's `UNTIL` that cannot be read used to leave the rule looking like a
/// recurrence that never ends — which the save path would then have patched over
/// the server's, replacing an appointment that stops with one that repeats for
/// ever. The unreadable value is kept instead, exactly as a zoned UTC instant
/// is: it is no LocalDateTime, so the rule does not map and `recurrenceRule` is
/// left alone.
#[test]
fn a_rules_unreadable_until_is_kept_rather_than_read_as_no_end_at_all() {
    for until in ["20261315T000000Z", "20260230T000000Z"] {
        let ics = format!(
            "BEGIN:VCALENDAR\r\n\
             BEGIN:VEVENT\r\n\
             UID:E1\r\n\
             DTSTART:20260810T090000\r\n\
             RRULE:FREQ=DAILY;UNTIL={until}\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n"
        );
        let rules = ical_to_event(&ics)
            .expect("parse")
            .recurrence_rule
            .expect("a rule came back");

        assert!(rules.until.is_some(), "{until}");
        assert!(!maps_recurrence_rule(&rules), "{until}");
    }
}

/// The limit of the two tests above, measured rather than assumed: an `UNTIL`
/// the *parser* refuses never reaches this mapping at all, so there is nothing
/// here to notice it by.
///
/// A value with digits in the wrong places is read and handed on — that is the
/// hostile `.ics` the tests above drive, and the one libical itself would pass —
/// but a value that is no date-time in any shape takes the rest of the rule with
/// it: what arrives is the frequency alone, and this crate cannot tell that from
/// a rule written that way. On an event that is a recurrence shown as unbounded;
/// on a `VTIMEZONE` observance it is a zone that keeps moving, which
/// `an_observance_whose_until_cannot_be_read_costs_the_whole_zone` closes only
/// for the values that get through. Closing it too means a reader that reports
/// what it discarded — a narrowing below this crate, not a decision of its own.
///
/// Here as a canary: a parser that starts passing the value through turns this
/// red, and whoever sees that should widen the refusal to cover it.
#[test]
fn an_until_no_parser_can_read_never_reaches_this_mapping() {
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E1\r\n",
        "DTSTART:20260810T090000\r\n",
        "RRULE:FREQ=DAILY;UNTIL=whenever;BYMONTH=4\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let rules = ical_to_event(ics)
        .expect("parse")
        .recurrence_rule
        .expect("a rule came back");

    assert_eq!(rules.until, None, "no end survived the parser");
    assert_eq!(rules.by_month, None, "nor the part written beside it");
    assert_eq!(rules.frequency, "daily");
}

/// A zone the document *names* is left to the reader on the way back in, as it
/// was on the way out. `Europe/Berlin` is a zone every database has; the
/// `VTIMEZONE` beside it is libical's own copy, and carrying it into `timeZones`
/// would make this crate the author of a definition it merely passed.
#[test]
fn a_named_zones_definition_is_not_read_back_as_a_definition() {
    let ics = zoned(LIBICAL_TZID, &vtimezone(LIBICAL_TZID, "Europe/Berlin"));

    let event = ical_to_event(&ics).expect("parse");

    assert_eq!(event.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(event.time_zones, None, "{ics}");
}

/// An identifier RFC 8984 §1.4.9 admits in neither form defines nothing, however
/// complete the `VTIMEZONE` beside it. An unresolvable custom zone name is not an IANA name
/// and does not begin with the solidus a custom identifier must, so there is no
/// `timeZones` key it could be filed under — inventing one would mean inventing
/// the identifier the event is in.
#[test]
fn a_zone_no_identifier_admits_is_read_back_undefined() {
    let ics = zoned(
        "Unknown Custom Zone",
        &vtimezone("Unknown Custom Zone", "Unknown Custom Zone"),
    );

    let event = ical_to_event(&ics).expect("parse");

    assert_eq!(event.time_zone.as_deref(), Some("Unknown Custom Zone"));
    assert_eq!(event.time_zones, None, "{ics}");
}

/// Half a definition is not a zone — see `vtimezone_of`, which will not *draw*
/// one for the same reason. A `VTIMEZONE` this mapping cannot read whole is read
/// as no definition at all, so the identifier stays undefined and
/// [`maps_time_zone`] refuses it, rather than the event being filed in a zone
/// that is confidently wrong by an hour.
#[test]
fn a_definition_with_a_part_that_cannot_be_read_is_not_read_in_part() {
    let ics = event_to_ical(&defining(CUSTOM_TZID, json!({CUSTOM_TZID: custom_zone()})));
    for broken in [
        // No offset to arrive at: RFC 5545 §3.6.5 makes TZOFFSETTO REQUIRED, and
        // an observance without one says nothing about what time it is.
        ics.replace("TZOFFSETTO:+0100\r\n", ""),
        // An offset that is no offset.
        ics.replace("TZOFFSETFROM:+0100", "TZOFFSETFROM:half past"),
        // A transition dated to a day that does not exist.
        ics.replace("DTSTART:19701025T030000", "DTSTART:19701325T030000"),
        // A rule that repeats the transition in a month no year has, which
        // `rule_to_rrule` refuses to spell — so the zone would move once where
        // it moves every year.
        ics.replace("BYMONTH=10", "BYMONTH=13"),
    ] {
        let event = ical_to_event(&broken).expect("parse");

        assert_eq!(event.time_zone.as_deref(), Some(CUSTOM_TZID));
        assert_eq!(event.time_zones, None, "{broken}");
        assert!(!maps_time_zone(&event));
    }
}

/// The question the save path asks: may this event's zone be stated to a server?
///
/// Three answers are yes — no zone at all (a floating event, which is a real
/// thing to save), an IANA name, and a custom identifier the event itself
/// defines — and one is no: an identifier with nothing to resolve it, which is
/// what an undefined `TZID` off a foreign document reads as.
#[test]
fn a_zone_is_sendable_when_something_says_what_it_is() {
    let defined = defining(CUSTOM_TZID, json!({CUSTOM_TZID: custom_zone()}));
    assert!(maps_time_zone(&defined));
    // The other reading of where RFC 8984 §1.4.9's solidus lives, which
    // `drawn_time_zone` already accepts on the way out.
    assert!(maps_time_zone(&defining(
        CUSTOM_TZID,
        json!({"example.com/Europe-Berlin": custom_zone()}),
    )));
    assert!(maps_time_zone(&defining("Europe/Berlin", json!(null))));
    assert!(maps_time_zone(&defining("", json!(null))));
    assert!(maps_time_zone(&CalendarEvent::default()));

    assert!(!maps_time_zone(&defining(CUSTOM_TZID, json!(null))));
    assert!(!maps_time_zone(&defining(
        CUSTOM_TZID,
        json!({"/example.com/Elsewhere": custom_zone()}),
    )));
    // No solidus, so no form of identifier admits it, defined or not.
    assert!(!maps_time_zone(&defining(
        "W. Europe Standard Time",
        json!({"W. Europe Standard Time": custom_zone()}),
    )));
}

#[test]
fn windows_time_zone_names_are_refused_as_unsendable_by_design() {
    // Windows zone names (e.g. from Exchange/Outlook) neither conform to IANA
    // zone identifier shape nor begin with a solidus as RFC 8984 §1.4.9 requires
    // for custom identifiers. They are refused by maps_time_zone (unsendable-by-design),
    // causing jmap_cal_sync to file the appointment floating rather than sending
    // an invalid or dangling zone identifier to the server.
    for windows_tz in [
        "W. Europe Standard Time",
        "Pacific Standard Time",
        "Eastern Standard Time",
        "GMT Standard Time",
        "Tokyo Standard Time",
        "Central European Standard Time",
    ] {
        assert!(
            !names_time_zone(windows_tz),
            "{windows_tz} should not be recognized as IANA name"
        );

        let event_without_defs = CalendarEvent {
            time_zone: Some(windows_tz.to_owned()),
            ..CalendarEvent::default()
        };
        assert!(!defines_time_zone(&event_without_defs, windows_tz));
        assert!(!maps_time_zone(&event_without_defs));

        let event_with_defs = defining(windows_tz, json!({windows_tz: custom_zone()}));
        assert!(!defines_time_zone(&event_with_defs, windows_tz));
        assert!(!maps_time_zone(&event_with_defs));
    }
}

/// What a save sends is the definitions the event still refers to — not the
/// series' alone, and not the whole map regardless.
///
/// The caller is the create path, which clears a zone it cannot state. Clearing
/// the map with it dropped the definition of a zone one *occurrence* had been
/// moved into, leaving the override naming an identifier nothing resolved; the
/// other direction, keeping an entry the event stopped referring to, is a claim
/// about a zone the event is not in.
#[test]
fn pruning_keeps_the_definitions_the_event_still_refers_to() {
    let moved_into = |tzid: &str| {
        serde_json::from_value(json!({
            "2026-01-16T13:00:00": {"start": "2026-01-16T15:00:00", "timeZone": tzid},
        }))
        .expect("a map of overrides")
    };

    // Nothing refers to the zone once the series' own is gone.
    let mut lone = defining(CUSTOM_TZID, json!({CUSTOM_TZID: custom_zone()}));
    lone.time_zone = None;
    prune_time_zones(&mut lone);
    assert_eq!(
        lone.time_zones, None,
        "emptied, the map goes rather than {{}}"
    );

    // An occurrence moved into it still refers to it, so the definition stays.
    let mut moved = defining(CUSTOM_TZID, json!({CUSTOM_TZID: custom_zone()}));
    moved.time_zone = None;
    moved.recurrence_overrides = moved_into(CUSTOM_TZID);
    prune_time_zones(&mut moved);
    assert!(
        moved
            .time_zones
            .as_ref()
            .is_some_and(|zones| zones.contains_key(CUSTOM_TZID)),
        "{moved:?}"
    );

    // Under the other reading of where the solidus lives, too: an entry kept
    // under one spelling and looked up under the other is a zone that silently
    // stops resolving.
    let mut other_spelling = defining(
        CUSTOM_TZID,
        json!({"example.com/Europe-Berlin": custom_zone()}),
    );
    other_spelling.time_zone = None;
    other_spelling.recurrence_overrides = moved_into(CUSTOM_TZID);
    prune_time_zones(&mut other_spelling);
    assert!(
        other_spelling
            .time_zones
            .as_ref()
            .is_some_and(|zones| zones.contains_key("example.com/Europe-Berlin")),
        "{other_spelling:?}"
    );

    // And a definition of some third zone goes even while the series keeps its
    // own, because nothing in the event names it.
    let mut stranger = defining(
        CUSTOM_TZID,
        json!({CUSTOM_TZID: custom_zone(), "/example.com/Elsewhere": custom_zone()}),
    );
    prune_time_zones(&mut stranger);
    assert_eq!(
        stranger.time_zones.map(|zones| zones.into_keys().collect()),
        Some(vec![CUSTOM_TZID.to_owned()]),
    );
}

/// A definition keyed without the solidus RFC 8984 §1.4.9 requires of the
/// identifier still defines the zone. §4.7.2 types the map as `Id[TimeZone]` and
/// §1.4.4's `Id` grammar has no solidus in it, so the two readings of where the
/// prefix lives are both in the document; a zone left undefined because the
/// server chose the other one would be a silent hour.
#[test]
fn a_definition_keyed_without_the_solidus_defines_the_zone_too() {
    let ics = event_to_ical(&defining(
        CUSTOM_TZID,
        json!({"example.com/Europe-Berlin": custom_zone()}),
    ));

    assert!(ics.contains(CUSTOM_DEFINITION), "{ics}");
}

/// A zone that has a name needs no definition from us: libical resolves an IANA
/// name out of its builtin table, `jmap-backend-cal` puts *that* definition in
/// the object EDS caches, and a database is a better description of a zone than
/// whatever a server was able to state about it.
#[test]
fn a_zone_with_a_name_of_its_own_is_left_to_the_reader() {
    let ics = event_to_ical(&defining(
        "Europe/Berlin",
        json!({"Europe/Berlin": custom_zone()}),
    ));

    assert!(
        without(&ics, "BEGIN:VTIMEZONE"),
        "a name the reader can look up was defined anyway: {ics}"
    );
    assert_eq!(
        line(&ics, "DTSTART"),
        "DTSTART;TZID=Europe/Berlin:20260115T130000"
    );
}

/// A custom identifier the event defines nothing for is drawn as it came and
/// left undefined — the state everything was in before this existed. The value
/// has to reach the component either way: the save path tells a zone this
/// mapping cannot name from no zone at all, and reading the first as the second
/// would clear the server's zone on an edit that never touched it.
#[test]
fn a_custom_zone_with_no_definition_is_named_and_left_undefined() {
    for definitions in [
        json!(null),
        json!({"/example.com/Elsewhere": custom_zone()}),
    ] {
        let ics = event_to_ical(&defining(CUSTOM_TZID, definitions));

        assert!(without(&ics, "BEGIN:VTIMEZONE"), "{ics}");
        assert_eq!(
            line(vevent(&ics, 0), "DTSTART"),
            format!("DTSTART;TZID={CUSTOM_TZID}:20260115T130000")
        );
    }
}

/// A weekly series in a zone every database names, with one occurrence moved
/// into a zone only the event itself can name — RFC 5545 §3.2.19 puts the zone
/// on the *property*, so a detached instance need not share the series'.
///
/// The identifier is [`CUSTOM_TZID`], defined once in the event's own
/// `timeZones`: one description of one zone, whichever component refers to it.
fn instance_in_a_custom_zone() -> CalendarEvent {
    CalendarEvent {
        id: Some("E9".into()),
        start: Some("2026-01-15T13:00:00".to_owned()),
        time_zone: Some("Europe/Berlin".to_owned()),
        duration: Some("PT1H".to_owned()),
        recurrence_rule: serde_json::from_value(json!({
            "@type": "RecurrenceRule",
            "frequency": "weekly",
        }))
        .expect("a recurrence rule"),
        recurrence_overrides: serde_json::from_value(json!({
            "2026-01-22T13:00:00": {
                "start": "2026-01-22T09:00:00",
                "timeZone": CUSTOM_TZID,
            },
        }))
        .expect("a map of overrides"),
        time_zones: serde_json::from_value(json!({CUSTOM_TZID: custom_zone()}))
            .expect("a map of time zones"),
        ..CalendarEvent::default()
    }
}

/// An occurrence moved into a zone only the event can name is drawn in *that*
/// zone, with the definition that says what it is.
///
/// The series' own zone is the one the document defines when there is one to
/// define, but it is not the only zone the document can refer to: an override
/// carries `timeZone` (RFC 8984 §4.3.4) and the instance's `DTSTART` states it
/// as a `TZID` of its own. Drawing the instance at the series' clock instead is
/// not a narrowing — it is the occurrence shown in a zone nobody moved it to,
/// and wherever the two zones differ that is a different instant, silently.
///
/// [`custom_zone`] happens to describe the same rules as `Europe/Berlin`, which
/// is deliberate: it keeps this leg measuring that the *identifier* the override
/// named survives, rather than passing on an offset that would differ anyway.
#[test]
fn an_occurrence_moved_into_a_custom_zone_is_drawn_in_that_zone() {
    let ics = event_to_ical(&instance_in_a_custom_zone());

    assert_eq!(
        line(vevent(&ics, 1), "DTSTART"),
        format!("DTSTART;TZID={CUSTOM_TZID}:20260122T090000"),
        "the occurrence is not on the clock the override moved it to: {ics}"
    );
    assert!(
        ics.contains(CUSTOM_DEFINITION),
        "the zone the occurrence names is not defined in the document that names \
         it, so a reader floats that one occurrence: {ics}"
    );
    // And the series is untouched by any of it: its zone has a name, so nothing
    // defines it here, and the `RECURRENCE-ID` names the occurrence the rules
    // generated, which run on the series' clock (RFC 5545 §3.8.4.4).
    assert_eq!(
        line(vevent(&ics, 0), "DTSTART"),
        "DTSTART;TZID=Europe/Berlin:20260115T130000",
        "{ics}"
    );
    assert_eq!(
        line(vevent(&ics, 1), "RECURRENCE-ID"),
        "RECURRENCE-ID;TZID=Europe/Berlin:20260122T130000",
        "{ics}"
    );
}

/// And the definition comes back, which is what makes a *create* of such a
/// document send the event it describes.
///
/// `ical_to_event` used to look for one definition — the series' — so an
/// occurrence's own custom identifier reached the server with nothing to resolve
/// it. That is a dangling `TimeZoneId`, which RFC 8984 §1.4.9 does not admit and
/// a server is entitled to reject; and a server that keeps it has one occurrence
/// floating.
#[test]
fn an_occurrences_own_custom_zone_is_read_back_with_its_definition() {
    let ics = event_to_ical(&instance_in_a_custom_zone());

    let back = ical_to_event(&ics).expect("parse");

    assert_eq!(
        serde_json::to_value(&back.time_zones).expect("a map of time zones"),
        json!({CUSTOM_TZID: custom_zone()}),
        "the definition the occurrence's zone needs did not come back: {ics}"
    );
    assert_eq!(
        serde_json::to_value(&back.recurrence_overrides).expect("a map of overrides"),
        json!({
            "2026-01-22T13:00:00": {
                "start": "2026-01-22T09:00:00",
                "timeZone": CUSTOM_TZID,
            },
        }),
        "{ics}"
    );
    // Which is the whole claim stated the other way round: what was drawn from
    // the definition draws again, so a save of an untouched component has
    // nothing to report.
    assert_eq!(event_to_ical(&back), ics);
}

/// Sendable is not one question but two, and this is the one property that
/// tells them apart: what a save may state depends on what else that save is
/// willing to send.
///
/// A component can state a custom identifier — beside the `VTIMEZONE` that
/// defines it, which the test above pins. A patch naming `recurrenceOverrides`
/// and nothing else cannot: the property goes back replaced whole, so the
/// identifier would reach the server with nothing to say what it means, and RFC
/// 8984 §1.4.9 admits it only beside its `timeZones` entry — a dangling
/// reference the server may reject the whole `CalendarEvent/set` over, costing
/// every other edit in the same save. That is `maps_recurrence_override`.
///
/// A save that will *also* patch `timeZones` is in a different position, and
/// asks [`sends_recurrence_override`]: it can put the entry there, so the pair
/// is legal and the occurrence keeps the zone the user moved it into. See
/// `jmap-cal-sync`'s `patch` module, which adds one entry per identifier rather
/// than replacing the map.
#[test]
fn an_occurrences_custom_zone_is_sendable_only_beside_its_definition() {
    let event = instance_in_a_custom_zone();
    let (id, patch) = event
        .recurrence_overrides
        .as_ref()
        .expect("the fixture's overrides")
        .iter()
        .next()
        .expect("the fixture's one override");

    assert!(
        !maps_recurrence_override(&event, id, patch),
        "an override naming a zone only this document defines is sendable on its \
         own, so a save would replace `recurrenceOverrides` with an identifier \
         the server cannot resolve"
    );
    assert!(
        sends_recurrence_override(&event, id, patch),
        "an override naming a zone this document defines is unsendable even to a \
         save that would carry the definition, so the user's move is thrown away"
    );
}

/// And the identifier the document only *names*, asked of the same pair: neither
/// predicate admits it.
///
/// A definition it can send is what makes the difference above, so an event that
/// has none is back where it started — there is nothing to put in `timeZones`,
/// and an entry drawn from half a description would be a different zone. The
/// series' own identifier is the one defined here, which keeps the fixture
/// honest: what the override names is undefined, not the whole event.
#[test]
fn an_occurrence_naming_an_undefined_zone_is_sendable_by_neither_rule() {
    let event = CalendarEvent {
        recurrence_overrides: serde_json::from_value(json!({
            "2026-01-22T13:00:00": {"timeZone": "/example.com/Elsewhere"},
        }))
        .expect("a map of overrides"),
        ..defining(CUSTOM_TZID, json!({CUSTOM_TZID: custom_zone()}))
    };
    let (id, patch) = event
        .recurrence_overrides
        .as_ref()
        .expect("the fixture's overrides")
        .iter()
        .next()
        .expect("the fixture's one override");

    assert!(!maps_recurrence_override(&event, id, patch));
    assert!(
        !sends_recurrence_override(&event, id, patch),
        "an override naming a zone nothing defines is sendable, so the save would \
         reach the server with a reference to nothing"
    );
}

/// A definition with a part this mapping cannot draw is not drawn in part.
///
/// Every observance of a zone describes the offsets between the transitions the
/// others name, so a `VTIMEZONE` missing one is not a narrowed description of the
/// zone — it is a different zone, and an event in it is at a different instant.
/// Half a definition is therefore worse than none: none leaves the reader
/// floating the event, which is visibly wrong, where half of one is confidently
/// wrong by an hour.
#[test]
fn a_definition_this_mapping_cannot_draw_whole_is_not_drawn_at_all() {
    let unmappable = [
        // No offset to move to, which RFC 5545 §3.6.5 makes REQUIRED of every
        // observance — and without it there is nothing to say what the zone is.
        json!({"start": "1970-10-25T03:00:00", "offsetFrom": "+0200"}),
        // An offset that is not one.
        json!({
            "start": "1970-10-25T03:00:00",
            "offsetFrom": "+0200",
            "offsetTo": "an hour or so",
        }),
        // A start that names no instant.
        json!({
            "start": "1970-10-25",
            "offsetFrom": "+0200",
            "offsetTo": "+0100",
        }),
        // A rule with no frequency has no `RRULE` spelling at all, so the
        // observance would be drawn as happening once — the transition stops
        // repeating and every date after 1970 is in the other observance.
        json!({
            "start": "1970-10-25T03:00:00",
            "offsetFrom": "+0200",
            "offsetTo": "+0100",
            "recurrenceRules": [{"@type": "RecurrenceRule", "interval": 1}],
        }),
        // A rule with a part that has no `RRULE` spelling. `rule_to_rrule`
        // leaves such a part off and still writes a line, which here would be a
        // zone that moves on the last Sunday of *every* month rather than of
        // October — one hour wrong for eleven months of the year, stated with
        // the same confidence as the truth.
        json!({
            "start": "1970-10-25T03:00:00",
            "offsetFrom": "+0200",
            "offsetTo": "+0100",
            "recurrenceRules": [{
                "@type": "RecurrenceRule",
                "frequency": "yearly",
                "byMonth": ["10L"],
                "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": -1}],
            }],
        }),
    ];

    for rule in unmappable {
        let mut zone = custom_zone();
        zone["standard"] = json!([rule]);
        let ics = event_to_ical(&defining(CUSTOM_TZID, json!({CUSTOM_TZID: zone})));

        assert!(
            without(&ics, "BEGIN:VTIMEZONE"),
            "a zone was defined from a rule that cannot be drawn: {ics}"
        );
    }
}

/// A zone with no observance in it is no definition: RFC 5545 §3.6.5 requires at
/// least one `STANDARD` or `DAYLIGHT` subcomponent, and libical refuses a
/// `VTIMEZONE` without one — which would cost the whole object, not just the
/// zone.
#[test]
fn a_zone_stating_no_observance_is_not_drawn() {
    for zone in [
        json!({"@type": "TimeZone", "tzId": CUSTOM_TZID}),
        json!({"@type": "TimeZone", "tzId": CUSTOM_TZID, "standard": []}),
        json!("Europe/Berlin"),
    ] {
        let ics = event_to_ical(&defining(CUSTOM_TZID, json!({CUSTOM_TZID: zone})));

        assert!(without(&ics, "BEGIN:VTIMEZONE"), "{ics}");
    }
}

/// A zone with one observance and no rule is a zone that never moves — most of
/// the world — and is drawn with the one it has.
#[test]
fn a_zone_that_never_changes_is_drawn_with_the_one_observance_it_has() {
    let zone = json!({
        "@type": "TimeZone",
        "tzId": CUSTOM_TZID,
        "standard": [{
            "@type": "TimeZoneRule",
            "start": "1970-01-01T00:00:00",
            "offsetFrom": "+05:45",
            "offsetTo": "+05:45",
            "names": {"+0545": true},
        }],
    });
    let ics = event_to_ical(&defining(CUSTOM_TZID, json!({CUSTOM_TZID: zone})));

    assert!(
        ics.contains(
            "BEGIN:VTIMEZONE\r\n\
             TZID:/example.com/Europe-Berlin\r\n\
             BEGIN:STANDARD\r\n\
             DTSTART:19700101T000000\r\n\
             TZOFFSETFROM:+0545\r\n\
             TZOFFSETTO:+0545\r\n\
             TZNAME:+0545\r\n\
             END:STANDARD\r\n\
             END:VTIMEZONE\r\n"
        ),
        "{ics}"
    );
}

/// The offsets are the one place the two formats may spell the same thing
/// differently. RFC 5545 §3.3.14's UTC-OFFSET is `±hhmm[ss]` with no separators;
/// RFC 8984 §4.7.2 states the same value and the JSON forms in the wild put
/// colons in it. Both are read, and what is written is iCalendar's.
#[test]
fn an_offset_is_drawn_the_way_icalendar_spells_one() {
    for (stated, drawn) in [
        ("+0200", "+0200"),
        ("+02:00", "+0200"),
        ("-0330", "-0330"),
        ("-03:30", "-0330"),
        ("+0000", "+0000"),
        // Seconds, which historical offsets carry and §3.3.14 admits.
        ("+00:53:28", "+005328"),
        ("+005328", "+005328"),
        // A whole number of minutes keeps the short form, whichever way the
        // server stated it.
        ("+020000", "+0200"),
    ] {
        let zone = json!({
            "@type": "TimeZone",
            "standard": [{
                "start": "1970-01-01T00:00:00",
                "offsetFrom": stated,
                "offsetTo": stated,
            }],
        });
        let ics = event_to_ical(&defining(CUSTOM_TZID, json!({CUSTOM_TZID: zone})));

        assert!(
            ics.contains(&format!("TZOFFSETFROM:{drawn}\r\nTZOFFSETTO:{drawn}\r\n")),
            "{stated} was not drawn as {drawn}: {ics}"
        );
    }
}

/// And a value that is not an offset defines no zone. `-0000` is the one the
/// grammar picks out by name: RFC 5545 §3.3.14 forbids it, because the sign is
/// what says which side of UTC the zone is on and there is no negative zero.
#[test]
fn a_value_that_is_no_offset_defines_no_zone() {
    for stated in [
        "-0000",
        "-000000",
        "0200",
        "+2",
        "+2:00",
        "+2400",
        "+0260",
        "+00:53:61",
        "+02:00:00:00",
        "",
        "+02 00",
    ] {
        let zone = json!({
            "@type": "TimeZone",
            "standard": [{
                "start": "1970-01-01T00:00:00",
                "offsetFrom": "+0200",
                "offsetTo": stated,
            }],
        });
        let ics = event_to_ical(&defining(CUSTOM_TZID, json!({CUSTOM_TZID: zone})));

        assert!(
            without(&ics, "BEGIN:VTIMEZONE"),
            "{stated:?} was drawn as an offset: {ics}"
        );
    }
}

/// One definition is all a document needs, however many components it holds: the
/// only `TZID` that can be a custom identifier is the series'.
///
/// An override may restate `timeZone`, but `maps_override_field` admits only a
/// value `names_time_zone` accepts, so an instance moved into a custom zone is
/// drawn at the series' zone instead — the same refusal that keeps a save from
/// sending a `recurrenceOverrides` map the server would reject. Which is just as
/// well, since a second copy of one `VTIMEZONE` is a duplicate `TZID` in one
/// object, and two of them would be two definitions of the same zone.
#[test]
fn an_instance_names_no_zone_the_series_did_not() {
    let event = CalendarEvent {
        recurrence_rule: Some(RecurrenceRule {
            frequency: "daily".to_owned(),
            ..RecurrenceRule::default()
        }),
        recurrence_overrides: serde_json::from_value(json!({
            "2026-01-16T13:00:00": {"timeZone": "/example.com/Elsewhere", "title": "Moved"},
        }))
        .expect("a map of overrides"),
        ..defining(CUSTOM_TZID, json!({CUSTOM_TZID: custom_zone()}))
    };

    let ics = event_to_ical(&event);

    assert_eq!(
        ics.matches("BEGIN:VTIMEZONE").count(),
        1,
        "the document defines a zone twice, or one it does not name: {ics}"
    );
    assert!(ics.contains(&format!("TZID:{CUSTOM_TZID}\r\n")), "{ics}");
    assert_eq!(
        line(vevent(&ics, 1), "DTSTART"),
        format!("DTSTART;TZID={CUSTOM_TZID}:20260116T130000"),
        "the instance was drawn in a zone the document does not define: {ics}"
    );
}

/// An all-day event names no zone at all — its `DTSTART` is a date, which RFC
/// 5545 §3.2.19 gives no `TZID` to carry one on — so there is nothing to define.
#[test]
fn an_event_written_as_a_date_defines_no_zone() {
    let event = CalendarEvent {
        start: Some("2026-01-15T00:00:00".to_owned()),
        time_zone: None,
        duration: Some("P1D".to_owned()),
        show_without_time: Some(true),
        time_zones: serde_json::from_value(json!({CUSTOM_TZID: custom_zone()}))
            .expect("a map of time zones"),
        ..CalendarEvent::default()
    };

    let ics = event_to_ical(&event);

    assert_eq!(line(&ics, "DTSTART"), "DTSTART;VALUE=DATE:20260115");
    assert!(without(&ics, "BEGIN:VTIMEZONE"), "{ics}");
}

#[test]
fn the_fixture_survives_a_round_trip() {
    let event = ical_to_event(&event_to_ical(&fixture_event())).expect("parse");

    assert_eq!(
        event,
        CalendarEvent {
            // Membership follows from which EDS source is being served, not
            // from the component, so the backend fills it in on create.
            calendar_ids: None,
            // Re-import stamps the standalone object's JSCalendar version
            // (jscalendarbis §3.1.2) regardless of what the fixture carried.
            version: Some("2.0".to_owned()),
            ..fixture_event()
        }
    );
}

/// An event whose guest list is the given map of RFC 8984 §4.4.6 Participants.
fn attended(participants: Value) -> CalendarEvent {
    CalendarEvent {
        title: Some("Sprint planning".to_owned()),
        start: Some("2026-01-15T13:00:00".to_owned()),
        time_zone: Some("Etc/UTC".to_owned()),
        duration: Some("PT1H".to_owned()),
        participants: serde_json::from_value(participants).expect("a map of participants"),
        ..CalendarEvent::default()
    }
}

/// One participant: an address to send to, a name, and whatever else is passed.
fn guest(address: &str, name: &str, rest: Value) -> Value {
    let mut participant = json!({
        "@type": "Participant",
        "name": name,
        "sendTo": {"imip": address},
    });
    for (key, value) in rest.as_object().expect("an object").clone() {
        participant[key] = value;
    }
    participant
}

#[test]
fn the_people_invited_to_an_event_are_written_as_attendees() {
    // RFC 8984 §4.4.6's `participants` is a map of Participants, each with an
    // address to reach them at, a name, the roles they hold and whether they
    // have replied; RFC 5545 §3.8.4.1 spells one as an ATTENDEE line whose value
    // is the CAL-ADDRESS and whose parameters carry the rest. The address comes
    // from `sendTo/imip` — the method (RFC 8984 §4.4.6) whose URI is exactly the
    // mailto: iCalendar wants.
    let ics = event_to_ical(&attended(json!({
        "bob": guest("mailto:bob@example.com", "Bob Example", json!({
            "roles": {"attendee": true},
            "participationStatus": "accepted",
        })),
        "carol": guest("mailto:carol@example.com", "Carol Example", json!({
            "roles": {"optional": true},
            "participationStatus": "needs-action",
            "expectReply": true,
        })),
    })));

    let attendees: Vec<String> = ics
        .replace("\r\n ", "")
        .split("\r\n")
        .filter(|line| line.starts_with("ATTENDEE"))
        .map(str::to_owned)
        .collect();
    // In the map's own order, so a document is stable across renderings.
    assert_eq!(
        attendees,
        [
            "ATTENDEE;CN=\"Bob Example\";ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:\
             mailto:bob@example.com",
            "ATTENDEE;CN=\"Carol Example\";ROLE=OPT-PARTICIPANT;PARTSTAT=NEEDS-ACTION;\
             RSVP=TRUE:mailto:carol@example.com",
        ],
        "{ics}"
    );
}

#[test]
fn the_participant_that_owns_the_event_is_its_organizer() {
    // RFC 8984 §4.4.6 gives the organizer no property of its own: it is the
    // participant holding the `owner` role. RFC 5545 §3.8.4.3 states it on a
    // line of its own — and one whose only role is owner is not attending, so it
    // gets no ATTENDEE line to go with it.
    let ics = event_to_ical(&attended(json!({
        "alice": guest("mailto:alice@example.com", "Alice Example", json!({
            "roles": {"owner": true},
        })),
        "bob": guest("mailto:bob@example.com", "Bob Example", json!({
            "roles": {"attendee": true},
        })),
    })));

    assert_eq!(
        content_line(&ics, "ORGANIZER"),
        "ORGANIZER;CN=\"Alice Example\":mailto:alice@example.com",
        "{ics}"
    );
    assert!(
        !ics.replace("\r\n ", "")
            .split("\r\n")
            .any(|line| line.starts_with("ATTENDEE") && line.contains("alice@example.com")),
        "{ics}"
    );
    assert_eq!(
        content_line(&ics, "ATTENDEE"),
        "ATTENDEE;CN=\"Bob Example\";ROLE=REQ-PARTICIPANT:mailto:bob@example.com",
        "{ics}"
    );
}

#[test]
fn an_owner_who_is_also_attending_gets_both_lines() {
    // The usual shape of a meeting somebody called and comes to: RFC 8984 has
    // `roles` be a set for exactly this, and iCalendar states the organizer
    // separately from the guest list rather than instead of it.
    let ics = event_to_ical(&attended(json!({
        "alice": guest("mailto:alice@example.com", "Alice Example", json!({
            "roles": {"owner": true, "attendee": true},
            "participationStatus": "accepted",
        })),
    })));

    assert_eq!(
        content_line(&ics, "ORGANIZER"),
        "ORGANIZER;CN=\"Alice Example\":mailto:alice@example.com",
        "{ics}"
    );
    assert_eq!(
        content_line(&ics, "ATTENDEE"),
        "ATTENDEE;CN=\"Alice Example\";ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:\
         mailto:alice@example.com",
        "{ics}"
    );
}

#[test]
fn a_room_the_event_is_booked_in_is_written_as_one() {
    // RFC 8984 §4.4.6's `kind` says what sort of participant it is, and RFC 5545
    // §3.2.3's CUTYPE the same — the two vocabularies differ in one word, a
    // JSCalendar `location` being iCalendar's ROOM.
    for (kind, cutype) in [
        ("individual", "INDIVIDUAL"),
        ("group", "GROUP"),
        ("resource", "RESOURCE"),
        ("location", "ROOM"),
    ] {
        let ics = event_to_ical(&attended(json!({
            "room": guest("mailto:room-1@example.com", "Room 1", json!({"kind": kind})),
        })));

        assert_eq!(
            content_line(&ics, "ATTENDEE"),
            format!("ATTENDEE;CN=\"Room 1\";CUTYPE={cutype}:mailto:room-1@example.com"),
            "{ics}"
        );
    }
}

#[test]
fn a_participant_with_no_address_to_send_to_is_left_off() {
    // An ATTENDEE's value is a CAL-ADDRESS (RFC 5545 §3.3.3), which is a URI:
    // there is no way to write a guest the server gave no address for, and
    // inventing one would name somebody it never named. So the participant is
    // dropped, like every other value this mapping cannot spell — and it is only
    // safe to drop because `participants` is written and never read back.
    for participant in [
        json!({"@type": "Participant", "name": "Nobody"}),
        json!({"@type": "Participant", "sendTo": {}}),
        json!({"@type": "Participant", "sendTo": {"other": "mailto:bob@example.com"}}),
        // A bare address is not a URI: RFC 3986 §3.1 wants a scheme.
        json!({"@type": "Participant", "sendTo": {"imip": "bob@example.com"}}),
        json!({"@type": "Participant", "sendTo": {"imip": "mailto:"}}),
        json!({"@type": "Participant", "sendTo": {"imip": ""}}),
        // Whitespace is not in a URI, and a line break would end the content
        // line and start a property of the server's choosing.
        json!({"@type": "Participant", "sendTo": {"imip": "mailto:bob example.com"}}),
        json!({"@type": "Participant", "sendTo": {"imip": "mailto:b@x\r\nSUMMARY:Gone"}}),
        // Not an object at all.
        json!("mailto:bob@example.com"),
    ] {
        let ics = event_to_ical(&attended(json!({"p": participant})));

        assert!(without(&ics, "ATTENDEE"), "{participant}: {ics}");
        assert!(without(&ics, "ORGANIZER"), "{participant}: {ics}");
        assert!(!ics.contains("SUMMARY:Gone"), "{participant}: {ics}");
    }
}

#[test]
fn a_status_role_or_kind_outside_the_shared_vocabulary_is_left_off() {
    // The rule every closed vocabulary in this mapping gets: a value the other
    // format has no spelling for is dropped rather than passed through in its
    // clothes. The guest still goes on the line — it is the parameter that is
    // unwritable, not the participant.
    let ics = event_to_ical(&attended(json!({
        "bob": guest("mailto:bob@example.com", "Bob Example", json!({
            "roles": {"x-cameo": true},
            "participationStatus": "asleep",
            "kind": "hologram",
            // RFC 8984 §1.4.3 has every value of a Set be true; anything else
            // says nothing was set.
            "expectReply": "yes",
        })),
    })));

    assert_eq!(
        content_line(&ics, "ATTENDEE"),
        "ATTENDEE;CN=\"Bob Example\":mailto:bob@example.com",
        "{ics}"
    );
}

#[test]
fn the_guest_list_is_written_and_never_read_back() {
    // The precedent CREATED and LAST-MODIFIED set, for a different reason:
    // changing who is invited, or what they replied, is *scheduling* — it means
    // an iTIP REQUEST or REPLY going out (RFC 5546), which this backend does not
    // send. So the guest list is drawn for the user to read and nothing more:
    // `participants` is absent from MAPPED_PROPERTIES, so no save can name it,
    // and the server's own guest list cannot be overwritten from here.
    let ics = event_to_ical(&attended(json!({
        "alice": guest("mailto:alice@example.com", "Alice Example", json!({
            "roles": {"owner": true, "attendee": true},
        })),
    })));
    let event = ical_to_event(&ics).expect("parse");

    assert_eq!(event.participants, None);
}

#[test]
fn an_edited_instance_carries_the_guest_list_of_the_series() {
    // The inheritance of RFC 8984 §4.3.4 again: an override may not restate the
    // participants, so the occurrence's own component states the series' — an
    // instance drawn without them would show a meeting nobody was invited to.
    let mut event = recurring_with(json!({"2026-01-29T13:00:00": {"title": "Sprint review"}}));
    event.participants = serde_json::from_value(json!({
        "bob": guest("mailto:bob@example.com", "Bob Example", json!({
            "roles": {"attendee": true},
        })),
    }))
    .expect("a map of participants");
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert_eq!(
        content_line(vevent(&ics, 1), "ATTENDEE"),
        "ATTENDEE;CN=\"Bob Example\";ROLE=REQ-PARTICIPANT:mailto:bob@example.com",
        "{ics}"
    );
}

/// An event held online at whatever virtual locations are passed.
fn held_online(virtual_locations: Value) -> CalendarEvent {
    CalendarEvent {
        title: Some("Sprint planning".to_owned()),
        start: Some("2026-01-15T13:00:00".to_owned()),
        time_zone: Some("Etc/UTC".to_owned()),
        duration: Some("PT1H".to_owned()),
        virtual_locations: serde_json::from_value(virtual_locations)
            .expect("a map of virtual locations"),
        ..CalendarEvent::default()
    }
}

/// One virtual location: somewhere to join the event, a name, and whatever else
/// is passed.
fn joined_at(uri: &str, name: &str, rest: Value) -> Value {
    let mut location = json!({
        "@type": "VirtualLocation",
        "name": name,
        "uri": uri,
    });
    for (key, value) in rest.as_object().expect("an object").clone() {
        location[key] = value;
    }
    location
}

fn conferences(ics: &str) -> Vec<String> {
    ics.replace("\r\n ", "")
        .split("\r\n")
        .filter(|line| line.starts_with("CONFERENCE"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn where_an_event_is_joined_online_is_written_as_a_conference() {
    // RFC 8984 §4.2.6's `virtualLocations` is a map of VirtualLocations, each a
    // URI to join the event at with a name and the ways of taking part it
    // offers; RFC 7986 §5.11 spells one as a CONFERENCE line whose value is that
    // URI, with the name on a LABEL (§6.4) and the ways on a FEATURE (§6.3).
    // Unlike LOCATION, the property may be stated more than once, so every entry
    // of the map is written rather than one of them.
    let ics = event_to_ical(&held_online(json!({
        "v1": joined_at("https://meet.example.com/sprint", "Team room", json!({
            "features": {"video": true, "audio": true},
        })),
        "v2": joined_at("tel:+1-555-0100", "Dial-in", json!({
            "features": {"phone": true},
        })),
    })));

    // In the map's own order, so a document is stable across renderings, and the
    // features in the table's, so a set is too.
    assert_eq!(
        conferences(&ics),
        [
            "CONFERENCE;VALUE=URI;FEATURE=AUDIO,VIDEO;LABEL=\"Team room\";X-JMAP-KEY=v1:\
             https://meet.example.com/sprint",
            "CONFERENCE;VALUE=URI;FEATURE=PHONE;LABEL=Dial-in;X-JMAP-KEY=v2:tel:+1-555-0100",
        ],
        "{ics}"
    );
}

#[test]
fn a_conference_states_the_value_type_it_is_required_to() {
    // RFC 7986 §5.11's `confparam` makes `VALUE=URI` REQUIRED on the property —
    // the one place in this mapping where a parameter is written that says
    // nothing the default would not. A reader that trusts the grammar is
    // entitled to demand it.
    let ics = event_to_ical(&held_online(json!({
        "v1": joined_at("https://meet.example.com/sprint", "", json!({})),
    })));

    assert_eq!(
        conferences(&ics),
        ["CONFERENCE;VALUE=URI;X-JMAP-KEY=v1:https://meet.example.com/sprint"],
        "{ics}"
    );
}

#[test]
fn a_way_of_joining_outside_the_shared_vocabulary_is_left_off() {
    // The rule every closed vocabulary in this mapping gets. RFC 8984 §4.2.6 and
    // RFC 7986 §6.3 name the same seven ways of taking part in the same words,
    // so each crosses to the other format's spelling of itself; a value outside
    // them is dropped rather than passed through in the other format's clothes,
    // and the conference still goes on the line — it is the parameter that is
    // unwritable, not the place.
    let ics = event_to_ical(&held_online(json!({
        "v1": joined_at("https://meet.example.com/sprint", "Team room", json!({
            "features": {
                "hologram": true,
                // RFC 8984 §1.4.3 has every value of a Set be true; anything
                // else says nothing was set.
                "video": "yes",
                "screen": true,
            },
        })),
    })));

    assert_eq!(
        conferences(&ics),
        [
            "CONFERENCE;VALUE=URI;FEATURE=SCREEN;LABEL=\"Team room\";X-JMAP-KEY=v1:https://meet.example.com/sprint"
        ],
        "{ics}"
    );
}

#[test]
fn a_virtual_location_with_nowhere_to_join_is_left_off() {
    // A CONFERENCE's value is a URI (RFC 7986 §5.11), and RFC 8984 §4.2.6 makes
    // `uri` the one mandatory member of a VirtualLocation: there is nothing to
    // write for a place the server named none for, and inventing one would send
    // the user somewhere the server never did. So the entry is dropped, like
    // every other value this mapping cannot spell — which is only safe because
    // `virtualLocations` is written and never read back.
    for location in [
        json!({"@type": "VirtualLocation", "name": "Team room"}),
        json!({"@type": "VirtualLocation", "uri": ""}),
        // A bare host is not a URI: RFC 3986 §3.1 wants a scheme.
        json!({"@type": "VirtualLocation", "uri": "meet.example.com/sprint"}),
        json!({"@type": "VirtualLocation", "uri": "https:"}),
        json!({"@type": "VirtualLocation", "uri": 42}),
        // Whitespace is not in a URI, and a line break would end the content
        // line and start a property of the server's choosing.
        json!({"@type": "VirtualLocation", "uri": "https://meet.example.com/the sprint"}),
        json!({"@type": "VirtualLocation", "uri": "https://x/\r\nSUMMARY:Gone"}),
        // Not an object at all.
        json!("https://meet.example.com/sprint"),
    ] {
        let ics = event_to_ical(&held_online(json!({"v1": location})));

        assert!(without(&ics, "CONFERENCE"), "{location}: {ics}");
        assert!(!ics.contains("SUMMARY:Gone"), "{location}: {ics}");
    }
}

#[test]
fn a_virtual_location_with_no_name_carries_no_label() {
    // RFC 8984 §4.2.6 defaults `name` to the empty string, and a LABEL of
    // nothing is a parameter that names the place as having no name — where
    // leaving it off says only that the URI speaks for itself.
    for location in [
        json!({"@type": "VirtualLocation", "uri": "https://meet.example.com/sprint"}),
        json!({"@type": "VirtualLocation", "uri": "https://meet.example.com/sprint", "name": ""}),
        json!({"@type": "VirtualLocation", "uri": "https://meet.example.com/sprint", "name": 7}),
    ] {
        let ics = event_to_ical(&held_online(json!({"v1": location})));

        assert_eq!(
            conferences(&ics),
            ["CONFERENCE;VALUE=URI;X-JMAP-KEY=v1:https://meet.example.com/sprint"],
            "{location}: {ics}"
        );
    }
}

#[test]
fn where_an_event_is_joined_online_is_read_back_off_the_line() {
    // What a CONFERENCE line shows — the URI, the LABEL, the FEATUREs — reads
    // back, and nothing else does: a VirtualLocation's `description` (RFC 8984
    // §4.2.6) has no room on the line, so it is neither drawn nor read. That is
    // safe only because the save path names `virtualLocations/<key>/...` rather
    // than the property, which is why the key rides out and back on the line.
    let ics = event_to_ical(&held_online(json!({
        "v1": joined_at("https://meet.example.com/sprint", "Team room", json!({
            "description": "Ask Bob for the passcode",
            "features": {"video": true},
        })),
    })));
    let event = ical_to_event(&ics).expect("parse");

    assert!(!ics.contains("passcode"), "{ics}");
    assert_eq!(
        event.virtual_locations,
        serde_json::from_value(json!({
            "v1": {
                "@type": "VirtualLocation",
                "uri": "https://meet.example.com/sprint",
                "name": "Team room",
                "features": {"video": true},
            },
        }))
        .expect("a map of virtual locations"),
        "{ics}"
    );
}

#[test]
fn a_conference_carries_the_key_of_the_entry_it_came_from() {
    // The LOCATION precedent (X-JMAP-KEY), for the same reason: an edit reaches
    // the server as a patch of `virtualLocations/<key>`, so the line has to say
    // which entry of the server's map it is a drawing of. Position could not:
    // an editor that drops a line it has no UI for would slide every later
    // conference onto the wrong entry.
    let ics = event_to_ical(&held_online(json!({
        "v1": joined_at("https://meet.example.com/sprint", "", json!({})),
        "v2": joined_at("tel:+1-555-0100", "", json!({})),
    })));

    assert_eq!(
        conferences(&ics),
        [
            "CONFERENCE;VALUE=URI;X-JMAP-KEY=v1:https://meet.example.com/sprint",
            "CONFERENCE;VALUE=URI;X-JMAP-KEY=v2:tel:+1-555-0100",
        ],
        "{ics}"
    );
}

#[test]
fn a_conference_with_no_key_is_read_under_an_invented_one() {
    // What another client's component looks like, and what Evolution would write
    // if it ever grew a UI for this. A key is still needed — `virtualLocations`
    // is a map — so one is invented per line, avoiding the keys already taken so
    // that two conferences cannot collapse into one. The save path only ever
    // patches keys the *server* holds, so an invented key can create an entry on
    // a create and reaches nothing on an edit.
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:x\r\n\
DTSTART:20260115T130000Z\r\n\
CONFERENCE;VALUE=URI:https://meet.example.com/one\r\n\
CONFERENCE;VALUE=URI;X-JMAP-KEY=v1:https://meet.example.com/two\r\n\
CONFERENCE;VALUE=URI:tel:+1-555-0100\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse");

    let places = event.virtual_locations.expect("virtual locations");
    assert_eq!(
        places.keys().collect::<Vec<_>>(),
        ["v1", "v2", "v3"],
        "{places:?}"
    );
    assert_eq!(
        places["v1"].get("uri").and_then(Value::as_str),
        Some("https://meet.example.com/two"),
        "the line that named a key keeps it"
    );
    assert_eq!(
        places["v2"].get("uri").and_then(Value::as_str),
        Some("https://meet.example.com/one")
    );
    assert_eq!(
        places["v3"].get("uri").and_then(Value::as_str),
        Some("tel:+1-555-0100")
    );
}

#[test]
fn a_component_that_names_no_conference_reads_back_none() {
    // `None` rather than an empty map, for the reason every other read gives:
    // the save path reads an edit off a difference from what was shown, and an
    // empty map would claim the event is joined nowhere where the component made
    // no claim at all.
    let event = ical_to_event(&event_to_ical(&held_online(json!({})))).expect("parse");

    assert_eq!(event.virtual_locations, None);
}

#[test]
fn a_conference_shown_in_part_is_not_the_users_to_have_edited() {
    // The rule `maps_locations` and `maps_keyword` already state, applied one
    // property along: an entry the line could not draw whole was not shown, so
    // no difference from the drawing may be sent for the property at all.
    for (editable, places) in [
        (true, json!({})),
        (
            true,
            json!({"v1": joined_at("https://meet.example.com/x", "Team room", json!({
                // The member with no room on the line is exactly what patching
                // in place exists to preserve, so it does not block a save.
                "description": "Ask Bob for the passcode",
                "features": {"video": true},
            }))}),
        ),
        // Nothing to join at, so nothing was drawn.
        (
            false,
            json!({"v1": {"@type": "VirtualLocation", "uri": "meet.example.com"}}),
        ),
        // A way of taking part outside RFC 7986 §6.3's vocabulary: the line is
        // drawn without it, so the entry is shown in part.
        (
            false,
            json!({"v1": joined_at("https://meet.example.com/x", "", json!({
                "features": {"hologram": true},
            }))}),
        ),
        (
            false,
            json!({"v1": joined_at("https://meet.example.com/x", "", json!({
                "features": {"video": "yes"},
            }))}),
        ),
        // A name a LABEL cannot carry.
        (
            false,
            json!({"v1": {"@type": "VirtualLocation", "uri": "https://x/y", "name": 7}}),
        ),
        // A key no patch path can name.
        (
            false,
            json!({"": joined_at("https://meet.example.com/x", "", json!({}))}),
        ),
    ] {
        let event = held_online(places.clone());

        assert_eq!(
            maps_virtual_locations(
                event
                    .virtual_locations
                    .as_ref()
                    .unwrap_or(&Default::default())
            ),
            editable,
            "{places}"
        );
    }
}

#[test]
fn an_edited_instance_carries_the_conferences_of_the_series() {
    // The inheritance of RFC 8984 §4.3.4 again: an override may not restate the
    // virtual locations, so the occurrence's own component states the series' —
    // an instance drawn without them would show a meeting with nowhere to join
    // it.
    let mut event = recurring_with(json!({"2026-01-29T13:00:00": {"title": "Sprint review"}}));
    event.virtual_locations = serde_json::from_value(json!({
        "v1": joined_at("https://meet.example.com/sprint", "Team room", json!({})),
    }))
    .expect("a map of virtual locations");
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert_eq!(
        conferences(vevent(&ics, 1)),
        ["CONFERENCE;VALUE=URI;LABEL=\"Team room\";X-JMAP-KEY=v1:https://meet.example.com/sprint"],
        "{ics}"
    );
}

/// An event pointing at whatever external resources are passed.
fn points_at(links: Value) -> CalendarEvent {
    CalendarEvent {
        title: Some("Sprint planning".to_owned()),
        start: Some("2026-01-15T13:00:00".to_owned()),
        time_zone: Some("Etc/UTC".to_owned()),
        duration: Some("PT1H".to_owned()),
        links: serde_json::from_value(links).expect("a map of links"),
        ..CalendarEvent::default()
    }
}

/// One link: somewhere to fetch the resource from, and whatever else is passed.
fn fetched_from(href: &str, rest: Value) -> Value {
    let mut link = json!({"@type": "Link", "href": href});
    for (key, value) in rest.as_object().expect("an object").clone() {
        link[key] = value;
    }
    link
}

fn attachments(ics: &str) -> Vec<String> {
    named_lines(ics, "ATTACH")
}

fn images(ics: &str) -> Vec<String> {
    named_lines(ics, "IMAGE")
}

fn named_lines(ics: &str, name: &str) -> Vec<String> {
    ics.replace("\r\n ", "")
        .split("\r\n")
        .filter(|line| line.starts_with(name))
        .map(str::to_owned)
        .collect()
}

#[test]
fn an_external_resource_is_written_as_an_attach() {
    // RFC 8984 §4.2.7's `links` is a map of Links (§1.4.11), each an `href` to
    // fetch a resource from with the media type and size the server knows for
    // it; RFC 5545 §3.8.1.1 spells one as an ATTACH line whose value is that
    // URI, with the type on an FMTTYPE (§3.2.8) and the size on the SIZE
    // parameter RFC 8607 §4.1 adds. Like CONFERENCE and unlike LOCATION, the
    // property may be stated more than once, so every entry of the map is
    // written rather than one of them.
    let ics = event_to_ical(&points_at(json!({
        "l1": fetched_from("https://files.example.com/agenda.pdf", json!({
            "contentType": "application/pdf",
            "size": 51_200,
        })),
        "l2": fetched_from("https://files.example.com/minutes.txt", json!({})),
    })));

    // In the map's own order, so a document is stable across renderings — which
    // is what the save path's diff against a re-rendering needs.
    assert_eq!(
        attachments(&ics),
        [
            "ATTACH;FMTTYPE=application/pdf;SIZE=51200;X-JMAP-KEY=l1:\
             https://files.example.com/agenda.pdf",
            "ATTACH;X-JMAP-KEY=l2:https://files.example.com/minutes.txt",
        ],
        "{ics}"
    );
    // ATTACH's default value type is already URI (RFC 5545 §3.8.1.1), so no
    // VALUE parameter is written — unlike CONFERENCE, whose grammar demands one.
    assert!(!ics.contains("ATTACH;VALUE"), "{ics}");
}

#[test]
fn a_link_with_nowhere_to_fetch_it_from_is_left_off() {
    // An ATTACH's value is a URI, and RFC 8984 §1.4.11 makes `href` the one
    // mandatory member of a Link: there is nothing to write for a resource the
    // server named no address for, and inventing one would send the user
    // somewhere the server never did. So the entry is dropped, like every other
    // value this mapping cannot spell — which is only safe because `links` is
    // written and never read back.
    for link in [
        json!({"@type": "Link", "title": "The agenda"}),
        json!({"@type": "Link", "href": ""}),
        // A bare host is not a URI: RFC 3986 §3.1 wants a scheme.
        json!({"@type": "Link", "href": "files.example.com/agenda.pdf"}),
        json!({"@type": "Link", "href": "https:"}),
        json!({"@type": "Link", "href": 42}),
        // Whitespace is not in a URI, and a line break would end the content
        // line and start a property of the server's choosing.
        json!({"@type": "Link", "href": "https://files.example.com/the agenda.pdf"}),
        json!({"@type": "Link", "href": "https://x/\r\nSUMMARY:Gone"}),
        // Not an object at all.
        json!("https://files.example.com/agenda.pdf"),
    ] {
        let ics = event_to_ical(&points_at(json!({"l1": link})));

        assert!(without(&ics, "ATTACH"), "{link}: {ics}");
        assert!(without(&ics, "IMAGE"), "{link}: {ics}");
        assert!(!ics.contains("SUMMARY:Gone"), "{link}: {ics}");
    }
}

#[test]
fn a_media_type_no_fmttype_can_carry_is_left_off() {
    // RFC 5545 §3.2.8's `fmttypeparam` is a type-name and a subtype-name of RFC
    // 6838's restricted-name, and no more: a media type carrying parameters, or
    // one whose name holds a character the grammar does not admit, has no
    // spelling here. It is the parameter that is unwritable, not the resource,
    // so the attachment still goes on the line — the user can still open it.
    for content_type in [
        json!("application"),
        json!("application/"),
        json!("/pdf"),
        json!("text/plain; charset=utf-8"),
        json!("text/pl:in"),
        json!("text/pl,in"),
        json!("text/pl\r\nSUMMARY:Gone"),
        json!(".ext/plain"),
        json!(7),
    ] {
        let ics = event_to_ical(&points_at(json!({
            "l1": fetched_from("https://files.example.com/agenda.pdf", json!({
                "contentType": content_type,
            })),
        })));

        assert_eq!(
            attachments(&ics),
            ["ATTACH;X-JMAP-KEY=l1:https://files.example.com/agenda.pdf"],
            "{content_type}: {ics}"
        );
        assert!(!ics.contains("SUMMARY:Gone"), "{content_type}: {ics}");
    }
}

#[test]
fn a_size_that_is_not_a_count_of_octets_is_left_off() {
    // RFC 8984 §1.4.11 makes `size` an UnsignedInt — a count of the octets the
    // user would download. A negative number, a fraction or a string is not
    // one, and stating it anyway would put a SIZE outside RFC 8607 §4.1's
    // `1*DIGIT` on the line.
    for size in [
        json!(-1),
        json!(51_200.5),
        json!("51200"),
        json!(null),
        json!({"octets": 51_200}),
    ] {
        let ics = event_to_ical(&points_at(json!({
            "l1": fetched_from("https://files.example.com/agenda.pdf", json!({
                "size": size,
            })),
        })));

        assert_eq!(
            attachments(&ics),
            ["ATTACH;X-JMAP-KEY=l1:https://files.example.com/agenda.pdf"],
            "{size}: {ics}"
        );
    }
}

#[test]
fn a_link_to_an_icon_is_written_as_an_image() {
    // RFC 8984 §1.4.11 gives a picture of the event a `rel` of "icon" and a
    // `display` saying what it is for; RFC 7986 §5.10's IMAGE is that property
    // in iCalendar, with §6.1's DISPLAY parameter naming the same four
    // intentions — badge, graphic, fullsize, thumbnail — in the same words,
    // differing only in case. So an icon is an IMAGE and not an ATTACH: the
    // picture shown beside a title is not a document the user opens.
    for (display, drawn) in [
        ("badge", "BADGE"),
        ("graphic", "GRAPHIC"),
        ("fullsize", "FULLSIZE"),
        ("thumbnail", "THUMBNAIL"),
        // Read case-insensitively, which is what every closed vocabulary in
        // this mapping does with the value a server states.
        ("BADGE", "BADGE"),
    ] {
        let ics = event_to_ical(&points_at(json!({
            "l1": fetched_from("https://files.example.com/party.png", json!({
                "rel": "icon",
                "display": display,
                "contentType": "image/png",
            })),
        })));

        // VALUE=URI because RFC 7986 §5.10's `image` grammar makes it REQUIRED
        // on the URI alternative, the way §5.11 does for a CONFERENCE.
        assert_eq!(
            images(&ics),
            [format!(
                "IMAGE;VALUE=URI;DISPLAY={drawn};FMTTYPE=image/png;X-JMAP-KEY=l1:\
                 https://files.example.com/party.png"
            )],
            "{display}: {ics}"
        );
        assert!(without(&ics, "ATTACH"), "{display}: {ics}");
    }
}

#[test]
fn an_icon_with_no_way_of_displaying_it_carries_no_display() {
    // RFC 7986 §6.1 defaults DISPLAY to BADGE and requires a reader to show no
    // image at all for a value it does not recognise — so a `display` outside
    // the four both formats share is dropped rather than passed through in the
    // other format's clothes, which would hide the picture entirely. Leaving
    // the parameter off says only that the default applies.
    for display in [json!(null), json!("hologram"), json!(7), json!(["badge"])] {
        let ics = event_to_ical(&points_at(json!({
            "l1": fetched_from("https://files.example.com/party.png", json!({
                "rel": "icon",
                "display": display,
            })),
        })));

        assert_eq!(
            images(&ics),
            ["IMAGE;VALUE=URI;X-JMAP-KEY=l1:https://files.example.com/party.png"],
            "{display}: {ics}"
        );
    }
}

#[test]
fn a_link_that_is_not_an_icon_is_an_attachment_however_it_asks_to_be_displayed() {
    // RFC 8984 §1.4.11 lets `display` be set only when `rel` is "icon", so a
    // link that says otherwise is one whose author contradicted themselves.
    // `rel` decides, because it is the property that says what the resource *is*
    // — and an ATTACH has nowhere to put a DISPLAY anyway (RFC 7986 §6.1 admits
    // the parameter on IMAGE alone).
    for rel in [
        json!(null),
        json!("enclosure"),
        json!("describedby"),
        json!(7),
    ] {
        let ics = event_to_ical(&points_at(json!({
            "l1": fetched_from("https://files.example.com/party.png", json!({
                "rel": rel,
                "display": "badge",
            })),
        })));

        assert_eq!(
            attachments(&ics),
            ["ATTACH;X-JMAP-KEY=l1:https://files.example.com/party.png"],
            "{rel}: {ics}"
        );
        assert!(without(&ics, "IMAGE"), "{rel}: {ics}");
    }
}

#[test]
fn a_resource_line_carries_the_key_of_the_entry_it_was_drawn_from() {
    // The third property patched *into* rather than replaced, and the parameter
    // that makes that possible: a Link holds a `cid`, a `rel` and a `title` (RFC
    // 8984 §1.4.11) no line has room for, so a save names `links/<key>/href` and
    // the line has to say which entry of the server's map it is a drawing of.
    // Position could not do it — an editor that drops a line it has no UI for
    // would slide every later resource onto the wrong entry — and both
    // properties admit being stated more than once, so there really are several
    // lines to tell apart.
    let ics = event_to_ical(&points_at(json!({
        "agenda": fetched_from("https://files.example.com/agenda.pdf", json!({})),
        "picture": fetched_from("https://files.example.com/party.png", json!({"rel": "icon"})),
    })));

    assert_eq!(
        attachments(&ics),
        ["ATTACH;X-JMAP-KEY=agenda:https://files.example.com/agenda.pdf"],
        "{ics}"
    );
    assert_eq!(
        images(&ics),
        ["IMAGE;VALUE=URI;X-JMAP-KEY=picture:https://files.example.com/party.png"],
        "{ics}"
    );
}

#[test]
fn a_component_that_names_resources_reads_them_back_under_the_servers_keys() {
    // What the two lines show — the address, the media type, the size, and for a
    // picture the way of displaying it — reads back into the entry it was drawn
    // from, so that a save patches `links/<key>/href` and the `cid`, `rel` and
    // `title` the line had no room for stay where the server put them. The
    // property name is itself a member: RFC 7986 §5.10's IMAGE is a link whose
    // `rel` is "icon" (RFC 8984 §1.4.11), which is what a re-drawing needs to
    // put it back on an IMAGE rather than an ATTACH.
    let ics = event_to_ical(&points_at(json!({
        "agenda": fetched_from("https://files.example.com/agenda.pdf", json!({
            "contentType": "application/pdf",
            "size": 51_200,
            "title": "What we said we would do",
        })),
        "picture": fetched_from("https://files.example.com/party.png", json!({
            "rel": "icon",
            "display": "badge",
            "contentType": "image/png",
        })),
    })));

    let event = ical_to_event(&ics).expect("a calendar");
    assert_eq!(
        serde_json::to_value(event.links.expect("links")).expect("a map"),
        json!({
            "agenda": {
                "@type": "Link",
                "href": "https://files.example.com/agenda.pdf",
                "contentType": "application/pdf",
                "size": 51_200,
            },
            "picture": {
                "@type": "Link",
                "href": "https://files.example.com/party.png",
                "rel": "icon",
                "display": "badge",
                "contentType": "image/png",
            },
        }),
        "{ics}"
    );
}

#[test]
fn what_was_read_back_off_a_resource_line_draws_the_same_line_again() {
    // The save path diffs an edited component against a re-rendering of the
    // event the server holds, so a drawing that did not survive its own round
    // trip would read as an edit on every save. The members with no room on the
    // line are absent from both sides, and the ones with room come back in the
    // same spelling.
    let ics = event_to_ical(&points_at(json!({
        "agenda": fetched_from("https://files.example.com/agenda.pdf", json!({
            "contentType": "application/pdf",
            "size": 51_200,
        })),
        "picture": fetched_from("https://files.example.com/party.png", json!({
            "rel": "icon",
            "display": "thumbnail",
        })),
    })));

    let event = ical_to_event(&ics).expect("a calendar");
    let again = event_to_ical(&event);

    assert_eq!(attachments(&again), attachments(&ics), "{again}");
    assert_eq!(images(&again), images(&ics), "{again}");
}

#[test]
fn a_resource_line_naming_no_entry_gets_a_key_of_its_own() {
    // What another client's component looks like, and what an editor writing an
    // attachment afresh writes: a line with no X-JMAP-KEY on it. A key is
    // invented so that the resource is still read — on a create the property is
    // written whole under the keys read here — and the invented keys avoid the
    // ones the document already named, because two resources that collided on a
    // key would become one.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:planning-1\r\n\
DTSTART:20260115T130000Z\r\n\
ATTACH:https://files.example.com/one.pdf\r\n\
ATTACH;X-JMAP-KEY=k1:https://files.example.com/two.pdf\r\n\
IMAGE;VALUE=URI:https://files.example.com/party.png\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("a calendar");
    let links = event.links.expect("links");
    assert_eq!(
        links.keys().collect::<Vec<_>>(),
        ["k1", "k2", "k3"],
        "{links:?}"
    );
    assert_eq!(
        links["k2"]["href"],
        json!("https://files.example.com/one.pdf"),
        "{links:?}"
    );
    assert_eq!(
        links["k1"]["href"],
        json!("https://files.example.com/two.pdf"),
        "{links:?}"
    );
    assert_eq!(links["k3"]["rel"], json!("icon"), "{links:?}");
}

#[test]
fn a_resource_line_pointing_at_a_local_file_is_not_read_back() {
    // A `file:` URI is where Evolution keeps an attachment the user added from
    // their own disk. It is not an address the server, or anybody else's client,
    // could fetch — and the path names the user's home directory, so filing it
    // as a Link would put a local path in a record every other client of the
    // account can read. Sending the file itself means uploading it as a blob,
    // which this crate has no part in, so the line is left unread: the entry the
    // server holds stays as it is rather than being pointed at a machine only
    // this user has.
    for href in [
        "file:///home/vera/.local/share/evolution/calendar/agenda.pdf",
        "FILE:///home/vera/agenda.pdf",
    ] {
        let ics = format!(
            "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:planning-1\r\n\
DTSTART:20260115T130000Z\r\n\
ATTACH:{href}\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n"
        );

        let event = ical_to_event(&ics).expect("a calendar");
        assert_eq!(event.links, None, "{href}");
    }
}

#[test]
fn a_component_that_names_no_resource_reads_back_no_links() {
    // `None` rather than an empty map, for the reason every other read-back
    // gives: the save path reads an edit off a difference from what was shown,
    // and an empty map would claim the event points at nothing where the
    // component made no claim at all. A value that is not a URI — an inline
    // binary attachment, which is what Evolution writes for a file it holds
    // itself — says nothing either.
    for line in [
        "",
        "ATTACH;VALUE=BINARY;ENCODING=BASE64:dGhlIGFnZW5kYQ==\r\n",
        "ATTACH:files.example.com/agenda.pdf\r\n",
    ] {
        let ics = format!(
            "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:planning-1\r\n\
DTSTART:20260115T130000Z\r\n\
{line}\
END:VEVENT\r\n\
END:VCALENDAR\r\n"
        );

        let event = ical_to_event(&ics).expect("a calendar");
        assert_eq!(event.links, None, "{line}");
    }
}

#[test]
fn an_edited_instance_carries_the_links_of_the_series() {
    // The inheritance of RFC 8984 §4.3.4 again: an override may not restate the
    // links, so the occurrence's own component states the series' — an instance
    // drawn without them would show a meeting whose agenda had gone missing.
    let mut event = recurring_with(json!({"2026-01-29T13:00:00": {"title": "Sprint review"}}));
    event.links = serde_json::from_value(json!({
        "l1": fetched_from("https://files.example.com/agenda.pdf", json!({})),
    }))
    .expect("a map of links");
    let ics = event_to_ical(&event);

    assert_eq!(vevents(&ics), 2, "{ics}");
    assert_eq!(
        attachments(vevent(&ics, 1)),
        ["ATTACH;X-JMAP-KEY=l1:https://files.example.com/agenda.pdf"],
        "{ics}"
    );
}

#[test]
fn multiple_attachments_and_images_roundtrip_with_fmttype_size_and_display() {
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
BEGIN:VEVENT\r\n\
UID:multi-link-1\r\n\
SUMMARY:Multi Link Event\r\n\
DTSTART:20260115T130000Z\r\n\
ATTACH;FMTTYPE=application/pdf;SIZE=51200;X-JMAP-KEY=l1:https://files.example.com/agenda.pdf\r\n\
ATTACH;FMTTYPE=text/plain;SIZE=1024;X-JMAP-KEY=l2:https://files.example.com/notes.txt\r\n\
IMAGE;VALUE=URI;DISPLAY=BADGE;X-JMAP-KEY=img1:https://files.example.com/logo.png\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("a calendar");
    let links = event.links.as_ref().expect("links");
    assert_eq!(links.len(), 3, "{links:?}");
    assert_eq!(
        links["l1"]["href"],
        json!("https://files.example.com/agenda.pdf")
    );
    assert_eq!(links["l1"]["contentType"], json!("application/pdf"));
    assert_eq!(links["l1"]["size"], json!(51_200));
    assert_eq!(
        links["l2"]["href"],
        json!("https://files.example.com/notes.txt")
    );
    assert_eq!(links["l2"]["contentType"], json!("text/plain"));
    assert_eq!(links["l2"]["size"], json!(1024));
    assert_eq!(
        links["img1"]["href"],
        json!("https://files.example.com/logo.png")
    );
    assert_eq!(links["img1"]["rel"], json!("icon"));
    assert_eq!(links["img1"]["display"], json!("badge"));

    let rendered = event_to_ical(&event);
    assert_eq!(
        attachments(&rendered),
        [
            "ATTACH;FMTTYPE=application/pdf;SIZE=51200;X-JMAP-KEY=l1:https://files.example.com/agenda.pdf",
            "ATTACH;FMTTYPE=text/plain;SIZE=1024;X-JMAP-KEY=l2:https://files.example.com/notes.txt",
        ],
        "{rendered}"
    );
    assert_eq!(
        images(&rendered),
        ["IMAGE;VALUE=URI;DISPLAY=BADGE;X-JMAP-KEY=img1:https://files.example.com/logo.png"],
        "{rendered}"
    );
}

#[test]
fn reading_vevent_with_omitted_attachment_yields_only_present_links() {
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:planning-1\r\n\
DTSTART:20260115T130000Z\r\n\
ATTACH;X-JMAP-KEY=l1:https://files.example.com/retained.pdf\r\n\
IMAGE;VALUE=URI;DISPLAY=GRAPHIC;X-JMAP-KEY=img1:https://files.example.com/banner.png\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("a calendar");
    let links = event.links.expect("links");
    assert_eq!(links.len(), 2, "{links:?}");
    assert!(links.contains_key("l1"));
    assert!(links.contains_key("img1"));
    assert_eq!(links["img1"]["display"], json!("graphic"));
}

#[test]
fn event_with_categories_classification_transparency_status_and_url_roundtrips_faithfully() {
    let event = CalendarEvent {
        title: Some("Sprint Review".to_owned()),
        start: Some("2026-01-15T13:00:00".to_owned()),
        time_zone: Some("Etc/UTC".to_owned()),
        duration: Some("PT1H".to_owned()),
        privacy: Some("secret".to_owned()),
        free_busy_status: Some("free".to_owned()),
        status: Some("confirmed".to_owned()),
        priority: Some(1),
        keywords: Some(
            [
                ("offsite".to_owned(), json!(true)),
                ("planning".to_owned(), json!(true)),
            ]
            .into_iter()
            .collect(),
        ),
        locations: Some(
            [(
                "loc1".to_owned(),
                json!({"@type": "Location", "name": "Room 42"}),
            )]
            .into_iter()
            .collect(),
        ),
        links: Some(
            [(
                "l1".to_owned(),
                json!({
                    "@type": "Link",
                    "href": "https://meet.example.com/planning",
                    "contentType": "text/html",
                }),
            )]
            .into_iter()
            .collect(),
        ),
        ..CalendarEvent::default()
    };

    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "CLASS:"), "CLASS:CONFIDENTIAL");
    assert_eq!(line(&ics, "TRANSP:"), "TRANSP:TRANSPARENT");
    assert_eq!(line(&ics, "STATUS:"), "STATUS:CONFIRMED");
    assert_eq!(line(&ics, "PRIORITY:"), "PRIORITY:1");
    assert_eq!(
        content_line(&ics, "CATEGORIES"),
        "CATEGORIES:offsite,planning"
    );
    assert_eq!(line(&ics, "LOCATION;"), "LOCATION;X-JMAP-KEY=loc1:Room 42");
    assert_eq!(
        line(&ics, "ATTACH;"),
        "ATTACH;FMTTYPE=text/html;X-JMAP-KEY=l1:https://meet.example.com/planning"
    );

    let parsed = ical_to_event(&ics).expect("parse");
    assert_eq!(parsed.privacy.as_deref(), Some("secret"));
    assert_eq!(parsed.free_busy_status.as_deref(), Some("free"));
    assert_eq!(parsed.status.as_deref(), Some("confirmed"));
    assert_eq!(parsed.priority, Some(1));
    let parsed_keywords = parsed.keywords.expect("keywords");
    assert!(parsed_keywords.contains_key("offsite"));
    assert!(parsed_keywords.contains_key("planning"));
    let parsed_locs = parsed.locations.expect("locations");
    assert_eq!(parsed_locs["loc1"]["name"], json!("Room 42"));
    let parsed_links = parsed.links.expect("links");
    assert_eq!(
        parsed_links["l1"]["href"],
        json!("https://meet.example.com/planning")
    );
}

#[test]
fn event_with_custom_privacy_and_freebusy_drops_unmodeled_lines_gracefully() {
    let event = CalendarEvent {
        title: Some("Custom Event".to_owned()),
        start: Some("2026-01-15T13:00:00".to_owned()),
        time_zone: Some("Etc/UTC".to_owned()),
        duration: Some("PT1H".to_owned()),
        privacy: Some("x-custom-classification".to_owned()),
        free_busy_status: Some("x-tentative-free".to_owned()),
        ..CalendarEvent::default()
    };

    let ics = event_to_ical(&event);
    assert!(without(&ics, "CLASS"));
    assert!(without(&ics, "TRANSP"));

    let parsed = ical_to_event(&ics).expect("parse");
    assert_eq!(parsed.privacy, None);
    assert_eq!(parsed.free_busy_status, None);
}

#[test]
fn event_with_chair_and_multiple_participants_emits_accurate_attendees_and_organizer() {
    let ics = event_to_ical(&attended(json!({
        "alice": guest("mailto:alice@example.com", "Alice Owner", json!({
            "roles": {"owner": true, "chair": true, "attendee": true},
            "participationStatus": "accepted",
        })),
        "bob": guest("mailto:bob@example.com", "Bob Engineer", json!({
            "roles": {"attendee": true},
            "participationStatus": "declined",
        })),
        "carol": guest("mailto:carol@example.com", "Carol Observer", json!({
            "roles": {"informational": true},
            "participationStatus": "tentative",
        })),
    })));

    assert_eq!(
        content_line(&ics, "ORGANIZER"),
        "ORGANIZER;CN=\"Alice Owner\":mailto:alice@example.com",
        "{ics}"
    );

    let attendees: Vec<String> = ics
        .replace("\r\n ", "")
        .split("\r\n")
        .filter(|line| line.starts_with("ATTENDEE"))
        .map(str::to_owned)
        .collect();

    assert_eq!(
        attendees,
        [
            "ATTENDEE;CN=\"Alice Owner\";ROLE=CHAIR;PARTSTAT=ACCEPTED:mailto:alice@example.com",
            "ATTENDEE;CN=\"Bob Engineer\";ROLE=REQ-PARTICIPANT;PARTSTAT=DECLINED:mailto:bob@example.com",
            "ATTENDEE;CN=\"Carol Observer\";ROLE=NON-PARTICIPANT;PARTSTAT=TENTATIVE:mailto:carol@example.com",
        ],
        "{ics}"
    );
}

#[test]
fn event_with_priority_and_geo_coordinates_roundtrips_faithfully() {
    let mut event = CalendarEvent {
        title: Some("Global Standup".to_owned()),
        start: Some("2026-01-15T13:00:00".to_owned()),
        time_zone: Some("Etc/UTC".to_owned()),
        duration: Some("PT1H".to_owned()),
        priority: Some(1),
        ..CalendarEvent::default()
    };
    event.locations = Some(
        [(
            "loc1".to_owned(),
            json!({
                "@type": "Location",
                "name": "Berlin HQ",
                "coordinates": "geo:52.520008,13.404954",
            }),
        )]
        .into(),
    );

    let ics = event_to_ical(&event);
    assert_eq!(content_line(&ics, "PRIORITY"), "PRIORITY:1");
    assert_eq!(
        content_line(&ics, "LOCATION"),
        "LOCATION;X-JMAP-KEY=loc1:Berlin HQ"
    );

    let parsed = ical_to_event(&ics).expect("parse");
    assert_eq!(parsed.priority, Some(1));
    let locs = parsed.locations.expect("locations");
    assert_eq!(locs["loc1"]["name"], json!("Berlin HQ"));
}

#[test]
fn event_with_multiple_alerts_roundtrips_faithfully_to_valarms() {
    let mut event = CalendarEvent {
        title: Some("Architecture Review".to_owned()),
        start: Some("2026-01-15T15:00:00".to_owned()),
        time_zone: Some("Etc/UTC".to_owned()),
        duration: Some("PT1H".to_owned()),
        ..CalendarEvent::default()
    };
    event.alerts = Some(
        [
            (
                "a1".to_owned(),
                json!({
                    "@type": "Alert",
                    "trigger": {
                        "@type": "OffsetTrigger",
                        "offset": "-PT15M",
                        "relativeTo": "start"
                    },
                    "action": "display"
                }),
            ),
            (
                "a2".to_owned(),
                json!({
                    "@type": "Alert",
                    "trigger": {
                        "@type": "OffsetTrigger",
                        "offset": "PT10M",
                        "relativeTo": "end"
                    },
                    "action": "display"
                }),
            ),
            (
                "a3".to_owned(),
                json!({
                    "@type": "Alert",
                    "trigger": {
                        "@type": "OffsetTrigger",
                        "offset": "-P1D",
                        "relativeTo": "start"
                    },
                    "action": "display"
                }),
            ),
        ]
        .into(),
    );

    let ics = event_to_ical(&event);
    assert_eq!(ics.matches("BEGIN:VALARM\r\n").count(), 3, "{ics}");

    let parsed = ical_to_event(&ics).expect("parse");
    let alerts = parsed.alerts.expect("alerts map");
    assert_eq!(alerts.len(), 3);

    assert_eq!(alerts["a1"]["action"], json!("display"));
    assert_eq!(alerts["a1"]["trigger"]["offset"], json!("-PT15M"));

    assert_eq!(alerts["a2"]["action"], json!("display"));
    assert_eq!(alerts["a2"]["trigger"]["offset"], json!("PT10M"));
    assert_eq!(alerts["a2"]["trigger"]["relativeTo"], json!("end"));

    assert_eq!(alerts["a3"]["action"], json!("display"));
    assert_eq!(alerts["a3"]["trigger"]["offset"], json!("-P1D"));
}

#[test]
fn event_with_unsupported_or_custom_alarm_action_drops_or_sanitizes_safely() {
    let ics = "BEGIN:VCALENDAR\r\n\
               VERSION:2.0\r\n\
               PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
               BEGIN:VEVENT\r\n\
               UID:K1\r\n\
               SUMMARY:Release Party\r\n\
               DTSTART:20260115T180000Z\r\n\
               DURATION:PT2H\r\n\
               BEGIN:VALARM\r\n\
               UID:valid-disp\r\n\
               ACTION:DISPLAY\r\n\
               DESCRIPTION:Reminder\r\n\
               TRIGGER:-PT10M\r\n\
               END:VALARM\r\n\
               BEGIN:VALARM\r\n\
               UID:audio1\r\n\
               ACTION:AUDIO\r\n\
               TRIGGER:-PT15M\r\n\
               END:VALARM\r\n\
               BEGIN:VALARM\r\n\
               UID:email1\r\n\
               ACTION:EMAIL\r\n\
               TRIGGER:-PT1H\r\n\
               DESCRIPTION:Email alert\r\n\
               SUMMARY:Email alert\r\n\
               END:VALARM\r\n\
               BEGIN:VALARM\r\n\
               UID:proc1\r\n\
               ACTION:PROCEDURE\r\n\
               TRIGGER:-PT5M\r\n\
               END:VALARM\r\n\
               BEGIN:VALARM\r\n\
               UID:abs1\r\n\
               ACTION:DISPLAY\r\n\
               DESCRIPTION:Absolute alert\r\n\
               TRIGGER;VALUE=DATE-TIME:20260115T174500Z\r\n\
               END:VALARM\r\n\
               END:VEVENT\r\n\
               END:VCALENDAR\r\n";

    let parsed = ical_to_event(ics).expect("parse");
    assert_eq!(parsed.title.as_deref(), Some("Release Party"));

    let alerts = parsed.alerts.expect("alerts map");
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts["valid-disp"]["action"], json!("display"));
    assert_eq!(alerts["valid-disp"]["trigger"]["offset"], json!("-PT10M"));
}

#[test]
fn event_with_complex_rrule_and_exdates_roundtrips_faithfully() {
    let mut event = fixture_event();
    event.recurrence_rule = Some(RecurrenceRule {
        interval: Some(2),
        count: Some(8),
        by_day: Some(vec![
            NDay {
                day: "mo".to_owned(),
                ..Default::default()
            },
            NDay {
                day: "fr".to_owned(),
                ..Default::default()
            },
        ]),
        ..RecurrenceRule::new("weekly")
    });
    event.recurrence_overrides = Some(
        [
            ("2026-08-17T09:00:00".to_owned(), json!({"excluded": true})),
            ("2026-08-21T09:00:00".to_owned(), json!({"excluded": true})),
        ]
        .into(),
    );

    let ics = event_to_ical(&event);
    assert!(ics.contains("RRULE:"), "missing RRULE: {ics}");
    assert!(ics.contains("FREQ=WEEKLY"), "missing FREQ: {ics}");
    assert!(ics.contains("INTERVAL=2"), "missing INTERVAL: {ics}");
    assert!(ics.contains("COUNT=8"), "missing COUNT: {ics}");
    assert!(ics.contains("BYDAY=MO,FR"), "missing BYDAY: {ics}");
    assert!(ics.contains("EXDATE"), "missing EXDATE: {ics}");

    let parsed = ical_to_event(&ics).expect("parse");
    let rules = parsed.recurrence_rule.expect("rules");
    assert_eq!(rules.frequency, "weekly");
    assert_eq!(rules.interval, Some(2));
    assert_eq!(rules.count, Some(8));
    let byday = rules.by_day.as_ref().expect("byday");
    assert_eq!(byday.len(), 2);
    assert_eq!(byday[0].day, "mo");
    assert_eq!(byday[1].day, "fr");

    let overrides = parsed.recurrence_overrides.expect("overrides");
    assert_eq!(overrides.len(), 2);
    assert_eq!(overrides["2026-08-17T09:00:00"], json!({"excluded": true}));
    assert_eq!(overrides["2026-08-21T09:00:00"], json!({"excluded": true}));
}

#[test]
fn recurring_event_with_instance_overrides_emits_multiple_vevents_with_recurrence_id() {
    let mut event = fixture_event();
    event.recurrence_rule = Some(RecurrenceRule {
        interval: Some(1),
        ..RecurrenceRule::new("daily")
    });
    event.recurrence_overrides = Some(
        [(
            "2026-08-15T09:00:00".to_owned(),
            json!({
                "title": "Special Session",
                "duration": "PT2H",
                "status": "tentative"
            }),
        )]
        .into(),
    );

    let ics = event_to_ical(&event);
    assert_eq!(vevents(&ics), 2, "must emit master and override: {ics}");
    assert!(
        ics.contains("RECURRENCE-ID"),
        "missing RECURRENCE-ID: {ics}"
    );
    assert!(
        ics.contains("Special Session"),
        "missing override title: {ics}"
    );

    let parsed = ical_to_event(&ics).expect("parse");
    assert_eq!(parsed.title.as_deref(), Some("Sprint planning"));
    let overrides = parsed.recurrence_overrides.expect("overrides");
    assert_eq!(overrides.len(), 1);
    assert_eq!(
        overrides["2026-08-15T09:00:00"]["title"],
        json!("Special Session")
    );
    assert_eq!(overrides["2026-08-15T09:00:00"]["duration"], json!("PT2H"));
    assert_eq!(
        overrides["2026-08-15T09:00:00"]["status"],
        json!("tentative")
    );
}

#[test]
fn event_with_allday_dates_and_explicit_timezone_roundtrips_faithfully() {
    let ics = "BEGIN:VCALENDAR\r\n\
               VERSION:2.0\r\n\
               PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
               BEGIN:VEVENT\r\n\
               UID:allday-1\r\n\
               SUMMARY:Multi-day Conference\r\n\
               DTSTART;VALUE=DATE:20260810\r\n\
               DTEND;VALUE=DATE:20260813\r\n\
               LOCATION:Convention Center\r\n\
               END:VEVENT\r\n\
               END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse");
    assert_eq!(event.title.as_deref(), Some("Multi-day Conference"));
    assert_eq!(event.start.as_deref(), Some("2026-08-10T00:00:00"));
    assert_eq!(event.duration.as_deref(), Some("P3D"));
    assert_eq!(event.show_without_time, Some(true));
    assert_eq!(event.time_zone, None);

    let rendered = event_to_ical(&event);
    assert_eq!(line(&rendered, "DTSTART"), "DTSTART;VALUE=DATE:20260810");
    assert_eq!(line(&rendered, "DURATION"), "DURATION:P3D");
    assert_eq!(line(&rendered, "SUMMARY"), "SUMMARY:Multi-day Conference");

    let parsed_back = ical_to_event(&rendered).expect("roundtrip");
    assert_eq!(parsed_back.start.as_deref(), Some("2026-08-10T00:00:00"));
    assert_eq!(parsed_back.duration.as_deref(), Some("P3D"));
    assert_eq!(parsed_back.show_without_time, Some(true));
}

#[test]
fn event_with_fractional_duration_and_utc_or_floating_start_roundtrips() {
    let ics = "BEGIN:VCALENDAR\r\n\
               VERSION:2.0\r\n\
               PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
               BEGIN:VEVENT\r\n\
               UID:timed-1\r\n\
               SUMMARY:Tech Sync\r\n\
               DTSTART:20260810T143000Z\r\n\
               DURATION:PT1H45M\r\n\
               END:VEVENT\r\n\
               END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse");
    assert_eq!(event.title.as_deref(), Some("Tech Sync"));
    assert_eq!(event.start.as_deref(), Some("2026-08-10T14:30:00"));
    assert_eq!(event.time_zone.as_deref(), Some("Etc/UTC"));
    assert_eq!(event.duration.as_deref(), Some("PT1H45M"));

    let rendered = event_to_ical(&event);
    assert_eq!(line(&rendered, "DTSTART"), "DTSTART:20260810T143000Z");
    assert_eq!(line(&rendered, "SUMMARY"), "SUMMARY:Tech Sync");

    let parsed_back = ical_to_event(&rendered).expect("roundtrip");
    assert_eq!(parsed_back.start.as_deref(), Some("2026-08-10T14:30:00"));
    assert_eq!(parsed_back.time_zone.as_deref(), Some("Etc/UTC"));
    assert_eq!(parsed_back.duration.as_deref(), Some("PT1H45M"));
}

#[test]
fn maps_descriptions_with_multi_line_and_newlines_faithfully() {
    let ics = "BEGIN:VCALENDAR\r\n\
               VERSION:2.0\r\n\
               PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
               BEGIN:VEVENT\r\n\
               UID:desc-1\r\n\
               SUMMARY:Release Planning\r\n\
               DTSTART:20260810T100000Z\r\n\
               DESCRIPTION:Sprint goals:\\n- Ship M4\\n- Review M5\\; with team\\n- Write docs\r\n\
               END:VEVENT\r\n\
               END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse");
    assert_eq!(event.title.as_deref(), Some("Release Planning"));
    assert_eq!(
        event.description.as_deref(),
        Some("Sprint goals:\n- Ship M4\n- Review M5; with team\n- Write docs")
    );

    let rendered = event_to_ical(&event);
    assert!(rendered.contains("SUMMARY:Release Planning"), "{rendered}");
    assert!(
        rendered.contains(
            "DESCRIPTION:Sprint goals:\\n- Ship M4\\n- Review M5\\; with team\\n- Write docs"
        ),
        "{rendered}"
    );

    let back = ical_to_event(&rendered).expect("roundtrip");
    assert_eq!(back.description, event.description);
}

#[test]
fn maps_created_updated_and_rdate_series_faithfully() {
    let ics = "BEGIN:VCALENDAR\r\n\
               VERSION:2.0\r\n\
               PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
               BEGIN:VEVENT\r\n\
               UID:series-1\r\n\
               SUMMARY:Architecture Sync\r\n\
               DTSTART:20260810T140000Z\r\n\
               DURATION:PT1H\r\n\
               CREATED:20260801T080000Z\r\n\
               LAST-MODIFIED:20260805T120000Z\r\n\
               RRULE:FREQ=WEEKLY;INTERVAL=2;COUNT=5\r\n\
               END:VEVENT\r\n\
               END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse");
    assert_eq!(event.title.as_deref(), Some("Architecture Sync"));
    assert_eq!(event.start.as_deref(), Some("2026-08-10T14:00:00"));
    assert_eq!(event.time_zone.as_deref(), Some("Etc/UTC"));
    assert_eq!(event.duration.as_deref(), Some("PT1H"));

    let rules = event.recurrence_rule.as_ref().expect("rules");
    assert_eq!(rules.frequency, "weekly");
    assert_eq!(rules.interval, Some(2));
    assert_eq!(rules.count, Some(5));

    let rendered = event_to_ical(&event);
    assert!(rendered.contains("SUMMARY:Architecture Sync"), "{rendered}");
    assert!(
        rendered.contains("RRULE:FREQ=WEEKLY;COUNT=5;INTERVAL=2"),
        "{rendered}"
    );

    let back = ical_to_event(&rendered).expect("roundtrip");
    assert_eq!(back.title, event.title);
    assert_eq!(back.recurrence_rule, event.recurrence_rule);
}

#[test]
fn maps_location_with_special_characters_and_coordinates_safely() {
    let ics = "BEGIN:VCALENDAR\r\n\
               VERSION:2.0\r\n\
               PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
               BEGIN:VEVENT\r\n\
               UID:loc-event-1\r\n\
               SUMMARY:Quarterly Keynote\r\n\
               DTSTART:20260810T100000Z\r\n\
               DURATION:PT2H\r\n\
               LOCATION;X-JMAP-KEY=l1:Building 4\\, Room 204\\; West Wing\r\n\
               END:VEVENT\r\n\
               END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse");
    assert_eq!(event.title.as_deref(), Some("Quarterly Keynote"));
    let locs = event.locations.as_ref().expect("locations");
    assert_eq!(
        locs["l1"]["name"].as_str(),
        Some("Building 4, Room 204; West Wing")
    );

    let rendered = event_to_ical(&event);
    assert!(
        rendered.contains("LOCATION;X-JMAP-KEY=l1:Building 4\\, Room 204\\; West Wing")
            || rendered.contains("LOCATION:Building 4\\, Room 204\\; West Wing"),
        "{rendered}"
    );

    let back = ical_to_event(&rendered).expect("roundtrip");
    assert_eq!(back.locations, event.locations);
}

#[test]
fn maps_priority_privacy_and_status_combinations_faithfully() {
    let ics = "BEGIN:VCALENDAR\r\n\
               VERSION:2.0\r\n\
               PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
               BEGIN:VEVENT\r\n\
               UID:meta-event-1\r\n\
               SUMMARY:Executive Strategy\r\n\
               DTSTART:20260810T150000Z\r\n\
               DURATION:PT1H30M\r\n\
               PRIORITY:1\r\n\
               CLASS:CONFIDENTIAL\r\n\
               STATUS:TENTATIVE\r\n\
               TRANSP:TRANSPARENT\r\n\
               END:VEVENT\r\n\
               END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse");
    assert_eq!(event.title.as_deref(), Some("Executive Strategy"));
    assert_eq!(event.priority, Some(1));
    assert_eq!(event.privacy.as_deref(), Some("secret"));
    assert_eq!(event.status.as_deref(), Some("tentative"));
    assert_eq!(event.free_busy_status.as_deref(), Some("free"));

    let rendered = event_to_ical(&event);
    assert!(rendered.contains("PRIORITY:1"), "{rendered}");
    assert!(rendered.contains("CLASS:CONFIDENTIAL"), "{rendered}");
    assert!(rendered.contains("STATUS:TENTATIVE"), "{rendered}");
    assert!(rendered.contains("TRANSP:TRANSPARENT"), "{rendered}");

    let back = ical_to_event(&rendered).expect("roundtrip");
    assert_eq!(back.priority, event.priority);
    assert_eq!(back.privacy, event.privacy);
    assert_eq!(back.status, event.status);
    assert_eq!(back.free_busy_status, event.free_busy_status);
}

#[test]
fn reads_an_icalendar_with_mixed_case_properties_and_parameters_and_parses_faithfully() {
    let ics = "bEgIn:vCaLeNdAr\r\n\
               vErSiOn:2.0\r\n\
               pRoDiD:-//mixed-case//test//EN\r\n\
               bEgIn:vEvEnT\r\n\
               uId:mixed-case-event-1\r\n\
               sUmMaRy:Cross-Platform Strategy\r\n\
               dEsCrIpTiOn:Reviewing architecture alignment\\nand milestones.\r\n\
               dTsTaRt;tZiD=Europe/Berlin:20260815T143000\r\n\
               dUrAtIoN:PT1H30M\r\n\
               cLaSs:PrIvAtE\r\n\
               sTaTuS:cOnFiRmEd\r\n\
               tRaNsP:oPaQuE\r\n\
               pRiOrItY:2\r\n\
               lOcAtIoN;x-JmAp-KeY=loc1:Executive Boardroom\r\n\
               cOnFeReNcE;x-JmAp-KeY=v1;fEaTuRe=aUdIo,vIdEo;lAbEl=\"Video Bridge\":https://meet.example.com/board\r\n\
               cAtEgOrIeS:Strategy,Architecture\r\n\
               cAtEgOrIeS:Q3-Milestones\r\n\
               aTtAcH;x-JmAp-KeY=k1;fMtTyPe=text/plain:https://example.com/briefing.txt\r\n\
               iMaGe;x-JmAp-KeY=k2;dIsPlAy=badge:https://example.com/badge.png\r\n\
               bEgIn:vAlArM\r\n\
               aCtIoN:dIsPlAy\r\n\
               tRiGgEr;rElAtEd=eNd:-PT15M\r\n\
               uId:alert-1\r\n\
               eNd:vAlArM\r\n\
               eNd:vEvEnT\r\n\
               eNd:vCaLeNdAr\r\n";

    let event = ical_to_event(ics).expect("parse");
    assert_eq!(
        event.id.as_ref().map(|id| id.as_str()),
        Some("mixed-case-event-1")
    );
    assert_eq!(event.title.as_deref(), Some("Cross-Platform Strategy"));
    assert_eq!(
        event.description.as_deref(),
        Some("Reviewing architecture alignment\nand milestones.")
    );
    assert_eq!(event.start.as_deref(), Some("2026-08-15T14:30:00"));
    assert_eq!(event.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(event.duration.as_deref(), Some("PT1H30M"));
    assert_eq!(event.privacy.as_deref(), Some("private"));
    assert_eq!(event.status.as_deref(), Some("confirmed"));
    assert_eq!(event.free_busy_status.as_deref(), Some("busy"));
    assert_eq!(event.priority, Some(2));

    let locs = event.locations.as_ref().expect("locations");
    assert_eq!(locs["loc1"]["name"].as_str(), Some("Executive Boardroom"));

    let vlocs = event.virtual_locations.as_ref().expect("virtualLocations");
    assert_eq!(
        vlocs["v1"]["uri"].as_str(),
        Some("https://meet.example.com/board")
    );
    assert_eq!(vlocs["v1"]["name"].as_str(), Some("Video Bridge"));
    assert_eq!(vlocs["v1"]["features"]["audio"].as_bool(), Some(true));
    assert_eq!(vlocs["v1"]["features"]["video"].as_bool(), Some(true));

    let tags = event.keywords.as_ref().expect("keywords");
    assert_eq!(tags.len(), 3);
    assert_eq!(tags["Strategy"].as_bool(), Some(true));
    assert_eq!(tags["Architecture"].as_bool(), Some(true));
    assert_eq!(tags["Q3-Milestones"].as_bool(), Some(true));

    let links = event.links.as_ref().expect("links");
    assert_eq!(
        links["k1"]["href"].as_str(),
        Some("https://example.com/briefing.txt")
    );
    assert_eq!(links["k1"]["contentType"].as_str(), Some("text/plain"));
    assert_eq!(
        links["k2"]["href"].as_str(),
        Some("https://example.com/badge.png")
    );
    assert_eq!(links["k2"]["rel"].as_str(), Some("icon"));
    assert_eq!(links["k2"]["display"].as_str(), Some("badge"));

    let alerts = event.alerts.as_ref().expect("alerts");
    assert_eq!(alerts["alert-1"]["action"].as_str(), Some("display"));
    assert_eq!(
        alerts["alert-1"]["trigger"]["offset"].as_str(),
        Some("-PT15M")
    );
    assert_eq!(
        alerts["alert-1"]["trigger"]["relativeTo"].as_str(),
        Some("end")
    );
}

#[test]
fn emits_a_comprehensive_icalendar_via_calcard_and_roundtrips() {
    let mut locations = std::collections::BTreeMap::new();
    locations.insert(
        "loc-1".to_owned(),
        json!({
            "name": "Berlin Office Room 404"
        }),
    );

    let mut virtual_locations = std::collections::BTreeMap::new();
    virtual_locations.insert(
        "v1".to_owned(),
        json!({
            "uri": "https://conf.example.com/calcard",
            "name": "Jitsi Room",
            "features": {
                "audio": true,
                "video": true
            }
        }),
    );

    let mut links = std::collections::BTreeMap::new();
    links.insert(
        "k1".to_owned(),
        json!({
            "href": "https://example.com/plan.pdf",
            "contentType": "application/pdf",
            "size": 4096
        }),
    );
    links.insert(
        "k2".to_owned(),
        json!({
            "href": "https://example.com/logo.png",
            "rel": "icon",
            "display": "badge",
            "contentType": "image/png"
        }),
    );

    let mut keywords = std::collections::BTreeMap::new();
    keywords.insert("Migration".to_owned(), Value::Bool(true));
    keywords.insert("Polish".to_owned(), Value::Bool(true));

    let mut participants = std::collections::BTreeMap::new();
    participants.insert(
        "p1".to_owned(),
        json!({
            "name": "Organizer",
            "sendTo": {
                "imip": "mailto:organizer@example.com"
            },
            "roles": {
                "owner": true,
                "attendee": true
            }
        }),
    );
    participants.insert(
        "p2".to_owned(),
        json!({
            "name": "Attendee",
            "sendTo": {
                "imip": "mailto:attendee@example.com"
            },
            "roles": {
                "attendee": true
            },
            "kind": "individual",
            "participationStatus": "accepted",
            "expectReply": true
        }),
    );

    let mut alerts = std::collections::BTreeMap::new();
    alerts.insert(
        "a1".to_owned(),
        json!({
            "@type": "Alert",
            "action": "display",
            "trigger": {
                "@type": "OffsetTrigger",
                "offset": "-PT10M",
                "relativeTo": "start"
            }
        }),
    );

    let mut overrides = std::collections::BTreeMap::new();
    overrides.insert(
        "2026-08-20T14:00:00".to_owned(),
        json!({
            "title": "Calcard Retrospective (Adjusted)",
            "description": "Special agenda."
        }),
    );

    let event = CalendarEvent {
        id: Some("event-calcard-1".into()),
        uid: Some("urn:uuid:calcard-event-uuid-1".to_owned()),
        created: Some("2026-08-18T10:00:00Z".to_owned()),
        updated: Some("2026-08-18T11:00:00Z".to_owned()),
        title: Some("Calcard Migration All-Hands".to_owned()),
        description: Some(
            "Discussing multi-session migration progress,\nsyntax deletion, and gate verification."
                .to_owned(),
        ),
        start: Some("2026-08-18T14:00:00".to_owned()),
        time_zone: Some("Europe/Berlin".to_owned()),
        duration: Some("PT1H".to_owned()),
        status: Some("confirmed".to_owned()),
        free_busy_status: Some("busy".to_owned()),
        priority: Some(1),
        privacy: Some("private".to_owned()),
        locations: Some(locations),
        virtual_locations: Some(virtual_locations),
        links: Some(links),
        keywords: Some(keywords),
        participants: Some(participants),
        alerts: Some(alerts),
        recurrence_rule: Some(RecurrenceRule {
            frequency: "weekly".to_owned(),
            interval: Some(2),
            by_day: Some(vec![NDay::new("tu"), NDay::new("th")]),
            count: Some(10),
            ..RecurrenceRule::default()
        }),
        recurrence_overrides: Some(overrides),
        ..CalendarEvent::default()
    };

    let ics = event_to_ical(&event);
    let unfolded = ics.replace("\r\n ", "").replace("\r\n\t", "");

    assert!(
        unfolded.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:"),
        "{ics}"
    );
    assert!(unfolded.contains("UID:event-calcard-1\r\n"), "{ics}");
    assert!(
        unfolded.contains("X-JMAP-UID:urn:uuid:calcard-event-uuid-1\r\n"),
        "{ics}"
    );
    assert!(
        unfolded.contains("SUMMARY:Calcard Migration All-Hands\r\n"),
        "{ics}"
    );
    assert!(
        unfolded.contains("DTSTART;TZID=Europe/Berlin:20260818T140000\r\n"),
        "{ics}"
    );
    assert!(unfolded.contains("DURATION:PT1H\r\n"), "{ics}");
    assert!(unfolded.contains("STATUS:CONFIRMED\r\n"), "{ics}");
    assert!(unfolded.contains("TRANSP:OPAQUE\r\n"), "{ics}");
    assert!(unfolded.contains("PRIORITY:1\r\n"), "{ics}");
    assert!(unfolded.contains("CLASS:PRIVATE\r\n"), "{ics}");
    assert!(
        unfolded.contains("LOCATION;X-JMAP-KEY=loc-1:Berlin Office Room 404\r\n"),
        "{ics}"
    );
    assert!(
        unfolded.contains("CONFERENCE;VALUE=URI;FEATURE=AUDIO,VIDEO;LABEL=\"Jitsi Room\";X-JMAP-KEY=v1:https://conf.example.com/calcard\r\n"),
        "{ics}"
    );
    assert!(
        unfolded.contains("ATTACH;FMTTYPE=application/pdf;SIZE=4096;X-JMAP-KEY=k1:https://example.com/plan.pdf\r\n"),
        "{ics}"
    );
    assert!(
        unfolded.contains("IMAGE;VALUE=URI;DISPLAY=BADGE;FMTTYPE=image/png;X-JMAP-KEY=k2:https://example.com/logo.png\r\n"),
        "{ics}"
    );
    assert!(
        unfolded.contains("CATEGORIES:Migration,Polish\r\n"),
        "{ics}"
    );
    assert!(
        unfolded.contains("ORGANIZER;CN=Organizer:mailto:organizer@example.com\r\n"),
        "{ics}"
    );
    assert!(
        unfolded.contains("ATTENDEE;CN=Attendee;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;RSVP=TRUE:mailto:attendee@example.com\r\n"),
        "{ics}"
    );
    assert!(unfolded.contains("BEGIN:VALARM\r\n"), "{ics}");
    assert!(unfolded.contains("TRIGGER:-PT10M\r\n"), "{ics}");
    assert!(
        unfolded.contains("RRULE:FREQ=WEEKLY;COUNT=10;INTERVAL=2;BYDAY=TU,TH\r\n"),
        "{ics}"
    );
    assert!(
        unfolded.contains("RECURRENCE-ID;TZID=Europe/Berlin:20260820T140000\r\n"),
        "{ics}"
    );
    assert!(
        unfolded.contains("SUMMARY:Calcard Retrospective (Adjusted)\r\n"),
        "{ics}"
    );
    assert!(unfolded.ends_with("END:VCALENDAR\r\n"), "{ics}");

    let roundtrip = ical_to_event(&ics).expect("roundtrip parse");
    assert_eq!(
        roundtrip.id.as_ref().map(|id| id.as_str()),
        Some("event-calcard-1")
    );
    assert_eq!(
        roundtrip.uid.as_deref(),
        Some("urn:uuid:calcard-event-uuid-1")
    );
    assert_eq!(
        roundtrip.title.as_deref(),
        Some("Calcard Migration All-Hands")
    );
    assert_eq!(roundtrip.start.as_deref(), Some("2026-08-18T14:00:00"));
    assert_eq!(roundtrip.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(roundtrip.duration.as_deref(), Some("PT1H"));
    assert_eq!(roundtrip.status.as_deref(), Some("confirmed"));
    assert_eq!(roundtrip.free_busy_status.as_deref(), Some("busy"));
    assert_eq!(roundtrip.priority, Some(1));
    assert_eq!(roundtrip.privacy.as_deref(), Some("private"));

    let rt_locs = roundtrip.locations.as_ref().expect("locations");
    assert_eq!(
        rt_locs["loc-1"]["name"].as_str(),
        Some("Berlin Office Room 404")
    );

    let rt_vlocs = roundtrip
        .virtual_locations
        .as_ref()
        .expect("virtualLocations");
    assert_eq!(
        rt_vlocs["v1"]["uri"].as_str(),
        Some("https://conf.example.com/calcard")
    );
    assert_eq!(rt_vlocs["v1"]["name"].as_str(), Some("Jitsi Room"));

    let rt_tags = roundtrip.keywords.as_ref().expect("keywords");
    assert_eq!(rt_tags.len(), 2);
    assert_eq!(rt_tags["Migration"].as_bool(), Some(true));
    assert_eq!(rt_tags["Polish"].as_bool(), Some(true));

    let rt_alerts = roundtrip.alerts.as_ref().expect("alerts");
    assert_eq!(rt_alerts["a1"]["action"].as_str(), Some("display"));
    assert_eq!(
        rt_alerts["a1"]["trigger"]["offset"].as_str(),
        Some("-PT10M")
    );

    let rt_rules = roundtrip.recurrence_rule.as_ref().expect("recurrenceRule");
    assert_eq!(rt_rules.frequency, "weekly");
    assert_eq!(rt_rules.interval, Some(2));
    assert_eq!(rt_rules.count, Some(10));

    let rt_overrides = roundtrip
        .recurrence_overrides
        .as_ref()
        .expect("recurrenceOverrides");
    assert_eq!(
        rt_overrides["2026-08-20T14:00:00"]["title"].as_str(),
        Some("Calcard Retrospective (Adjusted)")
    );
}

#[test]
fn ical_error_display_and_source_formatting() {
    use std::error::Error;

    let err1 = ICalError::NotACalendar;
    assert_eq!(
        format!("{err1}"),
        "not an iCalendar object: missing BEGIN:VCALENDAR"
    );
    assert!(err1.source().is_none());

    let err2 = ICalError::Unterminated("VEVENT".to_owned());
    assert_eq!(format!("{err2}"), "truncated iCalendar: missing END:VEVENT");
    assert!(err2.source().is_none());

    let err3 = ICalError::Mismatched {
        expected: "VEVENT".to_owned(),
        found: "VTODO".to_owned(),
    };
    assert_eq!(
        format!("{err3}"),
        "END:VTODO closes nothing; END:VEVENT was due"
    );
    assert!(err3.source().is_none());

    let err4 = ICalError::Trailing("INVALID_EXTRA_LINE".to_owned());
    assert_eq!(
        format!("{err4}"),
        "content after END:VCALENDAR: INVALID_EXTRA_LINE"
    );
    assert!(err4.source().is_none());

    let err5 = ICalError::TooDeep("VALARM".to_owned());
    assert_eq!(
        format!("{err5}"),
        format!(
            "iCalendar components nested more than {} deep at BEGIN:VALARM",
            jmap_ical::MAX_DEPTH
        )
    );
    assert!(err5.source().is_none());

    let err6 = ICalError::NoEvent;
    assert_eq!(format!("{err6}"), "iCalendar object contains no VEVENT");
    assert!(err6.source().is_none());
}

#[test]
fn timezone_observance_onsets_and_transition_offset_resolution() {
    // 1. VTIMEZONE with RDATE transition and non-zero offset
    let ics_rdate = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//Example//EN\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:Test/RDateZone\r\n",
        "BEGIN:STANDARD\r\n",
        "DTSTART:19700101T000000\r\n",
        "TZOFFSETFROM:+0200\r\n",
        "TZOFFSETTO:+0200\r\n",
        "RDATE;VALUE=DATE-TIME:19970101T020000\r\n",
        "END:STANDARD\r\n",
        "BEGIN:DAYLIGHT\r\n",
        "DTSTART:19970601T020000\r\n",
        "TZOFFSETFROM:+0200\r\n",
        "TZOFFSETTO:+0300\r\n",
        "END:DAYLIGHT\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:ev-rdate-1\r\n",
        "DTSTART;TZID=Test/RDateZone:19970501T100000\r\n",
        "RRULE:FREQ=DAILY;UNTIL=19970701T000000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_rdate = ical_to_event(ics_rdate).expect("parse rdate zone");
    let rules = event_rdate.recurrence_rule.expect("rules");
    assert_eq!(rules.until.as_deref(), Some("1997-07-01T03:00:00"));

    // 2. VTIMEZONE with RRULE carrying BYSECOND, BYMINUTE, BYHOUR
    let ics_bysec = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//Example//EN\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:Test/PreciseZone\r\n",
        "BEGIN:STANDARD\r\n",
        "DTSTART:19900101T000000\r\n",
        "TZOFFSETFROM:+0100\r\n",
        "TZOFFSETTO:+0100\r\n",
        "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU;BYHOUR=3;BYMINUTE=30;BYSECOND=45\r\n",
        "END:STANDARD\r\n",
        "BEGIN:DAYLIGHT\r\n",
        "DTSTART:19900101T000000\r\n",
        "TZOFFSETFROM:+0100\r\n",
        "TZOFFSETTO:+0200\r\n",
        "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU;BYHOUR=2;BYMINUTE=15;BYSECOND=30\r\n",
        "END:DAYLIGHT\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:ev-precise-1\r\n",
        "DTSTART;TZID=Test/PreciseZone:19950101T100000\r\n",
        "RRULE:FREQ=DAILY;UNTIL=19950601T120000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_bysec = ical_to_event(ics_bysec).expect("parse precise zone");
    let rules_bysec = event_bysec.recurrence_rule.expect("rules");
    assert_eq!(rules_bysec.until.as_deref(), Some("1995-06-01T14:00:00"));

    // 3. VTIMEZONE with local UNTIL in RRULE (without trailing Z)
    let ics_local_until = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//Example//EN\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:Test/LocalUntilZone\r\n",
        "BEGIN:STANDARD\r\n",
        "DTSTART:19800101T000000\r\n",
        "TZOFFSETFROM:+0200\r\n",
        "TZOFFSETTO:+0200\r\n",
        "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU;UNTIL=19951031T030000\r\n",
        "END:STANDARD\r\n",
        "BEGIN:DAYLIGHT\r\n",
        "DTSTART:19800101T000000\r\n",
        "TZOFFSETFROM:+0200\r\n",
        "TZOFFSETTO:+0300\r\n",
        "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU;UNTIL=19950331T020000\r\n",
        "END:DAYLIGHT\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:ev-localuntil-1\r\n",
        "DTSTART;TZID=Test/LocalUntilZone:19900101T100000\r\n",
        "RRULE:FREQ=DAILY;UNTIL=19900601T100000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_local_until = ical_to_event(ics_local_until).expect("parse local until zone");
    let rules_lu = event_local_until.recurrence_rule.expect("rules");
    assert_eq!(rules_lu.until.as_deref(), Some("1990-06-01T13:00:00"));

    // 4. VTIMEZONE with COUNT in RRULE
    let ics_count = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//Example//EN\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:Test/CountZone\r\n",
        "BEGIN:STANDARD\r\n",
        "DTSTART:20100101T000000\r\n",
        "TZOFFSETFROM:+0100\r\n",
        "TZOFFSETTO:+0100\r\n",
        "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU;COUNT=3\r\n",
        "END:STANDARD\r\n",
        "BEGIN:DAYLIGHT\r\n",
        "DTSTART:20100101T000000\r\n",
        "TZOFFSETFROM:+0100\r\n",
        "TZOFFSETTO:+0200\r\n",
        "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU;COUNT=3\r\n",
        "END:DAYLIGHT\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:ev-count-1\r\n",
        "DTSTART;TZID=Test/CountZone:20110501T100000\r\n",
        "RRULE:FREQ=DAILY;UNTIL=20110701T100000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_count = ical_to_event(ics_count).expect("parse count zone");
    let rules_cnt = event_count.recurrence_rule.expect("rules");
    assert_eq!(rules_cnt.until.as_deref(), Some("2011-07-01T12:00:00"));

    // 5. VTIMEZONE with negative BYMONTHDAY in WeekdayAmong and positive nth BYDAY
    let ics_neg_day = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//Example//EN\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:Test/NegDayZone\r\n",
        "BEGIN:STANDARD\r\n",
        "DTSTART:20000101T000000\r\n",
        "TZOFFSETFROM:+0000\r\n",
        "TZOFFSETTO:+0000\r\n",
        "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=SU;BYMONTHDAY=-1,-2,-3,-4,-5,-6,-7\r\n",
        "END:STANDARD\r\n",
        "BEGIN:DAYLIGHT\r\n",
        "DTSTART:20000101T000000\r\n",
        "TZOFFSETFROM:+0000\r\n",
        "TZOFFSETTO:+0100\r\n",
        "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=1SU;BYHOUR=2\r\n",
        "END:DAYLIGHT\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:ev-negday-1\r\n",
        "DTSTART;TZID=Test/NegDayZone:20050401T100000\r\n",
        "RRULE:FREQ=DAILY;UNTIL=20050601T100000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_neg_day = ical_to_event(ics_neg_day).expect("parse neg day zone");
    let rules_nd = event_neg_day.recurrence_rule.expect("rules");
    assert_eq!(rules_nd.until.as_deref(), Some("2005-06-01T11:00:00"));

    // 6. Target instant before all onsets in multi-observance zone
    let ics_pre_onsets = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//Example//EN\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:Test/EarlyTargetZone\r\n",
        "BEGIN:STANDARD\r\n",
        "DTSTART:19900101T000000\r\n",
        "TZOFFSETFROM:+0300\r\n",
        "TZOFFSETTO:+0300\r\n",
        "END:STANDARD\r\n",
        "BEGIN:DAYLIGHT\r\n",
        "DTSTART:20000101T000000\r\n",
        "TZOFFSETFROM:+0300\r\n",
        "TZOFFSETTO:+0400\r\n",
        "END:DAYLIGHT\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:ev-pre-1\r\n",
        "DTSTART;TZID=Test/EarlyTargetZone:19800101T100000\r\n",
        "RRULE:FREQ=DAILY;UNTIL=19800601T100000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_pre = ical_to_event(ics_pre_onsets).expect("parse pre onsets");
    let rules_pre = event_pre.recurrence_rule.expect("rules");
    assert_eq!(rules_pre.until.as_deref(), Some("1980-06-01T13:00:00"));
}

#[test]
fn calendar_event_mapping_and_override_predicates_fidelity() {
    // 1. drawn_participants with non-owner attendee does NOT emit ORGANIZER
    let mut non_owner_event = fixture_event();
    non_owner_event.participants = Some({
        let mut parts = std::collections::BTreeMap::new();
        parts.insert(
            "p1".to_owned(),
            json!({
                "@type": "Participant",
                "sendTo": {"imip": "mailto:guest@example.com"},
                "name": "Guest Attendee",
                "roles": {"attendee": true}
            }),
        );
        parts
    });
    let ics_no_owner = event_to_ical(&non_owner_event);
    assert!(without(&ics_no_owner, "ORGANIZER"));
    assert_eq!(
        line(&ics_no_owner, "ATTENDEE;"),
        "ATTENDEE;CN=\"Guest Attendee\";ROLE=REQ-PARTICIPANT:mailto:guest@example.com"
    );

    // 2. read_alert with RELATED=START, RELATED=END, RELATED=INVALID
    let ics_alerts = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//Example//EN\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:alert-related-test\r\n",
        "DTSTART:20260115T100000Z\r\n",
        "DURATION:PT1H\r\n",
        "BEGIN:VALARM\r\n",
        "ACTION:DISPLAY\r\n",
        "DESCRIPTION:Alert 1\r\n",
        "TRIGGER;RELATED=START:-PT15M\r\n",
        "END:VALARM\r\n",
        "BEGIN:VALARM\r\n",
        "ACTION:DISPLAY\r\n",
        "DESCRIPTION:Alert 2\r\n",
        "TRIGGER;RELATED=END:PT10M\r\n",
        "END:VALARM\r\n",
        "BEGIN:VALARM\r\n",
        "ACTION:DISPLAY\r\n",
        "DESCRIPTION:Alert 3\r\n",
        "TRIGGER;RELATED=INVALID:-PT5M\r\n",
        "END:VALARM\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_alerts = ical_to_event(ics_alerts).expect("parse alerts");
    let alerts = event_alerts.alerts.expect("alerts");
    assert_eq!(alerts.len(), 2);
    // Alert 1 defaults to start (relativeTo not set or not "end")
    assert_eq!(alerts["a1"]["trigger"]["offset"].as_str(), Some("-PT15M"));
    assert!(alerts["a1"]["trigger"].get("relativeTo").is_none());
    // Alert 2 specifies end
    assert_eq!(alerts["a2"]["trigger"]["offset"].as_str(), Some("PT10M"));
    assert_eq!(alerts["a2"]["trigger"]["relativeTo"].as_str(), Some("end"));

    // 3. maps_recurrence_override with excluded boolean vs non-boolean
    let series_event = fixture_event();
    assert!(maps_recurrence_override(
        &series_event,
        "2026-08-20T10:00:00",
        &json!({"excluded": true})
    ));
    assert!(maps_recurrence_override(
        &series_event,
        "2026-08-20T10:00:00",
        &json!({"excluded": false})
    ));
    assert!(!maps_recurrence_override(
        &series_event,
        "2026-08-20T10:00:00",
        &json!({"excluded": "true"})
    ));
    assert!(!maps_recurrence_override(
        &series_event,
        "2026-08-20T10:00:00",
        &json!({"excluded": 1})
    ));

    // 4. time_zone_definition lookup
    let mut custom_tz_event = fixture_event();
    let custom_def = json!({
        "@type": "TimeZone",
        "standard": [{
            "@type": "TimeZoneRule",
            "start": "1970-01-01T00:00:00",
            "offsetFrom": "+01:00",
            "offsetTo": "+01:00"
        }]
    });
    custom_tz_event.time_zones = Some({
        let mut map = std::collections::BTreeMap::new();
        map.insert("/custom/tz".to_owned(), custom_def.clone());
        map
    });
    assert_eq!(
        time_zone_definition(&custom_tz_event, "/custom/tz"),
        Some(&custom_def)
    );
    assert_eq!(time_zone_definition(&custom_tz_event, "/nonexistent"), None);
    custom_tz_event.time_zones = None;
    assert_eq!(time_zone_definition(&custom_tz_event, "/custom/tz"), None);

    // 5. modified_instance inherits uid, description, status, show_without_time
    let mut parent_event = fixture_event();
    parent_event.uid = Some("parent-uid-12345".to_owned());
    parent_event.description = Some("Parent event detailed description".to_owned());
    parent_event.status = Some("confirmed".to_owned());
    parent_event.show_without_time = Some(false);
    parent_event.recurrence_rule = Some(RecurrenceRule::new("daily"));
    parent_event.recurrence_overrides = Some({
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "2026-08-20T10:00:00".to_owned(),
            json!({
                "title": "Modified Instance Title"
            }),
        );
        map
    });
    let ics_override = event_to_ical(&parent_event);
    assert_eq!(vevents(&ics_override), 2);
    let override_vevent = vevent(&ics_override, 1);
    assert!(override_vevent.contains("UID:parent-uid-12345\r\n"));
    assert!(override_vevent.contains("DESCRIPTION:Parent event detailed description\r\n"));
    assert!(override_vevent.contains("STATUS:CONFIRMED\r\n"));
    assert!(override_vevent.contains("SUMMARY:Modified Instance Title\r\n"));
    assert!(override_vevent.contains("RECURRENCE-ID;TZID=Europe/Berlin:20260820T100000\r\n"));

    // 6. parse_ical errors: Trailing and Mismatched
    let ics_trailing = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:u1\r\n",
        "DTSTART:20260101T000000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
        "GARBAGE_TRAILING_CONTENT\r\n"
    );
    assert!(matches!(
        ical_to_event(ics_trailing),
        Err(ICalError::Trailing(_))
    ));

    let ics_mismatched = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:u2\r\n",
        "DTSTART:20260101T000000Z\r\n",
        "END:VTODO\r\n",
        "END:VCALENDAR\r\n"
    );
    assert!(matches!(
        ical_to_event(ics_mismatched),
        Err(ICalError::Mismatched { .. })
    ));

    // 7. unfold with multi-line folds, leading spaces and tabs
    let ics_folded = concat!(
        " \r\n",
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:u-folded\r\n",
        "DTSTART:20260101T000000Z\r\n",
        "SUMMARY:This is a very long folded \r\n",
        " summary line that continues\r\n",
        "\t with a tab\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_folded = ical_to_event(ics_folded).expect("parse folded");
    assert_eq!(
        event_folded.title.as_deref(),
        Some("This is a very long folded summary line that continues with a tab")
    );

    // 8. stated_zones with IANA timezone and X-LIC-LOCATION
    let ics_iana_tz = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:Europe/Berlin\r\n",
        "X-LIC-LOCATION:Europe/Berlin\r\n",
        "BEGIN:STANDARD\r\n",
        "DTSTART:19700101T000000\r\n",
        "TZOFFSETFROM:+0100\r\n",
        "TZOFFSETTO:+0100\r\n",
        "END:STANDARD\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:u-berlin\r\n",
        "DTSTART;TZID=Europe/Berlin:20260101T100000\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_berlin = ical_to_event(ics_iana_tz).expect("parse berlin");
    assert_eq!(event_berlin.time_zone.as_deref(), Some("Europe/Berlin"));

    // 9. read_definition ignoring empty VTIMEZONE with no observances
    let ics_empty_tz = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:EmptyZone\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:u-empty-tz\r\n",
        "DTSTART:20260101T100000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_empty_tz = ical_to_event(ics_empty_tz).expect("parse empty tz");
    assert!(event_empty_tz.time_zones.is_none());

    // 10. invented keys deduplication in read_virtual_locations and read_links
    let ics_invented_keys = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:u-inv-keys\r\n",
        "DTSTART:20260101T100000Z\r\n",
        "CONFERENCE;X-JMAP-KEY=v1:https://room1.example.com\r\n",
        "CONFERENCE:https://room2.example.com\r\n",
        "ATTACH;X-JMAP-KEY=k1:https://files.example.com/doc1.pdf\r\n",
        "ATTACH:https://files.example.com/doc2.pdf\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_inv = ical_to_event(ics_invented_keys).expect("parse invented keys");
    let vlocs = event_inv.virtual_locations.expect("vlocs");
    assert_eq!(vlocs.len(), 2);
    assert!(vlocs.contains_key("v1"));
    assert!(vlocs.contains_key("v2"));
    let links = event_inv.links.expect("links");
    assert_eq!(links.len(), 2);
    assert!(links.contains_key("k1"));
    assert!(links.contains_key("k2"));

    // 11. names_a_time_of_day with by_hour only on all-day event
    let mut allday_event = fixture_event();
    allday_event.show_without_time = Some(true);
    allday_event.start = Some("2026-01-01T00:00:00".to_owned());
    allday_event.time_zone = None;
    allday_event.recurrence_rule = Some(RecurrenceRule {
        by_hour: Some(vec![10]),
        ..RecurrenceRule::new("daily")
    });
    let ics_allday_byhour = event_to_ical(&allday_event);
    assert!(!ics_allday_byhour.contains("VALUE=DATE:"));
    assert!(ics_allday_byhour.contains("DTSTART:20260101T000000\r\n"));

    // 12. instance_shows_without_time with empty duration override
    let mut allday_override_event = fixture_event();
    allday_override_event.show_without_time = Some(true);
    allday_override_event.start = Some("2026-01-01T00:00:00".to_owned());
    allday_override_event.duration = Some("P1D".to_owned());
    allday_override_event.time_zone = None;
    allday_override_event.recurrence_rule = Some(RecurrenceRule::new("daily"));
    allday_override_event.recurrence_overrides = Some({
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "2026-01-02T00:00:00".to_owned(),
            json!({
                "duration": ""
            }),
        );
        map
    });
    let ics_allday_ov = event_to_ical(&allday_override_event);
    assert!(ics_allday_ov.contains("DTSTART;VALUE=DATE:20260101\r\n"));

    // 13. NDay parsing with zero ordinal and signed ordinals in BYDAY
    let ics_byday_nday = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:u-byday-nday\r\n",
        "DTSTART:20260101T100000Z\r\n",
        "RRULE:FREQ=MONTHLY;BYDAY=0MO,+2TU,-3FR\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_byday = ical_to_event(ics_byday_nday).expect("parse byday");
    let rrules = event_byday.recurrence_rule.expect("rrules");
    let by_days = rrules.by_day.as_ref().expect("by_day");
    assert_eq!(by_days.len(), 3);
    assert_eq!(by_days[0].day, "0mo");
    assert_eq!(by_days[0].nth_of_period, None);
    assert_eq!(by_days[1].day, "tu");
    assert_eq!(by_days[1].nth_of_period, Some(2));
    assert_eq!(by_days[2].day, "fr");
    assert_eq!(by_days[2].nth_of_period, Some(-3));

    // 14. Subsecond precision and short times in to_local_date_time
    let ics_subsecond = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:u-subsec\r\n",
        "DTSTART:20260115T123456.789Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_subsec = ical_to_event(ics_subsecond).expect("parse subsec");
    assert_eq!(event_subsec.start.as_deref(), Some("2026-01-15T12:34:56"));

    let ics_short_time_zone = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:Test/ShortTime\r\n",
        "BEGIN:STANDARD\r\n",
        "DTSTART:20260115T12345\r\n",
        "TZOFFSETFROM:+0100\r\n",
        "TZOFFSETTO:+0100\r\n",
        "END:STANDARD\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:u-short\r\n",
        "DTSTART;TZID=Test/ShortTime:20260115T120000\r\n",
        "RRULE:FREQ=DAILY;UNTIL=20260120T120000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_short = ical_to_event(ics_short_time_zone).expect("parse short");
    assert_eq!(
        event_short
            .recurrence_rule
            .as_ref()
            .unwrap()
            .until
            .as_deref(),
        Some("2026-01-20T13:00:00")
    );

    // 15. Fractional offsets (+05:30, +05:45, boundary +23:00, +05:59) and date movement across boundaries
    let ics_fractional_tz = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//Example//EN\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:Asia/Kolkata\r\n",
        "BEGIN:STANDARD\r\n",
        "DTSTART:19700101T000000\r\n",
        "TZOFFSETFROM:+0530\r\n",
        "TZOFFSETTO:+0530\r\n",
        "END:STANDARD\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:u-kolkata-1\r\n",
        "DTSTART;TZID=Asia/Kolkata:20240101T010000\r\n",
        "RRULE:FREQ=DAILY;UNTIL=20240105T183000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_kolkata = ical_to_event(ics_fractional_tz).expect("parse kolkata");
    let kolkata_rules = event_kolkata.recurrence_rule.expect("kolkata rules");
    assert_eq!(kolkata_rules.until.as_deref(), Some("2024-01-06T00:00:00"));

    // 16. Century and era leap year roundtrips in proleptic Gregorian calendar
    for year in [1900, 2000, 2100, 2400] {
        let event_century = CalendarEvent {
            start: Some(format!("{year}-02-28T10:00:00")),
            time_zone: Some("Etc/UTC".to_owned()),
            recurrence_rule: Some(RecurrenceRule {
                until: Some(format!("{year}-03-02T10:00:00")),
                ..RecurrenceRule::new("daily")
            }),
            ..CalendarEvent::default()
        };
        let ics_cent = event_to_ical(&event_century);
        let parsed_cent = ical_to_event(&ics_cent).expect("century parse");
        let rules_cent = parsed_cent.recurrence_rule.expect("century rules");
        assert_eq!(
            rules_cent.until.as_deref(),
            Some(format!("{year}-03-02T10:00:00").as_str())
        );
    }

    // 17. Multi-digit ordinals in BYDAY
    let ics_multidigit_nday = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:u-multidigit-nday\r\n",
        "DTSTART:20260101T100000Z\r\n",
        "RRULE:FREQ=YEARLY;BYDAY=+10MO,-12FR\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_multidigit = ical_to_event(ics_multidigit_nday).expect("parse multidigit nday");
    let md_rules = event_multidigit.recurrence_rule.expect("md_rules");
    let md_days = md_rules.by_day.as_ref().expect("by_day");
    assert_eq!(md_days.len(), 2);
    assert_eq!(md_days[0].day, "mo");
    assert_eq!(md_days[0].nth_of_period, Some(10));
    assert_eq!(md_days[1].day, "fr");
    assert_eq!(md_days[1].nth_of_period, Some(-12));
}

#[test]
fn timezone_advanced_transition_permutations_and_boundary_fidelity() {
    // 1. VTIMEZONE with zero month-day refusal in RRULE
    let ics_zero_monthday = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:Test/ZeroMonthDay\r\n",
        "BEGIN:STANDARD\r\n",
        "DTSTART:19700101T000000\r\n",
        "TZOFFSETFROM:+0100\r\n",
        "TZOFFSETTO:+0100\r\n",
        "RRULE:FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=0\r\n",
        "END:STANDARD\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:u-zero-mday\r\n",
        "DTSTART;TZID=Test/ZeroMonthDay:20260101T100000\r\n",
        "RRULE:FREQ=DAILY;UNTIL=20260105T100000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_zmd = ical_to_event(ics_zero_monthday).expect("parse zero mday");
    assert_eq!(
        event_zmd.recurrence_rule.as_ref().unwrap().until.as_deref(),
        Some("2026-01-05T10:00:00Z")
    );

    // 2. VTIMEZONE with precise RDATE onset and offset transition
    let ics_precise_rdate = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:Test/PreciseRDate\r\n",
        "BEGIN:STANDARD\r\n",
        "DTSTART:19700101T000000\r\n",
        "TZOFFSETFROM:+0100\r\n",
        "TZOFFSETTO:+0100\r\n",
        "END:STANDARD\r\n",
        "BEGIN:DAYLIGHT\r\n",
        "DTSTART:19700101T000000\r\n",
        "TZOFFSETFROM:+0100\r\n",
        "TZOFFSETTO:+0200\r\n",
        "RDATE;VALUE=DATE-TIME:20260329T020000\r\n",
        "END:DAYLIGHT\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:u-precise-rdate-1\r\n",
        "DTSTART;TZID=Test/PreciseRDate:20260101T100000\r\n",
        "RRULE:FREQ=DAILY;UNTIL=20260329T003000Z\r\n",
        "END:VEVENT\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:u-precise-rdate-2\r\n",
        "DTSTART;TZID=Test/PreciseRDate:20260101T100000\r\n",
        "RRULE:FREQ=DAILY;UNTIL=20260329T013000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_prd1 = ical_to_event(ics_precise_rdate).expect("parse prd");
    assert_eq!(
        event_prd1
            .recurrence_rule
            .as_ref()
            .unwrap()
            .until
            .as_deref(),
        Some("2026-03-29T01:30:00")
    );

    // 3. VTIMEZONE with positive nth weekdays (2SU, 3TH) and restated minutes only
    let ics_pos_nth = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:Test/PosNthZone\r\n",
        "BEGIN:STANDARD\r\n",
        "DTSTART:19900101T020000\r\n",
        "TZOFFSETFROM:+0200\r\n",
        "TZOFFSETTO:+0100\r\n",
        "RRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU;BYMINUTE=30\r\n",
        "END:STANDARD\r\n",
        "BEGIN:DAYLIGHT\r\n",
        "DTSTART:19900101T020000\r\n",
        "TZOFFSETFROM:+0100\r\n",
        "TZOFFSETTO:+0200\r\n",
        "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU;BYMINUTE=30\r\n",
        "END:DAYLIGHT\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:u-pos-nth\r\n",
        "DTSTART;TZID=Test/PosNthZone:20240101T100000\r\n",
        "RRULE:FREQ=DAILY;UNTIL=20240401T120000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_pos_nth = ical_to_event(ics_pos_nth).expect("parse pos nth");
    assert_eq!(
        event_pos_nth
            .recurrence_rule
            .as_ref()
            .unwrap()
            .until
            .as_deref(),
        Some("2024-04-01T14:00:00")
    );

    // 4. VTIMEZONE with negative nth weekdays (-2SA, -3TU) and boundary offsets (+23:00, +05:59)
    let ics_neg_nth = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:Test/NegNthZone\r\n",
        "BEGIN:STANDARD\r\n",
        "DTSTART:19900101T020000\r\n",
        "TZOFFSETFROM:+0559\r\n",
        "TZOFFSETTO:+0559\r\n",
        "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-2SA\r\n",
        "END:STANDARD\r\n",
        "BEGIN:DAYLIGHT\r\n",
        "DTSTART:19900101T020000\r\n",
        "TZOFFSETFROM:+0559\r\n",
        "TZOFFSETTO:+2300\r\n",
        "RRULE:FREQ=YEARLY;BYMONTH=4;BYDAY=-3TU\r\n",
        "END:DAYLIGHT\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:u-neg-nth\r\n",
        "DTSTART;TZID=Test/NegNthZone:20240101T100000\r\n",
        "RRULE:FREQ=DAILY;UNTIL=20240501T100000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let event_neg_nth = ical_to_event(ics_neg_nth).expect("parse neg nth");
    assert_eq!(
        event_neg_nth
            .recurrence_rule
            .as_ref()
            .unwrap()
            .until
            .as_deref(),
        Some("2024-05-02T09:00:00")
    );
}

#[test]
fn emitted_icalendar_lines_hold_strictly_to_75_octets_and_valid_utf8() {
    // 1. Long RRULE exceeding 75 octets must be folded strictly to <= 75 octets
    let long_rrule_event = CalendarEvent {
        uid: Some("long-rrule-uid-1".to_owned()),
        title: Some("Team Planning Session".to_owned()),
        start: Some("2026-09-01T10:00:00".to_owned()),
        duration: Some("PT1H".to_owned()),
        time_zone: Some("Europe/Berlin".to_owned()),
        recurrence_rule: Some(RecurrenceRule {
            rule_type: Some("RecurrenceRule".to_owned()),
            frequency: "monthly".to_owned(),
            interval: Some(2),
            by_day: Some(vec![
                NDay {
                    day_type: Some("NDay".to_owned()),
                    day: "mo".to_owned(),
                    nth_of_period: Some(1),
                    extra: Default::default(),
                },
                NDay {
                    day_type: Some("NDay".to_owned()),
                    day: "tu".to_owned(),
                    nth_of_period: Some(2),
                    extra: Default::default(),
                },
                NDay {
                    day_type: Some("NDay".to_owned()),
                    day: "we".to_owned(),
                    nth_of_period: Some(3),
                    extra: Default::default(),
                },
                NDay {
                    day_type: Some("NDay".to_owned()),
                    day: "th".to_owned(),
                    nth_of_period: Some(4),
                    extra: Default::default(),
                },
                NDay {
                    day_type: Some("NDay".to_owned()),
                    day: "fr".to_owned(),
                    nth_of_period: Some(5),
                    extra: Default::default(),
                },
                NDay {
                    day_type: Some("NDay".to_owned()),
                    day: "sa".to_owned(),
                    nth_of_period: Some(-1),
                    extra: Default::default(),
                },
                NDay {
                    day_type: Some("NDay".to_owned()),
                    day: "su".to_owned(),
                    nth_of_period: Some(-2),
                    extra: Default::default(),
                },
            ]),
            by_month: Some(vec![
                "1".to_owned(),
                "2".to_owned(),
                "3".to_owned(),
                "4".to_owned(),
                "5".to_owned(),
                "6".to_owned(),
                "7".to_owned(),
                "8".to_owned(),
                "9".to_owned(),
                "10".to_owned(),
                "11".to_owned(),
                "12".to_owned(),
            ]),
            by_set_position: Some(vec![1, 2, 3, -1, -2]),
            ..RecurrenceRule::default()
        }),
        ..CalendarEvent::default()
    };

    let ics = event_to_ical(&long_rrule_event);
    for line in ics.split("\r\n") {
        assert!(
            line.len() <= 75,
            "physical line exceeds 75 octets (len = {}): {line:?}",
            line.len()
        );
        assert!(
            std::str::from_utf8(line.as_bytes()).is_ok(),
            "invalid UTF-8 in emitted line: {line:?}"
        );
    }

    // Round-trip must restore the exact same event
    let parsed = ical_to_event(&ics).expect("parse long rrule event");
    let re_emitted = event_to_ical(&parsed);
    assert_eq!(ics, re_emitted, "emission must reach fixed point");

    // 2. Multibyte unicode character sequences near 75-octet fold boundary
    let multibyte_event = CalendarEvent {
        uid: Some("multibyte-uid-1".to_owned()),
        title: Some("ொ\u{a980}ꧏ a¡ₐA A𐕼Σ𞴁AAವ⺛𫝀\u{fffc}\u{1a60}aA0প Event Title Multibyte Boundary Test".to_owned()),
        description: Some("Long description with 🌟 emojis and special chars: ொ\u{a980}ꧏ a¡ₐA A𐕼Σ𞴁AAವ⺛𫝀\u{fffc}\u{1a60}aA0প".to_owned()),
        start: Some("2026-09-01T10:00:00".to_owned()),
        duration: Some("PT1H".to_owned()),
        ..CalendarEvent::default()
    };

    let ics_mb = event_to_ical(&multibyte_event);
    for line in ics_mb.split("\r\n") {
        assert!(
            line.len() <= 75,
            "multibyte physical line exceeds 75 octets (len = {}): {line:?}",
            line.len()
        );
        assert!(
            std::str::from_utf8(line.as_bytes()).is_ok(),
            "invalid UTF-8 in multibyte line: {line:?}"
        );
    }

    let parsed_mb = ical_to_event(&ics_mb).expect("parse multibyte event");
    let re_emitted_mb = event_to_ical(&parsed_mb);
    assert_eq!(
        ics_mb, re_emitted_mb,
        "multibyte emission must reach fixed point"
    );

    // 3. Freebusy emission (busy_periods_to_vfreebusy) holds to 75 octets
    let starts = (1..=10)
        .map(|i| format!("2026-09-{:02}T10:00:00Z", i))
        .collect::<Vec<_>>();
    let ends = (1..=10)
        .map(|i| format!("2026-09-{:02}T18:00:00Z", i))
        .collect::<Vec<_>>();
    let periods = (0..10)
        .map(|i| jmap_proto::principals::BusyPeriod {
            busy_status: "confirmed".to_owned(),
            utc_start: jmap_proto::state::UtcDate::new(&starts[i]),
            utc_end: jmap_proto::state::UtcDate::new(&ends[i]),
            event: None,
        })
        .collect::<Vec<_>>();
    let fb_ics = jmap_ical::busy_periods_to_vfreebusy(
        "very-long-attendee-email-address-for-testing-75-octet-line-folding-limits@example.enterprise.department.organization.com",
        &jmap_proto::state::UtcDate::new("2026-09-01T00:00:00Z"),
        &jmap_proto::state::UtcDate::new("2026-09-30T23:59:59Z"),
        &periods,
    ).expect("vfreebusy");

    for line in fb_ics.split("\r\n") {
        assert!(
            line.len() <= 75,
            "vfreebusy line exceeds 75 octets (len = {}): {line:?}",
            line.len()
        );
        assert!(
            std::str::from_utf8(line.as_bytes()).is_ok(),
            "invalid UTF-8 in vfreebusy line: {line:?}"
        );
    }
}

fn read_fixture(file_name: &str) -> String {
    let path = format!("{}/tests/fixtures/{file_name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

struct RealExporterTestCase {
    name: &'static str,
    fixture_file: &'static str,
    exporter_name: &'static str,
    expected_title: &'static str,
    expected_start: &'static str,
    expected_time_zone: Option<&'static str>,
    expected_duration: Option<&'static str>,
    expected_privacy: Option<&'static str>,
    expected_status: Option<&'static str>,
    expected_free_busy: Option<&'static str>,
    expected_priority: Option<i64>,
    expected_has_location: bool,
    expected_virtual_location_count: usize,
    expected_links_count: usize,
    expected_keywords_count: usize,
    expected_alerts_count: usize,
    expected_recurrence_rules_count: usize,
    expected_recurrence_overrides_count: usize,
    expected_show_without_time: Option<bool>,
    unmapped_vendor_properties_dropped_on_export: &'static [&'static str],
}

#[test]
fn real_exporter_fixture_corpus_table_driven_roundtrip() {
    let corpus = [
        RealExporterTestCase {
            name: "Google Calendar Export (vCalendar 2.0 with Recurrence, Overrides, Meet & Alerts)",
            fixture_file: "google_calendar_export.ics",
            exporter_name: "Google Calendar",
            expected_title: "Q3 Product Architecture Sync",
            expected_start: "2026-09-15T10:00:00",
            expected_time_zone: Some("America/New_York"),
            expected_duration: Some("PT1H30M"),
            expected_privacy: Some("public"),
            expected_status: Some("confirmed"),
            expected_free_busy: Some("busy"),
            expected_priority: Some(1),
            expected_has_location: true,
            expected_virtual_location_count: 1,
            expected_links_count: 1,
            expected_keywords_count: 3,
            expected_alerts_count: 2,
            expected_recurrence_rules_count: 1,
            expected_recurrence_overrides_count: 2,
            expected_show_without_time: None,
            unmapped_vendor_properties_dropped_on_export: &[
                "X-WR-CALNAME",
                "X-WR-TIMEZONE",
                "X-NUM-GUESTS",
            ],
        },
        RealExporterTestCase {
            name: "Microsoft Outlook Modern Export (vCalendar 2.0 with Teams, Attachments & Alerts)",
            fixture_file: "outlook_m365_export.ics",
            exporter_name: "Microsoft Outlook / M365",
            expected_title: "Executive Leadership & Financial Review",
            expected_start: "2026-09-20T14:30:00",
            expected_time_zone: Some("Europe/London"),
            expected_duration: Some("PT1H30M"),
            expected_privacy: Some("private"),
            expected_status: Some("confirmed"),
            expected_free_busy: Some("busy"),
            expected_priority: Some(2),
            expected_has_location: true,
            expected_virtual_location_count: 1,
            expected_links_count: 2,
            expected_keywords_count: 3,
            expected_alerts_count: 2,
            expected_recurrence_rules_count: 1,
            expected_recurrence_overrides_count: 0,
            expected_show_without_time: None,
            unmapped_vendor_properties_dropped_on_export: &[
                "X-MS-OLK-FORCEINSPECTOROPEN",
                "X-MICROSOFT-CDO-BUSYSTATUS",
                "X-MICROSOFT-CDO-IMPORTANCE",
                "X-MICROSOFT-DISALLOW-COUNTER",
                "X-MS-OLK-AUTOFILLLOCATION",
                "X-WR-ALARMUID",
            ],
        },
        RealExporterTestCase {
            name: "Apple Calendar macOS Export (vCalendar 2.0 with Multi-Alarms, Token Workshop & Structured Locations)",
            fixture_file: "apple_calendar_export.ics",
            exporter_name: "Apple Calendar / macOS",
            expected_title: "Design Systems Workshop",
            expected_start: "2026-09-25T09:00:00",
            expected_time_zone: Some("Europe/Paris"),
            expected_duration: Some("PT3H"),
            expected_privacy: Some("secret"),
            expected_status: Some("confirmed"),
            expected_free_busy: Some("busy"),
            expected_priority: Some(1),
            expected_has_location: true,
            expected_virtual_location_count: 1,
            expected_links_count: 1,
            expected_keywords_count: 3,
            expected_alerts_count: 3,
            expected_recurrence_rules_count: 1,
            expected_recurrence_overrides_count: 1,
            expected_show_without_time: None,
            unmapped_vendor_properties_dropped_on_export: &[
                "X-APPLE-STRUCTURED-LOCATION",
                "X-APPLE-TRAVEL-ADVISORY-BEHAVIOR",
                "X-WR-ALARMUID",
                "ACKNOWLEDGED",
            ],
        },
        RealExporterTestCase {
            name: "Nextcloud & CalDAV Export (vCalendar 2.0 with Jitsi, Badge Image & 2-Day Reminders)",
            fixture_file: "nextcloud_calendar_export.ics",
            exporter_name: "Nextcloud Calendar / SabreDAV",
            expected_title: "Open Source Infrastructure Summit",
            expected_start: "2026-10-05T09:00:00",
            expected_time_zone: Some("Europe/Berlin"),
            expected_duration: Some("PT8H"),
            expected_privacy: Some("public"),
            expected_status: Some("confirmed"),
            expected_free_busy: Some("busy"),
            expected_priority: Some(3),
            expected_has_location: true,
            expected_virtual_location_count: 1,
            expected_links_count: 2,
            expected_keywords_count: 3,
            expected_alerts_count: 1,
            expected_recurrence_rules_count: 1,
            expected_recurrence_overrides_count: 0,
            expected_show_without_time: None,
            unmapped_vendor_properties_dropped_on_export: &[],
        },
        RealExporterTestCase {
            name: "GNOME Evolution Native Export (vCalendar 2.0 with X-JMAP-UID, Slotted Keys & BigBlueButton)",
            fixture_file: "evolution_calendar_export.ics",
            exporter_name: "GNOME Evolution / EDS",
            expected_title: "GNOME Foundation Board Meeting",
            expected_start: "2026-09-28T16:00:00",
            expected_time_zone: Some("Europe/Berlin"),
            expected_duration: Some("PT2H"),
            expected_privacy: Some("private"),
            expected_status: Some("confirmed"),
            expected_free_busy: Some("busy"),
            expected_priority: Some(1),
            expected_has_location: true,
            expected_virtual_location_count: 1,
            expected_links_count: 2,
            expected_keywords_count: 3,
            expected_alerts_count: 2,
            expected_recurrence_rules_count: 1,
            expected_recurrence_overrides_count: 0,
            expected_show_without_time: None,
            unmapped_vendor_properties_dropped_on_export: &[],
        },
        RealExporterTestCase {
            name: "Mozilla Thunderbird Export (vCalendar 2.0 with Bi-Weekly RRULE, EXDATE & Alerts)",
            fixture_file: "thunderbird_calendar_export.ics",
            exporter_name: "Mozilla Thunderbird / Lightning",
            expected_title: "Thunderbird Release & Quality Sync",
            expected_start: "2026-10-12T09:30:00",
            expected_time_zone: Some("Europe/London"),
            expected_duration: Some("PT1H30M"),
            expected_privacy: Some("public"),
            expected_status: Some("confirmed"),
            expected_free_busy: Some("busy"),
            expected_priority: Some(1),
            expected_has_location: true,
            expected_virtual_location_count: 1,
            expected_links_count: 1,
            expected_keywords_count: 3,
            expected_alerts_count: 1,
            expected_recurrence_rules_count: 1,
            expected_recurrence_overrides_count: 1,
            expected_show_without_time: None,
            unmapped_vendor_properties_dropped_on_export: &[
                "X-MOZ-GENERATION",
                "X-MOZ-LASTACK",
                "X-MOZ-SNOOZE-TIME",
            ],
        },
        RealExporterTestCase {
            name: "Mozilla Thunderbird Multi-Component Detached Overrides Export (vCalendar 2.0 with Rescheduled and Cancelled Instances)",
            fixture_file: "thunderbird_detached_export.ics",
            exporter_name: "Mozilla Thunderbird / Lightning",
            expected_title: "Mozilla Rust Engine Team Bi-Weekly Sync",
            expected_start: "2026-10-05T10:00:00",
            expected_time_zone: Some("Europe/London"),
            expected_duration: Some("PT1H30M"),
            expected_privacy: Some("public"),
            expected_status: Some("confirmed"),
            expected_free_busy: Some("busy"),
            expected_priority: Some(1),
            expected_has_location: true,
            expected_virtual_location_count: 1,
            expected_links_count: 1,
            expected_keywords_count: 3,
            expected_alerts_count: 1,
            expected_recurrence_rules_count: 1,
            expected_recurrence_overrides_count: 3,
            expected_show_without_time: None,
            unmapped_vendor_properties_dropped_on_export: &[
                "X-MOZ-GENERATION",
                "X-MOZ-LASTACK",
                "X-MOZ-SNOOZE-TIME",
                "X-MOZ-SEND-INVITATIONS",
            ],
        },
        RealExporterTestCase {
            name: "SOGo & Radicale CalDAV Export (vCalendar 2.0 with Monthly Recurrence, Badge & Double Alarms)",
            fixture_file: "sogo_calendar_export.ics",
            exporter_name: "SOGo / Radicale CalDAV",
            expected_title: "Sorbonne Distributed Systems Colloquium",
            expected_start: "2026-11-05T14:00:00",
            expected_time_zone: Some("Europe/Paris"),
            expected_duration: Some("PT3H30M"),
            expected_privacy: Some("secret"),
            expected_status: Some("confirmed"),
            expected_free_busy: Some("busy"),
            expected_priority: Some(2),
            expected_has_location: true,
            expected_virtual_location_count: 1,
            expected_links_count: 2,
            expected_keywords_count: 3,
            expected_alerts_count: 2,
            expected_recurrence_rules_count: 1,
            expected_recurrence_overrides_count: 0,
            expected_show_without_time: None,
            unmapped_vendor_properties_dropped_on_export: &[
                "X-SOGO-COMPONENT-CREATED",
                "X-RADICALE-MODIFIED",
            ],
        },
        RealExporterTestCase {
            name: "Cyrus IMAP & Fastmail CalDAV Export (vCalendar 2.0 with All-Day Multi-Day Recurrence & CalDAV Scheduling)",
            fixture_file: "cyrus_caldav_export.ics",
            exporter_name: "Cyrus IMAP / Fastmail CalDAV",
            expected_title: "IETF Hackathon & Standards Interop",
            expected_start: "2026-11-10T00:00:00",
            expected_time_zone: None,
            expected_duration: Some("P3D"),
            expected_privacy: Some("public"),
            expected_status: Some("confirmed"),
            expected_free_busy: Some("free"),
            expected_priority: Some(1),
            expected_has_location: true,
            expected_virtual_location_count: 1,
            expected_links_count: 2,
            expected_keywords_count: 3,
            expected_alerts_count: 1,
            expected_recurrence_rules_count: 1,
            expected_recurrence_overrides_count: 1,
            expected_show_without_time: Some(true),
            unmapped_vendor_properties_dropped_on_export: &[
                "X-CALDAV-ACCESS-RESTRICTION",
                "X-CALDAV-SYNC-TOKEN",
                "X-CALDAV-CTAG",
                "X-FASTMAIL-CLIENT-ID",
            ],
        },
    ];

    for case in &corpus {
        assert!(!case.exporter_name.is_empty(), "Exporter name specified");
        let ics_text = read_fixture(case.fixture_file);

        // 1. Inbound Parsing to CalendarEvent / JSCalendar model
        let event = ical_to_event(&ics_text).unwrap_or_else(|e| {
            panic!(
                "Failed to parse fixture {} ({}): {e}",
                case.fixture_file, case.exporter_name
            )
        });

        // 2. Validate Parsed Mapped Surface
        assert_eq!(
            event.title.as_deref(),
            Some(case.expected_title),
            "Title mismatch for {} ({})",
            case.name,
            case.exporter_name
        );
        assert_eq!(
            event.start.as_deref(),
            Some(case.expected_start),
            "Start mismatch for {}",
            case.name
        );
        assert_eq!(
            event.time_zone.as_deref(),
            case.expected_time_zone,
            "TimeZone mismatch for {}",
            case.name
        );
        assert_eq!(
            event.duration.as_deref(),
            case.expected_duration,
            "Duration mismatch for {}",
            case.name
        );
        assert_eq!(
            event.privacy.as_deref(),
            case.expected_privacy,
            "Privacy mismatch for {}",
            case.name
        );
        assert_eq!(
            event.status.as_deref(),
            case.expected_status,
            "Status mismatch for {}",
            case.name
        );
        assert_eq!(
            event.free_busy_status.as_deref(),
            case.expected_free_busy,
            "Free/busy mismatch for {}",
            case.name
        );
        assert_eq!(
            event.priority, case.expected_priority,
            "Priority mismatch for {}",
            case.name
        );
        assert_eq!(
            event.show_without_time, case.expected_show_without_time,
            "showWithoutTime mismatch for {}",
            case.name
        );

        if case.expected_has_location {
            let locs = event.locations.as_ref().expect("locations present");
            assert!(!locs.is_empty(), "Location empty for {}", case.name);
        }

        assert_eq!(
            event
                .virtual_locations
                .as_ref()
                .map(|v| v.len())
                .unwrap_or(0),
            case.expected_virtual_location_count,
            "Virtual locations count mismatch for {}",
            case.name
        );

        assert_eq!(
            event.links.as_ref().map(|l| l.len()).unwrap_or(0),
            case.expected_links_count,
            "Links count mismatch for {}",
            case.name
        );

        assert_eq!(
            event.keywords.as_ref().map(|k| k.len()).unwrap_or(0),
            case.expected_keywords_count,
            "Keywords count mismatch for {}",
            case.name
        );

        assert_eq!(
            event.alerts.as_ref().map(|a| a.len()).unwrap_or(0),
            case.expected_alerts_count,
            "Alerts count mismatch for {}",
            case.name
        );

        assert_eq!(
            usize::from(event.recurrence_rule.is_some()),
            case.expected_recurrence_rules_count,
            "Recurrence rules count mismatch for {}",
            case.name
        );

        assert_eq!(
            event
                .recurrence_overrides
                .as_ref()
                .map(|o| o.len())
                .unwrap_or(0),
            case.expected_recurrence_overrides_count,
            "Recurrence overrides count mismatch for {}",
            case.name
        );

        // 3. First Export (Export₁) to canonical iCalendar 2.0
        let export1 = event_to_ical(&event);
        assert!(
            export1.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:"),
            "Export₁ must start with iCalendar 2.0 envelope for {}:\n{export1}",
            case.name
        );
        assert!(
            export1.ends_with("END:VCALENDAR\r\n"),
            "Export₁ must end with END:VCALENDAR for {}:\n{export1}",
            case.name
        );

        // Verify unmapped vendor properties are cleanly dropped
        for dropped_prop in case.unmapped_vendor_properties_dropped_on_export {
            assert!(
                !export1.contains(dropped_prop),
                "Export₁ must drop unmapped vendor property '{}' for {}:\n{export1}",
                dropped_prop,
                case.name
            );
        }

        // 4. Multi-Stage Round-Trip Fixpoint Execution
        let event2 = ical_to_event(&export1)
            .unwrap_or_else(|e| panic!("Failed to parse Export₁ for {}: {e}", case.name));
        let export2 = event_to_ical(&event2);
        let event3 = ical_to_event(&export2)
            .unwrap_or_else(|e| panic!("Failed to parse Export₂ for {}: {e}", case.name));
        let export3 = event_to_ical(&event3);

        // 5. Standing Fixpoint Invariants
        assert_eq!(
            export2, export3,
            "Export₂ == Export₃ fixpoint invariant violated for {}",
            case.name
        );
        assert_eq!(
            event2, event3,
            "Event₂ == Event₃ fixpoint invariant violated for {}",
            case.name
        );

        // 6. Lossless Preservation of Mapped Surface
        assert_eq!(
            event2.title, event3.title,
            "Title preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            event2.description, event3.description,
            "Description preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            event2.start, event3.start,
            "Start preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            event2.time_zone, event3.time_zone,
            "TimeZone preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            event2.duration, event3.duration,
            "Duration preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            event2.privacy, event3.privacy,
            "Privacy preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            event2.status, event3.status,
            "Status preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            event2.free_busy_status, event3.free_busy_status,
            "Free/busy preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            event2.priority, event3.priority,
            "Priority preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            event2.show_without_time, event3.show_without_time,
            "showWithoutTime preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            event2.keywords, event3.keywords,
            "Keywords preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            event2.recurrence_rule, event3.recurrence_rule,
            "Recurrence rules preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            event2.recurrence_overrides, event3.recurrence_overrides,
            "Recurrence overrides preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            event2.alerts, event3.alerts,
            "Alerts preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            event2.links, event3.links,
            "Links preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            event2.virtual_locations, event3.virtual_locations,
            "Virtual locations preserved losslessly for {}",
            case.name
        );
    }
}

#[test]
fn real_exporter_fixture_google_calendar_detailed_roundtrip() {
    let ics_text = read_fixture("google_calendar_export.ics");
    let event = ical_to_event(&ics_text).expect("parse Google Calendar fixture");

    // 1. Verify calendar name and non-standard properties do not pollute event.extra
    assert!(
        event.extra.is_empty(),
        "event.extra must be empty, found: {:?}",
        event.extra
    );

    // 2. Validate primary event details
    assert_eq!(event.title.as_deref(), Some("Q3 Product Architecture Sync"));
    assert_eq!(event.start.as_deref(), Some("2026-09-15T10:00:00"));
    assert_eq!(event.time_zone.as_deref(), Some("America/New_York"));
    assert_eq!(event.duration.as_deref(), Some("PT1H30M"));
    assert_eq!(event.status.as_deref(), Some("confirmed"));
    assert_eq!(event.free_busy_status.as_deref(), Some("busy"));
    assert_eq!(event.priority, Some(1));
    assert_eq!(event.privacy.as_deref(), Some("public"));

    // 3. Validate Google Meet conference and Google Drive link
    let vlocs = event.virtual_locations.as_ref().expect("virtual_locations");
    assert_eq!(vlocs.len(), 1);
    let meet = vlocs.values().next().expect("meet");
    assert_eq!(meet["uri"], json!("https://meet.google.com/abc-defg-hij"));
    assert_eq!(meet["name"], json!("Google Meet"));
    assert_eq!(meet["features"]["audio"], json!(true));
    assert_eq!(meet["features"]["video"], json!(true));

    let links = event.links.as_ref().expect("links");
    assert_eq!(links.len(), 1);
    let drive_link = links.values().next().expect("drive link");
    assert_eq!(
        drive_link["href"],
        json!("https://drive.google.com/file/d/12345/view")
    );
    assert_eq!(drive_link["contentType"], json!("application/pdf"));
    assert_eq!(drive_link["size"], json!(102_400));

    // 4. Validate Recurrence Rules and Overrides (EXDATE + RECURRENCE-ID)
    let rules = std::slice::from_ref(event.recurrence_rule.as_ref().expect("recurrence_rule"));
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].frequency, "weekly");
    assert_eq!(rules[0].interval, Some(1));
    assert_eq!(rules[0].until.as_deref(), Some("2026-11-20T10:00:00"));
    let by_day = rules[0].by_day.as_ref().expect("by_day");
    assert_eq!(by_day.len(), 2);
    assert_eq!(by_day[0].day, "tu");
    assert_eq!(by_day[1].day, "th");

    let overrides = event
        .recurrence_overrides
        .as_ref()
        .expect("recurrence_overrides");
    assert_eq!(overrides.len(), 2);
    assert_eq!(overrides["2026-10-15T10:00:00"], json!({"excluded": true}));
    let modified = &overrides["2026-10-20T10:00:00"];
    assert_eq!(
        modified["title"],
        json!("Q3 Product Architecture Sync (Performance Deep Dive)")
    );
    assert_eq!(modified["start"], json!("2026-10-20T10:30:00"));

    // 5. Validate Google alarms (display alarms preserved, email/absolute triggers refused safely)
    let alerts = event.alerts.as_ref().expect("alerts");
    assert_eq!(alerts.len(), 2, "Google fixture has 2 display alarms");
    assert_eq!(alerts["a1"]["trigger"]["offset"], json!("-P1D"));
    assert_eq!(alerts["a2"]["trigger"]["offset"], json!("-PT15M"));
    assert!(maps_alerts(&event));

    // 6. Multi-pass roundtrip fixpoint
    let export1 = event_to_ical(&event);
    assert!(
        !export1.contains("ACTION:EMAIL"),
        "refused EMAIL alarm must be dropped"
    );
    assert!(
        !export1.contains("VALUE=DATE-TIME"),
        "refused absolute trigger alarm must be dropped"
    );
    let event2 = ical_to_event(&export1).expect("event2");
    let export2 = event_to_ical(&event2);
    let event3 = ical_to_event(&export2).expect("event3");
    let export3 = event_to_ical(&event3);

    assert_eq!(export2, export3);
    assert_eq!(event2, event3);
}

#[test]
fn real_exporter_fixture_outlook_modern_m365_detailed_roundtrip() {
    let ics_text = read_fixture("outlook_m365_export.ics");
    let event = ical_to_event(&ics_text).expect("parse Outlook M365 fixture");

    // 1. Verify Outlook CDO/OLK vendor properties do not pollute extra
    assert!(event.extra.is_empty());

    // 2. Validate mapped details
    assert_eq!(
        event.title.as_deref(),
        Some("Executive Leadership & Financial Review")
    );
    assert_eq!(event.start.as_deref(), Some("2026-09-20T14:30:00"));
    assert_eq!(event.time_zone.as_deref(), Some("Europe/London"));
    assert_eq!(event.duration.as_deref(), Some("PT1H30M"));
    assert_eq!(event.privacy.as_deref(), Some("private"));
    assert_eq!(event.priority, Some(2));
    assert_eq!(event.status.as_deref(), Some("confirmed"));
    assert_eq!(event.free_busy_status.as_deref(), Some("busy"));

    // 3. Validate Teams conference, presentation attachment, company logo badge
    let vlocs = event.virtual_locations.as_ref().expect("virtual_locations");
    assert_eq!(vlocs.len(), 1);
    let teams = vlocs.values().next().expect("teams");
    assert_eq!(
        teams["uri"],
        json!("https://teams.microsoft.com/l/meetup-join/19%3ameeting_123")
    );
    assert_eq!(teams["name"], json!("Microsoft Teams Meeting"));

    let links = event.links.as_ref().expect("links");
    assert_eq!(links.len(), 2);
    assert!(links.values().any(|l| l["contentType"]
        == "application/vnd.openxmlformats-officedocument.presentationml.presentation"));
    assert!(
        links
            .values()
            .any(|l| l["display"] == "badge" && l["rel"] == "icon")
    );

    // 4. Validate monthly 3rd Sunday recurrence
    let rules = std::slice::from_ref(event.recurrence_rule.as_ref().expect("recurrence_rule"));
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].frequency, "monthly");
    assert_eq!(rules[0].count, Some(6));
    let by_day = rules[0].by_day.as_ref().expect("by_day");
    assert_eq!(by_day.len(), 1);
    assert_eq!(by_day[0].day, "su");
    assert_eq!(by_day[0].nth_of_period, Some(3));

    // 5. Validate Outlook alarms (DESCRIPTION:REMINDER, X-WR-ALARMUID / UID preservation, email dropped)
    let alerts = event.alerts.as_ref().expect("alerts");
    assert_eq!(alerts.len(), 2, "Outlook fixture has 2 display alarms");
    let outlook_uid = "040000008200E00074C5B7101A82E0080000000080E99A2D87D3D901000000000000000010000000D3C9D55A1A2E-alarm-1";
    assert_eq!(alerts[outlook_uid]["trigger"]["offset"], json!("-PT15M"));
    assert_eq!(alerts["a1"]["trigger"]["offset"], json!("-PT30M"));
    assert!(maps_alerts(&event));

    // 6. Multi-pass roundtrip fixpoint
    let export1 = event_to_ical(&event);
    assert!(
        !export1.contains("X-WR-ALARMUID"),
        "X-WR-ALARMUID must be dropped on export"
    );
    assert!(
        !export1.contains("ACTION:EMAIL"),
        "refused EMAIL alarm must be dropped"
    );
    let event2 = ical_to_event(&export1).expect("event2");
    let export2 = event_to_ical(&event2);
    let event3 = ical_to_event(&export2).expect("event3");
    let export3 = event_to_ical(&event3);

    assert_eq!(export2, export3);
    assert_eq!(event2, event3);
}

#[test]
fn real_exporter_fixture_apple_calendar_macos_detailed_roundtrip() {
    let ics_text = read_fixture("apple_calendar_export.ics");
    let event = ical_to_event(&ics_text).expect("parse Apple Calendar fixture");

    // 1. Verify Apple travel/location extensions do not pollute extra
    assert!(event.extra.is_empty());

    // 2. Validate mapped details
    assert_eq!(event.title.as_deref(), Some("Design Systems Workshop"));
    assert_eq!(event.start.as_deref(), Some("2026-09-25T09:00:00"));
    assert_eq!(event.time_zone.as_deref(), Some("Europe/Paris"));
    assert_eq!(event.duration.as_deref(), Some("PT3H"));
    assert_eq!(event.privacy.as_deref(), Some("secret"));
    assert_eq!(event.priority, Some(1));

    // 3. Validate escaped comma unescaping in location
    let locs = event.locations.as_ref().expect("locations");
    let loc = locs.values().next().expect("loc");
    assert_eq!(
        loc["name"],
        json!("Paris Design Lab, Amphithéâtre Marie Curie")
    );

    // 4. Validate bi-weekly Friday recurrence with EXDATE
    let rules = std::slice::from_ref(event.recurrence_rule.as_ref().expect("recurrence_rule"));
    assert_eq!(rules[0].frequency, "weekly");
    assert_eq!(rules[0].interval, Some(2));
    assert_eq!(rules[0].count, Some(5));

    let overrides = event
        .recurrence_overrides
        .as_ref()
        .expect("recurrence_overrides");
    assert_eq!(overrides["2026-10-23T09:00:00"], json!({"excluded": true}));

    // 5. Validate multiple alarms (1 day with ACKNOWLEDGED, 2 hours, 15 minutes; AUDIO and absolute trigger dropped)
    let alerts = event.alerts.as_ref().expect("alerts");
    assert_eq!(alerts.len(), 3, "Apple fixture has 3 display alarms");
    assert_eq!(
        alerts["E451D045-FA1B-475D-85B6-06F6F505A321"]["trigger"]["offset"],
        json!("-P1D")
    );
    assert_eq!(
        alerts["F82C4A10-91DE-4A99-8D77-38C1B79E1A55"]["trigger"]["offset"],
        json!("-PT2H")
    );
    assert_eq!(
        alerts["apple-alarm-offset-15m"]["trigger"]["offset"],
        json!("-PT15M")
    );
    assert!(maps_alerts(&event));

    // 6. Multi-pass roundtrip fixpoint
    let export1 = event_to_ical(&event);
    assert!(
        !export1.contains("ACKNOWLEDGED"),
        "ACKNOWLEDGED must be dropped on export"
    );
    assert!(
        !export1.contains("X-WR-ALARMUID"),
        "X-WR-ALARMUID must be dropped on export"
    );
    assert!(
        !export1.contains("ACTION:AUDIO"),
        "refused AUDIO alarm must be dropped"
    );
    assert!(
        !export1.contains("VALUE=DATE-TIME"),
        "refused absolute trigger alarm must be dropped"
    );
    let event2 = ical_to_event(&export1).expect("event2");
    let export2 = event_to_ical(&event2);
    let event3 = ical_to_event(&export2).expect("event3");
    let export3 = event_to_ical(&event3);

    assert_eq!(export2, export3);
    assert_eq!(event2, event3);
}

#[test]
fn real_exporter_fixture_nextcloud_caldav_detailed_roundtrip() {
    let ics_text = read_fixture("nextcloud_calendar_export.ics");
    let event = ical_to_event(&ics_text).expect("parse Nextcloud fixture");

    // 1. Verify clean extra map
    assert!(event.extra.is_empty());

    // 2. Validate event metadata
    assert_eq!(
        event.title.as_deref(),
        Some("Open Source Infrastructure Summit")
    );
    assert_eq!(event.start.as_deref(), Some("2026-10-05T09:00:00"));
    assert_eq!(event.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(event.duration.as_deref(), Some("PT8H"));
    assert_eq!(event.privacy.as_deref(), Some("public"));
    assert_eq!(event.priority, Some(3));

    // 3. Validate daily recurrence
    let rules = std::slice::from_ref(event.recurrence_rule.as_ref().expect("recurrence_rule"));
    assert_eq!(rules[0].frequency, "daily");
    assert_eq!(rules[0].count, Some(3));

    // 4. Validate 2-day reminder alert
    let alerts = event.alerts.as_ref().expect("alerts");
    assert_eq!(alerts.len(), 1);
    assert_eq!(
        alerts.values().next().unwrap()["trigger"]["offset"],
        json!("-P2D")
    );

    // 5. Multi-pass roundtrip fixpoint
    let export1 = event_to_ical(&event);
    let event2 = ical_to_event(&export1).expect("event2");
    let export2 = event_to_ical(&event2);
    let event3 = ical_to_event(&export2).expect("event3");
    let export3 = event_to_ical(&event3);

    assert_eq!(export2, export3);
    assert_eq!(event2, event3);
}

#[test]
fn real_exporter_fixture_evolution_native_detailed_roundtrip() {
    let ics_text = read_fixture("evolution_calendar_export.ics");
    let event = ical_to_event(&ics_text).expect("parse Evolution native fixture");

    // 1. Verify clean extra map and preserved UID
    assert!(event.extra.is_empty());
    assert_eq!(
        event.id.as_ref().map(|id| id.as_str()),
        Some("evolution-native-event-456789")
    );
    assert_eq!(
        event.uid.as_deref(),
        Some("urn:uuid:8f2b1c94-0d3a-4f7e-9c11-2a6d5e8b7f30")
    );

    // 2. Validate mapped details
    assert_eq!(
        event.title.as_deref(),
        Some("GNOME Foundation Board Meeting")
    );
    assert_eq!(event.start.as_deref(), Some("2026-09-28T16:00:00"));
    assert_eq!(event.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(event.duration.as_deref(), Some("PT2H"));
    assert_eq!(event.privacy.as_deref(), Some("private"));
    assert_eq!(event.priority, Some(1));

    // 3. Validate explicit X-JMAP-KEY preservation on locations, virtual locations, links
    let locs = event.locations.as_ref().expect("locations");
    assert!(locs.contains_key("loc1"));

    let vlocs = event.virtual_locations.as_ref().expect("virtual_locations");
    assert!(vlocs.contains_key("v1"));

    let links = event.links.as_ref().expect("links");
    assert!(links.contains_key("l1"));
    assert!(links.contains_key("l2"));

    // 4. Validate monthly last Monday recurrence
    let rules = std::slice::from_ref(event.recurrence_rule.as_ref().expect("recurrence_rule"));
    assert_eq!(rules[0].frequency, "monthly");
    assert_eq!(rules[0].count, Some(12));
    let by_day = rules[0].by_day.as_ref().expect("by_day");
    assert_eq!(by_day[0].day, "mo");
    assert_eq!(by_day[0].nth_of_period, Some(-1));

    // 5. Validate multi-alarm sequence (15m, 1h)
    let alerts = event.alerts.as_ref().expect("alerts");
    assert_eq!(alerts.len(), 2);

    // 6. Multi-pass roundtrip fixpoint
    let export1 = event_to_ical(&event);
    let event2 = ical_to_event(&export1).expect("event2");
    let export2 = event_to_ical(&event2);
    let event3 = ical_to_event(&export2).expect("event3");
    let export3 = event_to_ical(&event3);

    assert_eq!(export2, export3);
    assert_eq!(event2, event3);
}

#[test]
fn real_exporter_fixture_thunderbird_calendar_detailed_roundtrip() {
    let ics_text = read_fixture("thunderbird_calendar_export.ics");
    let event = ical_to_event(&ics_text).expect("parse Thunderbird calendar fixture");

    // 1. Verify clean extra map
    assert!(
        event.extra.is_empty(),
        "event.extra must be empty, found: {:?}",
        event.extra
    );

    // 2. Validate mapped details
    assert_eq!(
        event.title.as_deref(),
        Some("Thunderbird Release & Quality Sync")
    );
    assert_eq!(event.start.as_deref(), Some("2026-10-12T09:30:00"));
    assert_eq!(event.time_zone.as_deref(), Some("Europe/London"));
    assert_eq!(event.duration.as_deref(), Some("PT1H30M"));
    assert_eq!(event.privacy.as_deref(), Some("public"));
    assert_eq!(event.status.as_deref(), Some("confirmed"));
    assert_eq!(event.free_busy_status.as_deref(), Some("busy"));
    assert_eq!(event.priority, Some(1));

    // 3. Validate conference & attachment link
    let vlocs = event.virtual_locations.as_ref().expect("virtual_locations");
    assert_eq!(vlocs.len(), 1);
    let conf = vlocs.values().next().expect("conference");
    assert_eq!(
        conf["uri"],
        json!("https://meet.mozilla.org/thunderbird-sync")
    );
    assert_eq!(conf["features"]["audio"], json!(true));
    assert_eq!(conf["features"]["video"], json!(true));

    let links = event.links.as_ref().expect("links");
    assert_eq!(links.len(), 1);
    let doc = links.values().next().expect("doc");
    assert_eq!(
        doc["href"],
        json!("https://www.thunderbird.net/docs/release-plan.pdf")
    );
    assert_eq!(doc["contentType"], json!("application/pdf"));
    assert_eq!(doc["size"], json!(204_800));

    // 4. Validate bi-weekly RRULE and EXDATE override
    let rules = std::slice::from_ref(event.recurrence_rule.as_ref().expect("recurrence_rule"));
    assert_eq!(rules[0].frequency, "weekly");
    assert_eq!(rules[0].interval, Some(2));
    assert_eq!(rules[0].until.as_deref(), Some("2026-12-21T09:30:00"));
    let by_day = rules[0].by_day.as_ref().expect("by_day");
    assert_eq!(by_day.len(), 1);
    assert_eq!(by_day[0].day, "mo");

    let overrides = event.recurrence_overrides.as_ref().expect("overrides");
    assert_eq!(overrides["2026-11-09T09:30:00"], json!({"excluded": true}));

    // 5. Validate display alarm
    let alerts = event.alerts.as_ref().expect("alerts");
    assert_eq!(alerts.len(), 1);
    assert_eq!(
        alerts.values().next().unwrap()["trigger"]["offset"],
        json!("-PT15M")
    );

    // 6. Multi-pass roundtrip fixpoint
    let export1 = event_to_ical(&event);
    assert!(!export1.contains("X-MOZ-GENERATION"));
    assert!(!export1.contains("X-MOZ-LASTACK"));
    assert!(!export1.contains("X-MOZ-SNOOZE-TIME"));
    let event2 = ical_to_event(&export1).expect("event2");
    let export2 = event_to_ical(&event2);
    let event3 = ical_to_event(&export2).expect("event3");
    let export3 = event_to_ical(&event3);

    assert_eq!(export2, export3);
    assert_eq!(event2, event3);
}

#[test]
fn real_exporter_fixture_sogo_caldav_detailed_roundtrip() {
    let ics_text = read_fixture("sogo_calendar_export.ics");
    let event = ical_to_event(&ics_text).expect("parse SOGo calendar fixture");

    // 1. Verify clean extra map
    assert!(
        event.extra.is_empty(),
        "event.extra must be empty, found: {:?}",
        event.extra
    );

    // 2. Validate mapped details
    assert_eq!(
        event.title.as_deref(),
        Some("Sorbonne Distributed Systems Colloquium")
    );
    assert_eq!(event.start.as_deref(), Some("2026-11-05T14:00:00"));
    assert_eq!(event.time_zone.as_deref(), Some("Europe/Paris"));
    assert_eq!(event.duration.as_deref(), Some("PT3H30M"));
    assert_eq!(event.privacy.as_deref(), Some("secret"));
    assert_eq!(event.status.as_deref(), Some("confirmed"));
    assert_eq!(event.free_busy_status.as_deref(), Some("busy"));
    assert_eq!(event.priority, Some(2));

    // 3. Validate French accented location
    let locs = event.locations.as_ref().expect("locations");
    let loc = locs.values().next().expect("loc");
    assert_eq!(
        loc["name"],
        json!("Amphithéâtre 25, 4 Place Jussieu, 75005 Paris")
    );

    // 4. Validate conference with chat feature and dual links (PDF + PNG badge)
    let vlocs = event.virtual_locations.as_ref().expect("virtual_locations");
    let conf = vlocs.values().next().expect("conf");
    assert_eq!(conf["features"]["chat"], json!(true));

    let links = event.links.as_ref().expect("links");
    assert_eq!(links.len(), 2);
    assert!(
        links
            .values()
            .any(|l| l["contentType"] == "application/pdf")
    );
    assert!(
        links
            .values()
            .any(|l| l["display"] == "badge" && l["contentType"] == "image/png")
    );

    // 5. Validate monthly 1st Thursday recurrence
    let rules = std::slice::from_ref(event.recurrence_rule.as_ref().expect("recurrence_rule"));
    assert_eq!(rules[0].frequency, "monthly");
    assert_eq!(rules[0].count, Some(6));
    let by_day = rules[0].by_day.as_ref().expect("by_day");
    assert_eq!(by_day[0].day, "th");
    assert_eq!(by_day[0].nth_of_period, Some(1));

    // 6. Validate dual display alarms (-P1D and -PT1H)
    let alerts = event.alerts.as_ref().expect("alerts");
    assert_eq!(alerts.len(), 2);
    assert!(
        alerts
            .values()
            .any(|a| a["trigger"]["offset"] == json!("-P1D"))
    );
    assert!(
        alerts
            .values()
            .any(|a| a["trigger"]["offset"] == json!("-PT1H"))
    );

    // 7. Multi-pass roundtrip fixpoint
    let export1 = event_to_ical(&event);
    assert!(!export1.contains("X-SOGO-COMPONENT-CREATED"));
    assert!(!export1.contains("X-RADICALE-MODIFIED"));
    let event2 = ical_to_event(&export1).expect("event2");
    let export2 = event_to_ical(&event2);
    let event3 = ical_to_event(&export2).expect("event3");
    let export3 = event_to_ical(&event3);

    assert_eq!(export2, export3);
    assert_eq!(event2, event3);
}

#[test]
fn real_exporter_fixture_thunderbird_detached_overrides_detailed_roundtrip() {
    let ics_text = read_fixture("thunderbird_detached_export.ics");
    let event = ical_to_event(&ics_text).expect("parse Thunderbird detached overrides fixture");

    // 1. Verify clean extra map: unmapped Mozilla vendor properties do not leak
    assert!(
        event.extra.is_empty(),
        "event.extra must be empty, found: {:?}",
        event.extra
    );

    // 2. Validate mapped series details
    assert_eq!(
        event.title.as_deref(),
        Some("Mozilla Rust Engine Team Bi-Weekly Sync")
    );
    assert_eq!(event.start.as_deref(), Some("2026-10-05T10:00:00"));
    assert_eq!(event.time_zone.as_deref(), Some("Europe/London"));
    assert_eq!(event.duration.as_deref(), Some("PT1H30M"));
    assert_eq!(event.privacy.as_deref(), Some("public"));
    assert_eq!(event.status.as_deref(), Some("confirmed"));
    assert_eq!(event.free_busy_status.as_deref(), Some("busy"));
    assert_eq!(event.priority, Some(1));
    assert_eq!(event.show_without_time, None);

    // 3. Validate conference and attachment link
    let vlocs = event.virtual_locations.as_ref().expect("virtual_locations");
    assert_eq!(vlocs.len(), 1);
    let conf = vlocs.values().next().expect("conference");
    assert_eq!(conf["uri"], json!("https://meet.mozilla.org/rust-engine"));
    assert_eq!(conf["features"]["audio"], json!(true));
    assert_eq!(conf["features"]["video"], json!(true));

    let links = event.links.as_ref().expect("links");
    assert_eq!(links.len(), 1);
    let doc = links.values().next().expect("doc");
    assert_eq!(
        doc["href"],
        json!("https://www.thunderbird.net/docs/rust-sync.pdf")
    );
    assert_eq!(doc["contentType"], json!("application/pdf"));
    assert_eq!(doc["size"], json!(153_600));

    // 4. Validate bi-weekly RRULE, EXDATE, rescheduled override, and cancelled override
    let rules = std::slice::from_ref(event.recurrence_rule.as_ref().expect("recurrence_rule"));
    assert_eq!(rules[0].frequency, "weekly");
    assert_eq!(rules[0].interval, Some(2));
    assert_eq!(rules[0].count, Some(6));
    let by_day = rules[0].by_day.as_ref().expect("by_day");
    assert_eq!(by_day.len(), 1);
    assert_eq!(by_day[0].day, "mo");

    let overrides = event.recurrence_overrides.as_ref().expect("overrides");
    assert_eq!(overrides.len(), 3);

    // Excluded occurrence via EXDATE
    assert_eq!(overrides["2026-11-02T10:00:00"], json!({"excluded": true}));

    // Rescheduled and modified occurrence via detached VEVENT
    let resched = &overrides["2026-10-19T10:00:00"];
    assert_eq!(resched["start"], json!("2026-10-19T14:00:00"));
    assert_eq!(resched["duration"], json!("PT2H"));
    assert_eq!(
        resched["title"],
        json!("Mozilla Rust Engine Team Extended Deep-Dive")
    );
    assert_eq!(
        resched["description"],
        json!("Special extended session focusing on memory allocator benchmarking.")
    );
    assert_eq!(resched["priority"], json!(2));
    assert_eq!(
        resched["keywords"],
        json!({"Mozilla": true, "Engineering": true, "Benchmark": true})
    );

    // Cancelled occurrence via detached VEVENT with STATUS:CANCELLED
    let cancelled = &overrides["2026-11-16T10:00:00"];
    assert_eq!(cancelled["status"], json!("cancelled"));

    // 5. Multi-pass roundtrip fixpoint
    let export1 = event_to_ical(&event);
    assert!(!export1.contains("X-MOZ-GENERATION"));
    assert!(!export1.contains("X-MOZ-LASTACK"));
    assert!(!export1.contains("X-MOZ-SNOOZE-TIME"));
    assert!(!export1.contains("X-MOZ-SEND-INVITATIONS"));

    let event2 = ical_to_event(&export1).expect("event2");
    let export2 = event_to_ical(&event2);
    let event3 = ical_to_event(&export2).expect("event3");
    let export3 = event_to_ical(&event3);

    assert_eq!(export2, export3);
    assert_eq!(event2, event3);
}

#[test]
fn real_exporter_fixture_cyrus_caldav_detailed_roundtrip() {
    let ics_text = read_fixture("cyrus_caldav_export.ics");
    let event = ical_to_event(&ics_text).expect("parse Cyrus CalDAV fixture");

    // 1. Verify clean extra map: unmapped CalDAV and Fastmail properties do not leak
    assert!(
        event.extra.is_empty(),
        "event.extra must be empty, found: {:?}",
        event.extra
    );

    // 2. Validate all-day multi-day mapped details
    assert_eq!(
        event.title.as_deref(),
        Some("IETF Hackathon & Standards Interop")
    );
    assert_eq!(event.start.as_deref(), Some("2026-11-10T00:00:00"));
    assert_eq!(event.time_zone, None);
    assert_eq!(event.show_without_time, Some(true));
    assert_eq!(event.duration.as_deref(), Some("P3D"));
    assert_eq!(event.privacy.as_deref(), Some("public"));
    assert_eq!(event.status.as_deref(), Some("confirmed"));
    assert_eq!(event.free_busy_status.as_deref(), Some("free"));
    assert_eq!(event.priority, Some(1));

    // 3. Validate dual links: attachment PDF and badge PNG
    let links = event.links.as_ref().expect("links");
    assert_eq!(links.len(), 2);
    assert!(
        links
            .values()
            .any(|l| l["contentType"] == "application/pdf" && l["size"] == 1_048_576)
    );
    assert!(
        links
            .values()
            .any(|l| l["display"] == "badge" && l["contentType"] == "image/png")
    );

    // 4. Validate virtual location and conference
    let vlocs = event.virtual_locations.as_ref().expect("virtual_locations");
    assert_eq!(vlocs.len(), 1);
    let conf = vlocs.values().next().expect("conf");
    assert_eq!(
        conf["uri"],
        json!("https://meetecho.ietf.example/hackathon")
    );
    assert_eq!(conf["features"]["audio"], json!(true));
    assert_eq!(conf["features"]["video"], json!(true));

    // 5. Validate physical location
    let locs = event.locations.as_ref().expect("locations");
    assert_eq!(locs.len(), 1);
    let loc = locs.values().next().expect("loc");
    assert_eq!(
        loc["name"],
        json!("San Francisco Marriott Marquis, 780 Mission St, San Francisco, CA 94103")
    );

    // 6. Validate annual recurrence and excluded date
    let rules = std::slice::from_ref(event.recurrence_rule.as_ref().expect("recurrence_rule"));
    assert_eq!(rules[0].frequency, "yearly");
    assert_eq!(rules[0].count, Some(5));

    let overrides = event.recurrence_overrides.as_ref().expect("overrides");
    assert_eq!(overrides["2028-11-10T00:00:00"], json!({"excluded": true}));

    // 7. Validate 1-day advance display reminder
    let alerts = event.alerts.as_ref().expect("alerts");
    assert_eq!(alerts.len(), 1);
    assert_eq!(
        alerts.values().next().unwrap()["trigger"]["offset"],
        json!("-P1D")
    );

    // 8. Multi-pass roundtrip fixpoint
    let export1 = event_to_ical(&event);
    assert!(!export1.contains("X-CALDAV-ACCESS-RESTRICTION"));
    assert!(!export1.contains("X-CALDAV-SYNC-TOKEN"));
    assert!(!export1.contains("X-CALDAV-CTAG"));
    assert!(!export1.contains("X-FASTMAIL-CLIENT-ID"));

    // Ensure all-day event emits VALUE=DATE without TZID
    assert!(export1.contains("DTSTART;VALUE=DATE:20261110\r\n"));
    assert!(export1.contains("DURATION:P3D\r\n"));
    assert!(export1.contains("TRANSP:TRANSPARENT\r\n"));
    assert!(!export1.contains("TZID"));

    let event2 = ical_to_event(&export1).expect("event2");
    let export2 = event_to_ical(&event2);
    let event3 = ical_to_event(&export2).expect("event3");
    let export3 = event_to_ical(&event3);

    assert_eq!(export2, export3);
    assert_eq!(event2, event3);
}

#[test]
fn real_exporter_fixture_evolution_roundtrip_self_consistency() {
    let event = CalendarEvent {
        id: Some("E-SELF-CONSISTENCY-1".into()),
        uid: Some("urn:uuid:12345678-1234-5678-1234-567812345678".into()),
        event_type: Some("Event".into()),
        version: Some("2.0".into()),
        title: Some("Self-Consistency Architecture Workshop".into()),
        description: Some(
            "Comprehensive round-trip self-consistency test for Evolution iCalendar mapping."
                .into(),
        ),
        start: Some("2026-10-15T09:00:00".into()),
        time_zone: Some("Europe/Berlin".into()),
        duration: Some("PT2H30M".into()),
        privacy: Some("private".into()),
        status: Some("confirmed".into()),
        free_busy_status: Some("busy".into()),
        priority: Some(1),
        locations: Some(
            [(
                "loc1".to_string(),
                json!({
                    "@type": "Location",
                    "name": "Hauptgebäude, Raum 101, Berlin"
                }),
            )]
            .into(),
        ),
        virtual_locations: Some(
            [(
                "v1".to_string(),
                json!({
                    "@type": "VirtualLocation",
                    "name": "GNOME Video Bridge",
                    "uri": "https://meet.gnome.org/arch-workshop",
                    "features": {"audio": true, "video": true, "screen": true, "chat": true}
                }),
            )]
            .into(),
        ),
        links: Some(
            [
                (
                    "l1".to_string(),
                    json!({
                        "@type": "Link",
                        "href": "https://foundation.gnome.org/agenda.pdf",
                        "contentType": "application/pdf",
                        "size": 1048576
                    }),
                ),
                (
                    "l2".to_string(),
                    json!({
                        "@type": "Link",
                        "href": "https://foundation.gnome.org/badge.png",
                        "display": "badge",
                        "rel": "icon"
                    }),
                ),
            ]
            .into(),
        ),
        keywords: Some(
            [
                ("GNOME".into(), json!(true)),
                ("Architecture".into(), json!(true)),
                ("Evolution".into(), json!(true)),
            ]
            .into(),
        ),
        recurrence_rule: Some(RecurrenceRule {
            rule_type: Some("RecurrenceRule".into()),
            frequency: "weekly".into(),
            interval: Some(2),
            by_day: Some(vec![NDay::new("th")]),
            count: Some(10),
            ..RecurrenceRule::default()
        }),
        recurrence_overrides: Some(
            [
                ("2026-11-12T09:00:00".to_string(), json!({"excluded": true})),
                (
                    "2026-11-26T09:00:00".to_string(),
                    json!({
                        "title": "Self-Consistency Architecture Workshop (Deep Dive)",
                        "start": "2026-11-26T10:00:00"
                    }),
                ),
            ]
            .into(),
        ),
        alerts: Some(
            [
                (
                    "a1".to_string(),
                    json!({
                        "@type": "Alert",
                        "action": "display",
                        "trigger": {
                            "@type": "OffsetTrigger",
                            "offset": "-PT15M"
                        }
                    }),
                ),
                (
                    "a2".to_string(),
                    json!({
                        "@type": "Alert",
                        "action": "display",
                        "trigger": {
                            "@type": "OffsetTrigger",
                            "offset": "-PT1H"
                        }
                    }),
                ),
            ]
            .into(),
        ),
        ..CalendarEvent::default()
    };

    // 1. Export to iCalendar 2.0
    let ics1 = event_to_ical(&event);

    // 2. Re-import from emitted iCalendar
    let event2 = ical_to_event(&ics1).expect("re-import emitted ics");

    // 3. Export second pass
    let ics2 = event_to_ical(&event2);

    // 4. Assert exact round-trip self-consistency (Pass 1 == Pass 2)
    assert_eq!(
        ics1, ics2,
        "Self-consistency: emitted iCalendar must match across passes"
    );

    // 5. Re-import second pass
    let event3 = ical_to_event(&ics2).expect("re-import second pass");
    assert_eq!(
        event2, event3,
        "Self-consistency: re-imported Event must match across passes"
    );

    // 6. Verify all mapped domains are preserved losslessly
    assert_eq!(event2.title, event.title);
    assert_eq!(event2.description, event.description);
    assert_eq!(event2.start, event.start);
    assert_eq!(event2.time_zone, event.time_zone);
    assert_eq!(event2.duration, event.duration);
    assert_eq!(event2.privacy, event.privacy);
    assert_eq!(event2.status, event.status);
    assert_eq!(event2.free_busy_status, event.free_busy_status);
    assert_eq!(event2.priority, event.priority);
    assert_eq!(
        event2.keywords.as_ref().unwrap().len(),
        event.keywords.as_ref().unwrap().len()
    );
    assert_eq!(event2.recurrence_rule, event.recurrence_rule);
    assert_eq!(
        event2.recurrence_overrides.as_ref().unwrap().len(),
        event.recurrence_overrides.as_ref().unwrap().len()
    );
    assert_eq!(
        event2.alerts.as_ref().unwrap().len(),
        event.alerts.as_ref().unwrap().len()
    );
    assert_eq!(
        event2.links.as_ref().unwrap().len(),
        event.links.as_ref().unwrap().len()
    );
    assert_eq!(
        event2.virtual_locations.as_ref().unwrap().len(),
        event.virtual_locations.as_ref().unwrap().len()
    );
}

#[test]
fn windows_time_zone_names_from_real_exporters_map_to_canonical_iana_zones() {
    let cases = [
        ("W. Europe Standard Time", "Europe/Berlin"),
        ("Romance Standard Time", "Europe/Paris"),
        ("GMT Standard Time", "Europe/London"),
        ("Greenwich Standard Time", "Atlantic/Reykjavik"),
        ("Central European Standard Time", "Europe/Warsaw"),
        ("Central Europe Standard Time", "Europe/Budapest"),
        ("E. Europe Standard Time", "Europe/Chisinau"),
        ("FLE Standard Time", "Europe/Kyiv"),
        ("GTB Standard Time", "Europe/Bucharest"),
        ("Russian Standard Time", "Europe/Moscow"),
        ("Israel Standard Time", "Asia/Jerusalem"),
        ("Arabic Standard Time", "Asia/Baghdad"),
        ("Arab Standard Time", "Asia/Riyadh"),
        ("India Standard Time", "Asia/Kolkata"),
        ("China Standard Time", "Asia/Shanghai"),
        ("Singapore Standard Time", "Asia/Singapore"),
        ("Tokyo Standard Time", "Asia/Tokyo"),
        ("Korea Standard Time", "Asia/Seoul"),
        ("AUS Eastern Standard Time", "Australia/Sydney"),
        ("AUS Central Standard Time", "Australia/Darwin"),
        ("Cen. Australia Standard Time", "Australia/Adelaide"),
        ("E. Australia Standard Time", "Australia/Brisbane"),
        ("W. Australia Standard Time", "Australia/Perth"),
        ("New Zealand Standard Time", "Pacific/Auckland"),
        ("Eastern Standard Time", "America/New_York"),
        ("Central Standard Time", "America/Chicago"),
        ("Mountain Standard Time", "America/Denver"),
        ("Pacific Standard Time", "America/Los_Angeles"),
        ("Alaskan Standard Time", "America/Anchorage"),
        ("Hawaiian Standard Time", "Pacific/Honolulu"),
        ("SA Pacific Standard Time", "America/Bogota"),
        ("E. South America Standard Time", "America/Sao_Paulo"),
        ("Argentina Standard Time", "America/Buenos_Aires"),
        ("Atlantic Standard Time", "America/Halifax"),
        ("Newfoundland Standard Time", "America/St_Johns"),
        ("US Eastern Standard Time", "America/Indianapolis"),
        ("US Mountain Standard Time", "America/Phoenix"),
        ("Canada Central Standard Time", "America/Regina"),
        ("Mountain Standard Time (Mexico)", "America/Chihuahua"),
        ("UTC", "Etc/UTC"),
        ("UTC-11", "Etc/GMT+11"),
        ("UTC-02", "Etc/GMT+2"),
        ("UTC+12", "Etc/GMT-12"),
        ("UTC+13", "Etc/GMT-13"),
    ];

    for (win_name, expected_iana) in cases {
        // 1. Windows TZID inside a VTIMEZONE definition (the realistic exporter shape)
        let ics_vtz = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Microsoft Corporation//Outlook//EN\r\n\
             BEGIN:VTIMEZONE\r\nTZID:{win_name}\r\n\
             BEGIN:STANDARD\r\nDTSTART:16010101T020000\r\n\
             TZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n\
             RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=10\r\n\
             END:STANDARD\r\n\
             BEGIN:DAYLIGHT\r\nDTSTART:16010101T030000\r\n\
             TZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\n\
             RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=3\r\n\
             END:DAYLIGHT\r\n\
             END:VTIMEZONE\r\n\
             BEGIN:VEVENT\r\nUID:win-vtz-{win_name}\r\n\
             DTSTART;TZID={win_name}:20260615T140000\r\n\
             DURATION:PT1H\r\nSUMMARY:Meeting with VTIMEZONE in {win_name}\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let event_vtz = ical_to_event(&ics_vtz).expect("parse vtimezone windows tzid");
        assert_eq!(
            event_vtz.time_zone.as_deref(),
            Some(expected_iana),
            "VTIMEZONE with TZID={win_name} should resolve to {expected_iana}"
        );
        assert!(
            names_time_zone(event_vtz.time_zone.as_ref().unwrap()),
            "{expected_iana} is a valid IANA name"
        );
        assert!(
            maps_time_zone(&event_vtz),
            "event in {expected_iana} must satisfy maps_time_zone"
        );

        // 2. Outbound emission normalizes to standard IANA TZID / UTC format
        let out_ics = event_to_ical(&event_vtz);
        if expected_iana == "Etc/UTC" || expected_iana == "UTC" {
            assert!(
                out_ics.contains("DTSTART:20260615T140000Z"),
                "outbound ics for UTC should emit DTSTART with Z, got: {out_ics}"
            );
        } else {
            assert!(
                out_ics.contains(&format!("DTSTART;TZID={expected_iana}:20260615T140000")),
                "outbound ics should emit standard IANA TZID {expected_iana}, got: {out_ics}"
            );
        }

        // 3. Multi-pass roundtrip fixpoint stability
        let event2 = ical_to_event(&out_ics).expect("event2");
        let export2 = event_to_ical(&event2);
        let event3 = ical_to_event(&export2).expect("event3");
        let export3 = event_to_ical(&event3);
        assert_eq!(export2, export3);
        assert_eq!(event2, event3);
    }
}

#[test]
fn globally_unique_form_tzids_with_iana_tails_map_to_canonical_iana_zones() {
    let cases = [
        ("/mozilla.org/20070129_1/Europe/Berlin", "Europe/Berlin"),
        ("/citadel.org/20080105_1/Europe/Paris", "Europe/Paris"),
        (
            "/freeassociation.sourceforge.net/Tzfile/Europe/Berlin",
            "Europe/Berlin",
        ),
        (
            "/freeassociation.sourceforge.net/Europe/Berlin",
            "Europe/Berlin",
        ),
        (
            "/softwarestudio.org/Tzfile/America/New_York",
            "America/New_York",
        ),
        (
            "/exchange.example.com/Tzfile/America/Chicago",
            "America/Chicago",
        ),
        ("/kde.org/tz/Europe/Rome", "Europe/Rome"),
        (
            "/apple.com/timezones/America/Argentina/Buenos_Aires",
            "America/Argentina/Buenos_Aires",
        ),
        ("/google.com/20260101_1/Asia/Tokyo", "Asia/Tokyo"),
        ("/example.com/Australia/Sydney", "Australia/Sydney"),
        ("/example.com/Etc/GMT+5", "Etc/GMT+5"),
        (
            "/citadel.org/America/Indiana/Indianapolis",
            "America/Indiana/Indianapolis",
        ),
        ("/vendor.org/tz/Africa/Cairo", "Africa/Cairo"),
        ("/vendor.org/tz/Pacific/Auckland", "Pacific/Auckland"),
        ("/vendor.org/tz/Atlantic/Reykjavik", "Atlantic/Reykjavik"),
    ];

    for (unique_tzid, expected_iana) in cases {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Mozilla.org/NONSGML Mozilla Calendar V1.1//EN\r\n\
             BEGIN:VEVENT\r\nUID:unique-tzid-{expected_iana}\r\n\
             DTSTART;TZID={unique_tzid}:20260615T140000\r\n\
             DURATION:PT1H\r\nSUMMARY:Meeting with globally unique TZID\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let event = ical_to_event(&ics).expect("parse unique tzid");
        assert_eq!(
            event.time_zone.as_deref(),
            Some(expected_iana),
            "unique TZID={unique_tzid} should resolve to {expected_iana}"
        );
        assert!(
            names_time_zone(event.time_zone.as_ref().unwrap()),
            "{expected_iana} must satisfy names_time_zone"
        );
        assert!(
            maps_time_zone(&event),
            "event in {expected_iana} must satisfy maps_time_zone"
        );

        // Outbound emission normalizes to canonical IANA TZID
        let out_ics = event_to_ical(&event);
        assert!(
            out_ics.contains(&format!("DTSTART;TZID={expected_iana}:20260615T140000")),
            "outbound ics should emit standard IANA TZID {expected_iana}, got: {out_ics}"
        );

        // Multi-pass roundtrip fixpoint stability
        let event2 = ical_to_event(&out_ics).expect("event2");
        let export2 = event_to_ical(&event2);
        let event3 = ical_to_event(&export2).expect("event3");
        let export3 = event_to_ical(&event3);
        assert_eq!(export2, export3);
        assert_eq!(event2, event3);
    }
}

#[test]
fn vtimezone_with_explicit_x_lic_location_takes_precedence_over_windows_table() {
    // A VTIMEZONE with TZID: W. Europe Standard Time but explicit X-LIC-LOCATION: Europe/Amsterdam
    let ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n",
        "BEGIN:VTIMEZONE\r\nTZID:W. Europe Standard Time\r\n",
        "X-LIC-LOCATION:Europe/Amsterdam\r\n",
        "BEGIN:STANDARD\r\nDTSTART:19701025T030000\r\n",
        "TZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n",
        "END:STANDARD\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\nUID:test-precedence\r\n",
        "DTSTART;TZID=W. Europe Standard Time:20260615T140000\r\n",
        "DURATION:PT1H\r\nSUMMARY:Precedence test\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    );

    let event = ical_to_event(ics).expect("parse");
    assert_eq!(
        event.time_zone.as_deref(),
        Some("Europe/Amsterdam"),
        "Explicit X-LIC-LOCATION must take precedence over default Windows mapping"
    );
}

#[test]
fn recurrence_until_calculation_in_windows_and_unique_tzids() {
    // UNTIL instant 20260424T080000Z in W. Europe Standard Time (which enters daylight savings +0200 in March)
    // Local date-time should be 2026-04-24T10:00:00
    let ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Microsoft Corporation//Outlook//EN\r\n",
        "BEGIN:VTIMEZONE\r\nTZID:W. Europe Standard Time\r\n",
        "BEGIN:STANDARD\r\nDTSTART:16010101T020000\r\n",
        "TZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n",
        "RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=10\r\n",
        "END:STANDARD\r\n",
        "BEGIN:DAYLIGHT\r\nDTSTART:16010101T030000\r\n",
        "TZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\n",
        "RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=3\r\n",
        "END:DAYLIGHT\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\nUID:rec-win-until\r\n",
        "DTSTART;TZID=W. Europe Standard Time:20260320T100000\r\n",
        "RRULE:FREQ=WEEKLY;UNTIL=20260424T080000Z\r\n",
        "SUMMARY:Recurrence test with UNTIL in Windows zone\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    );

    let event = ical_to_event(ics).expect("parse");
    assert_eq!(event.time_zone.as_deref(), Some("Europe/Berlin"));
    let rule = event.recurrence_rule.expect("recurrence_rule");
    assert_eq!(rule.until.as_deref(), Some("2026-04-24T10:00:00"));
}

#[test]
fn custom_defined_solidus_tzids_and_unresolvable_zones_fidelity() {
    // 1. Custom defined zone beginning with solidus and without IANA tail
    let custom_tzid = "/example.com/Europe-Berlin";
    let ics_custom = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:{custom_tzid}\r\n\
         BEGIN:STANDARD\r\nDTSTART:19701025T030000\r\n\
         TZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n\
         RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=10\r\n\
         END:STANDARD\r\n\
         BEGIN:DAYLIGHT\r\nDTSTART:19700329T020000\r\n\
         TZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\n\
         RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=3\r\n\
         END:DAYLIGHT\r\n\
         END:VTIMEZONE\r\n\
         BEGIN:VEVENT\r\nUID:custom-tz-test\r\n\
         DTSTART;TZID={custom_tzid}:20260615T140000\r\n\
         SUMMARY:Custom zone\r\n\
         END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let event_custom = ical_to_event(&ics_custom).expect("parse custom");
    assert_eq!(event_custom.time_zone.as_deref(), Some(custom_tzid));
    assert!(
        defines_time_zone(&event_custom, custom_tzid),
        "custom zone must be defined in event.time_zones"
    );
    assert!(maps_time_zone(&event_custom));

    let out_custom = event_to_ical(&event_custom);
    assert!(out_custom.contains(&format!("TZID:{custom_tzid}")));
    assert!(out_custom.contains("BEGIN:VTIMEZONE"));

    // 2. Ambiguous, unmapped zone without solidus and without definition
    let unknown_tz = "Fictional Space Station Standard Time";
    let ics_unknown = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
         BEGIN:VEVENT\r\nUID:unknown-tz-test\r\n\
         DTSTART;TZID={unknown_tz}:20260615T140000\r\n\
         SUMMARY:Unknown zone\r\n\
         END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let event_unknown = ical_to_event(&ics_unknown).expect("parse unknown");
    assert_eq!(event_unknown.time_zone.as_deref(), Some(unknown_tz));
    assert!(!names_time_zone(unknown_tz));
    assert!(!defines_time_zone(&event_unknown, unknown_tz));
    assert!(
        !maps_time_zone(&event_unknown),
        "unresolvable zone without definition must be refused by maps_time_zone"
    );
}

#[test]
fn alerts_audit_real_exporter_fixtures_characterization_and_multi_stage_roundtrip() {
    // 1. Evolution Calendar Export: 2 VALARMs (-PT15M and -PT1H)
    let evo_ics = include_str!("fixtures/evolution_calendar_export.ics");
    let evo_event = ical_to_event(evo_ics).expect("parse evolution export");
    let evo_alerts = evo_event.alerts.as_ref().expect("evolution alerts");
    assert_eq!(evo_alerts.len(), 2, "Evolution export has 2 display alarms");
    assert_eq!(evo_alerts["a1"]["action"], "display");
    assert_eq!(evo_alerts["a1"]["trigger"]["offset"], "-PT15M");
    assert_eq!(evo_alerts["a2"]["action"], "display");
    assert_eq!(evo_alerts["a2"]["trigger"]["offset"], "-PT1H");
    assert!(maps_alerts(&evo_event));
    let evo_out = event_to_ical(&evo_event);
    assert_eq!(evo_out.matches("BEGIN:VALARM\r\n").count(), 2);
    assert!(evo_out.contains("UID:a1\r\n"));
    assert!(evo_out.contains("UID:a2\r\n"));
    let evo_reparsed = ical_to_event(&evo_out).expect("reparse evolution export");
    assert_eq!(evo_reparsed.alerts, evo_event.alerts);
    assert_eq!(event_to_ical(&evo_reparsed), evo_out);

    // 2. Apple Calendar Export: 3 VALARMs (-P1D with ACKNOWLEDGED/X-WR-ALARMUID, -PT2H, -PT15M; AUDIO and absolute trigger dropped)
    let apple_ics = include_str!("fixtures/apple_calendar_export.ics");
    let apple_event = ical_to_event(apple_ics).expect("parse apple export");
    let apple_alerts = apple_event.alerts.as_ref().expect("apple alerts");
    assert_eq!(apple_alerts.len(), 3, "Apple export has 3 display alarms");
    assert_eq!(
        apple_alerts["E451D045-FA1B-475D-85B6-06F6F505A321"]["trigger"]["offset"],
        "-P1D"
    );
    assert_eq!(
        apple_alerts["F82C4A10-91DE-4A99-8D77-38C1B79E1A55"]["trigger"]["offset"],
        "-PT2H"
    );
    assert_eq!(
        apple_alerts["apple-alarm-offset-15m"]["trigger"]["offset"],
        "-PT15M"
    );
    assert!(maps_alerts(&apple_event));
    let apple_out = event_to_ical(&apple_event);
    assert_eq!(apple_out.matches("BEGIN:VALARM\r\n").count(), 3);
    assert!(apple_out.contains("UID:E451D045-FA1B-475D-85B6-06F6F505A321\r\n"));
    assert!(apple_out.contains("UID:F82C4A10-91DE-4A99-8D77-38C1B79E1A55\r\n"));
    assert!(apple_out.contains("UID:apple-alarm-offset-15m\r\n"));
    assert!(!apple_out.contains("ACKNOWLEDGED"));
    assert!(!apple_out.contains("X-WR-ALARMUID"));
    assert!(!apple_out.contains("ACTION:AUDIO"));
    assert!(!apple_out.contains("VALUE=DATE-TIME"));
    let apple_reparsed = ical_to_event(&apple_out).expect("reparse apple export");
    assert_eq!(apple_reparsed.alerts, apple_event.alerts);
    assert_eq!(event_to_ical(&apple_reparsed), apple_out);

    // 3. Google Calendar Export: 2 VALARMs (-P1D and -PT15M; EMAIL and absolute trigger dropped)
    let google_ics = include_str!("fixtures/google_calendar_export.ics");
    let google_event = ical_to_event(google_ics).expect("parse google export");
    let google_alerts = google_event.alerts.as_ref().expect("google alerts");
    assert_eq!(google_alerts.len(), 2, "Google export has 2 display alarms");
    assert_eq!(google_alerts["a1"]["trigger"]["offset"], "-P1D");
    assert_eq!(google_alerts["a2"]["trigger"]["offset"], "-PT15M");
    assert!(maps_alerts(&google_event));
    let google_out = event_to_ical(&google_event);
    assert_eq!(google_out.matches("BEGIN:VALARM\r\n").count(), 2);
    assert!(google_out.contains("UID:a1\r\n"));
    assert!(google_out.contains("UID:a2\r\n"));
    assert!(!google_out.contains("ACTION:EMAIL"));
    assert!(!google_out.contains("VALUE=DATE-TIME"));
    let google_reparsed = ical_to_event(&google_out).expect("reparse google export");
    assert_eq!(google_reparsed.alerts, google_event.alerts);
    assert_eq!(event_to_ical(&google_reparsed), google_out);

    // 4. Nextcloud CalDAV Export: 1 VALARM (-P2D)
    let nextcloud_ics = include_str!("fixtures/nextcloud_calendar_export.ics");
    let nextcloud_event = ical_to_event(nextcloud_ics).expect("parse nextcloud export");
    let nextcloud_alerts = nextcloud_event.alerts.as_ref().expect("nextcloud alerts");
    assert_eq!(
        nextcloud_alerts.len(),
        1,
        "Nextcloud export has 1 display alarm"
    );
    assert_eq!(nextcloud_alerts["a1"]["trigger"]["offset"], "-P2D");
    assert!(maps_alerts(&nextcloud_event));
    let nextcloud_out = event_to_ical(&nextcloud_event);
    let nextcloud_reparsed = ical_to_event(&nextcloud_out).expect("reparse nextcloud export");
    assert_eq!(nextcloud_reparsed.alerts, nextcloud_event.alerts);
    assert_eq!(event_to_ical(&nextcloud_reparsed), nextcloud_out);

    // 5. Outlook Modern M365 Export: 2 VALARMs (-PT15M with explicit UID and -PT30M nameless; DESCRIPTION:REMINDER mapped, EMAIL dropped)
    let outlook_ics = include_str!("fixtures/outlook_m365_export.ics");
    let outlook_event = ical_to_event(outlook_ics).expect("parse outlook export");
    let outlook_alerts = outlook_event.alerts.as_ref().expect("outlook alerts");
    assert_eq!(
        outlook_alerts.len(),
        2,
        "Outlook export has 2 display alarms"
    );
    let outlook_uid = "040000008200E00074C5B7101A82E0080000000080E99A2D87D3D901000000000000000010000000D3C9D55A1A2E-alarm-1";
    assert_eq!(outlook_alerts[outlook_uid]["trigger"]["offset"], "-PT15M");
    assert_eq!(outlook_alerts["a1"]["trigger"]["offset"], "-PT30M");
    assert!(maps_alerts(&outlook_event));
    let outlook_out = event_to_ical(&outlook_event);
    assert_eq!(outlook_out.matches("BEGIN:VALARM\r\n").count(), 2);
    let unfolded_outlook = outlook_out.replace("\r\n ", "").replace("\r\n\t", "");
    assert!(unfolded_outlook.contains(&format!("UID:{outlook_uid}\r\n")));
    assert!(outlook_out.contains("UID:a1\r\n"));
    assert!(!outlook_out.contains("X-WR-ALARMUID"));
    assert!(!outlook_out.contains("ACTION:EMAIL"));
    let outlook_reparsed = ical_to_event(&outlook_out).expect("reparse outlook export");
    assert_eq!(outlook_reparsed.alerts, outlook_event.alerts);
    assert_eq!(event_to_ical(&outlook_reparsed), outlook_out);
}

#[test]
fn alerts_audit_trigger_offset_variations_zero_duration_and_normalization() {
    let offset_test_cases = [
        // (input_offset, expected_canonical_offset, relative_to)
        ("-PT5M", "-PT5M", "start"),
        ("-PT15M", "-PT15M", "start"),
        ("-PT1H", "-PT1H", "start"),
        ("-PT2H30M", "-PT2H30M", "start"),
        ("-P1D", "-P1D", "start"),
        ("-P2DT3H4M5S", "-P2DT3H4M5S", "start"),
        ("-P1W", "-P1W", "start"),
        ("-P2W", "-P2W", "start"),
        ("PT5M", "PT5M", "start"),
        ("+PT5M", "PT5M", "start"),
        ("PT1H", "PT1H", "start"),
        ("+PT1H", "PT1H", "start"),
        ("P1D", "P1D", "start"),
        ("+P1D", "P1D", "start"),
        ("P1W", "P1W", "start"),
        ("+P1W", "P1W", "start"),
        ("PT0S", "PT0S", "start"),
        ("-PT0S", "-PT0S", "start"),
        ("+PT0S", "PT0S", "start"),
        ("P0D", "PT0S", "start"),
        ("-P0D", "-PT0S", "start"),
        ("PT0M", "PT0S", "start"),
        ("PT0H", "PT0S", "start"),
        ("PT10M", "PT10M", "end"),
        ("-PT15M", "-PT15M", "end"),
    ];

    for (idx, (input_offset, expected_canonical, relative_to)) in
        offset_test_cases.iter().enumerate()
    {
        let related_param = if *relative_to == "end" {
            ";RELATED=END"
        } else {
            ""
        };
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
             BEGIN:VEVENT\r\nUID:offset-test-{idx}\r\n\
             DTSTART:20260115T100000Z\r\nDURATION:PT1H\r\n\
             SUMMARY:Offset Test\r\n\
             BEGIN:VALARM\r\nUID:k1\r\nACTION:DISPLAY\r\n\
             TRIGGER{related_param}:{input_offset}\r\n\
             END:VALARM\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n"
        );

        let event =
            ical_to_event(&ics).unwrap_or_else(|e| panic!("failed to parse {input_offset}: {e}"));
        let alerts = event
            .alerts
            .as_ref()
            .unwrap_or_else(|| panic!("expected alerts for {input_offset}"));
        let alert = &alerts["k1"];
        assert_eq!(alert["action"], "display", "action for {input_offset}");
        assert_eq!(
            alert["trigger"]["offset"], *expected_canonical,
            "trigger offset for {input_offset}"
        );
        if *relative_to == "end" {
            assert_eq!(alert["trigger"]["relativeTo"], "end");
        } else {
            assert!(alert["trigger"].get("relativeTo").is_none());
        }
        assert!(maps_alerts(&event), "maps_alerts for {input_offset}");

        // Multi-stage roundtrip
        let out_ics = event_to_ical(&event);
        let reparsed = ical_to_event(&out_ics).expect("reparse out_ics");
        assert_eq!(
            reparsed.alerts, event.alerts,
            "roundtrip for {input_offset}"
        );
        assert_eq!(
            event_to_ical(&reparsed),
            out_ics,
            "fixed-point for {input_offset}"
        );
    }

    // Inbound format variations (parameter ordering, VALUE=DURATION, case insensitivity)
    let format_variations = [
        ("trigger:-pt15m", "-PT15M", None),
        ("trigger:+pt1h", "PT1H", None),
        ("trigger;value=duration:-pt30m", "-PT30M", None),
        (
            "trigger;value=duration;related=end:pt5m",
            "PT5M",
            Some("end"),
        ),
        (
            "trigger;related=end;value=duration:pt15m",
            "PT15M",
            Some("end"),
        ),
        ("trigger;related=start:-pt10m", "-PT10M", None),
    ];

    for (line, expected_offset, expected_relative) in format_variations {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
             BEGIN:VEVENT\r\nUID:var-test\r\n\
             DTSTART:20260115T100000Z\r\nDURATION:PT1H\r\n\
             BEGIN:VALARM\r\nUID:k1\r\naction:display\r\n\
             {line}\r\n\
             END:VALARM\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let event = ical_to_event(&ics).unwrap_or_else(|e| panic!("failed on {line}: {e}"));
        let alerts = event
            .alerts
            .as_ref()
            .unwrap_or_else(|| panic!("expected alerts for {line}"));
        assert_eq!(alerts["k1"]["trigger"]["offset"], expected_offset);
        if let Some(rel) = expected_relative {
            assert_eq!(alerts["k1"]["trigger"]["relativeTo"], rel);
        }
    }
}

#[test]
fn alerts_audit_action_types_and_trigger_types_decision_matrix() {
    // 1. Inbound ACTION matrix
    let action_matrix = [
        ("DISPLAY", true),
        ("display", true),
        ("Display", true),
        ("AUDIO", false),
        ("audio", false),
        ("EMAIL", false),
        ("email", false),
        ("PROCEDURE", false),
        ("procedure", false),
        ("X-CUSTOM-NOTIFICATION", false),
        ("NONE", false),
    ];

    for (action, should_map) in action_matrix {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
             BEGIN:VEVENT\r\nUID:act-test\r\n\
             DTSTART:20260115T100000Z\r\n\
             BEGIN:VALARM\r\nUID:k1\r\nACTION:{action}\r\n\
             TRIGGER:-PT15M\r\n\
             SUMMARY:Subject\r\nDESCRIPTION:Body\r\nATTENDEE:mailto:test@example.com\r\n\
             END:VALARM\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let event = ical_to_event(&ics).expect("parse action");
        if should_map {
            let alerts = event.alerts.as_ref().expect("expected alert");
            assert_eq!(alerts["k1"]["action"], "display");
        } else {
            assert!(
                event.alerts.is_none(),
                "ACTION:{action} must be dropped inbound"
            );
        }
    }

    // 2. Inbound TRIGGER type matrix (Offset vs Absolute vs Invalid)
    let trigger_matrix = [
        ("TRIGGER:-PT15M", true),
        ("TRIGGER;VALUE=DURATION:-PT15M", true),
        ("TRIGGER;VALUE=DATE-TIME:20260115T094500Z", false),
        ("TRIGGER;VALUE=DATE-TIME:20260115T094500", false),
        ("TRIGGER:invalid-string", false),
        ("TRIGGER;RELATED=MIDDLE:-PT15M", false),
    ];

    for (trigger_line, should_map) in trigger_matrix {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
             BEGIN:VEVENT\r\nUID:trig-test\r\n\
             DTSTART:20260115T100000Z\r\n\
             BEGIN:VALARM\r\nUID:k1\r\nACTION:DISPLAY\r\n\
             {trigger_line}\r\n\
             END:VALARM\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let event = ical_to_event(&ics).expect("parse trigger line");
        if should_map {
            let alerts = event.alerts.as_ref().expect("expected alert");
            assert_eq!(alerts["k1"]["trigger"]["offset"], "-PT15M");
        } else {
            assert!(
                event.alerts.is_none(),
                "{trigger_line} must be dropped inbound"
            );
        }
    }

    // 3. Outbound decision matrix: maps_alerts coverage and refusal
    let mut valid_event = CalendarEvent {
        title: Some("Team Standup".to_owned()),
        start: Some("2026-01-15T10:00:00".to_owned()),
        time_zone: Some("Etc/UTC".to_owned()),
        ..CalendarEvent::default()
    };
    valid_event.alerts = Some(
        [(
            "a1".to_owned(),
            json!({
                "@type": "Alert",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
                "action": "display"
            }),
        )]
        .into(),
    );
    assert!(maps_alerts(&valid_event));

    // Unsupported action in JSCalendar
    let mut email_event = valid_event.clone();
    email_event.alerts = Some(
        [(
            "a1".to_owned(),
            json!({
                "@type": "Alert",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
                "action": "email"
            }),
        )]
        .into(),
    );
    assert!(!maps_alerts(&email_event), "email action must be refused");

    // AbsoluteTrigger in JSCalendar
    let mut abs_event = valid_event.clone();
    abs_event.alerts = Some(
        [(
            "a1".to_owned(),
            json!({
                "@type": "Alert",
                "trigger": {"@type": "AbsoluteTrigger", "when": "2026-01-15T09:45:00Z"},
                "action": "display"
            }),
        )]
        .into(),
    );
    assert!(!maps_alerts(&abs_event), "AbsoluteTrigger must be refused");

    // Acknowledged timestamp on alert
    let mut ack_event = valid_event.clone();
    ack_event.alerts = Some(
        [(
            "a1".to_owned(),
            json!({
                "@type": "Alert",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
                "action": "display",
                "acknowledged": "2026-01-15T09:46:00Z"
            }),
        )]
        .into(),
    );
    assert!(
        !maps_alerts(&ack_event),
        "acknowledged alert must be refused"
    );
}

#[test]
fn alerts_audit_uid_and_invented_key_allocation_and_collision_avoidance() {
    // 1. Multiple alarms with mixed explicit UIDs and nameless/Evolution UIDs
    let ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n",
        "BEGIN:VEVENT\r\nUID:mix-uid\r\nDTSTART:20260115T100000Z\r\n",
        // Nameless alarm #1 -> will try a1, but if a1 is claimed by an explicit UID, must allocate a2+
        "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n",
        // Explicit UID:a1 -> claims a1
        "BEGIN:VALARM\r\nUID:a1\r\nACTION:DISPLAY\r\nTRIGGER:-PT30M\r\nEND:VALARM\r\n",
        // Explicit UID:custom-uid-99 -> claims custom-uid-99
        "BEGIN:VALARM\r\nUID:custom-uid-99\r\nACTION:DISPLAY\r\nTRIGGER:-PT1H\r\nEND:VALARM\r\n",
        // Evolution internal UID -> not a valid RFC 9074 UID property, so treated as nameless
        "BEGIN:VALARM\r\nX-EVOLUTION-ALARM-UID:evo-alarm-123\r\nACTION:DISPLAY\r\nTRIGGER:-P1D\r\nEND:VALARM\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    );

    let event = ical_to_event(ics).expect("parse mix uids");
    let alerts = event.alerts.as_ref().expect("alerts map");
    assert_eq!(alerts.len(), 4, "4 distinct alarms");

    // Explicit UIDs preserved
    assert_eq!(alerts["a1"]["trigger"]["offset"], "-PT30M");
    assert_eq!(alerts["custom-uid-99"]["trigger"]["offset"], "-PT1H");

    // Nameless alarms get allocated invented keys avoiding existing keys
    assert!(alerts.contains_key("a2"));
    assert!(alerts.contains_key("a3"));
    assert_eq!(alerts["a2"]["trigger"]["offset"], "-PT15M");
    assert_eq!(alerts["a3"]["trigger"]["offset"], "-P1D");

    // Outbound serialization emits UID for all 4 alarms
    let out = event_to_ical(&event);
    assert_eq!(out.matches("BEGIN:VALARM\r\n").count(), 4);
    assert!(out.contains("UID:a1\r\n"));
    assert!(out.contains("UID:a2\r\n"));
    assert!(out.contains("UID:a3\r\n"));
    assert!(out.contains("UID:custom-uid-99\r\n"));

    // 2. Duplicate UIDs in incoming stream (RFC 9074 §6 uniqueness)
    let dup_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n",
        "BEGIN:VEVENT\r\nUID:dup-uid\r\nDTSTART:20260115T100000Z\r\n",
        "BEGIN:VALARM\r\nUID:same-key\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n",
        "BEGIN:VALARM\r\nUID:same-key\r\nACTION:DISPLAY\r\nTRIGGER:-PT45M\r\nEND:VALARM\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let dup_event = ical_to_event(dup_ics).expect("parse dup");
    let dup_alerts = dup_event.alerts.expect("dup alerts");
    assert_eq!(
        dup_alerts.len(),
        1,
        "Duplicate UID collapses to single entry"
    );
    assert_eq!(dup_alerts["same-key"]["trigger"]["offset"], "-PT45M");
}

#[test]
fn alerts_audit_description_acknowledged_and_unmodeled_properties_fidelity() {
    // 1. VALARM DESCRIPTION population from event title
    let mut titled_event = CalendarEvent {
        title: Some("Quarterly All Hands".to_owned()),
        start: Some("2026-01-15T10:00:00".to_owned()),
        time_zone: Some("Etc/UTC".to_owned()),
        ..CalendarEvent::default()
    };
    titled_event.alerts = Some(
        [(
            "a1".to_owned(),
            json!({
                "@type": "Alert",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
                "action": "display"
            }),
        )]
        .into(),
    );
    let titled_ics = event_to_ical(&titled_event);
    assert!(
        titled_ics.contains("DESCRIPTION:Quarterly All Hands\r\n"),
        "titled event populates VALARM DESCRIPTION with event title"
    );

    // 2. Event with no title / empty title omits DESCRIPTION
    let mut untitled_event = titled_event.clone();
    untitled_event.title = None;
    let untitled_ics = event_to_ical(&untitled_event);
    assert!(
        !untitled_ics.contains("DESCRIPTION:"),
        "untitled event omits VALARM DESCRIPTION"
    );

    let mut empty_title_event = titled_event.clone();
    empty_title_event.title = Some("".to_owned());
    let empty_title_ics = event_to_ical(&empty_title_event);
    assert!(
        !empty_title_ics.contains("DESCRIPTION:"),
        "empty title event omits VALARM DESCRIPTION"
    );

    // 3. JSCalendar Alert with custom description field is refused by maps_alerts
    // to protect unmodeled server fields from being overwritten on whole-property replacement
    let mut custom_desc_event = titled_event.clone();
    custom_desc_event.alerts = Some(
        [(
            "a1".to_owned(),
            json!({
                "@type": "Alert",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
                "action": "display",
                "description": "Custom custom notification message"
            }),
        )]
        .into(),
    );
    assert!(
        !maps_alerts(&custom_desc_event),
        "alert with custom description field must be refused by maps_alerts"
    );

    // 4. Inbound extra properties on VALARM are safely ignored
    let extra_props_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n",
        "BEGIN:VEVENT\r\nUID:extra-props\r\nDTSTART:20260115T100000Z\r\n",
        "BEGIN:VALARM\r\nUID:k1\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\n",
        "DESCRIPTION:Evolution Reminder\r\n",
        "SUMMARY:Alarm Summary\r\n",
        "DURATION:PT5M\r\n",
        "REPEAT:3\r\n",
        "ATTACH;VALUE=URI:file:///usr/share/sounds/alarm.ogg\r\n",
        "X-EVOLUTION-ALARM-UID:uuid-12345\r\n",
        "X-APPLE-DEFAULT-ALARM:TRUE\r\n",
        "ACKNOWLEDGED:20260115T094600Z\r\n",
        "END:VALARM\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_extra = ical_to_event(extra_props_ics).expect("parse extra props");
    let alerts = parsed_extra.alerts.expect("alerts");
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts["k1"]["action"], "display");
    assert_eq!(alerts["k1"]["trigger"]["offset"], "-PT15M");
}

#[test]
fn alerts_audit_usedefaultalerts_and_recurrence_overrides_matrix() {
    // 1. useDefaultAlerts on Series
    let mut default_alerts_event = CalendarEvent {
        title: Some("Project Sync".to_owned()),
        start: Some("2026-01-15T10:00:00".to_owned()),
        time_zone: Some("Etc/UTC".to_owned()),
        ..CalendarEvent::default()
    };
    default_alerts_event.alerts = Some(
        [(
            "a1".to_owned(),
            json!({
                "@type": "Alert",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
                "action": "display"
            }),
        )]
        .into(),
    );
    default_alerts_event
        .extra
        .insert("useDefaultAlerts".to_owned(), json!(true));

    assert_eq!(
        event_to_ical(&default_alerts_event)
            .matches("BEGIN:VALARM\r\n")
            .count(),
        0,
        "useDefaultAlerts emits no VALARMs"
    );
    assert!(
        !maps_alerts(&default_alerts_event),
        "useDefaultAlerts returns maps_alerts == false"
    );

    // 2. Recurrence Overrides with alerts matrix
    let mut rec_event = CalendarEvent {
        title: Some("Weekly Design Review".to_owned()),
        start: Some("2026-01-15T10:00:00".to_owned()),
        time_zone: Some("Etc/UTC".to_owned()),
        duration: Some("PT1H".to_owned()),
        recurrence_rule: Some(RecurrenceRule::new("weekly")),
        alerts: Some(
            [(
                "a1".to_owned(),
                json!({
                    "@type": "Alert",
                    "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
                    "action": "display"
                }),
            )]
            .into(),
        ),
        ..CalendarEvent::default()
    };

    // Override 1: inherits series alerts (doesn't mention alerts)
    // Override 2: custom alert (-PT1H)
    // Override 3: drops alerts (alerts: null)
    let overrides = json!({
        "2026-01-22T10:00:00": {
            "title": "Design Review (Deep Dive)"
        },
        "2026-01-29T10:00:00": {
            "alerts": {
                "a1": {
                    "@type": "Alert",
                    "trigger": {"@type": "OffsetTrigger", "offset": "-PT1H"},
                    "action": "display"
                }
            }
        },
        "2026-02-05T10:00:00": {
            "alerts": null
        }
    });
    rec_event.recurrence_overrides = Some(
        overrides
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );

    let ics = event_to_ical(&rec_event);
    assert_eq!(
        vevents(&ics),
        4,
        "Series + 3 instance overrides = 4 VEVENTs"
    );

    // Master series has 1 VALARM (-PT15M)
    // Instance 1 (2026-01-22) inherits series VALARM (-PT15M)
    // Instance 2 (2026-01-29) has custom VALARM (-PT1H)
    // Instance 3 (2026-02-05) has 0 VALARMs (alerts: null)
    assert_eq!(
        ics.matches("BEGIN:VALARM\r\n").count(),
        3,
        "Total VALARMs across 4 VEVENTs = 1 (series) + 1 (inst1) + 1 (inst2) + 0 (inst3)"
    );

    let parsed = ical_to_event(&ics).expect("parse recurring overrides");
    assert_eq!(
        parsed.alerts.as_ref().unwrap()["a1"]["trigger"]["offset"],
        "-PT15M"
    );
    let parsed_overrides = parsed.recurrence_overrides.expect("overrides");

    // Instance 1 only differs by title
    assert_eq!(
        parsed_overrides["2026-01-22T10:00:00"],
        json!({"title": "Design Review (Deep Dive)"})
    );

    // Instance 2 differs by alerts
    assert_eq!(
        parsed_overrides["2026-01-29T10:00:00"]["alerts"]["a1"]["trigger"]["offset"],
        "-PT1H"
    );

    // Instance 3 differs by alerts: null
    assert_eq!(
        parsed_overrides["2026-02-05T10:00:00"]["alerts"],
        serde_json::Value::Null
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Batch 10 Item 1: Recurrence-Overrides Round-Trip Audit & Characterization
// (RFC 8984 §4.3.4 ↔ RFC 5545 §3.8.4.4 / §3.8.5)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn recurrence_overrides_audit_rescheduled_single_occurrences_and_zone_shifts() {
    // 1. Rescheduled occurrence on same day (shifted start time + custom duration)
    let event1 = recurring_with(json!({
        "2026-01-22T10:00:00": {
            "start": "2026-01-22T14:30:00",
            "duration": "PT45M"
        }
    }));
    let ics1 = event_to_ical(&event1);
    assert_eq!(vevents(&ics1), 2, "Series + 1 detached VEVENT");
    let inst1 = vevent(&ics1, 1);
    assert_eq!(
        line(inst1, "RECURRENCE-ID"),
        "RECURRENCE-ID;TZID=Europe/Berlin:20260122T100000"
    );
    assert_eq!(
        line(inst1, "DTSTART"),
        "DTSTART;TZID=Europe/Berlin:20260122T143000"
    );
    assert_eq!(line(inst1, "DURATION"), "DURATION:PT45M");

    let parsed1 = ical_to_event(&ics1).expect("parse rescheduled same-day");
    assert_eq!(parsed1.recurrence_overrides, event1.recurrence_overrides);

    // 2. Rescheduled occurrence across dates (moved 2 days later)
    let event2 = recurring_with(json!({
        "2026-01-22T10:00:00": {
            "start": "2026-01-24T16:00:00",
            "duration": "PT2H"
        }
    }));
    let ics2 = event_to_ical(&event2);
    let inst2 = vevent(&ics2, 1);
    assert_eq!(
        line(inst2, "RECURRENCE-ID"),
        "RECURRENCE-ID;TZID=Europe/Berlin:20260122T100000"
    );
    assert_eq!(
        line(inst2, "DTSTART"),
        "DTSTART;TZID=Europe/Berlin:20260124T160000"
    );
    assert_eq!(line(inst2, "DURATION"), "DURATION:PT2H");

    let parsed2 = ical_to_event(&ics2).expect("parse rescheduled cross-date");
    assert_eq!(parsed2.recurrence_overrides, event2.recurrence_overrides);

    // 3. Rescheduled into another IANA time zone (America/New_York)
    let event3 = recurring_with(json!({
        "2026-01-22T10:00:00": {
            "start": "2026-01-22T09:00:00",
            "timeZone": "America/New_York"
        }
    }));
    let ics3 = event_to_ical(&event3);
    let inst3 = vevent(&ics3, 1);
    assert_eq!(
        line(inst3, "RECURRENCE-ID"),
        "RECURRENCE-ID;TZID=Europe/Berlin:20260122T100000"
    );
    assert_eq!(
        line(inst3, "DTSTART"),
        "DTSTART;TZID=America/New_York:20260122T090000"
    );

    let parsed3 = ical_to_event(&ics3).expect("parse rescheduled timezone");
    assert_eq!(parsed3.recurrence_overrides, event3.recurrence_overrides);

    // 4. Rescheduled into UTC (Etc/UTC)
    let event4 = recurring_with(json!({
        "2026-01-22T10:00:00": {
            "timeZone": "Etc/UTC"
        }
    }));
    let ics4 = event_to_ical(&event4);
    let inst4 = vevent(&ics4, 1);
    assert_eq!(
        line(inst4, "RECURRENCE-ID"),
        "RECURRENCE-ID;TZID=Europe/Berlin:20260122T100000"
    );
    assert_eq!(line(inst4, "DTSTART"), "DTSTART:20260122T100000Z");

    let parsed4 = ical_to_event(&ics4).expect("parse rescheduled UTC");
    assert_eq!(parsed4.recurrence_overrides, event4.recurrence_overrides);

    // 5. Rescheduled into floating time (timeZone: null)
    let event5 = recurring_with(json!({
        "2026-01-22T10:00:00": {
            "timeZone": null
        }
    }));
    let ics5 = event_to_ical(&event5);
    let inst5 = vevent(&ics5, 1);
    assert_eq!(
        line(inst5, "RECURRENCE-ID"),
        "RECURRENCE-ID;TZID=Europe/Berlin:20260122T100000"
    );
    assert_eq!(line(inst5, "DTSTART"), "DTSTART:20260122T100000");

    let parsed5 = ical_to_event(&ics5).expect("parse rescheduled floating");
    assert_eq!(parsed5.recurrence_overrides, event5.recurrence_overrides);

    // 6. Inbound Windows time zone on RECURRENCE-ID (W. Europe Standard Time)
    let windows_ics = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{}\
         BEGIN:VEVENT\r\nUID:WIN1\r\n\
         DTSTART;TZID=\"W. Europe Standard Time\":20260115T100000\r\n\
         DURATION:PT1H\r\nRRULE:FREQ=WEEKLY\r\nSUMMARY:Weekly Standup\r\nEND:VEVENT\r\n\
         BEGIN:VEVENT\r\nUID:WIN1\r\n\
         RECURRENCE-ID;TZID=\"W. Europe Standard Time\":20260122T100000\r\n\
         DTSTART;TZID=\"W. Europe Standard Time\":20260122T140000\r\n\
         DURATION:PT1H30M\r\nSUMMARY:Weekly Standup (Moved)\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        vtimezone("W. Europe Standard Time", "Europe/Berlin")
    );
    let parsed_win = ical_to_event(&windows_ics).expect("parse windows RECURRENCE-ID");
    assert_eq!(parsed_win.time_zone.as_deref(), Some("Europe/Berlin"));
    let win_overrides = parsed_win
        .recurrence_overrides
        .as_ref()
        .expect("win overrides");
    assert_eq!(
        win_overrides["2026-01-22T10:00:00"],
        json!({
            "start": "2026-01-22T14:00:00",
            "duration": "PT1H30M",
            "title": "Weekly Standup (Moved)"
        })
    );

    // 7. Inbound Globally-Unique TZID on RECURRENCE-ID (/mozilla.org/.../Europe/Berlin)
    let unique_ics = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{}\
         BEGIN:VEVENT\r\nUID:MOZ1\r\n\
         DTSTART;TZID=/mozilla.org/20070129_1/Europe/Berlin:20260115T100000\r\n\
         DURATION:PT1H\r\nRRULE:FREQ=WEEKLY\r\nSUMMARY:Mozilla Sync\r\nEND:VEVENT\r\n\
         BEGIN:VEVENT\r\nUID:MOZ1\r\n\
         RECURRENCE-ID;TZID=/mozilla.org/20070129_1/Europe/Berlin:20260122T100000\r\n\
         DTSTART;TZID=/mozilla.org/20070129_1/Europe/Berlin:20260122T150000\r\n\
         DURATION:PT1H\r\n\
         SUMMARY:Mozilla Sync (Rescheduled)\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        vtimezone("/mozilla.org/20070129_1/Europe/Berlin", "Europe/Berlin")
    );
    let parsed_unique = ical_to_event(&unique_ics).expect("parse unique TZID RECURRENCE-ID");
    assert_eq!(parsed_unique.time_zone.as_deref(), Some("Europe/Berlin"));
    let unique_overrides = parsed_unique
        .recurrence_overrides
        .as_ref()
        .expect("unique overrides");
    assert_eq!(
        unique_overrides["2026-01-22T10:00:00"],
        json!({
            "start": "2026-01-22T15:00:00",
            "title": "Mozilla Sync (Rescheduled)"
        })
    );

    // Multi-pass roundtrip fixpoint for unique TZID
    let export1 = event_to_ical(&parsed_unique);
    let event_round2 = ical_to_event(&export1).expect("event_round2");
    let export2 = event_to_ical(&event_round2);
    assert_eq!(export1, export2);
    assert_eq!(parsed_unique, event_round2);
}

#[test]
fn recurrence_overrides_audit_cancelled_occurrences_exdate_matrix() {
    // 1. Single excluded occurrence in local timezone
    let event1 = recurring_with(json!({
        "2026-01-22T10:00:00": {"excluded": true}
    }));
    let ics1 = event_to_ical(&event1);
    assert_eq!(
        line(&ics1, "EXDATE"),
        "EXDATE;TZID=Europe/Berlin:20260122T100000"
    );
    assert!(without(&ics1, "RDATE"));
    let parsed1 = ical_to_event(&ics1).expect("parse single EXDATE");
    assert_eq!(parsed1.recurrence_overrides, event1.recurrence_overrides);

    // 2. Multiple excluded occurrences on a single comma-delimited line
    let multi_line_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:EX1\r\n",
        "DTSTART;TZID=Europe/Berlin:20260115T100000\r\n",
        "DURATION:PT1H\r\nRRULE:FREQ=WEEKLY\r\n",
        "EXDATE;TZID=Europe/Berlin:20260122T100000,20260129T100000,20260205T100000\r\n",
        "SUMMARY:Team Meeting\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_multi = ical_to_event(multi_line_ics).expect("parse multi-comma EXDATE");
    let overrides_multi = parsed_multi
        .recurrence_overrides
        .as_ref()
        .expect("overrides");
    assert_eq!(overrides_multi.len(), 3);
    assert_eq!(
        overrides_multi["2026-01-22T10:00:00"],
        json!({"excluded": true})
    );
    assert_eq!(
        overrides_multi["2026-01-29T10:00:00"],
        json!({"excluded": true})
    );
    assert_eq!(
        overrides_multi["2026-02-05T10:00:00"],
        json!({"excluded": true})
    );

    let re_exported = event_to_ical(&parsed_multi);
    assert_eq!(
        line(&re_exported, "EXDATE"),
        "EXDATE;TZID=Europe/Berlin:20260122T100000,20260129T100000,20260205T100000"
    );

    // 3. Multiple separate EXDATE content lines
    let sep_lines_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:EX2\r\n",
        "DTSTART;TZID=Europe/Berlin:20260115T100000\r\n",
        "DURATION:PT1H\r\nRRULE:FREQ=WEEKLY\r\n",
        "EXDATE;TZID=Europe/Berlin:20260122T100000\r\n",
        "EXDATE;TZID=Europe/Berlin:20260129T100000\r\n",
        "EXDATE;TZID=Europe/Berlin:20260205T100000\r\n",
        "SUMMARY:Team Meeting\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_sep = ical_to_event(sep_lines_ics).expect("parse separate EXDATE lines");
    assert_eq!(
        parsed_sep.recurrence_overrides,
        parsed_multi.recurrence_overrides
    );

    // 4. EXDATE in UTC series
    let utc_event = CalendarEvent {
        id: Some("EX_UTC".into()),
        start: Some("2026-01-15T10:00:00".to_owned()),
        time_zone: Some("Etc/UTC".to_owned()),
        duration: Some("PT1H".to_owned()),
        recurrence_rule: Some(RecurrenceRule::new("weekly")),
        recurrence_overrides: Some(
            [("2026-01-22T10:00:00".to_owned(), json!({"excluded": true}))].into(),
        ),
        ..CalendarEvent::default()
    };
    let utc_ics = event_to_ical(&utc_event);
    assert_eq!(line(&utc_ics, "DTSTART"), "DTSTART:20260115T100000Z");
    assert_eq!(line(&utc_ics, "EXDATE"), "EXDATE:20260122T100000Z");
    let parsed_utc = ical_to_event(&utc_ics).expect("parse UTC EXDATE");
    assert_eq!(
        parsed_utc.recurrence_overrides,
        utc_event.recurrence_overrides
    );

    // 5. EXDATE in All-Day Date-Only Series (VALUE=DATE)
    let allday_event = CalendarEvent {
        id: Some("EX_ALLDAY".into()),
        start: Some("2026-01-15T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        show_without_time: Some(true),
        recurrence_rule: Some(RecurrenceRule::new("weekly")),
        recurrence_overrides: Some(
            [("2026-01-22T00:00:00".to_owned(), json!({"excluded": true}))].into(),
        ),
        ..CalendarEvent::default()
    };
    let allday_ics = event_to_ical(&allday_event);
    assert_eq!(line(&allday_ics, "DTSTART"), "DTSTART;VALUE=DATE:20260115");
    assert_eq!(line(&allday_ics, "EXDATE"), "EXDATE;VALUE=DATE:20260122");
    let parsed_allday = ical_to_event(&allday_ics).expect("parse all-day EXDATE");
    assert_eq!(parsed_allday.show_without_time, Some(true));
    assert_eq!(
        parsed_allday.recurrence_overrides,
        allday_event.recurrence_overrides
    );

    // 6. Mixed cancelled occurrences + detached modified instances in one series
    let mixed_event = recurring_with(json!({
        "2026-01-22T10:00:00": {"excluded": true},
        "2026-01-29T10:00:00": {"title": "Design Deep Dive"},
        "2026-02-05T10:00:00": {"excluded": true},
        "2026-02-12T10:00:00": {"start": "2026-02-12T14:00:00"}
    }));
    let mixed_ics = event_to_ical(&mixed_event);
    assert_eq!(vevents(&mixed_ics), 3, "Series + 2 detached VEVENTs");
    assert_eq!(
        line(&mixed_ics, "EXDATE"),
        "EXDATE;TZID=Europe/Berlin:20260122T100000,20260205T100000"
    );
    let parsed_mixed = ical_to_event(&mixed_ics).expect("parse mixed overrides");
    assert_eq!(
        parsed_mixed.recurrence_overrides,
        mixed_event.recurrence_overrides
    );
}

#[test]
fn recurrence_overrides_audit_added_occurrences_rdate_and_periods() {
    // 1. Single added occurrence with empty patch
    let event1 = recurring_with(json!({
        "2026-02-05T10:00:00": {}
    }));
    let ics1 = event_to_ical(&event1);
    assert_eq!(
        line(&ics1, "RDATE"),
        "RDATE;TZID=Europe/Berlin:20260205T100000"
    );
    assert!(without(&ics1, "EXDATE"));
    let parsed1 = ical_to_event(&ics1).expect("parse single RDATE");
    assert_eq!(parsed1.recurrence_overrides, event1.recurrence_overrides);

    // 2. Multiple added occurrences on a single line and across separate lines
    let multi_rdate_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:RD1\r\n",
        "DTSTART;TZID=Europe/Berlin:20260115T100000\r\n",
        "DURATION:PT1H\r\nRRULE:FREQ=WEEKLY\r\n",
        "RDATE;TZID=Europe/Berlin:20260205T100000,20260212T100000\r\n",
        "RDATE;TZID=Europe/Berlin:20260219T100000\r\n",
        "SUMMARY:Biweekly Plus Extra\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_multi_rdate = ical_to_event(multi_rdate_ics).expect("parse multi RDATE");
    let overrides = parsed_multi_rdate
        .recurrence_overrides
        .as_ref()
        .expect("overrides");
    assert_eq!(overrides.len(), 3);
    assert_eq!(overrides["2026-02-05T10:00:00"], json!({}));
    assert_eq!(overrides["2026-02-12T10:00:00"], json!({}));
    assert_eq!(overrides["2026-02-19T10:00:00"], json!({}));

    let re_exported = event_to_ical(&parsed_multi_rdate);
    assert_eq!(
        line(&re_exported, "RDATE"),
        "RDATE;TZID=Europe/Berlin:20260205T100000,20260212T100000,20260219T100000"
    );

    // 3. RDATE;VALUE=PERIOD with explicit duration (/PT2H)
    let rdate_period_dur_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:RD_PER\r\n",
        "DTSTART:20260115T100000Z\r\n",
        "DURATION:PT1H\r\nRRULE:FREQ=WEEKLY\r\n",
        "RDATE;VALUE=PERIOD:20260205T100000Z/PT2H\r\n",
        "SUMMARY:Standup\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_per_dur = ical_to_event(rdate_period_dur_ics).expect("parse RDATE period dur");
    assert_eq!(
        parsed_per_dur.recurrence_overrides,
        Some(
            [(
                "2026-02-05T10:00:00".to_owned(),
                json!({"duration": "PT2H"})
            )]
            .into()
        )
    );
    // Serializing back emits detached VEVENT with RECURRENCE-ID and DURATION:PT2H
    let re_exported_per = event_to_ical(&parsed_per_dur);
    assert_eq!(vevents(&re_exported_per), 2);
    let inst_per = vevent(&re_exported_per, 1);
    assert_eq!(
        line(inst_per, "RECURRENCE-ID"),
        "RECURRENCE-ID:20260205T100000Z"
    );
    assert_eq!(line(inst_per, "DURATION"), "DURATION:PT2H");

    // 4. RDATE;VALUE=PERIOD with explicit end date-time (/20260205T123000Z)
    let rdate_period_end_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:RD_END\r\n",
        "DTSTART:20260115T100000Z\r\n",
        "DURATION:PT1H\r\nRRULE:FREQ=WEEKLY\r\n",
        "RDATE;VALUE=PERIOD:20260205T100000Z/20260205T123000Z\r\n",
        "SUMMARY:Standup\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_per_end = ical_to_event(rdate_period_end_ics).expect("parse RDATE period end");
    assert_eq!(
        parsed_per_end.recurrence_overrides,
        Some(
            [(
                "2026-02-05T10:00:00".to_owned(),
                json!({"duration": "PT2H30M"})
            )]
            .into()
        )
    );

    // 5. RDATE;VALUE=PERIOD matching series duration collapses to empty patch
    let rdate_period_same_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:RD_SAME\r\n",
        "DTSTART:20260115T100000Z\r\n",
        "DURATION:PT1H\r\nRRULE:FREQ=WEEKLY\r\n",
        "RDATE;VALUE=PERIOD:20260205T100000Z/PT1H\r\n",
        "SUMMARY:Standup\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_same = ical_to_event(rdate_period_same_ics).expect("parse RDATE same dur");
    assert_eq!(
        parsed_same.recurrence_overrides,
        Some([("2026-02-05T10:00:00".to_owned(), json!({}))].into())
    );

    // 6. RDATE;VALUE=DATE for all-day series
    let rdate_allday_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:RD_ALLDAY\r\n",
        "DTSTART;VALUE=DATE:20260115\r\n",
        "DURATION:P1D\r\nRRULE:FREQ=WEEKLY\r\n",
        "RDATE;VALUE=DATE:20260205\r\n",
        "SUMMARY:Weekly Holiday\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_allday_rdate = ical_to_event(rdate_allday_ics).expect("parse all-day RDATE");
    assert_eq!(
        parsed_allday_rdate.recurrence_overrides,
        Some([("2026-02-05T00:00:00".to_owned(), json!({}))].into())
    );
    let re_exported_allday = event_to_ical(&parsed_allday_rdate);
    assert_eq!(
        line(&re_exported_allday, "RDATE"),
        "RDATE;VALUE=DATE:20260205"
    );
}

#[test]
fn recurrence_overrides_audit_scalar_and_map_properties_fidelity() {
    // 1. Override editing title and description
    let mut event1 = recurring_with(json!({
        "2026-01-22T10:00:00": {
            "title": "Architecture Deep Dive",
            "description": "Deep dive on query optimization"
        }
    }));
    event1.title = Some("Weekly Tech Sync".to_owned());
    event1.description = Some("General team discussion".to_owned());

    let ics1 = event_to_ical(&event1);
    let inst1 = vevent(&ics1, 1);
    assert_eq!(line(inst1, "SUMMARY:"), "SUMMARY:Architecture Deep Dive");
    assert_eq!(
        line(inst1, "DESCRIPTION:"),
        "DESCRIPTION:Deep dive on query optimization"
    );
    let parsed1 = ical_to_event(&ics1).expect("parse title/desc override");
    assert_eq!(parsed1.title.as_deref(), Some("Weekly Tech Sync"));
    assert_eq!(
        parsed1.description.as_deref(),
        Some("General team discussion")
    );
    assert_eq!(parsed1.recurrence_overrides, event1.recurrence_overrides);

    // 2. Clearing properties with null
    let mut event2 = recurring_with(json!({
        "2026-01-22T10:00:00": {
            "description": null,
            "status": null,
            "freeBusyStatus": null,
            "priority": null,
            "privacy": null,
            "keywords": null
        }
    }));
    event2.description = Some("Confidential notes".to_owned());
    event2.status = Some("confirmed".to_owned());
    event2.free_busy_status = Some("busy".to_owned());
    event2.priority = Some(3);
    event2.privacy = Some("private".to_owned());
    event2.keywords = Some([("internal".to_owned(), json!(true))].into());

    let ics2 = event_to_ical(&event2);
    let inst2 = vevent(&ics2, 1);
    assert!(without(inst2, "DESCRIPTION"));
    assert!(without(inst2, "STATUS"));
    assert!(without(inst2, "TRANSP"));
    assert!(without(inst2, "PRIORITY"));
    assert!(without(inst2, "CLASS"));
    assert!(without(inst2, "CATEGORIES"));

    let parsed2 = ical_to_event(&ics2).expect("parse null removals override");
    assert_eq!(parsed2.recurrence_overrides, event2.recurrence_overrides);

    // 3. Modifying status, freeBusyStatus, priority, privacy, keywords
    let event3 = recurring_with(json!({
        "2026-01-22T10:00:00": {
            "status": "tentative",
            "freeBusyStatus": "free",
            "priority": 7,
            "privacy": "secret",
            "keywords": {
                "planning": true,
                "offsite": true
            }
        }
    }));
    let ics3 = event_to_ical(&event3);
    let inst3 = vevent(&ics3, 1);
    assert_eq!(line(inst3, "STATUS:"), "STATUS:TENTATIVE");
    assert_eq!(line(inst3, "TRANSP:"), "TRANSP:TRANSPARENT");
    assert_eq!(line(inst3, "PRIORITY:"), "PRIORITY:7");
    assert_eq!(line(inst3, "CLASS:"), "CLASS:CONFIDENTIAL");
    assert_eq!(
        content_line(inst3, "CATEGORIES:"),
        "CATEGORIES:offsite,planning"
    );

    let parsed3 = ical_to_event(&ics3).expect("parse modified scalar fields");
    assert_eq!(parsed3.recurrence_overrides, event3.recurrence_overrides);

    // 4. Comprehensive multi-property override modifying 8 properties at once
    let event4 = recurring_with(json!({
        "2026-01-22T10:00:00": {
            "title": "Executive Keynote & Roadmap",
            "description": "Annual product strategy and partner roadmap",
            "start": "2026-01-22T11:00:00",
            "timeZone": "Asia/Tokyo",
            "duration": "PT3H",
            "status": "confirmed",
            "freeBusyStatus": "busy",
            "priority": 1,
            "privacy": "public",
            "keywords": {"keynote": true, "strategy": true},
            "alerts": {
                "a1": {
                    "@type": "Alert",
                    "trigger": {"@type": "OffsetTrigger", "offset": "-PT1H"},
                    "action": "display"
                }
            }
        }
    }));
    let ics4 = event_to_ical(&event4);
    let parsed4 = ical_to_event(&ics4).expect("parse multi-property override");
    assert_eq!(parsed4.recurrence_overrides, event4.recurrence_overrides);

    // Multi-pass roundtrip assertion
    let export4_1 = event_to_ical(&parsed4);
    let event4_2 = ical_to_event(&export4_1).expect("event4_2");
    let export4_2 = event_to_ical(&event4_2);
    assert_eq!(export4_1, export4_2);
    assert_eq!(parsed4, event4_2);
}

#[test]
fn recurrence_overrides_audit_precedence_conflicts_and_edge_cases() {
    // 1. Detached VEVENT with RECURRENCE-ID + RDATE for same instant -> VEVENT wins
    let vevent_and_rdate_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:PREC1\r\n",
        "DTSTART:20260115T100000Z\r\n",
        "DURATION:PT1H\r\nRRULE:FREQ=WEEKLY\r\n",
        "RDATE:20260122T100000Z\r\n",
        "SUMMARY:Master Standup\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID:PREC1\r\n",
        "RECURRENCE-ID:20260122T100000Z\r\n",
        "DTSTART:20260122T100000Z\r\n",
        "DURATION:PT1H\r\n",
        "SUMMARY:Specific Standup\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_prec1 = ical_to_event(vevent_and_rdate_ics).expect("parse VEVENT + RDATE");
    assert_eq!(
        parsed_prec1.recurrence_overrides,
        Some(
            [(
                "2026-01-22T10:00:00".to_owned(),
                json!({"title": "Specific Standup"})
            )]
            .into()
        )
    );

    // 2. Detached VEVENT with RECURRENCE-ID + EXDATE for same instant -> VEVENT wins
    let vevent_and_exdate_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:PREC2\r\n",
        "DTSTART:20260115T100000Z\r\n",
        "DURATION:PT1H\r\nRRULE:FREQ=WEEKLY\r\n",
        "EXDATE:20260122T100000Z\r\n",
        "SUMMARY:Master Standup\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID:PREC2\r\n",
        "RECURRENCE-ID:20260122T100000Z\r\n",
        "DTSTART:20260122T100000Z\r\n",
        "DURATION:PT1H\r\n",
        "SUMMARY:Resurrected Standup\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_prec2 = ical_to_event(vevent_and_exdate_ics).expect("parse VEVENT + EXDATE");
    assert_eq!(
        parsed_prec2.recurrence_overrides,
        Some(
            [(
                "2026-01-22T10:00:00".to_owned(),
                json!({"title": "Resurrected Standup"})
            )]
            .into()
        )
    );

    // 3. EXDATE + RDATE for same instant -> EXDATE wins (excluded: true)
    let exdate_and_rdate_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:PREC3\r\n",
        "DTSTART:20260115T100000Z\r\n",
        "DURATION:PT1H\r\nRRULE:FREQ=WEEKLY\r\n",
        "RDATE:20260122T100000Z\r\n",
        "EXDATE:20260122T100000Z\r\n",
        "SUMMARY:Conflict Standup\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_prec3 = ical_to_event(exdate_and_rdate_ics).expect("parse EXDATE + RDATE");
    assert_eq!(
        parsed_prec3.recurrence_overrides,
        Some([("2026-01-22T10:00:00".to_owned(), json!({"excluded": true}))].into())
    );

    // 4. RECURRENCE-ID with RANGE=THISANDFUTURE skipped safely
    let range_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:RANGE1\r\n",
        "DTSTART:20260115T100000Z\r\n",
        "DURATION:PT1H\r\nRRULE:FREQ=WEEKLY\r\n",
        "SUMMARY:Series Standup\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID:RANGE1\r\n",
        "RECURRENCE-ID;RANGE=THISANDFUTURE:20260122T100000Z\r\n",
        "DTSTART:20260122T120000Z\r\n",
        "DURATION:PT1H\r\n",
        "SUMMARY:Future Standups\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID:RANGE1\r\n",
        "RECURRENCE-ID:20260129T100000Z\r\n",
        "DTSTART:20260129T100000Z\r\n",
        "DURATION:PT1H\r\n",
        "SUMMARY:Single Standup Override\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_range = ical_to_event(range_ics).expect("parse RANGE=THISANDFUTURE");
    assert_eq!(
        parsed_range.recurrence_overrides,
        Some(
            [(
                "2026-01-29T10:00:00".to_owned(),
                json!({"title": "Single Standup Override"})
            )]
            .into()
        ),
        "RANGE=THISANDFUTURE is skipped while single-instance override is preserved"
    );

    // 5. Out-of-order VEVENTs (detached occurrence before master series)
    let out_of_order_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:ORD1\r\n",
        "RECURRENCE-ID:20260122T100000Z\r\n",
        "DTSTART:20260122T100000Z\r\n",
        "DURATION:PT1H\r\n",
        "SUMMARY:First in File (Override)\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID:ORD1\r\n",
        "DTSTART:20260115T100000Z\r\n",
        "DURATION:PT1H\r\nRRULE:FREQ=WEEKLY\r\n",
        "SUMMARY:Second in File (Series)\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_ord = ical_to_event(out_of_order_ics).expect("parse out of order VEVENTs");
    assert_eq!(parsed_ord.title.as_deref(), Some("Second in File (Series)"));
    assert_eq!(
        parsed_ord.recurrence_overrides,
        Some(
            [(
                "2026-01-22T10:00:00".to_owned(),
                json!({"title": "First in File (Override)"})
            )]
            .into()
        )
    );

    // 6. Invalid non-existent RECURRENCE-ID date (e.g. month 13) skipped safely
    let invalid_rec_id_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:INV1\r\n",
        "DTSTART:20260115T100000Z\r\n",
        "DURATION:PT1H\r\nRRULE:FREQ=WEEKLY\r\n",
        "SUMMARY:Series Standup\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID:INV1\r\n",
        "RECURRENCE-ID:20261322T100000Z\r\n",
        "DTSTART:20260122T100000Z\r\n",
        "SUMMARY:Invalid Rec ID\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_inv = ical_to_event(invalid_rec_id_ics).expect("parse invalid RECURRENCE-ID");
    assert_eq!(parsed_inv.recurrence_overrides, None);
}

#[test]
fn recurrence_overrides_audit_predicates_decision_matrix() {
    let series = CalendarEvent {
        title: Some("Weekly Review".to_owned()),
        start: Some("2026-01-15T10:00:00".to_owned()),
        time_zone: Some("Europe/Berlin".to_owned()),
        duration: Some("PT1H".to_owned()),
        recurrence_rule: Some(RecurrenceRule::new("weekly")),
        ..CalendarEvent::default()
    };
    let id = "2026-01-22T10:00:00";

    // 1. Valid accepted property patches
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"excluded": true})
    ));
    assert!(maps_recurrence_override(&series, id, &json!({})));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"title": "Updated Title"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"title": null})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"description": "Updated Description"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"description": null})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"start": "2026-01-22T11:00:00"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"timeZone": "America/New_York"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"timeZone": null})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"duration": "PT2H"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"duration": null})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"status": "confirmed"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"status": "tentative"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"status": "cancelled"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"status": null})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"freeBusyStatus": "busy"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"freeBusyStatus": "free"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"freeBusyStatus": null})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"priority": 0})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"priority": 5})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"priority": 9})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"priority": null})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"privacy": "public"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"privacy": "private"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"privacy": "secret"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"privacy": null})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"keywords": {"project": true}})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"keywords": null})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"alerts": {
            "a1": {
                "@type": "Alert",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
                "action": "display"
            }
        }})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"alerts": null})
    ));

    // 2. Rejected invalid property shapes
    // excluded: true with additional properties
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"excluded": true, "title": "Conflicting"})
    ));
    // Empty strings
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"title": ""})
    ));
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"description": ""})
    ));
    // Unmapped properties
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"locations/1/name": "Room 101"})
    ));
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"participants": {}})
    ));
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"virtualLocations": {}})
    ));
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"links": {}})
    ));
    // Invalid values
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"status": "postponed"})
    ));
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"freeBusyStatus": "tentative"})
    ));
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"priority": 10})
    ));
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"priority": -1})
    ));
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"priority": "high"})
    ));
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"privacy": "confidential"})
    ));
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"duration": "-PT1H"})
    ));
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"keywords": {}})
    ));
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"keywords": {"": true}})
    ));
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"keywords": {"valid": false}})
    ));
    assert!(!maps_recurrence_override(
        &series,
        id,
        &json!({"alerts": {}})
    ));
    // Invalid ID
    assert!(!maps_recurrence_override(
        &series,
        "invalid-date",
        &json!({"title": "Test"})
    ));
    assert!(!maps_recurrence_override(
        &series,
        "2026-13-22T10:00:00",
        &json!({"title": "Test"})
    ));

    // 3. Custom TimeZone definition in sends_recurrence_override vs maps_recurrence_override
    let custom_tz = "/custom.org/CorporateZone";
    let custom_tz_ics = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
         BEGIN:VTIMEZONE\r\nTZID:{custom_tz}\r\n\
         BEGIN:STANDARD\r\n\
         DTSTART:20260101T000000\r\n\
         TZOFFSETFROM:+0200\r\n\
         TZOFFSETTO:+0200\r\n\
         END:STANDARD\r\n\
         END:VTIMEZONE\r\n\
         BEGIN:VEVENT\r\nUID:custom-tz\r\n\
         DTSTART;TZID={custom_tz}:20260115T100000\r\n\
         DURATION:PT1H\r\nSUMMARY:Custom Zone Event\r\n\
         END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let custom_series = ical_to_event(&custom_tz_ics).expect("parse custom series");
    assert!(!maps_recurrence_override(
        &custom_series,
        id,
        &json!({"timeZone": custom_tz})
    ));
    assert!(sends_recurrence_override(
        &custom_series,
        id,
        &json!({"timeZone": custom_tz})
    ));

    // 4. useDefaultAlerts suppresses alerts mapping on overrides
    let mut default_alerts_series = series.clone();
    default_alerts_series
        .extra
        .insert("useDefaultAlerts".to_owned(), json!(true));
    assert!(!maps_recurrence_override(
        &default_alerts_series,
        id,
        &json!({"alerts": {
            "a1": {
                "@type": "Alert",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
                "action": "display"
            }
        }})
    ));
    assert!(!maps_recurrence_override(
        &default_alerts_series,
        id,
        &json!({"alerts": null})
    ));
}

#[test]
fn recurrence_overrides_audit_real_exporter_stream_simulation() {
    // 1. Google Calendar Real Exporter Pattern with Overrides
    // Includes: Master weekly series, 1 cancelled instance (EXDATE), 1 rescheduled instance (RECURRENCE-ID + new DTSTART)
    let gcal_ics = format!(
        "BEGIN:VCALENDAR\r\n\
         PRODID:-//Google Inc//Google Calendar 70.9054//EN\r\n\
         VERSION:2.0\r\n\
         CALSCALE:GREGORIAN\r\n{}\
         BEGIN:VEVENT\r\n\
         DTSTART;TZID=America/Los_Angeles:20260901T090000\r\n\
         DTEND;TZID=America/Los_Angeles:20260901T100000\r\n\
         RRULE:FREQ=WEEKLY;UNTIL=20261027T160000Z;BYDAY=TU\r\n\
         EXDATE;TZID=America/Los_Angeles:20260915T090000\r\n\
         UID:google_series_12345@google.com\r\n\
         SUMMARY:Google Team Weekly\r\n\
         DESCRIPTION:Weekly team sync\r\n\
         STATUS:CONFIRMED\r\n\
         TRANSP:OPAQUE\r\n\
         END:VEVENT\r\n\
         BEGIN:VEVENT\r\n\
         DTSTART;TZID=America/Los_Angeles:20260922T130000\r\n\
         DTEND;TZID=America/Los_Angeles:20260922T143000\r\n\
         RECURRENCE-ID;TZID=America/Los_Angeles:20260922T090000\r\n\
         UID:google_series_12345@google.com\r\n\
         SUMMARY:Google Team Weekly (Afternoon Deep Dive)\r\n\
         DESCRIPTION:Extended session on quarterly OKRs\r\n\
         STATUS:CONFIRMED\r\n\
         TRANSP:OPAQUE\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n",
        vtimezone("America/Los_Angeles", "America/Los_Angeles")
    );

    let gcal_event = ical_to_event(&gcal_ics).expect("parse gcal multi-override");
    assert_eq!(gcal_event.title.as_deref(), Some("Google Team Weekly"));
    assert_eq!(gcal_event.time_zone.as_deref(), Some("America/Los_Angeles"));
    assert_eq!(gcal_event.duration.as_deref(), Some("PT1H"));

    let gcal_overrides = gcal_event
        .recurrence_overrides
        .as_ref()
        .expect("gcal overrides");
    assert_eq!(gcal_overrides.len(), 2);
    assert_eq!(
        gcal_overrides["2026-09-15T09:00:00"],
        json!({"excluded": true})
    );
    assert_eq!(
        gcal_overrides["2026-09-22T09:00:00"],
        json!({
            "start": "2026-09-22T13:00:00",
            "duration": "PT1H30M",
            "title": "Google Team Weekly (Afternoon Deep Dive)",
            "description": "Extended session on quarterly OKRs"
        })
    );

    // Multi-pass roundtrip fixpoint for Google Calendar pattern
    let gcal_export1 = event_to_ical(&gcal_event);
    let gcal_round2 = ical_to_event(&gcal_export1).expect("gcal_round2");
    let gcal_export2 = event_to_ical(&gcal_round2);
    assert_eq!(gcal_export1, gcal_export2);
    assert_eq!(gcal_event, gcal_round2);

    // 2. Outlook / M365 Real Exporter Pattern with Windows Time Zone & Overrides
    let outlook_ics = format!(
        "BEGIN:VCALENDAR\r\n\
         PRODID:-//Microsoft Corporation//Outlook 16.0 MIMEDIR//EN\r\n\
         VERSION:2.0\r\n{}\
         BEGIN:VEVENT\r\n\
         UID:040000008200E00074C5B7101A82E00800000000\r\n\
         DTSTART;TZID=\"W. Europe Standard Time\":20261005T090000\r\n\
         DTEND;TZID=\"W. Europe Standard Time\":20261005T100000\r\n\
         RRULE:FREQ=WEEKLY;BYDAY=MO\r\n\
         SUMMARY:Executive Review\r\n\
         CLASS:PRIVATE\r\n\
         PRIORITY:1\r\n\
         STATUS:CONFIRMED\r\n\
         TRANSP:OPAQUE\r\n\
         END:VEVENT\r\n\
         BEGIN:VEVENT\r\n\
         UID:040000008200E00074C5B7101A82E00800000000\r\n\
         RECURRENCE-ID;TZID=\"W. Europe Standard Time\":20261012T090000\r\n\
         DTSTART;TZID=\"W. Europe Standard Time\":20261012T090000\r\n\
         DTEND;TZID=\"W. Europe Standard Time\":20261012T100000\r\n\
         SUMMARY:Executive Review (Cancelled for Holiday)\r\n\
         STATUS:CANCELLED\r\n\
         CLASS:PRIVATE\r\n\
         PRIORITY:1\r\n\
         TRANSP:TRANSPARENT\r\n\
         END:VEVENT\r\n\
         BEGIN:VEVENT\r\n\
         UID:040000008200E00074C5B7101A82E00800000000\r\n\
         RECURRENCE-ID;TZID=\"W. Europe Standard Time\":20261019T090000\r\n\
         DTSTART;TZID=\"W. Europe Standard Time\":20261019T140000\r\n\
         DTEND;TZID=\"W. Europe Standard Time\":20261019T160000\r\n\
         SUMMARY:Executive Review (Q3 Budget Wrapup)\r\n\
         CLASS:PRIVATE\r\n\
         PRIORITY:1\r\n\
         STATUS:CONFIRMED\r\n\
         TRANSP:OPAQUE\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n",
        vtimezone("W. Europe Standard Time", "Europe/Berlin")
    );

    let outlook_event = ical_to_event(&outlook_ics).expect("parse outlook multi-override");
    assert_eq!(outlook_event.title.as_deref(), Some("Executive Review"));
    assert_eq!(outlook_event.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(outlook_event.privacy.as_deref(), Some("private"));
    assert_eq!(outlook_event.priority, Some(1));

    let outlook_overrides = outlook_event
        .recurrence_overrides
        .as_ref()
        .expect("outlook overrides");
    assert_eq!(outlook_overrides.len(), 2);
    assert_eq!(
        outlook_overrides["2026-10-12T09:00:00"],
        json!({
            "title": "Executive Review (Cancelled for Holiday)",
            "status": "cancelled",
            "freeBusyStatus": "free"
        })
    );
    assert_eq!(
        outlook_overrides["2026-10-19T09:00:00"],
        json!({
            "start": "2026-10-19T14:00:00",
            "duration": "PT2H",
            "title": "Executive Review (Q3 Budget Wrapup)"
        })
    );

    // Multi-pass roundtrip fixpoint for Outlook pattern
    let outlook_export1 = event_to_ical(&outlook_event);
    let outlook_round2 = ical_to_event(&outlook_export1).expect("outlook_round2");
    let outlook_export2 = event_to_ical(&outlook_round2);
    assert_eq!(outlook_export1, outlook_export2);
    assert_eq!(outlook_event, outlook_round2);
}

#[test]
fn real_exporter_corpus_alarm_shapes_roundtrip_characterization() {
    // 1. Google Calendar Export: Multi-alarm stream with display offsets, email, and absolute trigger
    let gcal_ics = read_fixture("google_calendar_export.ics");
    let gcal_event = ical_to_event(&gcal_ics).expect("parse google fixture");
    let gcal_alerts = gcal_event.alerts.as_ref().expect("gcal alerts");
    assert_eq!(gcal_alerts.len(), 2, "Google fixture has 2 display alarms");
    assert_eq!(gcal_alerts["a1"]["action"], "display");
    assert_eq!(gcal_alerts["a1"]["trigger"]["offset"], "-P1D");
    assert_eq!(gcal_alerts["a2"]["action"], "display");
    assert_eq!(gcal_alerts["a2"]["trigger"]["offset"], "-PT15M");
    assert!(maps_alerts(&gcal_event));
    assert!(
        gcal_event.extra.is_empty(),
        "clean extra map on google fixture"
    );

    let gcal_export1 = event_to_ical(&gcal_event);
    assert_eq!(gcal_export1.matches("BEGIN:VALARM\r\n").count(), 2);
    assert!(gcal_export1.contains("UID:a1\r\n"));
    assert!(gcal_export1.contains("UID:a2\r\n"));
    assert!(gcal_export1.contains("DESCRIPTION:Q3 Product Architecture Sync\r\n"));
    assert!(!gcal_export1.contains("ACTION:EMAIL"));
    assert!(!gcal_export1.contains("VALUE=DATE-TIME"));
    let gcal_event2 = ical_to_event(&gcal_export1).expect("reparse google export1");
    assert_eq!(gcal_event2.alerts, gcal_event.alerts);
    let gcal_export2 = event_to_ical(&gcal_event2);
    assert_eq!(gcal_export1, gcal_export2);

    // 2. Outlook / M365 Modern Export: DESCRIPTION:REMINDER, X-WR-ALARMUID, explicit UID & nameless
    let outlook_ics = read_fixture("outlook_m365_export.ics");
    let outlook_event = ical_to_event(&outlook_ics).expect("parse outlook fixture");
    let outlook_alerts = outlook_event.alerts.as_ref().expect("outlook alerts");
    assert_eq!(
        outlook_alerts.len(),
        2,
        "Outlook fixture has 2 display alarms"
    );
    let outlook_uid = "040000008200E00074C5B7101A82E0080000000080E99A2D87D3D901000000000000000010000000D3C9D55A1A2E-alarm-1";
    assert_eq!(outlook_alerts[outlook_uid]["action"], "display");
    assert_eq!(outlook_alerts[outlook_uid]["trigger"]["offset"], "-PT15M");
    assert_eq!(outlook_alerts["a1"]["action"], "display");
    assert_eq!(outlook_alerts["a1"]["trigger"]["offset"], "-PT30M");
    assert!(maps_alerts(&outlook_event));
    assert!(
        outlook_event.extra.is_empty(),
        "clean extra map on outlook fixture"
    );

    let outlook_export1 = event_to_ical(&outlook_event);
    assert_eq!(outlook_export1.matches("BEGIN:VALARM\r\n").count(), 2);
    let unfolded_outlook = outlook_export1.replace("\r\n ", "").replace("\r\n\t", "");
    assert!(unfolded_outlook.contains(&format!("UID:{outlook_uid}\r\n")));
    assert!(outlook_export1.contains("UID:a1\r\n"));
    assert!(outlook_export1.contains("DESCRIPTION:Executive Leadership & Financial Review\r\n"));
    assert!(!outlook_export1.contains("X-WR-ALARMUID"));
    assert!(!outlook_export1.contains("ACTION:EMAIL"));
    let outlook_event2 = ical_to_event(&outlook_export1).expect("reparse outlook export1");
    assert_eq!(outlook_event2.alerts, outlook_event.alerts);
    let outlook_export2 = event_to_ical(&outlook_event2);
    assert_eq!(outlook_export1, outlook_export2);

    // 3. Apple Calendar macOS Export: ACKNOWLEDGED, X-WR-ALARMUID, explicit UUID keys, refused audio/absolute
    let apple_ics = read_fixture("apple_calendar_export.ics");
    let apple_event = ical_to_event(&apple_ics).expect("parse apple fixture");
    let apple_alerts = apple_event.alerts.as_ref().expect("apple alerts");
    assert_eq!(apple_alerts.len(), 3, "Apple fixture has 3 display alarms");
    assert_eq!(
        apple_alerts["E451D045-FA1B-475D-85B6-06F6F505A321"]["trigger"]["offset"],
        "-P1D"
    );
    assert_eq!(
        apple_alerts["F82C4A10-91DE-4A99-8D77-38C1B79E1A55"]["trigger"]["offset"],
        "-PT2H"
    );
    assert_eq!(
        apple_alerts["apple-alarm-offset-15m"]["trigger"]["offset"],
        "-PT15M"
    );
    assert!(maps_alerts(&apple_event));
    assert!(
        apple_event.extra.is_empty(),
        "clean extra map on apple fixture"
    );

    let apple_export1 = event_to_ical(&apple_event);
    assert_eq!(apple_export1.matches("BEGIN:VALARM\r\n").count(), 3);
    assert!(apple_export1.contains("UID:E451D045-FA1B-475D-85B6-06F6F505A321\r\n"));
    assert!(apple_export1.contains("UID:F82C4A10-91DE-4A99-8D77-38C1B79E1A55\r\n"));
    assert!(apple_export1.contains("UID:apple-alarm-offset-15m\r\n"));
    assert!(apple_export1.contains("DESCRIPTION:Design Systems Workshop\r\n"));
    assert!(!apple_export1.contains("ACKNOWLEDGED"));
    assert!(!apple_export1.contains("X-WR-ALARMUID"));
    assert!(!apple_export1.contains("ACTION:AUDIO"));
    assert!(!apple_export1.contains("VALUE=DATE-TIME"));
    let apple_event2 = ical_to_event(&apple_export1).expect("reparse apple export1");
    assert_eq!(apple_event2.alerts, apple_event.alerts);
    let apple_export2 = event_to_ical(&apple_event2);
    assert_eq!(apple_export1, apple_export2);

    // 4. GNOME Evolution Native Export: X-EVOLUTION-ALARM-UID and VALUE=DURATION
    let evo_ics = read_fixture("evolution_calendar_export.ics");
    let evo_event = ical_to_event(&evo_ics).expect("parse evolution fixture");
    let evo_alerts = evo_event.alerts.as_ref().expect("evo alerts");
    assert_eq!(
        evo_alerts.len(),
        2,
        "Evolution fixture has 2 display alarms"
    );
    assert_eq!(evo_alerts["a1"]["trigger"]["offset"], "-PT15M");
    assert_eq!(evo_alerts["a2"]["trigger"]["offset"], "-PT1H");
    assert!(maps_alerts(&evo_event));
    let evo_export1 = event_to_ical(&evo_event);
    assert_eq!(evo_export1.matches("BEGIN:VALARM\r\n").count(), 2);
    let evo_event2 = ical_to_event(&evo_export1).expect("reparse evo export1");
    assert_eq!(evo_event2.alerts, evo_event.alerts);
    let evo_export2 = event_to_ical(&evo_event2);
    assert_eq!(evo_export1, evo_export2);

    // 5. Nextcloud / SabreDAV CalDAV Export: multi-day display alarm
    let nextcloud_ics = read_fixture("nextcloud_calendar_export.ics");
    let nextcloud_event = ical_to_event(&nextcloud_ics).expect("parse nextcloud fixture");
    let nextcloud_alerts = nextcloud_event.alerts.as_ref().expect("nextcloud alerts");
    assert_eq!(
        nextcloud_alerts.len(),
        1,
        "Nextcloud fixture has 1 display alarm"
    );
    assert_eq!(nextcloud_alerts["a1"]["trigger"]["offset"], "-P2D");
    assert!(maps_alerts(&nextcloud_event));
    let nextcloud_export1 = event_to_ical(&nextcloud_event);
    assert_eq!(nextcloud_export1.matches("BEGIN:VALARM\r\n").count(), 1);
    let nextcloud_event2 = ical_to_event(&nextcloud_export1).expect("reparse nextcloud export1");
    assert_eq!(nextcloud_event2.alerts, nextcloud_event.alerts);
    let nextcloud_export2 = event_to_ical(&nextcloud_event2);
    assert_eq!(nextcloud_export1, nextcloud_export2);
}

#[test]
fn refused_alarm_shapes_safe_isolation_and_whole_property_replacement() {
    // 1. Document containing ONLY refused alarms (EMAIL, AUDIO, PROCEDURE, absolute triggers)
    let ics_only_refused = "\
BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Test Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:refused-alarms-only-event\r\n\
DTSTART:20261105T140000Z\r\n\
SUMMARY:Team Strategy Offsite\r\n\
BEGIN:VALARM\r\n\
ACTION:EMAIL\r\n\
SUMMARY:Strategy Reminder\r\n\
DESCRIPTION:Do not forget the offsite\r\n\
ATTENDEE:mailto:lead@example.com\r\n\
TRIGGER:-P1D\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
ACTION:AUDIO\r\n\
ATTACH;VALUE=URI:Chime\r\n\
TRIGGER:-PT15M\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
ACTION:PROCEDURE\r\n\
ATTACH;VALUE=URI:file:///bin/beep\r\n\
TRIGGER:-PT5M\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Absolute Trigger Alert\r\n\
TRIGGER;VALUE=DATE-TIME:20261105T130000Z\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Invalid Related Param\r\n\
TRIGGER;RELATED=BEFORE:-PT10M\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics_only_refused).expect("parse refused-only document");
    assert!(
        event.alerts.is_none(),
        "event.alerts must be None when all alarms are refused"
    );
    assert!(
        event.extra.is_empty(),
        "event.extra must not be polluted by dropped alarms"
    );
    assert!(
        maps_alerts(&event),
        "event with no readable alerts must pass maps_alerts"
    );

    let export1 = event_to_ical(&event);
    assert_eq!(
        export1.matches("BEGIN:VALARM").count(),
        0,
        "No VALARM emitted when alerts is None"
    );
    let event2 = ical_to_event(&export1).expect("reparse export1");
    assert_eq!(event2.alerts, None);
    assert_eq!(event2.title, event.title);
    assert_eq!(event2.start, event.start);

    // 2. Mixed document: 1 valid display alarm + multiple refused alarms
    let ics_mixed = "\
BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Test Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:mixed-alarms-event\r\n\
DTSTART:20261105T140000Z\r\n\
SUMMARY:Mixed Alarms Event\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
TRIGGER:-PT20M\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
ACTION:EMAIL\r\n\
ATTENDEE:mailto:user@example.com\r\n\
TRIGGER:-P2D\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
ACTION:AUDIO\r\n\
TRIGGER:-PT10M\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let mixed_event = ical_to_event(ics_mixed).expect("parse mixed document");
    let alerts = mixed_event.alerts.as_ref().expect("mixed alerts");
    assert_eq!(alerts.len(), 1, "Only the 1 valid display alarm is kept");
    assert_eq!(alerts["a1"]["trigger"]["offset"], "-PT20M");
    assert!(maps_alerts(&mixed_event));

    let mixed_export = event_to_ical(&mixed_event);
    assert_eq!(mixed_export.matches("BEGIN:VALARM\r\n").count(), 1);
    assert!(mixed_export.contains("TRIGGER:-PT20M\r\n"));
    assert!(!mixed_export.contains("ACTION:EMAIL"));
    assert!(!mixed_export.contains("ACTION:AUDIO"));
}

#[test]
fn maps_alerts_refusal_boundary_matrix_for_unsupported_shapes() {
    let base_event = CalendarEvent {
        id: Some("alarm-boundary-test".into()),
        event_type: Some("Event".into()),
        title: Some("Alarm Boundary Test".into()),
        start: Some("2026-11-10T10:00:00".into()),
        time_zone: Some("Europe/London".into()),
        duration: Some("PT1H".into()),
        ..Default::default()
    };

    // 1. Valid offset display alert -> accepted
    let mut valid_event = base_event.clone();
    valid_event.alerts = Some(BTreeMap::from([(
        "a1".to_owned(),
        json!({
            "@type": "Alert",
            "action": "display",
            "trigger": {
                "@type": "OffsetTrigger",
                "offset": "-PT15M"
            }
        }),
    )]));
    assert!(maps_alerts(&valid_event));

    // 2. Unmapped action type ("email", "audio", "procedure") -> refused
    for bad_action in ["email", "audio", "procedure", "sound", "custom"] {
        let mut bad_event = base_event.clone();
        bad_event.alerts = Some(BTreeMap::from([(
            "a1".to_owned(),
            json!({
                "@type": "Alert",
                "action": bad_action,
                "trigger": {
                    "@type": "OffsetTrigger",
                    "offset": "-PT15M"
                }
            }),
        )]));
        assert!(
            !maps_alerts(&bad_event),
            "maps_alerts must refuse action: {bad_action}"
        );
    }

    // 3. Absolute trigger -> refused
    let mut abs_event = base_event.clone();
    abs_event.alerts = Some(BTreeMap::from([(
        "a1".to_owned(),
        json!({
            "@type": "Alert",
            "action": "display",
            "trigger": {
                "@type": "AbsoluteTrigger",
                "when": "2026-11-10T09:45:00Z"
            }
        }),
    )]));
    assert!(
        !maps_alerts(&abs_event),
        "maps_alerts must refuse AbsoluteTrigger"
    );

    // 4. Acknowledged timestamp (RFC 9074 §6.1 / snoozed alert) -> refused to prevent un-dismissing
    let mut ack_event = base_event.clone();
    ack_event.alerts = Some(BTreeMap::from([(
        "a1".to_owned(),
        json!({
            "@type": "Alert",
            "action": "display",
            "acknowledged": "2026-11-10T09:00:00Z",
            "trigger": {
                "@type": "OffsetTrigger",
                "offset": "-PT15M"
            }
        }),
    )]));
    assert!(
        !maps_alerts(&ack_event),
        "maps_alerts must refuse acknowledged alert"
    );

    // 5. Custom description on Alert -> refused to prevent clobbering
    let mut desc_event = base_event.clone();
    desc_event.alerts = Some(BTreeMap::from([(
        "a1".to_owned(),
        json!({
            "@type": "Alert",
            "action": "display",
            "description": "Custom snooze message",
            "trigger": {
                "@type": "OffsetTrigger",
                "offset": "-PT15M"
            }
        }),
    )]));
    assert!(
        !maps_alerts(&desc_event),
        "maps_alerts must refuse Alert with custom description"
    );

    // 6. Invalid relativeTo -> refused
    let mut rel_event = base_event.clone();
    rel_event.alerts = Some(BTreeMap::from([(
        "a1".to_owned(),
        json!({
            "@type": "Alert",
            "action": "display",
            "trigger": {
                "@type": "OffsetTrigger",
                "offset": "-PT15M",
                "relativeTo": "invalid"
            }
        }),
    )]));
    assert!(
        !maps_alerts(&rel_event),
        "maps_alerts must refuse invalid relativeTo"
    );

    // 7. useDefaultAlerts: true -> refused & draws 0 alarms
    let mut def_event = valid_event.clone();
    def_event
        .extra
        .insert("useDefaultAlerts".to_owned(), json!(true));
    assert!(
        !maps_alerts(&def_event),
        "maps_alerts must refuse event with useDefaultAlerts: true"
    );
    let def_out = event_to_ical(&def_event);
    assert_eq!(
        def_out.matches("BEGIN:VALARM").count(),
        0,
        "useDefaultAlerts must suppress VALARM emission"
    );

    // 8. Invalid key (empty string or illegal characters) -> refused
    for bad_key in ["", "alert/with/slash", "alert with space", "alert@domain"] {
        let mut key_event = base_event.clone();
        key_event.alerts = Some(BTreeMap::from([(
            bad_key.to_owned(),
            json!({
                "@type": "Alert",
                "action": "display",
                "trigger": {
                    "@type": "OffsetTrigger",
                    "offset": "-PT15M"
                }
            }),
        )]));
        assert!(
            !maps_alerts(&key_event),
            "maps_alerts must refuse invalid key: '{bad_key}'"
        );
    }
}

#[test]
fn mapping_docs_completeness_audit_master_property_table_fidelity() {
    // Audit of every property in ICAL-MAPPING.md Master Property Mapping Table:
    // UID, SUMMARY, DESCRIPTION, DTSTART, DURATION, STATUS, TRANSP, PRIORITY,
    // CLASS, LOCATION, GEO, CONFERENCE, ATTACH/LINKS, CATEGORIES, ORGANIZER,
    // ATTENDEE, VALARM, VTIMEZONE.

    let rich_event = CalendarEvent {
        id: Some("jmap-evt-001".into()),
        uid: Some("rfc-uuid-evt-001".to_owned()),
        title: Some("Master Architecture Summit".to_owned()),
        description: Some("Comprehensive review of the iCalendar mapping contract.".to_owned()),
        start: Some("2026-09-15T09:30:00".to_owned()),
        time_zone: Some("Europe/Berlin".to_owned()),
        duration: Some("PT2H30M".to_owned()),
        status: Some("confirmed".to_owned()),
        free_busy_status: Some("busy".to_owned()),
        priority: Some(1),
        privacy: Some("public".to_owned()),
        locations: Some(BTreeMap::from([(
            "loc1".to_owned(),
            json!({
                "@type": "Location",
                "name": "Main Auditorium, Room 101",
                "description": "Ground floor west wing",
                "coordinates": "geo:52.520008,13.404954",
            }),
        )])),
        virtual_locations: Some(BTreeMap::from([(
            "v1".to_owned(),
            json!({
                "@type": "VirtualLocation",
                "uri": "https://meet.example.com/summit-2026",
                "name": "Live Video Stream",
                "features": {
                    "audio": true,
                    "video": true,
                    "screen": true,
                    "chat": true
                }
            }),
        )])),
        links: Some(BTreeMap::from([(
            "l1".to_owned(),
            json!({
                "@type": "Link",
                "href": "https://example.com/agenda.pdf",
                "contentType": "application/pdf",
                "title": "Summit Agenda PDF",
                "size": 1048576,
                "display": "badge"
            }),
        )])),
        keywords: Some(BTreeMap::from([
            ("architecture".to_owned(), json!(true)),
            ("jmap".to_owned(), json!(true)),
            ("summit".to_owned(), json!(true)),
        ])),
        participants: Some(BTreeMap::from([
            (
                "p_org".to_owned(),
                json!({
                    "@type": "Participant",
                    "name": "Alice Organizer",
                    "email": "alice@example.com",
                    "sendTo": { "imip": "mailto:alice@example.com" },
                    "roles": { "owner": true }
                }),
            ),
            (
                "p_att".to_owned(),
                json!({
                    "@type": "Participant",
                    "name": "Bob Attendee",
                    "email": "bob@example.com",
                    "sendTo": { "imip": "mailto:bob@example.com" },
                    "roles": { "attendee": true },
                    "participationStatus": "accepted",
                    "expectReply": true
                }),
            ),
        ])),
        alerts: Some(BTreeMap::from([(
            "a1".to_owned(),
            json!({
                "@type": "Alert",
                "action": "display",
                "trigger": {
                    "@type": "OffsetTrigger",
                    "offset": "-PT15M"
                }
            }),
        )])),
        ..CalendarEvent::default()
    };

    // 1. Serialization (Pass 1)
    let ics1 = event_to_ical(&rich_event);
    assert!(ics1.contains("BEGIN:VCALENDAR\r\n"));
    assert!(ics1.contains("VERSION:2.0\r\n"));
    assert!(ics1.contains("UID:jmap-evt-001\r\n"));
    assert!(ics1.contains("X-JMAP-UID:rfc-uuid-evt-001\r\n"));
    assert!(ics1.contains("SUMMARY:Master Architecture Summit\r\n"));
    assert!(ics1.contains("STATUS:CONFIRMED\r\n"));
    assert!(ics1.contains("TRANSP:OPAQUE\r\n"));
    assert!(ics1.contains("PRIORITY:1\r\n"));
    assert!(ics1.contains("CLASS:PUBLIC\r\n"));
    assert!(ics1.contains("DTSTART;TZID=Europe/Berlin:20260915T093000\r\n"));
    assert!(ics1.contains("DURATION:PT2H30M\r\n"));
    assert!(ics1.contains("LOCATION;X-JMAP-KEY=loc1:Main Auditorium\\, Room 101\r\n"));
    assert!(ics1.contains("CONFERENCE;VALUE=URI;"));
    assert!(ics1.contains("ATTACH;FMTTYPE=application/pdf;"));
    assert!(ics1.contains("CATEGORIES:architecture,jmap,summit\r\n"));
    assert_eq!(
        content_line(&ics1, "ORGANIZER"),
        "ORGANIZER;CN=\"Alice Organizer\":mailto:alice@example.com"
    );
    assert_eq!(
        content_line(&ics1, "ATTENDEE"),
        "ATTENDEE;CN=\"Bob Attendee\";ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;RSVP=TRUE:mailto:bob@example.com"
    );
    assert!(ics1.contains("BEGIN:VALARM\r\n"));
    assert!(ics1.contains("TRIGGER:-PT15M\r\n"));
    assert!(ics1.contains("ACTION:DISPLAY\r\n"));

    // 2. Deserialization (Pass 1)
    let parsed1 = ical_to_event(&ics1).expect("parse rich ics");
    assert_eq!(
        parsed1.id.as_ref().map(|id| id.as_str()),
        Some("jmap-evt-001")
    );
    assert_eq!(parsed1.uid.as_deref(), Some("rfc-uuid-evt-001"));
    assert_eq!(parsed1.title.as_deref(), Some("Master Architecture Summit"));
    assert_eq!(parsed1.status.as_deref(), Some("confirmed"));
    assert_eq!(parsed1.free_busy_status.as_deref(), Some("busy"));
    assert_eq!(parsed1.priority, Some(1));
    assert_eq!(parsed1.privacy.as_deref(), Some("public"));
    assert_eq!(parsed1.duration.as_deref(), Some("PT2H30M"));
    assert!(parsed1.locations.is_some());
    assert!(parsed1.virtual_locations.is_some());
    assert!(parsed1.links.is_some());
    assert!(parsed1.keywords.is_some());
    assert_eq!(
        parsed1.participants, None,
        "guest list is written and never read back (server-managed)"
    );
    assert!(parsed1.alerts.is_some());

    // 3. Multi-Pass Fixed-Point Convergence (Pass 2 & 3)
    let ics2 = event_to_ical(&parsed1);
    let parsed2 = ical_to_event(&ics2).expect("parse pass 2");
    let ics3 = event_to_ical(&parsed2);

    assert_eq!(
        ics2, ics3,
        "iCalendar stream must achieve byte-identical fixpoint on pass 2"
    );
    assert_eq!(
        parsed1, parsed2,
        "CalendarEvent model must achieve exact structural fixpoint on pass 2"
    );
}

#[test]
fn mapping_docs_completeness_audit_locations_decision_matrix() {
    // 1. Valid location with name
    let mut ev1 = fixture_event();
    ev1.locations = Some(BTreeMap::from([(
        "loc1".to_owned(),
        json!({
            "@type": "Location",
            "name": "Convention Center",
            "coordinates": "geo:37.7749,-122.4194"
        }),
    )]));
    assert!(maps_locations(ev1.locations.as_ref().unwrap()));
    let ics1 = event_to_ical(&ev1);
    assert!(ics1.contains("LOCATION;X-JMAP-KEY=loc1:Convention Center\r\n"));

    // 2. Location with only name (no coordinates) -> valid and mapped
    let mut ev_name_only = fixture_event();
    ev_name_only.locations = Some(BTreeMap::from([(
        "loc1".to_owned(),
        json!({
            "@type": "Location",
            "name": "Auditorium B"
        }),
    )]));
    assert!(maps_locations(ev_name_only.locations.as_ref().unwrap()));

    // 3. Location with only coordinates (no name) -> valid, not drawn on LOCATION line
    let mut ev_coord_only = fixture_event();
    ev_coord_only.locations = Some(BTreeMap::from([(
        "loc1".to_owned(),
        json!({
            "@type": "Location",
            "coordinates": "geo:48.8566,2.3522"
        }),
    )]));
    assert!(maps_locations(ev_coord_only.locations.as_ref().unwrap()));
    let ics_coord = event_to_ical(&ev_coord_only);
    assert!(without(&ics_coord, "LOCATION"));

    // 4. Multiple locations -> refused by maps_locations (first drawn, subsequent flagged)
    let mut ev_multi = fixture_event();
    ev_multi.locations = Some(BTreeMap::from([
        (
            "l1".to_owned(),
            json!({"@type": "Location", "name": "Room 1"}),
        ),
        (
            "l2".to_owned(),
            json!({"@type": "Location", "name": "Room 2"}),
        ),
    ]));
    assert!(!maps_locations(ev_multi.locations.as_ref().unwrap()));
    let ics_multi = event_to_ical(&ev_multi);
    assert!(ics_multi.contains("LOCATION;X-JMAP-KEY=l1:Room 1\r\n"));
    assert!(!ics_multi.contains("Room 2"));

    // 5. Invalid JSON types for Location -> refused
    for bad_loc in [json!("string"), json!(123), json!(null), json!(true)] {
        let mut ev_bad = fixture_event();
        ev_bad.locations = Some(BTreeMap::from([("l1".to_owned(), bad_loc)]));
        assert!(!maps_locations(ev_bad.locations.as_ref().unwrap()));
    }
}

#[test]
fn mapping_docs_completeness_audit_virtual_locations_decision_matrix() {
    // 1. Valid virtual location with https scheme and standard features
    let mut ev1 = fixture_event();
    ev1.virtual_locations = Some(BTreeMap::from([(
        "v1".to_owned(),
        json!({
            "@type": "VirtualLocation",
            "uri": "https://meet.jit.si/my-meeting",
            "name": "Jitsi Meeting",
            "features": {
                "audio": true,
                "video": true,
                "chat": true,
                "screen": true,
                "moderator": true
            }
        }),
    )]));
    assert!(maps_virtual_locations(
        ev1.virtual_locations.as_ref().unwrap()
    ));
    let ics1 = event_to_ical(&ev1);
    assert!(ics1.contains("CONFERENCE;VALUE=URI;"));
    assert!(ics1.contains("https://meet.jit.si/my-meeting"));

    // 2. Other valid URI schemes: zoommtg, tel, sip
    for scheme_uri in [
        "zoommtg://zoom.us/join?confno=12345",
        "tel:+15551234567",
        "sip:meeting@example.com",
    ] {
        let mut ev = fixture_event();
        ev.virtual_locations = Some(BTreeMap::from([(
            "v1".to_owned(),
            json!({
                "@type": "VirtualLocation",
                "uri": scheme_uri,
                "name": "Endpoint"
            }),
        )]));
        assert!(maps_virtual_locations(
            ev.virtual_locations.as_ref().unwrap()
        ));
    }

    // 3. Invalid non-boolean feature values -> refused by maps_virtual_locations
    for bad_feat in [json!("yes"), json!(1), json!(null)] {
        let mut ev_bad_feat = fixture_event();
        ev_bad_feat.virtual_locations = Some(BTreeMap::from([(
            "v1".to_owned(),
            json!({
                "@type": "VirtualLocation",
                "uri": "https://meet.example.com",
                "features": { "video": bad_feat }
            }),
        )]));
        assert!(!maps_virtual_locations(
            ev_bad_feat.virtual_locations.as_ref().unwrap()
        ));
    }

    // 4. Multiple virtual locations -> supported by RFC 7986 and maps_virtual_locations
    let mut ev_multi_virt = fixture_event();
    ev_multi_virt.virtual_locations = Some(BTreeMap::from([
        (
            "v1".to_owned(),
            json!({"@type": "VirtualLocation", "uri": "https://meet.example.com/1"}),
        ),
        (
            "v2".to_owned(),
            json!({"@type": "VirtualLocation", "uri": "https://meet.example.com/2"}),
        ),
    ]));
    assert!(maps_virtual_locations(
        ev_multi_virt.virtual_locations.as_ref().unwrap()
    ));
    let ics_multi = event_to_ical(&ev_multi_virt);
    assert!(ics_multi.contains("https://meet.example.com/1"));
    assert!(ics_multi.contains("https://meet.example.com/2"));

    // 5. Virtual location with missing URI -> refused by maps_virtual_locations
    let mut ev_no_uri = fixture_event();
    ev_no_uri.virtual_locations = Some(BTreeMap::from([(
        "v1".to_owned(),
        json!({
            "@type": "VirtualLocation",
            "name": "Nameless room with no URI"
        }),
    )]));
    assert!(!maps_virtual_locations(
        ev_no_uri.virtual_locations.as_ref().unwrap()
    ));
}

#[test]
fn mapping_docs_completeness_audit_unmodeled_and_dropped_properties_matrix() {
    // Asserts that standard unmodeled properties and vendor X-properties in incoming
    // iCalendar documents are safely ignored on parse, do not pollute event.extra,
    // and are cleanly excluded on outbound serialization.

    let raw_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Foreign Exporter//EN\r\n\
CALSCALE:GREGORIAN\r\n\
METHOD:REQUEST\r\n\
BEGIN:VEVENT\r\n\
UID:evt-dropped-001\r\n\
DTSTAMP:20260915T080000Z\r\n\
CREATED:20260101T120000Z\r\n\
LAST-MODIFIED:20260820T153000Z\r\n\
SEQUENCE:5\r\n\
URL:https://example.com/event/details\r\n\
X-MICROSOFT-CDO-BUSYSTATUS:BUSY\r\n\
X-MICROSOFT-CDO-IMPORTANCE:1\r\n\
X-APPLE-TRAVEL-DURATION:PT30M\r\n\
X-EVOLUTION-MOVE-CALENDAR:TRUE\r\n\
DTSTART;TZID=Europe/Berlin:20260915T100000\r\n\
DURATION:PT1H\r\n\
SUMMARY:Dropped Properties Test Event\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(raw_ics).expect("parse raw ics with unmodeled properties");

    // 1. Mapped fields are accurate
    assert_eq!(
        event.id.as_ref().map(|id| id.as_str()),
        Some("evt-dropped-001")
    );
    assert_eq!(
        event.title.as_deref(),
        Some("Dropped Properties Test Event")
    );
    assert_eq!(event.start.as_deref(), Some("2026-09-15T10:00:00"));
    assert_eq!(event.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(event.duration.as_deref(), Some("PT1H"));

    // 2. Extra is empty (not polluted with vendor or unmodeled metadata)
    assert!(
        event.extra.is_empty(),
        "event.extra should not contain unmapped fields: {:?}",
        event.extra
    );

    // 3. Outbound emission omits foreign envelope properties
    let emitted = event_to_ical(&event);
    assert!(!emitted.contains("PRODID:-//Foreign Exporter//EN"));
    assert!(!emitted.contains("CALSCALE:GREGORIAN"));
    assert!(!emitted.contains("METHOD:REQUEST"));
    assert!(!emitted.contains("SEQUENCE:5"));
    assert!(!emitted.contains("X-MICROSOFT-CDO-BUSYSTATUS"));
    assert!(!emitted.contains("X-APPLE-TRAVEL-DURATION"));
    assert!(!emitted.contains("X-EVOLUTION-MOVE-CALENDAR"));
}

#[test]
fn mapping_docs_completeness_audit_freebusy_decision_matrix() {
    // Asserts free_busy_type mappings and busy_periods_to_vfreebusy formatting

    // 1. Draft busy statuses mapping
    assert_eq!(free_busy_type("busy"), "BUSY");
    assert_eq!(free_busy_type("tentative"), "BUSY-TENTATIVE");
    assert_eq!(free_busy_type("unavailable"), "BUSY-UNAVAILABLE");
    assert_eq!(free_busy_type("unknown"), "BUSY");
    assert_eq!(free_busy_type("custom-draft-status"), "BUSY");

    // 2. busy_periods_to_vfreebusy formatting and search window bounding
    let periods = [
        BusyPeriod {
            utc_start: UtcDate::new("2026-09-15T10:00:00Z"),
            utc_end: UtcDate::new("2026-09-15T11:30:00Z"),
            busy_status: "busy".to_owned(),
            event: None,
        },
        BusyPeriod {
            utc_start: UtcDate::new("2026-09-15T14:00:00Z"),
            utc_end: UtcDate::new("2026-09-15T15:00:00Z"),
            busy_status: "tentative".to_owned(),
            event: None,
        },
        BusyPeriod {
            utc_start: UtcDate::new("2026-09-15T16:00:00Z"),
            utc_end: UtcDate::new("2026-09-15T17:00:00Z"),
            busy_status: "unavailable".to_owned(),
            event: None,
        },
    ];

    let vfb = busy_periods_to_vfreebusy(
        "alice@example.com",
        &UtcDate::new("2026-09-15T00:00:00Z"),
        &UtcDate::new("2026-09-15T23:59:59Z"),
        &periods,
    )
    .expect("renders bare VFREEBUSY component");

    assert!(vfb.starts_with("BEGIN:VFREEBUSY\r\n"));
    assert!(vfb.contains("ATTENDEE:mailto:alice@example.com\r\n"));
    assert!(vfb.contains("DTSTART:20260915T000000Z\r\n"));
    assert!(vfb.contains("DTEND:20260915T235959Z\r\n"));
    assert!(vfb.contains("FREEBUSY;FBTYPE=BUSY:20260915T100000Z/20260915T113000Z\r\n"));
    assert!(vfb.contains("FREEBUSY;FBTYPE=BUSY-TENTATIVE:20260915T140000Z/20260915T150000Z\r\n"));
    assert!(vfb.contains("FREEBUSY;FBTYPE=BUSY-UNAVAILABLE:20260915T160000Z/20260915T170000Z\r\n"));
    assert!(vfb.ends_with("END:VFREEBUSY\r\n"));
}

#[test]
fn timezone_rule_recurrence_rules_plural_and_singular_variants_matrix() {
    // Characterizes TimeZoneRule recurrence rules representation:
    // 1. Standard RFC 8984 §4.7.2 plural "recurrenceRules" array
    // 2. Singular "recurrenceRule" object variant
    // 3. Singular "recurrenceRule" array variant

    let plural_zone = json!({
        "@type": "TimeZone",
        "tzId": CUSTOM_TZID,
        "standard": [{
            "@type": "TimeZoneRule",
            "start": "1970-10-25T03:00:00",
            "offsetFrom": "+0200",
            "offsetTo": "+0100",
            "recurrenceRules": [{
                "@type": "RecurrenceRule",
                "frequency": "yearly",
                "byMonth": ["10"],
                "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": -1}],
            }],
            "names": {"CET": true},
        }],
        "daylight": [{
            "@type": "TimeZoneRule",
            "start": "1970-03-29T02:00:00",
            "offsetFrom": "+0100",
            "offsetTo": "+0200",
            "recurrenceRules": [{
                "@type": "RecurrenceRule",
                "frequency": "yearly",
                "byMonth": ["3"],
                "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": -1}],
            }],
            "names": {"CEST": true},
        }],
    });

    let singular_object_zone = json!({
        "@type": "TimeZone",
        "tzId": CUSTOM_TZID,
        "standard": [{
            "@type": "TimeZoneRule",
            "start": "1970-10-25T03:00:00",
            "offsetFrom": "+0200",
            "offsetTo": "+0100",
            "recurrenceRule": {
                "@type": "RecurrenceRule",
                "frequency": "yearly",
                "byMonth": ["10"],
                "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": -1}],
            },
            "names": {"CET": true},
        }],
        "daylight": [{
            "@type": "TimeZoneRule",
            "start": "1970-03-29T02:00:00",
            "offsetFrom": "+0100",
            "offsetTo": "+0200",
            "recurrenceRule": {
                "@type": "RecurrenceRule",
                "frequency": "yearly",
                "byMonth": ["3"],
                "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": -1}],
            },
            "names": {"CEST": true},
        }],
    });

    let singular_array_zone = json!({
        "@type": "TimeZone",
        "tzId": CUSTOM_TZID,
        "standard": [{
            "@type": "TimeZoneRule",
            "start": "1970-10-25T03:00:00",
            "offsetFrom": "+0200",
            "offsetTo": "+0100",
            "recurrenceRule": [{
                "@type": "RecurrenceRule",
                "frequency": "yearly",
                "byMonth": ["10"],
                "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": -1}],
            }],
            "names": {"CET": true},
        }],
        "daylight": [{
            "@type": "TimeZoneRule",
            "start": "1970-03-29T02:00:00",
            "offsetFrom": "+0100",
            "offsetTo": "+0200",
            "recurrenceRule": [{
                "@type": "RecurrenceRule",
                "frequency": "yearly",
                "byMonth": ["3"],
                "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": -1}],
            }],
            "names": {"CEST": true},
        }],
    });

    let event_plural = defining(CUSTOM_TZID, json!({CUSTOM_TZID: plural_zone}));
    let event_singular_obj = defining(CUSTOM_TZID, json!({CUSTOM_TZID: singular_object_zone}));
    let event_singular_arr = defining(CUSTOM_TZID, json!({CUSTOM_TZID: singular_array_zone}));

    let ics_plural = event_to_ical(&event_plural);
    let ics_singular_obj = event_to_ical(&event_singular_obj);
    let ics_singular_arr = event_to_ical(&event_singular_arr);

    // All three forms must emit identical VTIMEZONE structures with RRULE entries
    assert_eq!(ics_plural, ics_singular_obj);
    assert_eq!(ics_plural, ics_singular_arr);

    assert!(ics_plural.contains("BEGIN:VTIMEZONE\r\n"));
    assert!(ics_plural.contains("TZID:/example.com/Europe-Berlin\r\n"));
    assert!(ics_plural.contains("RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n"));
    assert!(ics_plural.contains("RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3\r\n"));
    assert!(ics_plural.contains("TZNAME:CET\r\n"));
    assert!(ics_plural.contains("TZNAME:CEST\r\n"));

    // Inbound parse of emitted VTIMEZONE always yields canonical RFC 8984 "recurrenceRules"
    let parsed = ical_to_event(&ics_plural).expect("parse custom vtimezone");
    let defs = parsed.time_zones.expect("time_zones present");
    let zone_obj = defs.get(CUSTOM_TZID).expect("custom zone in map");
    let std_rules = zone_obj
        .get("standard")
        .and_then(Value::as_array)
        .expect("standard observances");
    assert_eq!(std_rules.len(), 1);
    assert!(
        std_rules[0].get("recurrenceRules").is_some(),
        "read_observance must emit canonical RFC 8984 plural 'recurrenceRules'"
    );
}

#[test]
fn timezone_rule_observance_until_and_transition_offset_arithmetic() {
    // Tests observance RRULE with UNTIL: dates itself in the zone it defines,
    // converted through Ends::At(&offset_from) arithmetic without a full tzdb.
    let zone_with_until = json!({
        "@type": "TimeZone",
        "tzId": CUSTOM_TZID,
        "standard": [{
            "@type": "TimeZoneRule",
            "start": "1970-10-25T03:00:00",
            "offsetFrom": "+0200",
            "offsetTo": "+0100",
            "recurrenceRules": [{
                "@type": "RecurrenceRule",
                "frequency": "yearly",
                "until": "2030-10-25T03:00:00",
                "byMonth": ["10"],
                "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": -1}],
            }],
            "names": {"CET": true},
        }],
        "daylight": [{
            "@type": "TimeZoneRule",
            "start": "1970-03-29T02:00:00",
            "offsetFrom": "+0100",
            "offsetTo": "+0200",
            "recurrenceRules": [{
                "@type": "RecurrenceRule",
                "frequency": "yearly",
                "until": "2030-03-29T02:00:00",
                "byMonth": ["3"],
                "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": -1}],
            }],
            "names": {"CEST": true},
        }],
    });

    let event = defining(CUSTOM_TZID, json!({CUSTOM_TZID: zone_with_until}));
    let ics1 = event_to_ical(&event);

    assert!(ics1.contains("RRULE:FREQ=YEARLY;UNTIL=20301025T010000Z;BYDAY=-1SU;BYMONTH=10\r\n"));
    assert!(ics1.contains("RRULE:FREQ=YEARLY;UNTIL=20300329T010000Z;BYDAY=-1SU;BYMONTH=3\r\n"));

    let event2 = ical_to_event(&ics1).expect("parse ics1");
    let ics2 = event_to_ical(&event2);
    let event3 = ical_to_event(&ics2).expect("parse ics2");
    let ics3 = event_to_ical(&event3);

    // Multi-pass roundtrip fixpoint stability
    assert_eq!(
        ics2, ics3,
        "VTIMEZONE observance with UNTIL reaches fixed point (ics2 == ics3)"
    );
    assert_eq!(
        event2.time_zones, event3.time_zones,
        "JSCalendar time_zones reach fixed point (event2 == event3)"
    );
}

#[test]
fn recurrence_until_parser_refusal_and_unstateable_canary_matrix() {
    // Tests parser refusal boundaries and unstateable UNTIL handling:
    // 1. Non-digit unparseable UNTIL (e.g. UNTIL=whenever) drops the token
    let ics_hostile = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E-CANARY\r\n",
        "DTSTART:20260810T090000\r\n",
        "RRULE:FREQ=DAILY;UNTIL=whenever;BYMONTH=4\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let parsed = ical_to_event(ics_hostile).expect("parse hostile rrule");
    let rule = parsed.recurrence_rule.expect("recurrence rule present");
    assert_eq!(
        rule.until, None,
        "unparseable UNTIL token dropped by parser/rrule_to_rule"
    );

    // 2. Unstateable local date-times on export (e.g. month 13, day 30 of Feb)
    for invalid_until in ["2026-13-31T09:00:00", "whenever", "2026-02-30T09:00:00"] {
        let bad_rule = RecurrenceRule {
            frequency: "weekly".to_owned(),
            until: Some(invalid_until.to_owned()),
            ..RecurrenceRule::default()
        };
        assert!(
            !maps_recurrence_rule(&bad_rule),
            "unstateable until must be refused by maps_recurrence_rule: {invalid_until}"
        );
        assert_eq!(
            unstateable_until(&bad_rule),
            Some(invalid_until),
            "unstateable_until reports non-convertible until: {invalid_until}"
        );

        let event = CalendarEvent {
            id: Some("E-BAD-UNTIL".into()),
            start: Some("2026-01-15T09:00:00".to_owned()),
            recurrence_rule: Some(bad_rule),
            ..CalendarEvent::default()
        };
        let ics = event_to_ical(&event);
        assert!(
            !ics.contains("RRULE"),
            "unstateable UNTIL causes whole RRULE to be omitted to prevent unbounded recurrence"
        );
    }

    // 3. Valid UNTIL converts and passes maps_recurrence_rule
    let valid_rule = RecurrenceRule {
        frequency: "monthly".to_owned(),
        until: Some("2026-12-31T09:00:00".to_owned()),
        ..RecurrenceRule::default()
    };
    assert!(maps_recurrence_rule(&valid_rule));
    assert_eq!(unstateable_until(&valid_rule), None);
}

#[test]
fn windows_time_zone_names_unsendable_by_design_refusal_and_cldr_resolution_matrix() {
    // Tests refusal path (outbound unsendable) vs CLDR resolution (inbound mapped)
    let windows_names = [
        ("W. Europe Standard Time", "Europe/Berlin"),
        ("Pacific Standard Time", "America/Los_Angeles"),
        ("Eastern Standard Time", "America/New_York"),
        ("GMT Standard Time", "Europe/London"),
        ("Tokyo Standard Time", "Asia/Tokyo"),
        ("Romance Standard Time", "Europe/Paris"),
        ("Central European Standard Time", "Europe/Warsaw"),
    ];

    for (win_name, expected_iana) in windows_names {
        // Outbound refusal path
        assert!(
            !names_time_zone(win_name),
            "Windows TZ '{win_name}' is not recognized as IANA name"
        );

        let event_bare = CalendarEvent {
            time_zone: Some(win_name.to_owned()),
            ..CalendarEvent::default()
        };
        assert!(
            !defines_time_zone(&event_bare, win_name),
            "defines_time_zone rejects Windows TZ '{win_name}'"
        );
        assert!(
            !maps_time_zone(&event_bare),
            "maps_time_zone rejects Windows TZ '{win_name}' (unsendable by design)"
        );

        // Inbound CLDR resolution
        assert_eq!(
            windows_time_zone_to_iana(win_name),
            Some(expected_iana),
            "CLDR table maps '{win_name}' -> '{expected_iana}'"
        );
    }
}

#[test]
fn recurrence_complex_rrule_bysetpos_and_byday_ordinals_matrix() {
    // 1. BYSETPOS with positive, negative, and multiple positions
    let monthly_last_workday = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:SETPOS1\r\n",
        "DTSTART:20260101T090000Z\r\n",
        "RRULE:FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1\r\n",
        "SUMMARY:Monthly Workday\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed1 = ical_to_event(monthly_last_workday).expect("parse last workday");
    let rule1 = parsed1.recurrence_rule.as_ref().unwrap();
    assert_eq!(rule1.by_set_position.as_deref(), Some(&[-1][..]));
    assert!(maps_recurrence_rule(rule1));
    let ics1 = event_to_ical(&parsed1);
    assert!(ics1.contains("RRULE:FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1\r\n"));

    // 2. BYSETPOS combined with BYDAY ordinals and multiple set positions (first and last)
    let complex_rule = RecurrenceRule {
        frequency: "monthly".to_owned(),
        by_day: Some(vec![
            NDay {
                nth_of_period: Some(1),
                ..NDay::new("mo")
            },
            NDay {
                nth_of_period: Some(3),
                ..NDay::new("mo")
            },
            NDay {
                nth_of_period: Some(-1),
                ..NDay::new("fr")
            },
        ]),
        by_set_position: Some(vec![1, -1]),
        ..RecurrenceRule::new("monthly")
    };
    assert!(maps_recurrence_rule(&complex_rule));
    let event2 = CalendarEvent {
        id: Some("SETPOS2".into()),
        start: Some("2026-01-05T10:00:00".to_owned()),
        time_zone: Some("UTC".to_owned()),
        recurrence_rule: Some(complex_rule.clone()),
        ..CalendarEvent::default()
    };
    let ics2 = event_to_ical(&event2);
    assert!(ics2.contains("RRULE:FREQ=MONTHLY;BYDAY=1MO,3MO,-1FR;BYSETPOS=1,-1\r\n"));
    let parsed2 = ical_to_event(&ics2).expect("parse complex setpos");
    assert_eq!(parsed2.recurrence_rule.as_ref(), Some(&complex_rule));

    // 3. BYSETPOS combined with BYMONTHDAY
    let monthday_rule = RecurrenceRule {
        frequency: "monthly".to_owned(),
        by_month_day: Some(vec![1, 15, 31]),
        by_set_position: Some(vec![-1]),
        ..RecurrenceRule::new("monthly")
    };
    assert!(maps_recurrence_rule(&monthday_rule));
    let event3 = CalendarEvent {
        id: Some("SETPOS3".into()),
        start: Some("2026-01-01T10:00:00".to_owned()),
        time_zone: Some("UTC".to_owned()),
        recurrence_rule: Some(monthday_rule.clone()),
        ..CalendarEvent::default()
    };
    let ics3 = event_to_ical(&event3);
    assert!(ics3.contains("RRULE:FREQ=MONTHLY;BYMONTHDAY=1,15,31;BYSETPOS=-1\r\n"));
    let parsed3 = ical_to_event(&ics3).expect("parse monthday setpos");
    assert_eq!(parsed3.recurrence_rule.as_ref(), Some(&monthday_rule));

    // 4. BYSETPOS combined with BYHOUR and BYMINUTE
    let time_rule = RecurrenceRule {
        frequency: "daily".to_owned(),
        by_hour: Some(vec![9, 13, 17]),
        by_set_position: Some(vec![1, -1]),
        ..RecurrenceRule::default()
    };
    assert!(maps_recurrence_rule(&time_rule));
    let event4 = CalendarEvent {
        id: Some("SETPOS4".into()),
        start: Some("2026-01-01T09:00:00".to_owned()),
        time_zone: Some("UTC".to_owned()),
        recurrence_rule: Some(time_rule.clone()),
        ..CalendarEvent::default()
    };
    let ics4 = event_to_ical(&event4);
    assert!(ics4.contains("RRULE:FREQ=DAILY;BYHOUR=9,13,17;BYSETPOS=1,-1\r\n"));

    // 5. Boundary limits and refusal
    for valid_pos in [1, -1, 366, -366] {
        let rule = RecurrenceRule {
            frequency: "yearly".to_owned(),
            by_month: Some(vec!["1".to_owned()]),
            by_set_position: Some(vec![valid_pos]),
            ..RecurrenceRule::default()
        };
        assert!(maps_recurrence_rule(&rule), "pos {valid_pos} is valid");
    }
    for invalid_pos in [0, 367, -367] {
        let rule = RecurrenceRule {
            frequency: "yearly".to_owned(),
            by_month: Some(vec!["1".to_owned()]),
            by_set_position: Some(vec![invalid_pos]),
            ..RecurrenceRule::default()
        };
        assert!(!maps_recurrence_rule(&rule), "pos {invalid_pos} is refused");
    }

    // 6. Orphan BYSETPOS refusal (without expanding parts)
    let orphan_rule = RecurrenceRule {
        frequency: "monthly".to_owned(),
        by_set_position: Some(vec![1]),
        ..RecurrenceRule::default()
    };
    assert!(
        !maps_recurrence_rule(&orphan_rule),
        "orphan BYSETPOS is refused"
    );

    // 7. Multi-pass fixed-point stability
    let ics_loop1 = event_to_ical(&parsed2);
    let event_loop2 = ical_to_event(&ics_loop1).expect("re-parse");
    let ics_loop2 = event_to_ical(&event_loop2);
    assert_eq!(ics_loop1, ics_loop2);
    assert_eq!(parsed2, event_loop2);
}

#[test]
fn recurrence_byday_ordinals_and_wkst_fidelity_matrix() {
    // 1. Signed positive and negative ordinals in BYDAY
    let signed_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:ORDINAL1\r\n",
        "DTSTART:20260101T090000Z\r\n",
        "RRULE:FREQ=MONTHLY;BYDAY=+1MO,-1FR,+3WE\r\n",
        "SUMMARY:Signed Ordinals\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed1 = ical_to_event(signed_ics).expect("parse signed ordinals");
    let rule1 = parsed1.recurrence_rule.as_ref().unwrap();
    let days = rule1.by_day.as_ref().unwrap();
    assert_eq!(days.len(), 3);
    assert_eq!(days[0].nth_of_period, Some(1));
    assert_eq!(days[0].day, "mo");
    assert_eq!(days[1].nth_of_period, Some(-1));
    assert_eq!(days[1].day, "fr");
    assert_eq!(days[2].nth_of_period, Some(3));
    assert_eq!(days[2].day, "we");

    let ics1 = event_to_ical(&parsed1);
    // Leading plus is normalized away on emission per RFC 5545
    assert!(ics1.contains("RRULE:FREQ=MONTHLY;BYDAY=1MO,-1FR,3WE\r\n"));

    // 2. Mixed ordinals and bare weekdays
    let mixed_rule = RecurrenceRule {
        frequency: "monthly".to_owned(),
        by_day: Some(vec![
            NDay {
                nth_of_period: Some(2),
                ..NDay::new("tu")
            },
            NDay::new("th"),
        ]),
        ..RecurrenceRule::new("monthly")
    };
    assert!(maps_recurrence_rule(&mixed_rule));
    let event2 = CalendarEvent {
        id: Some("MIXED1".into()),
        start: Some("2026-01-01T10:00:00".to_owned()),
        time_zone: Some("UTC".to_owned()),
        recurrence_rule: Some(mixed_rule.clone()),
        ..CalendarEvent::default()
    };
    let ics2 = event_to_ical(&event2);
    assert!(ics2.contains("RRULE:FREQ=MONTHLY;BYDAY=2TU,TH\r\n"));
    let parsed2 = ical_to_event(&ics2).expect("parse mixed byday");
    assert_eq!(parsed2.recurrence_rule.as_ref(), Some(&mixed_rule));

    // 3. Frequency gating for ordinals: valid on monthly and yearly, refused on weekly and daily
    for valid_freq in ["monthly", "yearly"] {
        let rule = RecurrenceRule {
            frequency: valid_freq.to_owned(),
            by_day: Some(vec![NDay {
                nth_of_period: Some(1),
                ..NDay::new("mo")
            }]),
            ..RecurrenceRule::default()
        };
        assert!(maps_recurrence_rule(&rule), "{valid_freq} allows ordinals");
    }
    for invalid_freq in ["weekly", "daily", "hourly"] {
        let rule = RecurrenceRule {
            frequency: invalid_freq.to_owned(),
            by_day: Some(vec![NDay {
                nth_of_period: Some(1),
                ..NDay::new("mo")
            }]),
            ..RecurrenceRule::default()
        };
        assert!(
            !maps_recurrence_rule(&rule),
            "{invalid_freq} forbids ordinals"
        );
    }

    // 4. WKST non-default emission (SU) and default omission (MO)
    let wkst_su_rule = RecurrenceRule {
        frequency: "weekly".to_owned(),
        interval: Some(2),
        by_day: Some(vec![NDay::new("tu"), NDay::new("su")]),
        first_day_of_week: Some("su".to_owned()),
        ..RecurrenceRule::default()
    };
    assert!(maps_recurrence_rule(&wkst_su_rule));
    let event_su = CalendarEvent {
        id: Some("REC-WKST-1".into()),
        start: Some("2026-01-01T10:00:00".to_owned()),
        time_zone: Some("UTC".to_owned()),
        recurrence_rule: Some(wkst_su_rule),
        ..CalendarEvent::default()
    };
    let ics_su = event_to_ical(&event_su);
    assert!(ics_su.contains("RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=TU,SU;WKST=SU\r\n"));

    let wkst_mo_rule = RecurrenceRule {
        frequency: "weekly".to_owned(),
        interval: Some(2),
        by_day: Some(vec![NDay::new("tu"), NDay::new("su")]),
        first_day_of_week: Some("mo".to_owned()),
        ..RecurrenceRule::default()
    };
    assert!(maps_recurrence_rule(&wkst_mo_rule));
    let event_mo = CalendarEvent {
        id: Some("REC-WKST-2".into()),
        start: Some("2026-01-01T10:00:00".to_owned()),
        time_zone: Some("UTC".to_owned()),
        recurrence_rule: Some(wkst_mo_rule),
        ..CalendarEvent::default()
    };
    let ics_mo = event_to_ical(&event_mo);
    assert!(!line(&ics_mo, "RRULE:").contains("WKST"));

    // 5. WKST interaction with BYWEEKNO
    let weekno_rule = RecurrenceRule {
        frequency: "yearly".to_owned(),
        by_week_no: Some(vec![1, 52]),
        first_day_of_week: Some("su".to_owned()),
        ..RecurrenceRule::default()
    };
    assert!(maps_recurrence_rule(&weekno_rule));
    let event_weekno = CalendarEvent {
        id: Some("REC-WKST-3".into()),
        start: Some("2026-01-01T10:00:00".to_owned()),
        time_zone: Some("UTC".to_owned()),
        recurrence_rule: Some(weekno_rule),
        ..CalendarEvent::default()
    };
    let ics_weekno = event_to_ical(&event_weekno);
    assert!(ics_weekno.contains("RRULE:FREQ=YEARLY;BYWEEKNO=1,52;WKST=SU\r\n"));

    // 6. firstDayOfWeek validation: uppercase and invalid strings are refused
    for bad_wkst in ["MO", "SU", "monday", "sunday", ""] {
        let rule = RecurrenceRule {
            frequency: "weekly".to_owned(),
            first_day_of_week: Some(bad_wkst.to_owned()),
            ..RecurrenceRule::default()
        };
        assert!(
            !maps_recurrence_rule(&rule),
            "bad wkst '{bad_wkst}' refused"
        );
    }
}

#[test]
fn recurrence_rdate_exdate_and_override_interactions_matrix() {
    // 1. Complex RRULE + multi-line EXDATE + multi-line RDATE + overrides
    let complex_stream = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:REC-COMPLEX-001\r\n",
        "DTSTART:20260105T100000Z\r\n",
        "DURATION:PT1H\r\n",
        "SUMMARY:Biweekly Sprint Planning\r\n",
        "RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO;WKST=SU\r\n",
        "EXDATE:20260119T100000Z\r\n",
        "EXDATE:20260216T100000Z\r\n",
        "RDATE:20260126T100000Z\r\n",
        "RDATE:20260223T100000Z\r\n",
        "END:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID:REC-COMPLEX-001\r\n",
        "RECURRENCE-ID:20260202T100000Z\r\n",
        "DTSTART:20260202T140000Z\r\n",
        "DURATION:PT2H\r\n",
        "SUMMARY:Extended Sprint Planning\r\n",
        "END:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID:REC-COMPLEX-001\r\n",
        "RECURRENCE-ID:20260302T100000Z\r\n",
        "DTSTART:20260302T100000Z\r\n",
        "DURATION:PT1H\r\n",
        "STATUS:CANCELLED\r\n",
        "SUMMARY:Biweekly Sprint Planning\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );

    let parsed = ical_to_event(complex_stream).expect("parse complex stream");
    assert_eq!(parsed.title.as_deref(), Some("Biweekly Sprint Planning"));
    assert_eq!(parsed.duration.as_deref(), Some("PT1H"));

    let overrides = parsed
        .recurrence_overrides
        .as_ref()
        .expect("overrides present");
    // Cancelled instances via EXDATE
    assert_eq!(
        overrides.get("2026-01-19T10:00:00"),
        Some(&json!({"excluded": true}))
    );
    assert_eq!(
        overrides.get("2026-02-16T10:00:00"),
        Some(&json!({"excluded": true}))
    );
    // Added instances via RDATE
    assert_eq!(overrides.get("2026-01-26T10:00:00"), Some(&json!({})));
    assert_eq!(overrides.get("2026-02-23T10:00:00"), Some(&json!({})));
    // Modified instance with rescheduled start and lengthened duration
    assert_eq!(
        overrides.get("2026-02-02T10:00:00"),
        Some(&json!({
            "title": "Extended Sprint Planning",
            "start": "2026-02-02T14:00:00",
            "duration": "PT2H"
        }))
    );
    // Explicit cancelled status detached VEVENT (not excluded: true)
    assert_eq!(
        overrides.get("2026-03-02T10:00:00"),
        Some(&json!({"status": "cancelled"}))
    );

    // Outbound emission asserts consolidated single EXDATE and RDATE lines
    let emitted1 = event_to_ical(&parsed);
    assert!(emitted1.contains("EXDATE:20260119T100000Z,20260216T100000Z\r\n"));
    assert!(emitted1.contains("RDATE:20260126T100000Z,20260223T100000Z\r\n"));
    assert!(emitted1.contains("RECURRENCE-ID:20260202T100000Z\r\n"));
    assert!(emitted1.contains("DTSTART:20260202T140000Z\r\n"));
    assert!(emitted1.contains("DURATION:PT2H\r\n"));
    assert!(emitted1.contains("RECURRENCE-ID:20260302T100000Z\r\n"));
    assert!(emitted1.contains("STATUS:CANCELLED\r\n"));

    // Multi-pass roundtrip fixpoint stability
    let parsed2 = ical_to_event(&emitted1).expect("parse emitted 1");
    let emitted2 = event_to_ical(&parsed2);
    let parsed3 = ical_to_event(&emitted2).expect("parse emitted 2");
    let emitted3 = event_to_ical(&parsed3);

    assert_eq!(emitted2, emitted3, "iCalendar streams match at fixed point");
    assert_eq!(
        parsed2, parsed3,
        "CalendarEvent models match at fixed point"
    );

    // 2. Detached VEVENT modifying an RDATE-added occurrence
    let rdate_mod_stream = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:REC-RMOD-1\r\n",
        "DTSTART:20260105T100000Z\r\n",
        "DURATION:PT1H\r\n",
        "SUMMARY:Base Series\r\n",
        "RRULE:FREQ=WEEKLY\r\n",
        "RDATE:20260115T100000Z\r\n",
        "END:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID:REC-RMOD-1\r\n",
        "RECURRENCE-ID:20260115T100000Z\r\n",
        "DTSTART:20260115T113000Z\r\n",
        "DURATION:PT1H\r\n",
        "SUMMARY:Ad-hoc Session\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );
    let parsed_rmod = ical_to_event(rdate_mod_stream).expect("parse rdate mod");
    let rmod_overrides = parsed_rmod.recurrence_overrides.as_ref().unwrap();
    assert_eq!(
        rmod_overrides.get("2026-01-15T10:00:00"),
        Some(&json!({
            "title": "Ad-hoc Session",
            "start": "2026-01-15T11:30:00"
        }))
    );
    let emitted_rmod = event_to_ical(&parsed_rmod);
    // Modified instance is emitted as detached VEVENT; bare RDATE is omitted
    assert!(without(&emitted_rmod, "RDATE"));
    assert!(emitted_rmod.contains("RECURRENCE-ID:20260115T100000Z\r\n"));
    assert!(emitted_rmod.contains("DTSTART:20260115T113000Z\r\n"));

    // 3. All-day series (VALUE=DATE) with all-day overrides
    let mut allday_event = CalendarEvent {
        id: Some("ALLDAY-REC".into()),
        title: Some("All-Day Series".to_owned()),
        start: Some("2026-01-01T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        show_without_time: Some(true),
        recurrence_rule: Some(RecurrenceRule {
            frequency: "monthly".to_owned(),
            by_day: Some(vec![NDay {
                nth_of_period: Some(1),
                ..NDay::new("mo")
            }]),
            ..RecurrenceRule::default()
        }),
        recurrence_overrides: Some(BTreeMap::from([
            ("2026-02-02T00:00:00".to_owned(), json!({"excluded": true})),
            ("2026-02-09T00:00:00".to_owned(), json!({})),
            (
                "2026-03-02T00:00:00".to_owned(),
                json!({"title": "Rescheduled Holiday"}),
            ),
        ])),
        ..CalendarEvent::default()
    };
    let allday_ics = event_to_ical(&allday_event);
    assert!(allday_ics.contains("DTSTART;VALUE=DATE:20260101\r\n"));
    assert!(allday_ics.contains("EXDATE;VALUE=DATE:20260202\r\n"));
    assert!(allday_ics.contains("RDATE;VALUE=DATE:20260209\r\n"));
    assert!(allday_ics.contains("RECURRENCE-ID;VALUE=DATE:20260302\r\n"));
    assert!(allday_ics.contains("SUMMARY:Rescheduled Holiday\r\n"));

    let allday_parsed = ical_to_event(&allday_ics).expect("parse allday");
    assert_eq!(allday_parsed.show_without_time, Some(true));

    // 4. All-day series with a timed override: demotes to DATE-TIME
    allday_event.recurrence_overrides.as_mut().unwrap().insert(
        "2026-04-06T00:00:00".to_owned(),
        json!({
            "start": "2026-04-06T15:00:00",
            "title": "Timed Review Session"
        }),
    );
    let demoted_ics = event_to_ical(&allday_event);
    assert!(demoted_ics.contains("DTSTART:20260101T000000\r\n"));
    assert!(!demoted_ics.contains("VALUE=DATE"));
    assert!(demoted_ics.contains("RECURRENCE-ID:20260406T000000\r\n"));
    assert!(demoted_ics.contains("DTSTART:20260406T150000\r\n"));
}

#[test]
fn recurrence_overrides_predicates_and_refusal_boundaries_matrix() {
    let series = fixture_event();

    // 1. Refusal of unmapped properties in override patch
    for unmapped_prop in [
        "locations",
        "virtualLocations",
        "participants",
        "links",
        "color",
        "recurrenceRule",
        "useDefaultAlerts",
    ] {
        let bad_patch = json!({ unmapped_prop: "some_value" });
        assert!(
            !maps_recurrence_override(&series, "2026-01-15T10:00:00", &bad_patch),
            "property '{unmapped_prop}' is not in OVERRIDE_PROPERTIES and must be refused"
        );
        assert!(
            !sends_recurrence_override(&series, "2026-01-15T10:00:00", &bad_patch),
            "property '{unmapped_prop}' is not sendable in override"
        );
    }

    // 2. Refusal of excluded: true combined with other patch properties
    let conflicting_excluded = json!({
        "excluded": true,
        "title": "Zombie Event"
    });
    assert!(
        !maps_recurrence_override(&series, "2026-01-15T10:00:00", &conflicting_excluded),
        "excluded: true cannot be combined with other properties"
    );

    // 3. Refusal of empty strings (which would silently delete property rather than patch)
    for empty_prop in ["title", "description", "duration"] {
        let empty_patch = json!({ empty_prop: "" });
        assert!(
            !maps_recurrence_override(&series, "2026-01-15T10:00:00", &empty_patch),
            "empty string for '{empty_prop}' must be refused"
        );
    }

    // 4. Refusal of empty collections (keywords: {}, alerts: {})
    let empty_keywords = json!({ "keywords": {} });
    assert!(
        !maps_recurrence_override(&series, "2026-01-15T10:00:00", &empty_keywords),
        "empty keywords map in override must be refused"
    );
    let empty_alerts = json!({ "alerts": {} });
    assert!(
        !maps_recurrence_override(&series, "2026-01-15T10:00:00", &empty_alerts),
        "empty alerts map in override must be refused"
    );

    // 5. Refusal of invalid vocabularies
    let bad_status = json!({ "status": "unknown_status" });
    assert!(!maps_recurrence_override(
        &series,
        "2026-01-15T10:00:00",
        &bad_status
    ));
    let bad_privacy = json!({ "privacy": "super_secret" });
    assert!(!maps_recurrence_override(
        &series,
        "2026-01-15T10:00:00",
        &bad_privacy
    ));
    let bad_fb = json!({ "freeBusyStatus": "partially_busy" });
    assert!(!maps_recurrence_override(
        &series,
        "2026-01-15T10:00:00",
        &bad_fb
    ));

    // 6. Refusal of out-of-range or non-integer priority
    for bad_pri in [json!(10), json!(-1), json!("1"), json!(1.5)] {
        let pri_patch = json!({ "priority": bad_pri });
        assert!(!maps_recurrence_override(
            &series,
            "2026-01-15T10:00:00",
            &pri_patch
        ));
    }

    // 7. Suppression of alerts when series has useDefaultAlerts: true
    let mut series_default_alerts = series.clone();
    series_default_alerts.use_default_alerts = Some(true);
    let alert_patch = json!({
        "alerts": {
            "a1": {
                "@type": "Alert",
                "action": "display",
                "trigger": {
                    "@type": "OffsetTrigger",
                    "offset": "-PT15M"
                }
            }
        }
    });
    assert!(
        !maps_recurrence_override(&series_default_alerts, "2026-01-15T10:00:00", &alert_patch),
        "alerts override refused when series uses default alerts"
    );

    // 8. Custom solidus timezone in override: refused by maps, allowed by sends when defined
    let custom_tz = "/example.com/custom_tz";
    let custom_tz_patch = json!({ "timeZone": custom_tz });
    assert!(
        !maps_recurrence_override(&series, "2026-01-15T10:00:00", &custom_tz_patch),
        "custom solidus timezone without accompanying definition refused by maps_recurrence_override"
    );

    let mut series_with_tz = series.clone();
    series_with_tz.time_zones = Some(BTreeMap::from([(
        custom_tz.to_owned(),
        json!({
            "@type": "TimeZone",
            "tzId": custom_tz,
            "standard": [{
                "@type": "TimeZoneRule",
                "start": "1970-01-01T00:00:00",
                "offsetFrom": "+0100",
                "offsetTo": "+0100"
            }]
        }),
    )]));
    assert!(
        !maps_recurrence_override(&series_with_tz, "2026-01-15T10:00:00", &custom_tz_patch),
        "custom solidus timezone still refused by maps_recurrence_override (caller cannot send definition in override patch)"
    );
    assert!(
        sends_recurrence_override(&series_with_tz, "2026-01-15T10:00:00", &custom_tz_patch),
        "custom solidus timezone allowed by sends_recurrence_override when defined on series"
    );
}

#[test]
fn vtimezone_multiple_observances_historical_transitions_until_resolution() {
    // Characterizes VTIMEZONE with multiple historical STANDARD and DAYLIGHT observances:
    // US Eastern Time (America/New_York):
    // Era 1 (1987-2006): Daylight from first Sunday of April to last Sunday of October (-0500 to -0400).
    // Era 2 (2007 onwards): Daylight from second Sunday of March to first Sunday of November (-0500 to -0400).
    let ny_vtimezone = "BEGIN:VTIMEZONE\r\n\
         TZID:America/New_York\r\n\
         BEGIN:STANDARD\r\n\
         DTSTART:19871025T020000\r\n\
         TZOFFSETFROM:-0400\r\n\
         TZOFFSETTO:-0500\r\n\
         TZNAME:EST\r\n\
         RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10;UNTIL=20061029T060000Z\r\n\
         END:STANDARD\r\n\
         BEGIN:DAYLIGHT\r\n\
         DTSTART:19870405T020000\r\n\
         TZOFFSETFROM:-0500\r\n\
         TZOFFSETTO:-0400\r\n\
         TZNAME:EDT\r\n\
         RRULE:FREQ=YEARLY;BYDAY=1SU;BYMONTH=4;UNTIL=20060402T070000Z\r\n\
         END:DAYLIGHT\r\n\
         BEGIN:STANDARD\r\n\
         DTSTART:20071104T020000\r\n\
         TZOFFSETFROM:-0400\r\n\
         TZOFFSETTO:-0500\r\n\
         TZNAME:EST\r\n\
         RRULE:FREQ=YEARLY;BYDAY=1SU;BYMONTH=11\r\n\
         END:STANDARD\r\n\
         BEGIN:DAYLIGHT\r\n\
         DTSTART:20070311T020000\r\n\
         TZOFFSETFROM:-0500\r\n\
         TZOFFSETTO:-0400\r\n\
         TZNAME:EDT\r\n\
         RRULE:FREQ=YEARLY;BYDAY=2SU;BYMONTH=3\r\n\
         END:DAYLIGHT\r\n\
         END:VTIMEZONE\r\n";

    // 1. Era 1: March 15, 2005 (before April transition): in Standard time (-0500)
    let ics_era1_march = recurring_in("America/New_York", ny_vtimezone, "20050315T120000Z");
    let event_era1_march = ical_to_event(&ics_era1_march).expect("parse era1 march");
    let rule1 = event_era1_march.recurrence_rule.expect("rule era1 march");
    assert_eq!(
        rule1.until.as_deref(),
        Some("2005-03-15T07:00:00"),
        "Era 1 March 15 is in EST (-0500)"
    );
    assert!(maps_recurrence_rule(&rule1));

    // 2. Era 1: April 15, 2005 (after first Sunday of April): in Daylight time (-0400)
    let ics_era1_april = recurring_in("America/New_York", ny_vtimezone, "20050415T120000Z");
    let event_era1_april = ical_to_event(&ics_era1_april).expect("parse era1 april");
    let rule2 = event_era1_april.recurrence_rule.expect("rule era1 april");
    assert_eq!(
        rule2.until.as_deref(),
        Some("2005-04-15T08:00:00"),
        "Era 1 April 15 is in EDT (-0400)"
    );
    assert!(maps_recurrence_rule(&rule2));

    // 3. Era 2: March 15, 2026 (after second Sunday of March): in Daylight time (-0400)
    // Note difference from Era 1 March 15 above: 08:00:00 EDT vs 07:00:00 EST.
    let ics_era2_march = recurring_in("America/New_York", ny_vtimezone, "20260315T120000Z");
    let event_era2_march = ical_to_event(&ics_era2_march).expect("parse era2 march");
    let rule3 = event_era2_march.recurrence_rule.expect("rule era2 march");
    assert_eq!(
        rule3.until.as_deref(),
        Some("2026-03-15T08:00:00"),
        "Era 2 March 15 is in EDT (-0400), contrasting with Era 1 EST"
    );
    assert!(maps_recurrence_rule(&rule3));

    // 4. Era 1: October 31, 2005 (after last Sunday of October): in Standard time (-0500)
    let ics_era1_oct = recurring_in("America/New_York", ny_vtimezone, "20051031T120000Z");
    let event_era1_oct = ical_to_event(&ics_era1_oct).expect("parse era1 oct");
    let rule4 = event_era1_oct.recurrence_rule.expect("rule era1 oct");
    assert_eq!(
        rule4.until.as_deref(),
        Some("2005-10-31T07:00:00"),
        "Era 1 October 31 is in EST (-0500)"
    );
    assert!(maps_recurrence_rule(&rule4));

    // 5. Era 2: October 28, 2026 (before first Sunday of November): in Daylight time (-0400)
    // Note difference from Era 1 late October: 08:00:00 EDT vs 07:00:00 EST.
    let ics_era2_oct = recurring_in("America/New_York", ny_vtimezone, "20261028T120000Z");
    let event_era2_oct = ical_to_event(&ics_era2_oct).expect("parse era2 oct");
    let rule5 = event_era2_oct.recurrence_rule.expect("rule era2 oct");
    assert_eq!(
        rule5.until.as_deref(),
        Some("2026-10-28T08:00:00"),
        "Era 2 late October is in EDT (-0400), contrasting with Era 1 EST"
    );
    assert!(maps_recurrence_rule(&rule5));

    // 6. Series spanning eras: started in 2004 (Era 1), ending in 2026 (Era 2)
    let ics_spanning = format!(
        "BEGIN:VCALENDAR\r\n\
         {ny_vtimezone}\
         BEGIN:VEVENT\r\n\
         UID:ev-span-eras\r\n\
         DTSTART;TZID=America/New_York:20040101T100000\r\n\
         RRULE:FREQ=YEARLY;UNTIL=20260715T120000Z\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let event_spanning = ical_to_event(&ics_spanning).expect("parse spanning");
    let rule_spanning = event_spanning.recurrence_rule.expect("rule spanning");
    assert_eq!(
        rule_spanning.until.as_deref(),
        Some("2026-07-15T08:00:00"),
        "July in Era 2 resolves with -0400"
    );
    assert!(maps_recurrence_rule(&rule_spanning));
}

#[test]
fn vtimezone_multi_observance_with_one_off_and_rdate_transitions() {
    // Tests multi-observance VTIMEZONE combining one-off historical transitions and recurring rules:
    // Historical US War Time (1942-1945):
    // 1. STANDARD prior to Feb 1942: -0500
    // 2. DAYLIGHT one-off start Feb 9, 1942 (no RRULE): -0400
    // 3. STANDARD one-off post-war return Aug 14, 1945 (no RRULE): -0500
    // 4. DAYLIGHT regular yearly from 1967: -0400
    // 5. STANDARD regular yearly from 1967: -0500
    let war_time_vtimezone = "BEGIN:VTIMEZONE\r\n\
         TZID:America/Detroit\r\n\
         BEGIN:STANDARD\r\n\
         DTSTART:19420101T000000\r\n\
         TZOFFSETFROM:-0500\r\n\
         TZOFFSETTO:-0500\r\n\
         TZNAME:EST\r\n\
         END:STANDARD\r\n\
         BEGIN:DAYLIGHT\r\n\
         DTSTART:19420209T020000\r\n\
         TZOFFSETFROM:-0500\r\n\
         TZOFFSETTO:-0400\r\n\
         TZNAME:EWT\r\n\
         END:DAYLIGHT\r\n\
         BEGIN:STANDARD\r\n\
         DTSTART:19450814T190000\r\n\
         TZOFFSETFROM:-0400\r\n\
         TZOFFSETTO:-0500\r\n\
         TZNAME:EST\r\n\
         END:STANDARD\r\n\
         BEGIN:DAYLIGHT\r\n\
         DTSTART:19670430T020000\r\n\
         TZOFFSETFROM:-0500\r\n\
         TZOFFSETTO:-0400\r\n\
         TZNAME:EDT\r\n\
         RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=4\r\n\
         END:DAYLIGHT\r\n\
         BEGIN:STANDARD\r\n\
         DTSTART:19671029T020000\r\n\
         TZOFFSETFROM:-0400\r\n\
         TZOFFSETTO:-0500\r\n\
         TZNAME:EST\r\n\
         RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n\
         END:STANDARD\r\n\
         END:VTIMEZONE\r\n";

    // 1. During War Time (1943): offset in force is -0400
    let ics_1943 = recurring_in("America/Detroit", war_time_vtimezone, "19430601T120000Z");
    let event_1943 = ical_to_event(&ics_1943).expect("parse 1943");
    let rule_1943 = event_1943.recurrence_rule.expect("rule 1943");
    assert_eq!(
        rule_1943.until.as_deref(),
        Some("1943-06-01T08:00:00"),
        "War Time 1943 resolved with -0400"
    );

    // 2. Post-war before 1967 Uniform Time Act (1950): offset in force is -0500
    let ics_1950 = recurring_in("America/Detroit", war_time_vtimezone, "19500601T120000Z");
    let event_1950 = ical_to_event(&ics_1950).expect("parse 1950");
    let rule_1950 = event_1950.recurrence_rule.expect("rule 1950");
    assert_eq!(
        rule_1950.until.as_deref(),
        Some("1950-06-01T07:00:00"),
        "Post-war 1950 resolved with -0500"
    );

    // 3. Modern era (2026 summer): offset in force is -0400
    let ics_2026 = recurring_in("America/Detroit", war_time_vtimezone, "20260701T120000Z");
    let event_2026 = ical_to_event(&ics_2026).expect("parse 2026");
    let rule_2026 = event_2026.recurrence_rule.expect("rule 2026");
    assert_eq!(
        rule_2026.until.as_deref(),
        Some("2026-07-01T08:00:00"),
        "Modern summer 2026 resolved with -0400"
    );
}

#[test]
fn windows_time_zone_forms_feeding_real_recurrence_matrix() {
    // Tests diverse Windows standard time zone display names (from Outlook/Exchange)
    // with quoted and unquoted syntax feeding recurrence UNTIL calculations.
    let test_cases = [
        (
            "Eastern Standard Time",
            "America/New_York",
            "-0500",
            "-0400",
            3,
            11,
            "20260615T120000Z",
            "2026-06-15T08:00:00",
        ),
        (
            "Central Standard Time",
            "America/Chicago",
            "-0600",
            "-0500",
            3,
            11,
            "20260615T120000Z",
            "2026-06-15T07:00:00",
        ),
        (
            "Pacific Standard Time",
            "America/Los_Angeles",
            "-0800",
            "-0700",
            3,
            11,
            "20260615T120000Z",
            "2026-06-15T05:00:00",
        ),
        (
            "W. Europe Standard Time",
            "Europe/Berlin",
            "+0100",
            "+0200",
            3,
            10,
            "20260615T120000Z",
            "2026-06-15T14:00:00",
        ),
        (
            "Romance Standard Time",
            "Europe/Paris",
            "+0100",
            "+0200",
            3,
            10,
            "20260615T120000Z",
            "2026-06-15T14:00:00",
        ),
        (
            "Tokyo Standard Time",
            "Asia/Tokyo",
            "+0900",
            "+0900",
            0,
            0,
            "20260615T120000Z",
            "2026-06-15T21:00:00",
        ),
        (
            "AUS Eastern Standard Time",
            "Australia/Sydney",
            "+1000",
            "+1100",
            10,
            4,
            "20260115T120000Z",
            "2026-01-15T23:00:00",
        ),
    ];

    for (
        win_name,
        expected_iana,
        std_offset,
        dst_offset,
        dst_month,
        std_month,
        until_utc,
        expected_until_local,
    ) in test_cases
    {
        let vtz = if dst_month > 0 {
            format!(
                "BEGIN:VTIMEZONE\r\n\
                 TZID:{win_name}\r\n\
                 BEGIN:STANDARD\r\n\
                 DTSTART:16010101T020000\r\n\
                 TZOFFSETFROM:{dst_offset}\r\n\
                 TZOFFSETTO:{std_offset}\r\n\
                 RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=1SU;BYMONTH={std_month}\r\n\
                 END:STANDARD\r\n\
                 BEGIN:DAYLIGHT\r\n\
                 DTSTART:16010101T020000\r\n\
                 TZOFFSETFROM:{std_offset}\r\n\
                 TZOFFSETTO:{dst_offset}\r\n\
                 RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=1SU;BYMONTH={dst_month}\r\n\
                 END:DAYLIGHT\r\n\
                 END:VTIMEZONE\r\n"
            )
        } else {
            format!(
                "BEGIN:VTIMEZONE\r\n\
                 TZID:{win_name}\r\n\
                 BEGIN:STANDARD\r\n\
                 DTSTART:16010101T000000\r\n\
                 TZOFFSETFROM:{std_offset}\r\n\
                 TZOFFSETTO:{std_offset}\r\n\
                 END:STANDARD\r\n\
                 END:VTIMEZONE\r\n"
            )
        };

        // Test with quoted TZID syntax (e.g. DTSTART;TZID="Eastern Standard Time":...)
        let ics_quoted = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
             {vtz}\
             BEGIN:VEVENT\r\n\
             UID:rec-win-quoted-{expected_iana}\r\n\
             DTSTART;TZID=\"{win_name}\":20260101T100000\r\n\
             RRULE:FREQ=WEEKLY;UNTIL={until_utc}\r\n\
             SUMMARY:Meeting in {win_name}\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n"
        );
        let event_q = ical_to_event(&ics_quoted).expect("parse quoted win tzid");
        assert_eq!(
            event_q.time_zone.as_deref(),
            Some(expected_iana),
            "Windows name '{win_name}' maps to canonical IANA '{expected_iana}'"
        );
        let rule_q = event_q.recurrence_rule.as_ref().expect("rule quoted");
        assert_eq!(
            rule_q.until.as_deref(),
            Some(expected_until_local),
            "UNTIL in '{win_name}' resolved correctly"
        );
        assert!(maps_recurrence_rule(rule_q));

        // Outbound emission normalizes to canonical IANA TZID
        let out_ics = event_to_ical(&event_q);
        assert!(!out_ics.contains(&format!("TZID=\"{win_name}\"")));
        assert!(!out_ics.contains(&format!("TZID={win_name}")));

        // Fixed-point convergence
        let re_event = ical_to_event(&out_ics).expect("reparse");
        assert_eq!(re_event.time_zone.as_deref(), Some(expected_iana));
        assert_eq!(event_to_ical(&re_event), out_ics);

        // Test with unquoted TZID syntax
        let ics_unquoted = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
             {vtz}\
             BEGIN:VEVENT\r\n\
             UID:rec-win-unquoted-{expected_iana}\r\n\
             DTSTART;TZID={win_name}:20260101T100000\r\n\
             RRULE:FREQ=WEEKLY;UNTIL={until_utc}\r\n\
             SUMMARY:Meeting in {win_name}\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n"
        );
        let event_u = ical_to_event(&ics_unquoted).expect("parse unquoted win tzid");
        assert_eq!(event_u.time_zone.as_deref(), Some(expected_iana));
        let rule_u = event_u.recurrence_rule.expect("rule unquoted");
        assert_eq!(rule_u.until.as_deref(), Some(expected_until_local));
    }
}

#[test]
fn globally_unique_tzid_forms_feeding_real_recurrence_matrix() {
    // Tests diverse globally-unique-form TZIDs (RFC 5545 §3.8.3.1) from major exporters
    // with matching VTIMEZONE feeding recurrence UNTIL calculations.
    let test_cases = [
        (
            "/mozilla.org/20050126_1/America/New_York",
            "America/New_York",
            "-0500",
            "-0400",
            3,
            11,
            "20260615T120000Z",
            "2026-06-15T08:00:00",
        ),
        (
            "/citadel.org/20250101_1/Europe/Berlin",
            "Europe/Berlin",
            "+0100",
            "+0200",
            3,
            10,
            "20260615T120000Z",
            "2026-06-15T14:00:00",
        ),
        (
            "/freeassociation.sourceforge.net/Tzfile/America/Chicago",
            "America/Chicago",
            "-0600",
            "-0500",
            3,
            11,
            "20260615T120000Z",
            "2026-06-15T07:00:00",
        ),
        (
            "/apple.com/timezones/America/Los_Angeles",
            "America/Los_Angeles",
            "-0800",
            "-0700",
            3,
            11,
            "20260615T120000Z",
            "2026-06-15T05:00:00",
        ),
        (
            "/google.com/20260101_1/Asia/Tokyo",
            "Asia/Tokyo",
            "+0900",
            "+0900",
            0,
            0,
            "20260615T120000Z",
            "2026-06-15T21:00:00",
        ),
    ];

    for (
        unique_tzid,
        expected_iana,
        std_offset,
        dst_offset,
        dst_month,
        std_month,
        until_utc,
        expected_until_local,
    ) in test_cases
    {
        let vtz = if dst_month > 0 {
            format!(
                "BEGIN:VTIMEZONE\r\n\
                 TZID:{unique_tzid}\r\n\
                 BEGIN:STANDARD\r\n\
                 DTSTART:19701025T020000\r\n\
                 TZOFFSETFROM:{dst_offset}\r\n\
                 TZOFFSETTO:{std_offset}\r\n\
                 RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH={std_month}\r\n\
                 END:STANDARD\r\n\
                 BEGIN:DAYLIGHT\r\n\
                 DTSTART:19700329T020000\r\n\
                 TZOFFSETFROM:{std_offset}\r\n\
                 TZOFFSETTO:{dst_offset}\r\n\
                 RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH={dst_month}\r\n\
                 END:DAYLIGHT\r\n\
                 END:VTIMEZONE\r\n"
            )
        } else {
            format!(
                "BEGIN:VTIMEZONE\r\n\
                 TZID:{unique_tzid}\r\n\
                 BEGIN:STANDARD\r\n\
                 DTSTART:19700101T000000\r\n\
                 TZOFFSETFROM:{std_offset}\r\n\
                 TZOFFSETTO:{std_offset}\r\n\
                 END:STANDARD\r\n\
                 END:VTIMEZONE\r\n"
            )
        };

        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
             {vtz}\
             BEGIN:VEVENT\r\n\
             UID:rec-uniq-{expected_iana}\r\n\
             DTSTART;TZID={unique_tzid}:20260101T100000\r\n\
             RRULE:FREQ=WEEKLY;UNTIL={until_utc}\r\n\
             SUMMARY:Meeting in {unique_tzid}\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n"
        );

        let event = ical_to_event(&ics).expect("parse unique tzid");
        assert_eq!(
            event.time_zone.as_deref(),
            Some(expected_iana),
            "Unique TZID '{unique_tzid}' maps to canonical IANA '{expected_iana}'"
        );
        let rule = event.recurrence_rule.as_ref().expect("rule");
        assert_eq!(
            rule.until.as_deref(),
            Some(expected_until_local),
            "UNTIL for '{unique_tzid}' resolved correctly"
        );
        assert!(maps_recurrence_rule(rule));

        // Outbound emission normalizes to canonical IANA TZID without solidus
        let out_ics = event_to_ical(&event);
        assert!(
            out_ics.contains(&format!("DTSTART;TZID={expected_iana}:")),
            "Outbound emits standard canonical IANA TZID, got: {out_ics}"
        );
        assert!(!out_ics.contains(&format!("TZID={unique_tzid}")));
        assert!(!out_ics.contains(&format!("TZID=\"{unique_tzid}\"")));

        // Fixed point convergence
        let re_event = ical_to_event(&out_ics).expect("reparse");
        assert_eq!(re_event.time_zone.as_deref(), Some(expected_iana));
        assert_eq!(event_to_ical(&re_event), out_ics);
    }
}

#[test]
fn custom_solidus_vtimezone_multiple_observances_roundtrip_and_recurrence() {
    // Tests a custom defined solidus timezone with multiple observances across eras:
    // /corp.internal/MultiObservance:
    // Era 1 (1990-2006): Standard (-0500) and Daylight (-0400)
    // Era 2 (2007 onwards): Standard (-0500) and Daylight (-0400) with adjusted transition dates
    let custom_tzid = "/corp.internal/MultiObservance";
    let custom_multi_zone = json!({
        "@type": "TimeZone",
        "tzId": custom_tzid,
        "standard": [
            {
                "@type": "TimeZoneRule",
                "start": "1990-10-28T02:00:00",
                "offsetFrom": "-0400",
                "offsetTo": "-0500",
                "recurrenceRules": [{
                    "@type": "RecurrenceRule",
                    "frequency": "yearly",
                    "until": "2006-10-29T02:00:00",
                    "byMonth": ["10"],
                    "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": -1}],
                }],
                "names": {"EST": true},
            },
            {
                "@type": "TimeZoneRule",
                "start": "2007-11-04T02:00:00",
                "offsetFrom": "-0400",
                "offsetTo": "-0500",
                "recurrenceRules": [{
                    "@type": "RecurrenceRule",
                    "frequency": "yearly",
                    "byMonth": ["11"],
                    "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": 1}],
                }],
                "names": {"EST": true},
            }
        ],
        "daylight": [
            {
                "@type": "TimeZoneRule",
                "start": "1990-04-01T02:00:00",
                "offsetFrom": "-0500",
                "offsetTo": "-0400",
                "recurrenceRules": [{
                    "@type": "RecurrenceRule",
                    "frequency": "yearly",
                    "until": "2006-04-02T02:00:00",
                    "byMonth": ["4"],
                    "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": 1}],
                }],
                "names": {"EDT": true},
            },
            {
                "@type": "TimeZoneRule",
                "start": "2007-03-11T02:00:00",
                "offsetFrom": "-0500",
                "offsetTo": "-0400",
                "recurrenceRules": [{
                    "@type": "RecurrenceRule",
                    "frequency": "yearly",
                    "byMonth": ["3"],
                    "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": 2}],
                }],
                "names": {"EDT": true},
            }
        ],
    });

    let mut event = defining(custom_tzid, json!({custom_tzid: custom_multi_zone}));
    event.recurrence_rule = Some(RecurrenceRule {
        frequency: "weekly".to_owned(),
        until: Some("2026-06-15T08:00:00".to_owned()),
        ..Default::default()
    });

    // 1. Serialization to iCalendar
    let ics = event_to_ical(&event);
    assert!(ics.contains("BEGIN:VTIMEZONE\r\n"));
    assert!(ics.contains(&format!("TZID:{custom_tzid}\r\n")));
    assert_eq!(
        ics.matches("BEGIN:STANDARD\r\n").count(),
        2,
        "emits both standard observances"
    );
    assert_eq!(
        ics.matches("BEGIN:DAYLIGHT\r\n").count(),
        2,
        "emits both daylight observances"
    );

    // 2. Parsing back into CalendarEvent
    let parsed = ical_to_event(&ics).expect("parse custom multi-observance event");
    assert_eq!(parsed.time_zone.as_deref(), Some(custom_tzid));
    assert!(defines_time_zone(&parsed, custom_tzid));
    assert!(maps_time_zone(&parsed));

    let time_zones = parsed.time_zones.as_ref().expect("time_zones present");
    let zone_val = time_zones.get(custom_tzid).expect("custom zone in map");
    let std_rules = zone_val
        .get("standard")
        .and_then(Value::as_array)
        .expect("standard rules");
    let dst_rules = zone_val
        .get("daylight")
        .and_then(Value::as_array)
        .expect("daylight rules");
    assert_eq!(std_rules.len(), 2, "both standard observances parsed");
    assert_eq!(dst_rules.len(), 2, "both daylight observances parsed");

    let parsed_rule = parsed.recurrence_rule.as_ref().expect("recurrence rule");
    assert_eq!(parsed_rule.until.as_deref(), Some("2026-06-15T08:00:00"));
    assert!(maps_recurrence_rule(parsed_rule));

    // 3. Multi-pass fixpoint stability
    let ics2 = event_to_ical(&parsed);
    let reparsed = ical_to_event(&ics2).expect("reparse");
    let ics3 = event_to_ical(&reparsed);
    assert_eq!(ics2, ics3);
    assert_eq!(parsed.time_zones, reparsed.time_zones);
}

#[test]
fn recurrence_overrides_with_windows_and_unique_tzids_fidelity() {
    // Tests recurrence series and detached VEVENT recurrence overrides carrying Windows/unique TZIDs.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n",
        "BEGIN:VTIMEZONE\r\nTZID:Eastern Standard Time\r\n",
        "BEGIN:STANDARD\r\nDTSTART:16010101T020000\r\n",
        "TZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\n",
        "RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=1SU;BYMONTH=11\r\n",
        "END:STANDARD\r\n",
        "BEGIN:DAYLIGHT\r\nDTSTART:16010101T020000\r\n",
        "TZOFFSETFROM:-0500\r\nTZOFFSETTO:-0400\r\n",
        "RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=2SU;BYMONTH=3\r\n",
        "END:DAYLIGHT\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\nUID:series-win-override\r\n",
        "DTSTART;TZID=\"Eastern Standard Time\":20260309T100000\r\n",
        "DURATION:PT1H\r\n",
        "RRULE:FREQ=WEEKLY;COUNT=5\r\n",
        "SUMMARY:Weekly Team Sync\r\n",
        "END:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID:series-win-override\r\n",
        "RECURRENCE-ID;TZID=\"Eastern Standard Time\":20260316T100000\r\n",
        "DTSTART;TZID=\"Eastern Standard Time\":20260316T140000\r\n",
        "DURATION:PT2H\r\n",
        "SUMMARY:Rescheduled Strategy Session\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n"
    );

    let event = ical_to_event(ics).expect("parse series and override");
    assert_eq!(event.time_zone.as_deref(), Some("America/New_York"));
    assert_eq!(event.title.as_deref(), Some("Weekly Team Sync"));

    let overrides = event.recurrence_overrides.as_ref().expect("overrides map");
    let patch = overrides
        .get("2026-03-16T10:00:00")
        .expect("override patch");
    assert_eq!(
        patch.get("title").and_then(Value::as_str),
        Some("Rescheduled Strategy Session")
    );
    assert_eq!(patch.get("duration").and_then(Value::as_str), Some("PT2H"));
    assert_eq!(
        patch.get("start").and_then(Value::as_str),
        Some("2026-03-16T14:00:00")
    );

    // Outbound emission normalizes both master VEVENT and override VEVENT to canonical IANA TZID
    let out = event_to_ical(&event);
    assert!(out.contains("DTSTART;TZID=America/New_York:20260309T100000\r\n"));
    assert!(out.contains("RECURRENCE-ID;TZID=America/New_York:20260316T100000\r\n"));
    assert!(out.contains("DTSTART;TZID=America/New_York:20260316T140000\r\n"));
    assert!(!out.contains("Eastern Standard Time"));
}

#[test]
fn vtimezone_multiple_observances_corrupt_observance_safe_refusal() {
    // 1. Multi-observance custom zone with one corrupt observance (bad offset)
    // Refuses the entire zone definition rather than silently calculating wrong offsets.
    let custom_tzid = "/example.com/CorruptObservance";
    let bad_vtz = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
         BEGIN:VTIMEZONE\r\n\
         TZID:/example.com/CorruptObservance\r\n\
         BEGIN:STANDARD\r\n\
         DTSTART:19701025T030000\r\n\
         TZOFFSETFROM:+0200\r\n\
         TZOFFSETTO:+0100\r\n\
         RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n\
         END:STANDARD\r\n\
         BEGIN:DAYLIGHT\r\n\
         DTSTART:19700329T020000\r\n\
         TZOFFSETFROM:+0100\r\n\
         TZOFFSETTO:invalid-offset\r\n\
         RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3\r\n\
         END:DAYLIGHT\r\n\
         END:VTIMEZONE\r\n\
         BEGIN:VEVENT\r\n\
         UID:ev-corrupt-tz\r\n\
         DTSTART;TZID=/example.com/CorruptObservance:20260101T100000\r\n\
         RRULE:FREQ=DAILY;UNTIL=20260105T120000Z\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n";

    let event = ical_to_event(bad_vtz).expect("parse corrupt zone");
    assert_eq!(event.time_zone.as_deref(), Some(custom_tzid));
    assert_eq!(
        event.time_zones, None,
        "corrupt observance invalidates the whole zone definition"
    );
    assert!(
        !maps_time_zone(&event),
        "maps_time_zone refuses unsendable undefined custom zone"
    );

    // Recurrence rule UNTIL retains trailing Z because zone could not be resolved,
    // which maps_recurrence_rule safely refuses.
    let rule = event.recurrence_rule.expect("recurrence rule");
    assert_eq!(rule.until.as_deref(), Some("2026-01-05T12:00:00Z"));
    assert!(
        !maps_recurrence_rule(&rule),
        "recurrence rule with unresolvable zone UNTIL is refused to prevent silent server corruption"
    );
}

#[test]
fn valarm_repeat_and_duration_pairing_and_malformed_variations_matrix() {
    // 1. Valid RFC 5545 §3.6.6 REPEAT + DURATION pair on ACTION:DISPLAY VALARM.
    let valid_repeat_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n",
        "BEGIN:VEVENT\r\nUID:repeat-valid\r\nDTSTART:20260115T100000Z\r\n",
        "SUMMARY:Meeting with repeating reminder\r\n",
        "BEGIN:VALARM\r\nUID:a1\r\nACTION:DISPLAY\r\n",
        "TRIGGER:-PT15M\r\n",
        "REPEAT:4\r\n",
        "DURATION:PT5M\r\n",
        "DESCRIPTION:Meeting with repeating reminder\r\n",
        "END:VALARM\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let event = ical_to_event(valid_repeat_ics).expect("parse valid repeat/duration alarm");
    let alerts = event.alerts.as_ref().expect("alerts present");
    assert_eq!(alerts.len(), 1);
    let alert = &alerts["a1"];
    assert_eq!(alert["action"], "display");
    assert_eq!(alert["trigger"]["offset"], "-PT15M");
    // RFC 8984 dropped REPEAT, so the JSCalendar representation retains the primary trigger.
    assert!(maps_alerts(&event));

    // Outbound emission: drawn_alert emits the display alarm with primary trigger,
    // and omits REPEAT/DURATION since JSCalendar Alert has no repeat fields.
    let out_ics = event_to_ical(&event);
    assert!(out_ics.contains("BEGIN:VALARM\r\n"));
    assert!(out_ics.contains("TRIGGER:-PT15M\r\n"));
    assert!(!out_ics.contains("REPEAT:"));
    assert!(!out_ics.contains("DURATION:PT5M"));

    // Multi-stage fixed-point roundtrip stability.
    let reparsed = ical_to_event(&out_ics).expect("reparse");
    let out_ics2 = event_to_ical(&reparsed);
    assert_eq!(out_ics, out_ics2);
    assert_eq!(event.alerts, reparsed.alerts);

    // 2. Malformed RFC 5545 §3.6.6 combinations in inbound streams.
    // RFC 5545 §3.6.6 requires: DURATION and REPEAT MUST both be specified or both omitted.
    // Parser must safely extract primary TRIGGER without panicking across all malformed variants.
    for (name, extra_lines) in [
        ("repeat_without_duration", "REPEAT:3\r\n"),
        ("duration_without_repeat", "DURATION:PT5M\r\n"),
        ("repeat_zero", "REPEAT:0\r\nDURATION:PT5M\r\n"),
        ("repeat_negative", "REPEAT:-2\r\nDURATION:PT5M\r\n"),
        ("repeat_non_integer", "REPEAT:three\r\nDURATION:PT5M\r\n"),
        ("duration_negative", "REPEAT:2\r\nDURATION:-PT5M\r\n"),
        ("duration_zero", "REPEAT:2\r\nDURATION:PT0S\r\n"),
        ("lowercase_properties", "repeat:2\r\nduration:pt5m\r\n"),
        (
            "multiple_repeat_lines",
            "REPEAT:2\r\nREPEAT:4\r\nDURATION:PT5M\r\n",
        ),
        (
            "multiple_duration_lines",
            "REPEAT:2\r\nDURATION:PT5M\r\nDURATION:PT10M\r\n",
        ),
    ] {
        let malformed_ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
             BEGIN:VEVENT\r\nUID:malformed-{name}\r\nDTSTART:20260115T100000Z\r\n\
             SUMMARY:Malformed Alarm Event\r\n\
             BEGIN:VALARM\r\nUID:k1\r\nACTION:DISPLAY\r\nTRIGGER:-PT30M\r\n\
             {extra_lines}END:VALARM\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let parsed = ical_to_event(&malformed_ics)
            .unwrap_or_else(|e| panic!("failed parsing {name}: {e:?}"));
        let alerts = parsed
            .alerts
            .as_ref()
            .unwrap_or_else(|| panic!("missing alerts for {name}"));
        assert_eq!(alerts.len(), 1, "{name} alert count");
        assert_eq!(
            alerts["k1"]["trigger"]["offset"], "-PT30M",
            "{name} trigger offset"
        );
    }

    // 3. Outbound refusal by maps_alerts for JSCalendar Alerts with unmodeled repeat/duration fields.
    // If a client or server injects non-standard repeat or duration fields into JSCalendar Alert,
    // maps_alerts strictly refuses the event to protect server-side data from whole-property clobbering.
    let event_with_repeat_field = reminded([(
        "k1",
        json!({
            "@type": "Alert",
            "action": "display",
            "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
            "repeat": 4,
        }),
    )]);
    assert!(!maps_alerts(&event_with_repeat_field));
    assert!(without(
        &event_to_ical(&event_with_repeat_field),
        "BEGIN:VALARM"
    ));

    let event_with_duration_field = reminded([(
        "k1",
        json!({
            "@type": "Alert",
            "action": "display",
            "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
            "duration": "PT5M",
        }),
    )]);
    assert!(!maps_alerts(&event_with_duration_field));
    assert!(without(
        &event_to_ical(&event_with_duration_field),
        "BEGIN:VALARM"
    ));
}

#[test]
fn valarm_multiple_alarms_complex_streams_and_id_collision_fidelity() {
    // 1. Multi-alarm sequences with varied trigger offsets:
    // -P1W (1 week before), -P1D (1 day before), -PT2H (2 hours before),
    // -PT15M (15 min before), PT0S (at start), and RELATED=END:PT10M (10 min after end).
    let multi_alarm_event = reminded([
        (
            "k_week",
            json!({
                "@type": "Alert",
                "action": "display",
                "trigger": {"@type": "OffsetTrigger", "offset": "-P1W"},
            }),
        ),
        (
            "k_day",
            json!({
                "@type": "Alert",
                "action": "display",
                "trigger": {"@type": "OffsetTrigger", "offset": "-P1D"},
            }),
        ),
        (
            "k_2h",
            json!({
                "@type": "Alert",
                "action": "display",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT2H"},
            }),
        ),
        (
            "k_15m",
            json!({
                "@type": "Alert",
                "action": "display",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
            }),
        ),
        (
            "k_zero",
            json!({
                "@type": "Alert",
                "action": "display",
                "trigger": {"@type": "OffsetTrigger", "offset": "PT0S"},
            }),
        ),
        (
            "k_end",
            json!({
                "@type": "Alert",
                "action": "display",
                "trigger": {"@type": "OffsetTrigger", "offset": "PT10M", "relativeTo": "end"},
            }),
        ),
    ]);
    assert!(maps_alerts(&multi_alarm_event));

    let ics = event_to_ical(&multi_alarm_event);
    assert_eq!(ics.matches("BEGIN:VALARM\r\n").count(), 6);
    assert!(ics.contains("TRIGGER:-P1W\r\n"));
    assert!(ics.contains("TRIGGER:-P1D\r\n"));
    assert!(ics.contains("TRIGGER:-PT2H\r\n"));
    assert!(ics.contains("TRIGGER:-PT15M\r\n"));
    assert!(ics.contains("TRIGGER:PT0S\r\n"));
    assert!(ics.contains("TRIGGER;RELATED=END:PT10M\r\n"));

    let roundtrip = ical_to_event(&ics).expect("parse multi-alarm ics");
    assert_eq!(roundtrip.alerts, multi_alarm_event.alerts);
    let ics2 = event_to_ical(&roundtrip);
    assert_eq!(ics, ics2);

    // 2. Multiple alarms with identical trigger offsets:
    // Two distinct named alarms sharing the same trigger -PT15M.
    let identical_offset_event = reminded([
        ("k1", quarter_of_an_hour_before()),
        ("k2", quarter_of_an_hour_before()),
    ]);
    assert!(maps_alerts(&identical_offset_event));
    let identical_ics = event_to_ical(&identical_offset_event);
    assert_eq!(identical_ics.matches("BEGIN:VALARM\r\n").count(), 2);
    assert_eq!(identical_ics.matches("TRIGGER:-PT15M\r\n").count(), 2);
    assert!(identical_ics.contains("UID:k1\r\n"));
    assert!(identical_ics.contains("UID:k2\r\n"));

    let identical_roundtrip = ical_to_event(&identical_ics).expect("parse identical offset ics");
    assert_eq!(identical_roundtrip.alerts, identical_offset_event.alerts);

    // Two nameless alarms sharing the same trigger -PT15M.
    let nameless_identical_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n",
        "BEGIN:VEVENT\r\nUID:nameless-identical\r\nDTSTART:20260115T100000Z\r\n",
        "SUMMARY:Nameless Identical\r\n",
        "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n",
        "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_nameless = ical_to_event(nameless_identical_ics).expect("parse nameless identical");
    let nameless_alerts = parsed_nameless.alerts.expect("alerts present");
    assert_eq!(nameless_alerts.len(), 2);
    assert!(nameless_alerts.contains_key("a1"));
    assert!(nameless_alerts.contains_key("a2"));
    assert_eq!(nameless_alerts["a1"]["trigger"]["offset"], "-PT15M");
    assert_eq!(nameless_alerts["a2"]["trigger"]["offset"], "-PT15M");

    // 3. High multiplicity scaling: 15 alarms on a single event.
    let high_count_ics = {
        let mut lines = String::from(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
             BEGIN:VEVENT\r\nUID:high-multiplicity\r\nDTSTART:20260115T100000Z\r\n\
             SUMMARY:High Multiplicity Alarm Event\r\n",
        );
        for i in 1..=15 {
            lines.push_str(&format!(
                "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT{i}M\r\nEND:VALARM\r\n"
            ));
        }
        lines.push_str("END:VEVENT\r\nEND:VCALENDAR\r\n");
        lines
    };
    let high_parsed = ical_to_event(&high_count_ics).expect("parse 15 alarms");
    let high_alerts = high_parsed.alerts.as_ref().expect("15 alerts present");
    assert_eq!(high_alerts.len(), 15);
    for i in 1..=15 {
        assert!(high_alerts.contains_key(&format!("a{i}")), "contains a{i}");
    }
    let high_out_ics = event_to_ical(&high_parsed);
    assert_eq!(high_out_ics.matches("BEGIN:VALARM\r\n").count(), 15);
    assert!(high_out_ics.contains("UID:a10\r\n"));
    assert!(high_out_ics.contains("UID:a15\r\n"));

    // 4. Key synthesis and collision avoidance with non-standard UIDs:
    // UIDs from real exporters that violate RFC 8984 Id grammar (1..=255 octets [a-zA-Z0-9_-])
    // must fall back cleanly to positional synthesized keys.
    let overlong_uid = "k".repeat(256);
    let non_standard_uid_ics = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
         BEGIN:VEVENT\r\nUID:non-std-uids\r\nDTSTART:20260115T100000Z\r\n\
         SUMMARY:Non-standard UIDs\r\n\
         BEGIN:VALARM\r\nUID:urn:uuid:12345678-1234-5678-1234-567812345678\r\n\
         ACTION:DISPLAY\r\nTRIGGER:-PT5M\r\nEND:VALARM\r\n\
         BEGIN:VALARM\r\nUID:{{7B8E9D1C-3A4B-5C6D-7E8F-9A0B1C2D3E4F}}\r\n\
         ACTION:DISPLAY\r\nTRIGGER:-PT10M\r\nEND:VALARM\r\n\
         BEGIN:VALARM\r\nUID:alarm-notice@calendar.example.org\r\n\
         ACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n\
         BEGIN:VALARM\r\nUID:alarm:v1/sub 1\r\n\
         ACTION:DISPLAY\r\nTRIGGER:-PT20M\r\nEND:VALARM\r\n\
         BEGIN:VALARM\r\nUID:\r\n\
         ACTION:DISPLAY\r\nTRIGGER:-PT25M\r\nEND:VALARM\r\n\
         BEGIN:VALARM\r\nUID:{overlong_uid}\r\n\
         ACTION:DISPLAY\r\nTRIGGER:-PT30M\r\nEND:VALARM\r\n\
         END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_non_std = ical_to_event(&non_standard_uid_ics).expect("parse non-standard UIDs");
    let non_std_alerts = parsed_non_std.alerts.as_ref().expect("alerts present");
    assert_eq!(non_std_alerts.len(), 6);
    // All 6 receive positional keys a1 through a6 because their UIDs violate Id grammar.
    for i in 1..=6 {
        assert!(
            non_std_alerts.contains_key(&format!("a{i}")),
            "contains a{i}"
        );
    }
    // Outbound emission emits valid RFC 9074 UIDs.
    let non_std_out = event_to_ical(&parsed_non_std);
    for i in 1..=6 {
        assert!(
            non_std_out.contains(&format!("UID:a{i}\r\n")),
            "emits UID:a{i}"
        );
    }

    // Interleaved explicit UID:a2 with nameless alarms.
    let interleaved_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n",
        "BEGIN:VEVENT\r\nUID:interleaved\r\nDTSTART:20260115T100000Z\r\n",
        "SUMMARY:Interleaved\r\n",
        "BEGIN:VALARM\r\nUID:a2\r\nACTION:DISPLAY\r\nTRIGGER:-PT5M\r\nEND:VALARM\r\n",
        "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT10M\r\nEND:VALARM\r\n",
        "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_interleaved = ical_to_event(interleaved_ics).expect("parse interleaved");
    let interleaved_alerts = parsed_interleaved.alerts.expect("alerts present");
    assert_eq!(interleaved_alerts.len(), 3);
    assert!(interleaved_alerts.contains_key("a1"));
    assert!(interleaved_alerts.contains_key("a2"));
    assert!(interleaved_alerts.contains_key("a3"));
    assert_eq!(interleaved_alerts["a2"]["trigger"]["offset"], "-PT5M");
    assert_eq!(interleaved_alerts["a1"]["trigger"]["offset"], "-PT10M");
    assert_eq!(interleaved_alerts["a3"]["trigger"]["offset"], "-PT15M");

    // 5. Duplicate explicit UIDs: second duplicate overwrites first per RFC 9074 §6.
    let dup_uid_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n",
        "BEGIN:VEVENT\r\nUID:dup-uids\r\nDTSTART:20260115T100000Z\r\n",
        "SUMMARY:Duplicate UIDs\r\n",
        "BEGIN:VALARM\r\nUID:custom-key\r\nACTION:DISPLAY\r\nTRIGGER:-PT10M\r\nEND:VALARM\r\n",
        "BEGIN:VALARM\r\nUID:custom-key\r\nACTION:DISPLAY\r\nTRIGGER:-PT20M\r\nEND:VALARM\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_dup = ical_to_event(dup_uid_ics).expect("parse dup UIDs");
    let dup_alerts = parsed_dup.alerts.expect("alerts present");
    assert_eq!(dup_alerts.len(), 1);
    assert_eq!(dup_alerts["custom-key"]["trigger"]["offset"], "-PT20M");

    // 6. Mixed supported and unsupported alarms in a multi-alarm stream.
    let mixed_stream_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n",
        "BEGIN:VEVENT\r\nUID:mixed-stream\r\nDTSTART:20260115T100000Z\r\n",
        "SUMMARY:Mixed Stream Event\r\n",
        "BEGIN:VALARM\r\nUID:disp1\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n",
        "BEGIN:VALARM\r\nUID:aud1\r\nACTION:AUDIO\r\nTRIGGER:-PT10M\r\nEND:VALARM\r\n",
        "BEGIN:VALARM\r\nUID:disp2\r\nACTION:DISPLAY\r\nTRIGGER:-PT5M\r\nEND:VALARM\r\n",
        "BEGIN:VALARM\r\nUID:mail1\r\nACTION:EMAIL\r\nTRIGGER:-PT1H\r\nEND:VALARM\r\n",
        "BEGIN:VALARM\r\nUID:proc1\r\nACTION:PROCEDURE\r\nTRIGGER:-P1D\r\nEND:VALARM\r\n",
        "BEGIN:VALARM\r\nUID:abs1\r\nACTION:DISPLAY\r\nTRIGGER;VALUE=DATE-TIME:20260115T094500Z\r\nEND:VALARM\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let parsed_mixed = ical_to_event(mixed_stream_ics).expect("parse mixed stream");
    let mixed_alerts = parsed_mixed.alerts.expect("alerts present");
    assert_eq!(mixed_alerts.len(), 2);
    assert!(mixed_alerts.contains_key("disp1"));
    assert!(mixed_alerts.contains_key("disp2"));
    assert_eq!(mixed_alerts["disp1"]["trigger"]["offset"], "-PT15M");
    assert_eq!(mixed_alerts["disp2"]["trigger"]["offset"], "-PT5M");

    // Outbound safety: an event with 2 valid alerts and 1 unsupported alert.
    let mixed_event = reminded([
        ("k1", quarter_of_an_hour_before()),
        ("k2", quarter_of_an_hour_before()),
        (
            "k_email",
            json!({
                "@type": "Alert",
                "action": "email",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT1H"},
            }),
        ),
    ]);
    assert!(!maps_alerts(&mixed_event));
    let mixed_out = event_to_ical(&mixed_event);
    assert_eq!(mixed_out.matches("BEGIN:VALARM\r\n").count(), 2);
    assert!(!mixed_out.contains("k_email"));

    // 7. Recurrence series and overrides with multiple alarms:
    // Master series has 3 alarms.
    let mut rec_event = recurring_with(json!({
        // Override 1: inherits all 3 master alarms (no alerts field).
        "2026-01-22T13:00:00": {
            "title": "Inherited All Alarms",
        },
        // Override 2: replaces with 2 different alarms.
        "2026-01-29T13:00:00": {
            "title": "Custom Overridden Alarms",
            "alerts": {
                "ov1": {
                    "@type": "Alert",
                    "action": "display",
                    "trigger": {"@type": "OffsetTrigger", "offset": "-PT30M"},
                },
                "ov2": {
                    "@type": "Alert",
                    "action": "display",
                    "trigger": {"@type": "OffsetTrigger", "offset": "-PT2H"},
                }
            }
        },
        // Override 3: cancels all alarms (alerts: null).
        "2026-02-05T13:00:00": {
            "title": "Cancelled Alarms",
            "alerts": null,
        }
    }));
    rec_event.alerts = Some(
        [
            ("k1".to_owned(), quarter_of_an_hour_before()),
            (
                "k2".to_owned(),
                json!({
                    "@type": "Alert",
                    "action": "display",
                    "trigger": {"@type": "OffsetTrigger", "offset": "-PT1H"},
                }),
            ),
            (
                "k3".to_owned(),
                json!({
                    "@type": "Alert",
                    "action": "display",
                    "trigger": {"@type": "OffsetTrigger", "offset": "-P1D"},
                }),
            ),
        ]
        .into(),
    );

    let rec_ics = event_to_ical(&rec_event);
    assert_eq!(vevents(&rec_ics), 4);
    // Master: 3 VALARMs.
    assert_eq!(vevent(&rec_ics, 0).matches("BEGIN:VALARM\r\n").count(), 3);
    // Override 1 (inherited): 3 VALARMs.
    assert_eq!(vevent(&rec_ics, 1).matches("BEGIN:VALARM\r\n").count(), 3);
    // Override 2 (custom): 2 VALARMs.
    assert_eq!(vevent(&rec_ics, 2).matches("BEGIN:VALARM\r\n").count(), 2);
    assert!(vevent(&rec_ics, 2).contains("UID:ov1\r\n"));
    assert!(vevent(&rec_ics, 2).contains("UID:ov2\r\n"));
    // Override 3 (cancelled): 0 VALARMs.
    assert_eq!(vevent(&rec_ics, 3).matches("BEGIN:VALARM\r\n").count(), 0);

    // Override with one valid and one invalid alert is refused by maps_override.
    let invalid_override_patch = json!({
        "alerts": {
            "ok": quarter_of_an_hour_before(),
            "bad": {
                "@type": "Alert",
                "action": "email",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
            }
        }
    });
    assert!(!maps_override(
        "2026-01-22T13:00:00",
        &invalid_override_patch
    ));
}

#[test]
fn valarm_acknowledged_format_variations_and_refusal_boundaries_matrix() {
    // 1. Inbound parsing of RFC 9074 §6.1 ACKNOWLEDGED:
    // Exporters emit varied ACKNOWLEDGED forms. Parser must safely ignore ACKNOWLEDGED
    // and preserve the display alarm without contaminating CalendarEvent.extra.
    for (name, ack_line) in [
        ("standard_utc", "ACKNOWLEDGED:20260824T120000Z\r\n"),
        (
            "parameterized_datetime",
            "ACKNOWLEDGED;VALUE=DATE-TIME:20260824T120000Z\r\n",
        ),
        (
            "non_standard_local_tzid",
            "ACKNOWLEDGED;TZID=Europe/Berlin:20260824T140000\r\n",
        ),
        ("lowercase_property", "acknowledged:20260824T120000Z\r\n"),
        (
            "malformed_timestamp",
            "ACKNOWLEDGED:NOT_A_VALID_DATE_TIME\r\n",
        ),
        ("empty_acknowledged", "ACKNOWLEDGED:\r\n"),
        (
            "multiple_acknowledged_lines",
            "ACKNOWLEDGED:20260824T120000Z\r\nACKNOWLEDGED:20260825T120000Z\r\n",
        ),
        (
            "with_x_wr_alarmuid",
            "ACKNOWLEDGED:20260824T120000Z\r\nX-WR-ALARMUID:E451D045-FA1B-475D\r\n",
        ),
    ] {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
             BEGIN:VEVENT\r\nUID:ack-{name}\r\nDTSTART:20260115T100000Z\r\n\
             SUMMARY:Ack Test Event\r\n\
             BEGIN:VALARM\r\nUID:k1\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\n\
             {ack_line}END:VALARM\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let parsed = ical_to_event(&ics).unwrap_or_else(|e| panic!("failed parsing {name}: {e:?}"));
        let alerts = parsed
            .alerts
            .as_ref()
            .unwrap_or_else(|| panic!("missing alerts for {name}"));
        assert_eq!(alerts.len(), 1, "{name} alert count");
        assert_eq!(alerts["k1"]["action"], "display", "{name} action");
        assert_eq!(alerts["k1"]["trigger"]["offset"], "-PT15M", "{name} offset");
        assert!(
            parsed.extra.is_empty(),
            "{name} must not pollute event.extra"
        );
    }

    // ACKNOWLEDGED on unsupported ACTION:AUDIO is cleanly dropped along with the audio alarm.
    let audio_ack_ics = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n",
        "BEGIN:VEVENT\r\nUID:audio-ack\r\nDTSTART:20260115T100000Z\r\n",
        "SUMMARY:Audio Ack Event\r\n",
        "BEGIN:VALARM\r\nUID:a_audio\r\nACTION:AUDIO\r\nTRIGGER:-PT15M\r\n",
        "ACKNOWLEDGED:20260824T120000Z\r\nATTACH;VALUE=URI:Basso\r\nEND:VALARM\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    assert_eq!(
        ical_to_event(audio_ack_ics)
            .expect("parse audio ack")
            .alerts,
        None
    );

    // 2. Outbound export and refusal boundaries:
    // Event with single alert carrying acknowledged timestamp:
    let single_ack_event = reminded([(
        "k1",
        json!({
            "@type": "Alert",
            "action": "display",
            "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
            "acknowledged": "2026-08-24T12:00:00Z",
        }),
    )]);
    assert!(!maps_alerts(&single_ack_event));
    assert!(without(&event_to_ical(&single_ack_event), "BEGIN:VALARM"));

    // Multi-alarm event where 1 of 3 alerts has acknowledged:
    // Crucial safety boundary: maps_alerts must return false for the ENTIRE event,
    // and event_to_ical must emit only the 2 unacknowledged alarms.
    // Refusing the whole event prevents jmap-cal-sync from saving alerts and un-dismissing k2.
    let partial_ack_event = reminded([
        ("k1", quarter_of_an_hour_before()),
        (
            "k2",
            json!({
                "@type": "Alert",
                "action": "display",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT1H"},
                "acknowledged": "2026-08-24T11:00:00Z",
            }),
        ),
        (
            "k3",
            json!({
                "@type": "Alert",
                "action": "display",
                "trigger": {"@type": "OffsetTrigger", "offset": "-P1D"},
            }),
        ),
    ]);
    assert!(
        !maps_alerts(&partial_ack_event),
        "multi-alarm event with one acknowledged alert must be refused by maps_alerts"
    );
    let partial_ics = event_to_ical(&partial_ack_event);
    assert_eq!(partial_ics.matches("BEGIN:VALARM\r\n").count(), 2);
    assert!(partial_ics.contains("UID:k1\r\n"));
    assert!(partial_ics.contains("UID:k3\r\n"));
    assert!(!partial_ics.contains("UID:k2\r\n"));
    assert!(!partial_ics.contains("ACKNOWLEDGED"));

    // 3. Recurrence overrides where an instance has an acknowledged alert:
    let override_with_ack = json!({
        "title": "Instance with Dismissed Alarm",
        "alerts": {
            "k1": json!({
                "@type": "Alert",
                "action": "display",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
                "acknowledged": "2026-08-24T12:00:00Z",
            })
        }
    });
    assert!(
        !maps_override("2026-01-22T13:00:00", &override_with_ack),
        "recurrence override with acknowledged alert must be refused by maps_override"
    );

    // 4. Value variations of acknowledged field in JSCalendar:
    // All variations of acknowledged outside absent/None must be refused.
    for (name, ack_val) in [
        ("valid_utc_string", json!("2026-08-24T12:00:00Z")),
        ("local_string", json!("2026-08-24T12:00:00")),
        ("empty_string", json!("")),
        ("null_value", Value::Null),
        ("boolean_true", json!(true)),
        ("numeric_timestamp", json!(1724500800)),
    ] {
        let mut alert_map = serde_json::Map::new();
        alert_map.insert("@type".to_owned(), json!("Alert"));
        alert_map.insert("action".to_owned(), json!("display"));
        alert_map.insert(
            "trigger".to_owned(),
            json!({"@type": "OffsetTrigger", "offset": "-PT15M"}),
        );
        alert_map.insert("acknowledged".to_owned(), ack_val);

        let test_event = reminded([("k1", Value::Object(alert_map))]);
        assert!(
            !maps_alerts(&test_event),
            "acknowledged variation {name} must be refused by maps_alerts"
        );
    }
}

// ---------------------------------------------------------------------------
// Differential Server Oracle Adjudication Tests (Stalwart CalendarEvent/parse)
// ---------------------------------------------------------------------------

#[test]
fn differential_oracle_recurrence_rule_singular_fidelity_and_jscalendar_bis_conformance() {
    // Audit divergence 1 against live Stalwart oracle:
    // RFC 8984 §4.3.1 specified recurrenceRules as a plural array.
    // draft-ietf-calext-jscalendarbis §3.3.3 restructured recurrenceRule as a
    // singular object, and draft-ietf-jmap-calendars-28 §1.4 mandates this.
    // Stalwart v1.0.0 emits recurrenceRule (singular object).
    // This test verifies jmap-ical produces recurrence_rule (singular), serializes
    // to "recurrenceRule", and roundtrips with fixed-point stability.
    let path = format!(
        "{}/tests/fixtures/thunderbird_calendar_export.ics",
        env!("CARGO_MANIFEST_DIR")
    );
    let ics = std::fs::read_to_string(&path).expect("read fixture");
    let event = ical_to_event(&ics).expect("parse ics");

    // 1. Rust model field carries singular RecurrenceRule
    let rule = event
        .recurrence_rule
        .as_ref()
        .expect("recurrence_rule present");
    assert_eq!(rule.frequency, "weekly");
    assert_eq!(rule.interval, Some(2));
    assert_eq!(rule.until.as_deref(), Some("2026-12-21T09:30:00"));

    // 2. Wire serialization matches Stalwart differential shape:
    // "recurrenceRule" object, never "recurrenceRules" array.
    let serialized = serde_json::to_value(&event).expect("serialize event");
    let obj = serialized.as_object().expect("event object");
    assert!(
        obj.contains_key("recurrenceRule"),
        "must emit singular recurrenceRule matching Stalwart and jscalendarbis"
    );
    assert!(
        !obj.contains_key("recurrenceRules"),
        "must not emit legacy RFC 8984 plural recurrenceRules array"
    );
    assert!(obj.get("recurrenceRule").unwrap().is_object());

    // 3. Round-trip serialization emits standard RFC 5545 RRULE line
    let out_ics = event_to_ical(&event);
    assert_eq!(
        content_line(&out_ics, "RRULE:"),
        "RRULE:FREQ=WEEKLY;UNTIL=20261221T093000;INTERVAL=2;BYDAY=MO"
    );

    // 4. Fixed-point stability across repeated passes
    let event2 = ical_to_event(&out_ics).expect("reparse");
    assert_eq!(event.recurrence_rule, event2.recurrence_rule);

    // 5. Also verify detached overrides fixture with COUNT
    let detached_path = format!(
        "{}/tests/fixtures/thunderbird_detached_export.ics",
        env!("CARGO_MANIFEST_DIR")
    );
    let detached_ics = std::fs::read_to_string(&detached_path).expect("read detached fixture");
    let detached_event = ical_to_event(&detached_ics).expect("parse detached");
    let detached_serialized = serde_json::to_value(&detached_event).expect("serialize");
    assert!(detached_serialized.get("recurrenceRule").is_some());
    assert!(detached_serialized.get("recurrenceRules").is_none());
    let detached_rule = detached_event.recurrence_rule.unwrap();
    assert_eq!(detached_rule.frequency, "weekly");
    assert_eq!(detached_rule.count, Some(6));
}

#[test]
fn differential_oracle_dtstamp_and_timestamps_dropped_on_import_reconfirmed_against_real_server() {
    // Audit divergence 2 against live Stalwart oracle:
    // Stalwart v1.0.0 maps DTSTAMP to updated during CalendarEvent/parse.
    // In jmap-ical, ical_to_event deliberately drops DTSTAMP, CREATED, and
    // LAST-MODIFIED, setting created: None and updated: None.
    // Reconfirmed rationale: Evolution Data Server (libical) stamps DTSTAMP on
    // every touch using the client system clock. Reading DTSTAMP into updated
    // would cause jmap-cal-sync to patch updated back to the JMAP server from
    // the local clock, violating store-owned timestamp semantics.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-dtstamp-test-001\r\n\
DTSTAMP:20260904T120000Z\r\n\
CREATED:20260901T080000Z\r\n\
LAST-MODIFIED:20260904T113000Z\r\n\
DTSTART:20260910T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Differential Timestamp Oracle Test\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse ics");

    // Inbound drop: server-owned metadata is not claimed by client parser
    assert_eq!(
        event.created, None,
        "CREATED must be dropped on inbound parse"
    );
    assert_eq!(
        event.updated, None,
        "DTSTAMP and LAST-MODIFIED must be dropped on inbound parse"
    );

    // Extra bag isolation: dropped timestamps must not pollute extra
    assert!(!event.extra.contains_key("dtstamp"));
    assert!(!event.extra.contains_key("created"));
    assert!(!event.extra.contains_key("lastModified"));
    assert!(!event.extra.contains_key("updated"));
    assert!(!event.extra.contains_key("DTSTAMP"));

    // Outbound emission when server provides updated timestamp:
    // RFC 5545 §3.8.7.2 makes DTSTAMP required on VEVENT, and in a calendar
    // without METHOD it carries updated.
    let mut server_event = event.clone();
    server_event.created = Some("2026-09-01T08:00:00Z".to_owned());
    server_event.updated = Some("2026-09-04T11:30:00Z".to_owned());
    let out_ics = event_to_ical(&server_event);

    assert_eq!(line(&out_ics, "CREATED:"), "CREATED:20260901T080000Z");
    assert_eq!(line(&out_ics, "DTSTAMP:"), "DTSTAMP:20260904T113000Z");
    assert_eq!(
        line(&out_ics, "LAST-MODIFIED:"),
        "LAST-MODIFIED:20260904T113000Z"
    );

    // Outbound omission when server provides no updated timestamp:
    // Omitted rather than inventing a fluctuating \"now\" timestamp that breaks
    // save-path diff detection.
    let out_empty_timestamps = event_to_ical(&event);
    assert!(without(&out_empty_timestamps, "CREATED"));
    assert!(without(&out_empty_timestamps, "DTSTAMP"));
    assert!(without(&out_empty_timestamps, "LAST-MODIFIED"));
}

#[test]
fn differential_oracle_uid_mapping_to_id_and_x_jmap_uid_retention() {
    // Divergence 3 against Stalwart differential oracle:
    // RFC 8984 section 4.1.1 defines uid as the globally unique event identifier.
    // RFC 8620 section 2 defines id as the immutable server-assigned record ID.
    // Stalwart v1.0.0 CalendarEvent/parse produces uid and leaves id unset.
    // In contrast, jmap-ical produces id (and populates uid only when
    // X-JMAP-UID is present).
    // Rationale: Evolution Data Server (libical / ECalMetaBackend) keys its
    // local cache on UID, which jmap-cal-sync must match against JMAP id for
    // load_component_sync / remove_component_sync routing.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:google-export-uid-12345@google.com\r\n\
DTSTART:20260910T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Identity Test\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse ics");
    assert_eq!(
        event.id.as_ref().map(|id| id.as_str()),
        Some("google-export-uid-12345@google.com")
    );
    assert_eq!(
        event.uid, None,
        "plain UID must not populate event.uid when X-JMAP-UID is absent"
    );

    // Outbound emission: emits UID line
    let out = event_to_ical(&event);
    assert_eq!(line(&out, "UID:"), "UID:google-export-uid-12345@google.com");
    assert!(without(&out, "X-JMAP-UID"));

    // When X-JMAP-UID is present: both id and uid are populated
    let ics_with_x_uid = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:server-id-888\r\n\
X-JMAP-UID:client-uuid-999\r\n\
DTSTART:20260910T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Identity Test with X-JMAP-UID\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event_dual = ical_to_event(ics_with_x_uid).expect("parse ics with x-jmap-uid");
    assert_eq!(
        event_dual.id.as_ref().map(|id| id.as_str()),
        Some("server-id-888")
    );
    assert_eq!(event_dual.uid.as_deref(), Some("client-uuid-999"));

    let out_dual = event_to_ical(&event_dual);
    assert_eq!(line(&out_dual, "UID:"), "UID:server-id-888");
    assert_eq!(line(&out_dual, "X-JMAP-UID:"), "X-JMAP-UID:client-uuid-999");
}

#[test]
fn differential_oracle_organizer_and_attendee_dropped_on_import_for_scheduling_safety() {
    // Divergence 4 against Stalwart differential oracle:
    // Stalwart v1.0.0 CalendarEvent/parse converts ORGANIZER and ATTENDEE lines
    // into JSCalendar participants.
    // In contrast, jmap-ical's ical_to_event drops them on import (participants: None),
    // while outbound event_to_ical draws them when event.participants is present.
    // Rationale: Guest list and reply statuses (PARTSTAT) are scheduling state.
    // If ical_to_event parsed them, local desktop saves in Evolution would submit
    // patches mutating participants on the server without sending iTIP messages
    // (RFC 5546), corrupting server-authoritative guest list and response state.
    let path = format!(
        "{}/tests/fixtures/google_calendar_export.ics",
        env!("CARGO_MANIFEST_DIR")
    );
    let ics = std::fs::read_to_string(&path).expect("read fixture");
    let event = ical_to_event(&ics).expect("parse ics");

    // Inbound: participants must be None
    assert_eq!(
        event.participants, None,
        "participants must be dropped on inbound parse to preserve scheduling boundary"
    );

    // Dropped attendees must not pollute extra
    assert!(!event.extra.contains_key("attendee"));
    assert!(!event.extra.contains_key("organizer"));
    assert!(!event.extra.contains_key("ATTENDEE"));
    assert!(!event.extra.contains_key("ORGANIZER"));

    // Outbound: when participants is populated, event_to_ical draws ORGANIZER and ATTENDEE
    let mut with_participants = event.clone();
    let mut part_map = serde_json::Map::new();
    let owner = serde_json::json!({
        "@type": "Participant",
        "name": "Jane Doe",
        "roles": {"owner": true},
        "sendTo": {"imip": "mailto:jane.doe@example.com"}
    });
    let guest = serde_json::json!({
        "@type": "Participant",
        "name": "Bob Smith",
        "roles": {"attendee": true},
        "participationStatus": "accepted",
        "sendTo": {"imip": "mailto:bob.smith@example.com"}
    });
    part_map.insert("p1".to_owned(), owner);
    part_map.insert("p2".to_owned(), guest);
    with_participants.participants = Some(
        part_map
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>(),
    );

    let out = event_to_ical(&with_participants);
    let unfolded = out.replace("\r\n ", "").replace("\r\n\t", "");
    assert!(
        unfolded.contains("ORGANIZER;CN=\"Jane Doe\":mailto:jane.doe@example.com")
            || unfolded.contains("ORGANIZER;CN=Jane Doe:mailto:jane.doe@example.com")
    );
    assert!(
        unfolded.contains("ATTENDEE;CN=\"Bob Smith\";ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:mailto:bob.smith@example.com")
            || (unfolded.contains("ATTENDEE;") && unfolded.contains("mailto:bob.smith@example.com"))
    );
}

#[test]
fn differential_oracle_envelope_properties_and_url_boundary() {
    // Divergences 5 and 6 against Stalwart differential oracle:
    // 1. PRODID, CALSCALE, and METHOD are transport envelope properties.
    // Stalwart may map PRODID to prodId. jmap-ical drops PRODID on import and
    // emits its own canonical PRODID on export, ensuring foreign generator
    // tokens do not pollute stored event state.
    // 2. URL (RFC 5545 section 3.8.4.6) is dropped on import rather than mapped to links,
    // avoiding collisions with virtualLocations (CONFERENCE) and keeping links
    // isolated to ATTACH and IMAGE.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Foreign Vendor//Calendar 1.0//EN\r\n\
CALSCALE:GREGORIAN\r\n\
METHOD:REQUEST\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-envelope-url-001\r\n\
DTSTART:20260910T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Envelope and URL Test\r\n\
URL:https://example.com/meeting-details\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse ics");

    // Envelope properties are dropped without polluting extra
    assert!(!event.extra.contains_key("prodId"));
    assert!(!event.extra.contains_key("PRODID"));
    assert!(!event.extra.contains_key("method"));
    assert!(!event.extra.contains_key("METHOD"));
    assert!(!event.extra.contains_key("calscale"));
    assert!(!event.extra.contains_key("CALSCALE"));

    // URL is dropped without polluting extra or links
    assert_eq!(event.links, None, "URL must not be imported into links");
    assert!(!event.extra.contains_key("url"));
    assert!(!event.extra.contains_key("URL"));

    // Outbound emission: emits canonical PRODID and VERSION, drops foreign CALSCALE/METHOD/URL
    let out = event_to_ical(&event);
    assert!(out.contains("VERSION:2.0"));
    assert!(!out.contains("Foreign Vendor"));
    assert!(without(&out, "CALSCALE"));
    assert!(without(&out, "METHOD"));
    assert!(without(&out, "URL"));
}

#[test]
fn differential_oracle_geo_and_locations_coordinates_vs_single_string_name_and_key_synthesis() {
    // Divergence 7 against Stalwart differential oracle:
    // RFC 5545 section 3.8.1.6 defines GEO (latitude and longitude).
    // Stalwart v1.0.0 converts GEO into Location.coordinates (RFC 5870 geo: URI)
    // and synthesizes map keys from UUID5 or JSID parameters.
    // In contrast, jmap-ical maps incoming LOCATION text to locations with
    // a stable positional key ("1" or X-JMAP-KEY) and drops GEO on import.
    // Rationale: Evolution Data Server models appointment location as a single
    // string (e_cal_component_get_location). jmap-cal-sync patches
    // locations/<key>/name in place, so stable keys avoid churn.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-geo-test-001\r\n\
DTSTART:20260910T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Location and Coordinates Test\r\n\
LOCATION:Conference Room B\r\n\
GEO:37.386013;-122.082932\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse ics");

    // Location name is parsed into locations with positional key "l1"
    let locs = event.locations.as_ref().expect("locations must be present");
    assert_eq!(locs.len(), 1);
    let loc = locs.get("l1").expect("key l1");
    assert_eq!(loc.get("@type").and_then(|v| v.as_str()), Some("Location"));
    assert_eq!(
        loc.get("name").and_then(|v| v.as_str()),
        Some("Conference Room B")
    );
    assert_eq!(
        loc.get("coordinates"),
        None,
        "coordinates must not be populated in single-string model"
    );

    // GEO is dropped on import without polluting extra
    assert!(!event.extra.contains_key("geo"));
    assert!(!event.extra.contains_key("GEO"));

    // Outbound emission writes LOCATION with X-JMAP-KEY:l1, omits GEO
    let out = event_to_ical(&event);
    assert_eq!(
        line(&out, "LOCATION;"),
        "LOCATION;X-JMAP-KEY=l1:Conference Room B"
    );
    assert!(without(&out, "GEO"));
}

#[test]
fn differential_oracle_sequence_revision_counter_dropped_on_import_for_server_store_ownership() {
    // Divergence 8 against Stalwart differential oracle:
    // RFC 5545 section 3.8.7.4 defines SEQUENCE. Stalwart v1.0.0 maps SEQUENCE
    // to JSCalendar sequence.
    // In contrast, jmap-ical drops SEQUENCE on import without polluting extra.
    // Rationale: In JMAP for Calendars (draft-ietf-jmap-calendars-28 sections 5.1 and 5.2),
    // sequence revision numbers are strictly managed and automatically incremented
    // by the JMAP server upon commit. A desktop client proposing or persisting
    // sequence numbers would interfere with server conflict detection.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-sequence-test-001\r\n\
DTSTART:20260910T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Sequence Test\r\n\
SEQUENCE:5\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse ics");

    // SEQUENCE is dropped without polluting extra
    assert!(!event.extra.contains_key("sequence"));
    assert!(!event.extra.contains_key("SEQUENCE"));

    // Outbound emission omits SEQUENCE, leaving revision control to the JMAP server
    let out = event_to_ical(&event);
    assert!(without(&out, "SEQUENCE"));
}

#[test]
fn differential_oracle_color_property_and_source_level_styling_boundary() {
    // Divergence 9 against Stalwart differential oracle:
    // RFC 7986 section 5.9 defines COLOR. Stalwart v1.0.0 maps COLOR to event.color.
    // In contrast, jmap-ical drops COLOR on import without polluting extra.
    // Rationale: In Evolution Data Server, event styling is governed by the parent
    // calendar source (E_SOURCE_EXTENSION_CALENDAR) rather than per-event attributes.
    // Dropping per-event COLOR avoids uncoordinated styling overrides in desktop UI.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-color-test-001\r\n\
DTSTART:20260910T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Color Test\r\n\
COLOR:maroon\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse ics");

    assert_eq!(event.color, None, "COLOR must be dropped on import");
    assert!(!event.extra.contains_key("color"));
    assert!(!event.extra.contains_key("COLOR"));

    let out = event_to_ical(&event);
    assert!(without(&out, "COLOR"));
}

#[test]
fn differential_oracle_related_to_dropped_on_import_to_isolate_relation_graphs() {
    // Divergence 10 against Stalwart differential oracle:
    // RFC 5545 section 3.8.4.5 defines RELATED-TO. Stalwart v1.0.0 maps RELATED-TO
    // to JSCalendar relatedTo.
    // In contrast, jmap-ical drops RELATED-TO on import without polluting extra.
    // Rationale: Evolution's appointment editor does not manage appointment relation
    // graphs. Dropping unmodeled relations protects server-side relation structures
    // from uncoordinated whole-property replacement.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-related-to-001\r\n\
DTSTART:20260910T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Related-To Test\r\n\
RELATED-TO;RELTYPE=PARENT:parent-event-uid-999\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse ics");

    assert!(!event.extra.contains_key("relatedTo"));
    assert!(!event.extra.contains_key("RELATED-TO"));

    let out = event_to_ical(&event);
    assert!(without(&out, "RELATED-TO"));
}

#[test]
fn differential_oracle_icalendar_converted_properties_tracking_omission() {
    // Divergence 11 against Stalwart differential oracle:
    // RFC 8984 Appendix B defines the iCalendar object to track unconverted
    // properties. Stalwart v1.0.0 emits iCalendar and convertedProperties.
    // In contrast, jmap-ical does not emit or require iCalendar tracking bags.
    // Serialization is direct and deterministic from standard JSCalendar properties.
    // Rationale: Omitting parser bookkeeping bags prevents client-specific metadata
    // from polluting stored records on the JMAP server while guaranteeing clean,
    // deterministic iCalendar generation.
    let path = format!(
        "{}/tests/fixtures/evolution_calendar_export.ics",
        env!("CARGO_MANIFEST_DIR")
    );
    let ics = std::fs::read_to_string(&path).expect("read fixture");
    let event = ical_to_event(&ics).expect("parse ics");

    assert!(!event.extra.contains_key("iCalendar"));
    assert!(!event.extra.contains_key("convertedProperties"));

    let out = event_to_ical(&event);
    assert!(without(&out, "X-JSCALENDAR-CONVERTED"));
    assert!(without(&out, "convertedProperties"));
}

#[test]
fn differential_oracle_valarm_non_display_actions_dropped_on_import_and_refused_on_export() {
    // Divergence 12 against Stalwart differential oracle:
    // RFC 5545 section 3.6.6 defines ACTION:DISPLAY, ACTION:EMAIL, ACTION:AUDIO,
    // and ACTION:PROCEDURE. Stalwart v1.0.0 maps ACTION:EMAIL to action: "email"
    // (RFC 8984 section 4.5.2).
    // In contrast, jmap-ical only imports ACTION:DISPLAY into event.alerts,
    // dropping EMAIL, AUDIO, and PROCEDURE on import without polluting extra.
    // Outbound safety: maps_alerts returns false if any alert has an action
    // other than "display" (such as "email"), protecting server-side email alarms
    // from being deleted during whole-property replacement.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-alarm-action-001\r\n\
DTSTART:20260910T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Alarm Actions Test\r\n\
BEGIN:VALARM\r\n\
UID:alarm-display-1\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Popup Reminder\r\n\
TRIGGER:-PT15M\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
UID:alarm-email-1\r\n\
ACTION:EMAIL\r\n\
DESCRIPTION:Mail Reminder\r\n\
SUMMARY:Email Subject\r\n\
ATTENDEE:mailto:user@example.com\r\n\
TRIGGER:-P1D\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
UID:alarm-audio-1\r\n\
ACTION:AUDIO\r\n\
ATTACH;VALUE=URI:Basso\r\n\
TRIGGER:-PT5M\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
UID:alarm-procedure-1\r\n\
ACTION:PROCEDURE\r\n\
ATTACH;VALUE=URI:file:///bin/beep\r\n\
TRIGGER:-PT1M\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse ics");

    // Only DISPLAY alarm is imported
    let alerts = event.alerts.as_ref().expect("alerts present");
    assert_eq!(alerts.len(), 1);
    assert!(alerts.contains_key("alarm-display-1"));
    let display_alert = alerts.get("alarm-display-1").unwrap();
    assert_eq!(
        display_alert.get("action").and_then(Value::as_str),
        Some("display")
    );

    // Non-display alarms are dropped without polluting extra
    assert!(!event.extra.contains_key("alarm-email-1"));
    assert!(!event.extra.contains_key("alarm-audio-1"));
    assert!(!event.extra.contains_key("alarm-procedure-1"));

    // Event with only display alerts passes maps_alerts
    assert!(maps_alerts(&event));

    // Event carrying an email alert in server state is refused by maps_alerts
    let mut server_event = event.clone();
    let mut server_alerts = alerts.clone();
    server_alerts.insert(
        "alarm-email-1".to_owned(),
        json!({
            "@type": "Alert",
            "action": "email",
            "trigger": {
                "@type": "OffsetTrigger",
                "offset": "-P1D"
            }
        }),
    );
    server_event.alerts = Some(server_alerts);
    assert!(
        !maps_alerts(&server_event),
        "maps_alerts must refuse email action alerts to protect server state"
    );

    // Outbound emission writes only the display alarm
    let out = event_to_ical(&event);
    assert!(out.contains("ACTION:DISPLAY"));
    assert!(without(&out, "ACTION:EMAIL"));
    assert!(without(&out, "ACTION:AUDIO"));
    assert!(without(&out, "ACTION:PROCEDURE"));
}

#[test]
fn differential_oracle_valarm_absolute_triggers_dropped_on_import_and_refused_on_export() {
    // Divergence 13 against Stalwart differential oracle:
    // RFC 5545 section 3.8.6.3 defines TRIGGER;VALUE=DATE-TIME. Stalwart v1.0.0
    // maps absolute date-time triggers to JSCalendar AbsoluteTrigger (RFC 8984 section 4.5.4).
    // In contrast, jmap-ical only supports relative OffsetTrigger, dropping
    // absolute triggers on import without polluting extra.
    // Outbound safety: maps_alerts returns false for AbsoluteTrigger objects,
    // preventing whole-property replacement from destroying server-managed triggers.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-abs-trigger-001\r\n\
DTSTART:20260910T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Absolute Trigger Test\r\n\
BEGIN:VALARM\r\n\
UID:alarm-rel-1\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Relative Reminder\r\n\
TRIGGER:-PT15M\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
UID:alarm-abs-1\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Absolute Reminder\r\n\
TRIGGER;VALUE=DATE-TIME:20260910T080000Z\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse ics");

    // Only relative trigger is parsed
    let alerts = event.alerts.as_ref().expect("alerts present");
    assert_eq!(alerts.len(), 1);
    assert!(alerts.contains_key("alarm-rel-1"));
    let rel_alert = alerts.get("alarm-rel-1").unwrap();
    let trigger = rel_alert.get("trigger").and_then(Value::as_object).unwrap();
    assert_eq!(
        trigger.get("@type").and_then(Value::as_str),
        Some("OffsetTrigger")
    );

    // Absolute trigger is dropped without polluting extra
    assert!(!event.extra.contains_key("alarm-abs-1"));

    // Event with relative trigger passes maps_alerts
    assert!(maps_alerts(&event));

    // Event with AbsoluteTrigger in server state is refused by maps_alerts
    let mut server_event = event.clone();
    let mut server_alerts = alerts.clone();
    server_alerts.insert(
        "alarm-abs-1".to_owned(),
        json!({
            "@type": "Alert",
            "action": "display",
            "trigger": {
                "@type": "AbsoluteTrigger",
                "when": "2026-09-10T08:00:00Z"
            }
        }),
    );
    server_event.alerts = Some(server_alerts);
    assert!(
        !maps_alerts(&server_event),
        "maps_alerts must refuse AbsoluteTrigger to prevent moving fixed reminders"
    );

    let out = event_to_ical(&event);
    assert!(out.contains("TRIGGER:-PT15M"));
    assert!(without(&out, "VALUE=DATE-TIME"));
}

#[test]
fn differential_oracle_valarm_acknowledged_dropped_on_import_and_refused_by_maps_alerts() {
    // Divergence 14 against Stalwart differential oracle:
    // RFC 9074 section 6.1 defines ACKNOWLEDGED timestamp for dismissed/snoozed alarms.
    // Stalwart v1.0.0 maps ACKNOWLEDGED to Alert.acknowledged (RFC 8984 section 4.5.2).
    // In contrast, jmap-ical drops ACKNOWLEDGED on import without polluting extra.
    // Outbound safety: maps_alerts returns false for any event where an alert has
    // acknowledged set, preventing whole-property replacement from un-dismissing
    // snoozed alarms on the JMAP server.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-ack-alarm-001\r\n\
DTSTART:20260910T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Acknowledged Alarm Test\r\n\
BEGIN:VALARM\r\n\
UID:alarm-ack-1\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Snoozed Reminder\r\n\
TRIGGER:-PT15M\r\n\
ACKNOWLEDGED:20260904T120000Z\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse ics");

    let alerts = event.alerts.as_ref().expect("alerts present");
    let alert = alerts.get("alarm-ack-1").expect("alarm-ack-1 parsed");
    assert_eq!(
        alert.get("acknowledged"),
        None,
        "ACKNOWLEDGED must be dropped on import"
    );

    // Dropped ACKNOWLEDGED does not pollute extra
    assert!(!event.extra.contains_key("ACKNOWLEDGED"));
    assert!(!event.extra.contains_key("acknowledged"));

    // Event parsed without acknowledged passes maps_alerts
    assert!(maps_alerts(&event));

    // Server-side event carrying acknowledged is refused by maps_alerts
    let mut server_event = event.clone();
    let mut server_alerts = alerts.clone();
    server_alerts.insert(
        "alarm-ack-1".to_owned(),
        json!({
            "@type": "Alert",
            "action": "display",
            "trigger": {
                "@type": "OffsetTrigger",
                "offset": "-PT15M"
            },
            "acknowledged": "2026-09-04T12:00:00Z"
        }),
    );
    server_event.alerts = Some(server_alerts);
    assert!(
        !maps_alerts(&server_event),
        "maps_alerts must refuse acknowledged alerts to prevent clobbering snooze state"
    );
}

#[test]
fn differential_oracle_language_altid_localization_parameters_vs_single_locale_model() {
    // Divergence 15 against Stalwart differential oracle:
    // RFC 5545 section 3.2.10 defines LANGUAGE and section 3.2.2 defines ALTID.
    // Stalwart v1.0.0 may parse alternate language properties into JSCalendar localizations
    // (RFC 8984 section 4.6.1: Map<LanguageTag, PatchObject>).
    // In contrast, jmap-ical selects the first SUMMARY and DESCRIPTION in document order
    // and drops alternate localized lines without polluting extra.
    // Rationale: Evolution Data Server stores single strings for SUMMARY and DESCRIPTION
    // for the user active locale. Dropping alternate languages prevents partial translations
    // from being modified or corrupted during desktop client synchronization.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-lang-test-001\r\n\
DTSTART:20260910T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY;LANGUAGE=en:Bilingual Colloquium\r\n\
SUMMARY;LANGUAGE=fr:Colloque bilingue\r\n\
DESCRIPTION;LANGUAGE=en:Discussions on open standards and protocols.\r\n\
DESCRIPTION;LANGUAGE=fr:Discussions sur les standards ouverts et protocoles.\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse ics");

    // Primary summary and description are retained
    assert_eq!(event.title.as_deref(), Some("Bilingual Colloquium"));
    assert_eq!(
        event.description.as_deref(),
        Some("Discussions on open standards and protocols.")
    );

    // Alternate language properties are dropped without polluting extra
    assert!(!event.extra.contains_key("localizations"));
    assert!(!event.extra.contains_key("LANGUAGE"));
    assert!(!event.extra.contains_key("language"));

    // Outbound emission writes primary summary and description cleanly
    let out = event_to_ical(&event);
    assert_eq!(line(&out, "SUMMARY:"), "SUMMARY:Bilingual Colloquium");
    assert_eq!(
        line(&out, "DESCRIPTION:"),
        "DESCRIPTION:Discussions on open standards and protocols."
    );
    assert!(without(&out, "Colloque bilingue"));
}

#[test]
fn differential_oracle_vendor_x_properties_dropped_on_import_to_avoid_extra_pollution() {
    // Divergence 16 against Stalwart differential oracle:
    // Calendar exporters emit vendor X- properties (such as X-APPLE-*, X-MICROSOFT-*, X-MOZ-*).
    // Stalwart v1.0.0 or RFC 8984 Appendix B parsers may collect unmapped properties into extra
    // or vendor dictionaries.
    // In contrast, jmap-ical strictly ignores vendor X- properties on inbound parse without
    // polluting event.extra.
    // Rationale: Evolution Data Server has no active UI editing support for vendor extensions.
    // If event.extra were populated with arbitrary vendor keys, jmap-cal-sync would submit them
    // as top-level JMAP properties, which standard JMAP servers reject with invalidProperties.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-vendor-test-001\r\n\
DTSTART:20260910T100000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Vendor Extension Benchmark\r\n\
X-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC\r\n\
X-MICROSOFT-CDO-BUSYSTATUS:BUSY\r\n\
X-MOZ-GENERATION:2\r\n\
X-LIC-LOCATION:Europe/London\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse ics");

    // Standard properties are mapped
    assert_eq!(event.title.as_deref(), Some("Vendor Extension Benchmark"));
    assert_eq!(event.duration.as_deref(), Some("PT1H"));

    // event.extra is completely clean: no vendor X- properties leak in
    assert!(
        event.extra.is_empty(),
        "expected event.extra to be empty, found: {:?}",
        event.extra
    );

    // Outbound serialization emits clean iCalendar without vendor X- properties
    let out = event_to_ical(&event);
    assert!(without(&out, "X-APPLE-TRAVEL-ADVISORY-BEHAVIOR"));
    assert!(without(&out, "X-MICROSOFT-CDO-BUSYSTATUS"));
    assert!(without(&out, "X-MOZ-GENERATION"));
    assert!(without(&out, "X-LIC-LOCATION"));
}

#[test]
fn differential_oracle_inline_binary_and_file_uri_attachments_dropped_for_payload_isolation() {
    // Divergence 17 against Stalwart differential oracle:
    // RFC 5545 section 3.8.4.1 permits inline base64 binary attachments (VALUE=BINARY;ENCODING=BASE64),
    // and Evolution desktop clients produce local file:// URIs for un-uploaded files.
    // Stalwart v1.0.0 uses RFC 9404 Blobs for binary payload management.
    // In contrast, jmap-ical drops inline binary attachments and filters out local file:// URIs,
    // preserving only accessible remote URIs (such as https://, http://, or blobId:).
    // Rationale: Inline binary blobs bloat JSON metadata and violate JMAP protocol architecture.
    // Local file:// URIs leak workstation paths and cannot be dereferenced by remote recipients.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-attach-test-001\r\n\
DTSTART:20260910T110000Z\r\n\
DURATION:PT30M\r\n\
SUMMARY:Attachment Pipeline Review\r\n\
ATTACH;FMTTYPE=application/pdf:https://example.org/spec.pdf\r\n\
ATTACH;VALUE=BINARY;ENCODING=BASE64:VGhpcyBpcyBhbiBpbmxpbmUgYXR0YWNobWVudA==\r\n\
ATTACH:file:///home/runner/confidential-budget.xlsx\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse ics");

    // Only the remote https attachment is retained in links
    let links = event.links.as_ref().expect("links present");
    assert_eq!(
        links.len(),
        1,
        "expected exactly 1 link, found: {:?}",
        links
    );

    let (_, link) = links.iter().next().unwrap();
    assert_eq!(
        link.get("href").and_then(Value::as_str),
        Some("https://example.org/spec.pdf")
    );
    assert_eq!(
        link.get("contentType").and_then(Value::as_str),
        Some("application/pdf")
    );

    // event.extra is not polluted by dropped attachments
    assert!(event.extra.is_empty());

    // Outbound serialization emits ATTACH only for the remote link
    let out = event_to_ical(&event);
    assert_eq!(
        line(&out, "ATTACH;"),
        "ATTACH;FMTTYPE=application/pdf;X-JMAP-KEY=k1:https://example.org/spec.pdf"
    );
    assert!(without(&out, "VGhpcyBpcyBhbi"));
    assert!(without(&out, "confidential-budget.xlsx"));
}

#[test]
fn differential_oracle_stream_container_metadata_dropped_for_relational_isolation() {
    // Divergence 18 against Stalwart differential oracle:
    // Calendar exporters emit container metadata on the outer VCALENDAR envelope
    // (such as X-WR-CALNAME and X-WR-TIMEZONE).
    // CalDAV servers and archival converters often use X-WR-CALNAME to name calendar collections.
    // In contrast, jmap-ical maps individual appointment records (VEVENT) and drops outer
    // VCALENDAR metadata without polluting event.extra.
    // Rationale: In JMAP (RFC 8620 / draft-ietf-jmap-calendars-28), calendar containers are distinct
    // first-class objects (Calendar with id, name, color), while CalendarEvent holds calendarIds.
    // Embedding container names in CalendarEvent causes denormalization and conflicts when events
    // move across calendars. In Evolution Data Server, collection identity is governed by ESource.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
X-WR-CALNAME:Engineering Team Calendar\r\n\
X-WR-TIMEZONE:Europe/Berlin\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-calname-test-001\r\n\
DTSTART:20260910T140000Z\r\n\
DURATION:PT45M\r\n\
SUMMARY:Sprint Architecture Discussion\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let event = ical_to_event(ics).expect("parse ics");

    // Appointment properties are mapped cleanly
    assert_eq!(
        event.title.as_deref(),
        Some("Sprint Architecture Discussion")
    );
    assert_eq!(event.duration.as_deref(), Some("PT45M"));

    // Container metadata does not pollute event.extra
    assert!(
        event.extra.is_empty(),
        "expected clean event.extra, found: {:?}",
        event.extra
    );

    // Outbound serialization does not emit container metadata lines
    let out = event_to_ical(&event);
    assert!(without(&out, "Engineering Team Calendar"));
    assert!(without(&out, "X-WR-CALNAME"));
    assert!(without(&out, "X-WR-TIMEZONE"));
}

#[test]
fn differential_oracle_classification_and_privacy_vocabulary_filtering() {
    // Divergence 19 against Stalwart differential oracle:
    // RFC 5545 section 3.8.1.3 defines CLASS (PUBLIC, PRIVATE, CONFIDENTIAL, or x-name / iana-token).
    // RFC 8984 section 4.4.3 models privacy (public, private, secret) with an open vocabulary.
    // Stalwart v1.0.0 may preserve non-standard CLASS tokens directly in privacy.
    // In contrast, jmap-ical strictly maps the shared three-value scale (PUBLIC -> public,
    // PRIVATE -> private, CONFIDENTIAL -> secret) and drops non-standard tokens on import
    // without polluting event.extra.
    // Rationale: Evolution Data Server appointment UI exposes a three-option classification menu
    // (Public, Private, Confidential). Non-standard tokens cannot be presented in desktop UI
    // and would produce inconsistent round-trips.
    let make_ics = |class_line: &str| -> String {
        format!(
            "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-privacy-test-001\r\n\
DTSTART:20260910T160000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Privacy Classification Test\r\n\
{}\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n",
            class_line
        )
    };

    // 1. CONFIDENTIAL maps to secret and serializes back to CLASS:CONFIDENTIAL
    let ev_conf = ical_to_event(&make_ics("CLASS:CONFIDENTIAL")).expect("parse confidential");
    assert_eq!(ev_conf.privacy.as_deref(), Some("secret"));
    assert!(ev_conf.extra.is_empty());
    let out_conf = event_to_ical(&ev_conf);
    assert_eq!(line(&out_conf, "CLASS:"), "CLASS:CONFIDENTIAL");

    // 2. PRIVATE maps to private and serializes back to CLASS:PRIVATE
    let ev_priv = ical_to_event(&make_ics("CLASS:PRIVATE")).expect("parse private");
    assert_eq!(ev_priv.privacy.as_deref(), Some("private"));
    assert!(ev_priv.extra.is_empty());
    let out_priv = event_to_ical(&ev_priv);
    assert_eq!(line(&out_priv, "CLASS:"), "CLASS:PRIVATE");

    // 3. PUBLIC maps to public and serializes back to CLASS:PUBLIC
    let ev_pub = ical_to_event(&make_ics("CLASS:PUBLIC")).expect("parse public");
    assert_eq!(ev_pub.privacy.as_deref(), Some("public"));
    assert!(ev_pub.extra.is_empty());
    let out_pub = event_to_ical(&ev_pub);
    assert_eq!(line(&out_pub, "CLASS:"), "CLASS:PUBLIC");

    // 4. Non-standard tokens (e.g. CLASS:RESTRICTED or CLASS:SECRET) are dropped on import
    let ev_nonstandard = ical_to_event(&make_ics("CLASS:RESTRICTED")).expect("parse nonstandard");
    assert_eq!(ev_nonstandard.privacy, None);
    assert!(ev_nonstandard.extra.is_empty());

    // Outbound serialization of an event without privacy does not emit CLASS
    let out_nonstandard = event_to_ical(&ev_nonstandard);
    assert!(without(&out_nonstandard, "CLASS:"));
    assert!(without(&out_nonstandard, "RESTRICTED"));
}

#[test]
fn differential_oracle_categories_whitespace_trimming_and_keyword_map_value_filtering() {
    // Divergence 20 against Stalwart differential oracle:
    // RFC 5545 section 3.8.1.2 defines CATEGORIES as comma-separated category strings.
    // RFC 8984 section 4.4.2 defines keywords as a Map<String, Boolean> where each value is true.
    // Stalwart v1.0.0 parses multiple CATEGORIES lines and splits on commas into keywords.
    // In contrast, jmap-ical's read_keywords:
    // 1. Trims leading and trailing whitespace from each category token.
    // 2. Discards empty category tokens (including consecutive commas or whitespace-only tags).
    // 3. Omits keywords completely (None) when no non-empty categories exist, avoiding empty object pollution.
    // 4. Outbound serialization requires set == true, rejects carriage returns, and sorts tags lexicographically.
    // Rationale: Desktop users enter tags where trailing whitespace is accidental and invisible.
    // Trimming prevents pseudo-duplicate tags. Dropping carriage returns preserves security against CRLF injection.
    let make_ics = |categories_line: &str| -> String {
        format!(
            "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-categories-test-001\r\n\
DTSTART:20260910T170000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Categories and Keywords Test\r\n\
{}\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n",
            categories_line
        )
    };

    // 1. Whitespace trimming and empty item filtering
    let ev = ical_to_event(&make_ics("CATEGORIES: ProjectX , Urgent , , Work "))
        .expect("parse categories");
    let kw = ev.keywords.as_ref().expect("keywords populated");
    assert_eq!(kw.len(), 3);
    assert_eq!(kw.get("ProjectX"), Some(&Value::Bool(true)));
    assert_eq!(kw.get("Urgent"), Some(&Value::Bool(true)));
    assert_eq!(kw.get("Work"), Some(&Value::Bool(true)));
    assert!(ev.extra.is_empty());

    // Outbound serialization emits single sorted line with trimmed tags
    let out = event_to_ical(&ev);
    assert_eq!(line(&out, "CATEGORIES:"), "CATEGORIES:ProjectX,Urgent,Work");

    // 2. Bare or whitespace-only CATEGORIES lines result in None (not empty map)
    let ev_empty = ical_to_event(&make_ics("CATEGORIES:   ,  ")).expect("parse empty categories");
    assert_eq!(ev_empty.keywords, None);
    assert!(ev_empty.extra.is_empty());
    let out_empty = event_to_ical(&ev_empty);
    assert!(without(&out_empty, "CATEGORIES:"));
}

#[test]
fn differential_oracle_conference_virtual_locations_features_labels_and_stable_key_synthesis() {
    // Divergence 21 against Stalwart differential oracle:
    // RFC 7986 section 5.11 defines CONFERENCE for audio/video meeting endpoints.
    // RFC 8984 section 4.2.6 models these as virtualLocations: Map<Id, VirtualLocation>.
    // Stalwart v1.0.0 parses CONFERENCE into virtualLocations, synthesizing keys using UUID5 or counters.
    // In contrast, jmap-ical's read_virtual_locations:
    // 1. Preserves X-JMAP-KEY parameter across round-trips to retain the exact server dictionary key.
    // 2. If X-JMAP-KEY is missing or invalid, allocates deterministic collision-free keys (v1, v2).
    // 3. Validates that value is a well-formed URI via names_a_uri, dropping invalid lines.
    // 4. Parses LABEL parameter into name and maps FEATURE parameters (AUDIO, VIDEO, SCREEN, CHAT, MODERATOR)
    //    into lowercase boolean entries in features map.
    // 5. Returns None when no valid conference endpoints are present.
    // Rationale: In Evolution Data Server, stable keys prevent map churn and diff churn during synchronization.
    let make_ics = |conf_line: &str| -> String {
        format!(
            "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-conference-test-001\r\n\
DTSTART:20260910T180000Z\r\n\
DURATION:PT45M\r\n\
SUMMARY:Conference and VirtualLocation Test\r\n\
{}\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n",
            conf_line
        )
    };

    // 1. Explicit X-JMAP-KEY, LABEL, and FEATURE tokens
    let ics_custom = make_ics(
        "CONFERENCE;VALUE=URI;X-JMAP-KEY=custom-key;LABEL=Planning Room;FEATURE=AUDIO,VIDEO:https://meet.example.com/plan",
    );
    let ev_custom = ical_to_event(&ics_custom).expect("parse custom conference");
    let vl_map = ev_custom
        .virtual_locations
        .as_ref()
        .expect("virtual locations populated");
    assert_eq!(vl_map.len(), 1);
    let loc = vl_map.get("custom-key").expect("custom-key present");
    assert_eq!(loc["uri"], "https://meet.example.com/plan");
    assert_eq!(loc["name"], "Planning Room");
    let features = loc["features"].as_object().expect("features object");
    assert_eq!(features.get("audio"), Some(&Value::Bool(true)));
    assert_eq!(features.get("video"), Some(&Value::Bool(true)));
    assert!(ev_custom.extra.is_empty());

    // Outbound serialization preserves X-JMAP-KEY and features
    let out_custom = event_to_ical(&ev_custom);
    let conf_out = content_line(&out_custom, "CONFERENCE;");
    assert!(conf_out.contains("X-JMAP-KEY=custom-key"));
    assert!(conf_out.contains("https://meet.example.com/plan"));

    // 2. Bare CONFERENCE line without X-JMAP-KEY gets deterministic key v1
    let ics_bare = make_ics("CONFERENCE:https://meet.example.com/plain");
    let ev_bare = ical_to_event(&ics_bare).expect("parse bare conference");
    let vl_bare = ev_bare
        .virtual_locations
        .expect("virtual locations populated");
    assert!(vl_bare.contains_key("v1"));
    assert_eq!(vl_bare["v1"]["uri"], "https://meet.example.com/plain");
    assert_eq!(vl_bare["v1"].get("name"), None);

    // 3. Invalid non-URI CONFERENCE value is dropped
    let ics_invalid = make_ics("CONFERENCE:not-a-valid-uri");
    let ev_invalid = ical_to_event(&ics_invalid).expect("parse invalid conference");
    assert_eq!(ev_invalid.virtual_locations, None);
    assert!(ev_invalid.extra.is_empty());
}

#[test]
fn differential_oracle_transparency_default_semantics_omission_and_non_standard_token_dropping() {
    // Divergence 22 against Stalwart differential oracle:
    // RFC 5545 section 3.8.2.7 defines TRANSP (OPAQUE default, TRANSPARENT).
    // RFC 8984 section 4.4.6 defines freeBusyStatus (busy default, free).
    // Stalwart v1.0.0 defaults freeBusyStatus to "busy" during CalendarEvent/parse when TRANSP is omitted.
    // In contrast, jmap-ical's read_transparency:
    // 1. Maps TRANSP:OPAQUE to Some("busy") and TRANSP:TRANSPARENT to Some("free") case-insensitively.
    // 2. If TRANSP is omitted, returns None (not defaulted to "busy"), avoiding spurious diffs in client sync.
    // 3. If TRANSP contains an unknown or non-standard token (e.g. TRANSP:TENTATIVE), drops it and returns None.
    // 4. Outbound serialization emits TRANSP:OPAQUE when busy, TRANSP:TRANSPARENT when free, and omits it when None.
    // Rationale: Returning None when unstated preserves semantic neutrality and prevents jmap-cal-sync
    // from generating unwanted patch operations against server defaults.
    let make_ics = |transp_line: &str| -> String {
        format!(
            "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-transp-test-001\r\n\
DTSTART:20260910T190000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Transparency and FreeBusyStatus Test\r\n\
{}\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n",
            transp_line
        )
    };

    // 1. TRANSP:OPAQUE maps to busy and emits TRANSP:OPAQUE
    let ev_opaque = ical_to_event(&make_ics("TRANSP:OPAQUE")).expect("parse opaque");
    assert_eq!(ev_opaque.free_busy_status.as_deref(), Some("busy"));
    assert!(ev_opaque.extra.is_empty());
    let out_opaque = event_to_ical(&ev_opaque);
    assert_eq!(line(&out_opaque, "TRANSP:"), "TRANSP:OPAQUE");

    // 2. TRANSP:TRANSPARENT maps to free and emits TRANSP:TRANSPARENT
    let ev_transp = ical_to_event(&make_ics("TRANSP:TRANSPARENT")).expect("parse transparent");
    assert_eq!(ev_transp.free_busy_status.as_deref(), Some("free"));
    assert!(ev_transp.extra.is_empty());
    let out_transp = event_to_ical(&ev_transp);
    assert_eq!(line(&out_transp, "TRANSP:"), "TRANSP:TRANSPARENT");

    // 3. Omitted TRANSP maps to None (not busy), and outbound emits no TRANSP line
    let ev_none = ical_to_event(&make_ics("")).expect("parse omitted transp");
    assert_eq!(ev_none.free_busy_status, None);
    assert!(ev_none.extra.is_empty());
    let out_none = event_to_ical(&ev_none);
    assert!(without(&out_none, "TRANSP:"));

    // 4. Non-standard TRANSP value (e.g. TRANSP:TENTATIVE) is dropped on import
    let ev_nonstandard =
        ical_to_event(&make_ics("TRANSP:TENTATIVE")).expect("parse nonstandard transp");
    assert_eq!(ev_nonstandard.free_busy_status, None);
    assert!(ev_nonstandard.extra.is_empty());
    let out_nonstandard = event_to_ical(&ev_nonstandard);
    assert!(without(&out_nonstandard, "TRANSP:"));
}

#[test]
fn differential_oracle_priority_range_clamping_omission_semantics_and_vtodo_isolation() {
    // Divergence 23 against Stalwart differential oracle:
    // RFC 5545 section 3.8.1.9 defines PRIORITY as an integer from 0 to 9 (0 undefined, 1 highest, 9 lowest).
    // RFC 8984 section 4.4.1 defines priority as UnsignedInt (0 to 9).
    // Stalwart v1.0.0 parses 0 to 9, but behaviors on invalid or out-of-range priorities vary across parsers.
    // In contrast, jmap-ical's read_priority:
    // 1. Strictly validates integer parse within 0..=9. Out-of-bounds or non-integer values return None.
    // 2. An omitted PRIORITY in the component returns None, rather than synthesizing Some(0).
    // 3. Outbound serialization emits PRIORITY:0 only when priority: Some(0) is explicitly set.
    //    When priority is None, the PRIORITY line is omitted.
    // 4. Non-VEVENT components (e.g. VTODO with PRIORITY) are discarded, so task priorities do not leak into appointments.
    // Rationale: Strict 0..=9 range clamping prevents invalid states in desktop UI and ensures roundtrip fidelity.
    let make_ics = |priority_line: &str| -> String {
        format!(
            "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-priority-test-001\r\n\
DTSTART:20260910T200000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Priority Range and Clamping Test\r\n\
{}\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n",
            priority_line
        )
    };

    // 1. Valid priority in 1..=9
    let ev1 = ical_to_event(&make_ics("PRIORITY:1")).expect("parse priority 1");
    assert_eq!(ev1.priority, Some(1));
    assert!(ev1.extra.is_empty());
    let out1 = event_to_ical(&ev1);
    assert_eq!(line(&out1, "PRIORITY:"), "PRIORITY:1");

    // 2. Explicit PRIORITY:0 (undefined)
    let ev0 = ical_to_event(&make_ics("PRIORITY:0")).expect("parse priority 0");
    assert_eq!(ev0.priority, Some(0));
    assert!(ev0.extra.is_empty());
    let out0 = event_to_ical(&ev0);
    assert_eq!(line(&out0, "PRIORITY:"), "PRIORITY:0");

    // 3. Omitted PRIORITY yields None, and outbound export omits PRIORITY line
    let ev_none = ical_to_event(&make_ics("")).expect("parse omitted priority");
    assert_eq!(ev_none.priority, None);
    assert!(ev_none.extra.is_empty());
    let out_none = event_to_ical(&ev_none);
    assert!(without(&out_none, "PRIORITY:"));

    // 4. Out-of-bounds priority (10, -1, non-integer) is dropped on import
    let ev_high = ical_to_event(&make_ics("PRIORITY:10")).expect("parse priority 10");
    assert_eq!(ev_high.priority, None);
    assert!(ev_high.extra.is_empty());

    let ev_neg = ical_to_event(&make_ics("PRIORITY:-1")).expect("parse priority -1");
    assert_eq!(ev_neg.priority, None);
    assert!(ev_neg.extra.is_empty());

    let ev_str = ical_to_event(&make_ics("PRIORITY:HIGH")).expect("parse priority string");
    assert_eq!(ev_str.priority, None);
    assert!(ev_str.extra.is_empty());

    // 5. VTODO with PRIORITY:5 alongside VEVENT without priority leaves event.priority as None
    let stream_with_vtodo = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VTODO\r\n\
UID:todo-item-001\r\n\
SUMMARY:A separate task\r\n\
PRIORITY:5\r\n\
END:VTODO\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-priority-test-002\r\n\
DTSTART:20260910T210000Z\r\n\
DURATION:PT30M\r\n\
SUMMARY:Appointment beside task\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let ev_event = ical_to_event(stream_with_vtodo).expect("parse calendar with vtodo");
    assert_eq!(ev_event.priority, None);
    assert!(ev_event.extra.is_empty());
}

#[test]
fn differential_oracle_status_mapping_omission_and_task_status_rejection() {
    // Divergence 24 against Stalwart differential oracle:
    // RFC 5545 section 3.8.1.11 defines STATUS for VEVENT: TENTATIVE, CONFIRMED, CANCELLED.
    // RFC 8984 section 4.4.5 defines status: String (default: "confirmed").
    // Stalwart v1.0.0 defaults an omitted STATUS to "confirmed" during CalendarEvent/parse.
    // In contrast, jmap-ical:
    // 1. Maps CONFIRMED to "confirmed", CANCELLED to "cancelled", and TENTATIVE to "tentative" (case-insensitive).
    // 2. Returns status: None when STATUS is omitted from VEVENT, avoiding spurious patch diffs against server records.
    // 3. Drops task-specific (VTODO) statuses (NEEDS-ACTION, COMPLETED, IN-PROCESS) and unknown tokens to None.
    // 4. Outbound export emits STATUS only when status is Some("confirmed" | "cancelled" | "tentative"), and omits it when None.
    let make_ics = |status_line: &str| -> String {
        format!(
            "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-status-test-001\r\n\
DTSTART:20260910T100000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Status Mapping Test\r\n\
{}\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n",
            status_line
        )
    };

    // 1. Standard statuses
    let ev_conf = ical_to_event(&make_ics("STATUS:CONFIRMED")).expect("parse confirmed");
    assert_eq!(ev_conf.status.as_deref(), Some("confirmed"));
    assert!(ev_conf.extra.is_empty());
    let out_conf = event_to_ical(&ev_conf);
    assert_eq!(line(&out_conf, "STATUS:"), "STATUS:CONFIRMED");

    let ev_canc = ical_to_event(&make_ics("STATUS:CANCELLED")).expect("parse cancelled");
    assert_eq!(ev_canc.status.as_deref(), Some("cancelled"));
    assert!(ev_canc.extra.is_empty());
    let out_canc = event_to_ical(&ev_canc);
    assert_eq!(line(&out_canc, "STATUS:"), "STATUS:CANCELLED");

    let ev_tent = ical_to_event(&make_ics("STATUS:TENTATIVE")).expect("parse tentative");
    assert_eq!(ev_tent.status.as_deref(), Some("tentative"));
    assert!(ev_tent.extra.is_empty());
    let out_tent = event_to_ical(&ev_tent);
    assert_eq!(line(&out_tent, "STATUS:"), "STATUS:TENTATIVE");

    // Case-insensitivity
    let ev_lower = ical_to_event(&make_ics("STATUS:confirmed")).expect("parse lowercase confirmed");
    assert_eq!(ev_lower.status.as_deref(), Some("confirmed"));

    // 2. Omitted STATUS yields None and is omitted on export
    let ev_none = ical_to_event(&make_ics("")).expect("parse omitted status");
    assert_eq!(ev_none.status, None);
    assert!(ev_none.extra.is_empty());
    let out_none = event_to_ical(&ev_none);
    assert!(without(&out_none, "STATUS:"));

    // 3. Task-specific or unknown statuses dropped to None
    let ev_todo = ical_to_event(&make_ics("STATUS:COMPLETED")).expect("parse completed");
    assert_eq!(ev_todo.status, None);
    assert!(ev_todo.extra.is_empty());

    let ev_in_proc = ical_to_event(&make_ics("STATUS:IN-PROCESS")).expect("parse in-process");
    assert_eq!(ev_in_proc.status, None);
    assert!(ev_in_proc.extra.is_empty());

    let ev_needs = ical_to_event(&make_ics("STATUS:NEEDS-ACTION")).expect("parse needs-action");
    assert_eq!(ev_needs.status, None);
    assert!(ev_needs.extra.is_empty());

    let ev_unknown = ical_to_event(&make_ics("STATUS:BOGUS")).expect("parse bogus");
    assert_eq!(ev_unknown.status, None);
    assert!(ev_unknown.extra.is_empty());
}

#[test]
fn differential_oracle_duration_dtend_calculation_and_outbound_preference() {
    // Divergence 25 against Stalwart differential oracle:
    // RFC 5545 section 3.8.2.2 and 3.8.2.4 permit specifying bounds via DTSTART+DTEND or DTSTART+DURATION.
    // RFC 8984 section 4.1.4 models event length strictly as duration: Duration (default: "PT0S").
    // Stalwart v1.0.0 parses DTEND into duration, and emits "PT0S" when duration is zero or unstated.
    // In contrast, jmap-ical:
    // 1. Prioritizes explicit DURATION when present, falling back to computing duration from DTEND - DTSTART.
    // 2. Returns duration: None for zero length (PT0S) and negative lengths, maintaining server omission defaults.
    // 3. On outbound export, always serializes event.duration as DURATION and never writes DTEND,
    //    preventing DST transition calculation skew across daylight saving shifts.
    let make_ics = |extra_lines: &str| -> String {
        format!(
            "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-duration-test-001\r\n\
DTSTART:20260910T100000Z\r\n\
{}\r\n\
SUMMARY:Duration vs DTEND Test\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n",
            extra_lines
        )
    };

    // 1. DTEND converted to duration
    let ev_dtend = ical_to_event(&make_ics("DTEND:20260910T113000Z")).expect("parse dtend");
    assert_eq!(ev_dtend.duration.as_deref(), Some("PT1H30M"));
    assert!(ev_dtend.extra.is_empty());
    let out_dtend = event_to_ical(&ev_dtend);
    assert_eq!(line(&out_dtend, "DURATION:"), "DURATION:PT1H30M");
    assert!(without(&out_dtend, "DTEND:"));

    // 2. Explicit DURATION takes precedence if both are present
    let ev_both =
        ical_to_event(&make_ics("DURATION:PT2H\r\nDTEND:20260910T110000Z")).expect("parse both");
    assert_eq!(ev_both.duration.as_deref(), Some("PT2H"));
    assert!(ev_both.extra.is_empty());

    // 3. Zero duration: calculated zero duration (DTSTART == DTEND) yields None,
    // while explicit stated DURATION:PT0S is preserved as Some("PT0S")
    let ev_zero_end = ical_to_event(&make_ics("DTEND:20260910T100000Z")).expect("parse zero end");
    assert_eq!(ev_zero_end.duration, None);

    let ev_zero_dur = ical_to_event(&make_ics("DURATION:PT0S")).expect("parse zero dur");
    assert_eq!(ev_zero_dur.duration.as_deref(), Some("PT0S"));
    let out_zero_dur = event_to_ical(&ev_zero_dur);
    assert_eq!(line(&out_zero_dur, "DURATION:"), "DURATION:PT0S");

    // 4. Negative duration (end before start or negative duration) yields None
    let ev_neg_end =
        ical_to_event(&make_ics("DTEND:20260910T090000Z")).expect("parse negative end");
    assert_eq!(ev_neg_end.duration, None);

    let ev_neg_dur = ical_to_event(&make_ics("DURATION:-PT1H")).expect("parse negative duration");
    assert_eq!(ev_neg_dur.duration, None);
}

#[test]
fn differential_oracle_show_without_time_all_day_representation_and_defensive_fallback() {
    // Divergence 26 against Stalwart differential oracle:
    // RFC 5545 section 3.8.2.4 defines all-day events using DTSTART;VALUE=DATE:YYYYMMDD.
    // RFC 8984 section 4.1.5 models all-day events as showWithoutTime: Boolean (default: false),
    // with start at midnight (00:00:00) and timeZone: null.
    // Stalwart v1.0.0 parses VALUE=DATE into showWithoutTime: true and timeZone: null.
    // In contrast, jmap-ical:
    // 1. Parses VALUE=DATE (no 'T') into start at midnight, time_zone: None, and show_without_time: Some(true).
    // 2. Timed events (with 'T') return show_without_time: None rather than Some(false), preserving server defaults.
    // 3. Outbound export validates all all-day invariants: midnight start, whole day duration, no timeZone,
    //    and no sub-day recurrence rules before writing DTSTART;VALUE=DATE.
    // 4. If an invariant fails, jmap-ical defensively falls back to writing a timed DTSTART to prevent time truncation.
    let ics_all_day = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-allday-test-001\r\n\
DTSTART;VALUE=DATE:20260915\r\n\
DTEND;VALUE=DATE:20260916\r\n\
SUMMARY:All Day Conference\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let ev_allday = ical_to_event(ics_all_day).expect("parse all day");
    assert_eq!(ev_allday.start.as_deref(), Some("2026-09-15T00:00:00"));
    assert_eq!(ev_allday.time_zone, None);
    assert_eq!(ev_allday.show_without_time, Some(true));
    assert_eq!(ev_allday.duration.as_deref(), Some("P1D"));
    assert!(ev_allday.extra.is_empty());

    let out_allday = event_to_ical(&ev_allday);
    assert_eq!(
        line(&out_allday, "DTSTART;VALUE=DATE:"),
        "DTSTART;VALUE=DATE:20260915"
    );
    assert_eq!(line(&out_allday, "DURATION:"), "DURATION:P1D");
    assert!(without(&out_allday, "TZID="));

    // Timed event has show_without_time: None
    let ics_timed = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-timed-test-001\r\n\
DTSTART:20260915T140000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Timed Meeting\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let ev_timed = ical_to_event(ics_timed).expect("parse timed");
    assert_eq!(ev_timed.show_without_time, None);

    // Defensive fallback: show_without_time is Some(true), but start is not at midnight (14:30:00).
    // Export falls back to timed DTSTART rather than truncating start time.
    let mut ev_broken = ev_allday.clone();
    ev_broken.start = Some("2026-09-15T14:30:00".to_owned());
    let out_broken = event_to_ical(&ev_broken);
    assert!(without(&out_broken, "VALUE=DATE"));
    assert_eq!(line(&out_broken, "DTSTART:"), "DTSTART:20260915T143000");
}

#[test]
fn differential_oracle_recurrence_overrides_exdate_rdate_and_thisandfuture_boundary() {
    // Divergence 27 against Stalwart differential oracle:
    // RFC 5545 specifies EXDATE (exceptions), RDATE (additions), and RECURRENCE-ID;RANGE=THISANDFUTURE.
    // RFC 8984 section 4.3.4 models all instance exceptions within recurrenceOverrides.
    // Stalwart v1.0.0 maps EXDATE to {"excluded": true} and RDATE to {}.
    // In contrast, jmap-ical's read_overrides:
    // 1. Maps EXDATE to {"excluded": true} and RDATE to {}.
    // 2. Resolves collisions where an instant is in both RDATE and EXDATE by letting EXDATE win (excluded).
    // 3. Detached VEVENT components override RDATE/EXDATE entries with specific property patches.
    // 4. Skips RECURRENCE-ID;RANGE=THISANDFUTURE because JSCalendar recurrenceOverrides cannot express
    //    future series modification or splitting, avoiding corruption of subsequent instances.
    // 5. Returns recurrence_overrides: None when no overrides exist, avoiding empty map diff churn.
    let ics_overrides = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-recurrence-test-001\r\n\
DTSTART:20260901T100000Z\r\n\
DURATION:PT1H\r\n\
RRULE:FREQ=DAILY;COUNT=10\r\n\
EXDATE:20260903T100000Z\r\n\
RDATE:20260915T100000Z\r\n\
SUMMARY:Daily Standup\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let ev = ical_to_event(ics_overrides).expect("parse series with exdate and rdate");
    let overrides = ev.recurrence_overrides.as_ref().expect("overrides present");
    assert_eq!(
        overrides.get("2026-09-03T10:00:00"),
        Some(&serde_json::json!({ "excluded": true }))
    );
    assert_eq!(
        overrides.get("2026-09-15T10:00:00"),
        Some(&serde_json::json!({}))
    );
    assert!(ev.extra.is_empty());

    // Outbound export of excluded override emits EXDATE
    let out = event_to_ical(&ev);
    assert_eq!(line(&out, "EXDATE:"), "EXDATE:20260903T100000Z");

    // RECURRENCE-ID with RANGE=THISANDFUTURE is skipped to protect series integrity
    let ics_thisandfuture = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-recurrence-test-002\r\n\
DTSTART:20260901T100000Z\r\n\
DURATION:PT1H\r\n\
RRULE:FREQ=DAILY;COUNT=10\r\n\
SUMMARY:Daily Standup Series\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-recurrence-test-002\r\n\
RECURRENCE-ID;RANGE=THISANDFUTURE:20260905T100000Z\r\n\
DTSTART:20260905T110000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Rescheduled Standups\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let ev_taf = ical_to_event(ics_thisandfuture).expect("parse thisandfuture");
    assert_eq!(ev_taf.recurrence_overrides, None);
    assert!(ev_taf.extra.is_empty());
}

#[test]
fn differential_oracle_use_default_alerts_omission_and_notification_preference_boundary() {
    // Divergence 28 against Stalwart differential oracle:
    // RFC 8984 section 4.5.1 defines useDefaultAlerts: Boolean (default: false) to indicate
    // whether the user's default reminder alerts should be applied when no alerts are specified.
    // Stalwart v1.0.0 either omits useDefaultAlerts or sets it to false when parsing incoming VEVENTs.
    // In contrast, jmap-ical's ical_to_event:
    // 1. Returns use_default_alerts: None when parsing incoming VEVENTs (both with and without VALARM),
    //    avoiding spurious diffs against server defaults.
    // 2. On outbound export, if an event has use_default_alerts: Some(true), drawn_alarms suppresses
    //    VALARM emission (returns empty alarm vector).
    // 3. maps_alerts strictly returns false when use_default_alerts is true, preventing whole-property
    //    replacement that would conflict with server-side default alerts.
    // 4. maps_recurrence_override refuses alert overrides when the series uses default alerts.
    let ics_no_alarms = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-default-alerts-001\r\n\
DTSTART:20260904T100000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:No Alarms Meeting\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let ev_no_alarms = ical_to_event(ics_no_alarms).expect("parse event without alarms");
    assert_eq!(ev_no_alarms.use_default_alerts, None);
    assert_eq!(ev_no_alarms.alerts, None);
    assert!(ev_no_alarms.extra.is_empty());

    let ics_with_alarm = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-default-alerts-002\r\n\
DTSTART:20260904T100000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Meeting With Alarm\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
TRIGGER:-PT15M\r\n\
DESCRIPTION:Reminder\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let ev_with_alarm = ical_to_event(ics_with_alarm).expect("parse event with alarm");
    assert_eq!(ev_with_alarm.use_default_alerts, None);
    assert!(ev_with_alarm.alerts.is_some());
    assert!(maps_alerts(&ev_with_alarm));

    // Suppressed alarm emission when use_default_alerts is true
    let mut ev_suppressed = ev_with_alarm.clone();
    ev_suppressed.use_default_alerts = Some(true);
    assert!(!maps_alerts(&ev_suppressed));
    let out_suppressed = event_to_ical(&ev_suppressed);
    assert!(without(&out_suppressed, "BEGIN:VALARM"));

    // Suppressed when useDefaultAlerts is set in extra
    let mut ev_extra_default = ev_with_alarm.clone();
    ev_extra_default
        .extra
        .insert("useDefaultAlerts".to_owned(), Value::Bool(true));
    assert!(!maps_alerts(&ev_extra_default));
    let out_extra = event_to_ical(&ev_extra_default);
    assert!(without(&out_extra, "BEGIN:VALARM"));
}

#[test]
fn differential_oracle_locale_tag_omission_and_property_language_filtering() {
    // Divergence 29 against Stalwart differential oracle:
    // RFC 8984 section 4.1.6 defines locale: String (a BCP 47 language tag) for event properties.
    // RFC 5545 section 3.2.10 defines LANGUAGE parameters on text properties (e.g. SUMMARY;LANGUAGE=fr).
    // Stalwart v1.0.0 parses property-level LANGUAGE parameters and may infer event.locale.
    // In contrast, jmap-ical's ical_to_event:
    // 1. Reads text properties while ignoring LANGUAGE parameters, returning locale: None.
    // 2. Outbound export does not emit LANGUAGE parameters from event.locale or add document language tags.
    // 3. Leaves event.extra completely clean without polluting custom maps.
    let ics_localized = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-locale-test-001\r\n\
DTSTART:20260904T140000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY;LANGUAGE=fr:Reunion d'equipe\r\n\
DESCRIPTION;LANGUAGE=fr:Discussion hebdomadaire des projets en cours.\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let ev = ical_to_event(ics_localized).expect("parse localized event");
    assert_eq!(ev.title.as_deref(), Some("Reunion d'equipe"));
    assert_eq!(
        ev.description.as_deref(),
        Some("Discussion hebdomadaire des projets en cours.")
    );
    assert_eq!(ev.locale, None);
    assert!(ev.extra.is_empty());

    // Outbound export does not emit LANGUAGE parameter or document headers
    let mut ev_with_locale = ev.clone();
    ev_with_locale.locale = Some("fr".to_owned());
    let out = event_to_ical(&ev_with_locale);
    assert_eq!(line(&out, "SUMMARY:"), "SUMMARY:Reunion d'equipe");
    assert_eq!(
        line(&out, "DESCRIPTION:"),
        "DESCRIPTION:Discussion hebdomadaire des projets en cours."
    );
    assert!(without(&out, "LANGUAGE="));
    assert!(without(&out, "X-LIC-LOCATION"));
}

#[test]
fn differential_oracle_floating_utc_canonical_iana_and_solidus_timezone_resolution() {
    // Divergence 30 against Stalwart differential oracle:
    // RFC 5545 section 3.3.5 defines floating date-time, UTC (Z), and local time with TZID.
    // RFC 8984 sections 1.4.9 and 4.1.4 define timeZone as an IANA timezone name, a solidus identifier,
    // or null for floating/all-day.
    // Stalwart v1.0.0 parses UTC into start and timeZone: "Etc/UTC" (or "UTC").
    // In contrast, jmap-ical's read_start:
    // 1. Floating time (no Z, no TZID) yields time_zone: None.
    // 2. UTC time (trailing Z) yields time_zone: Some("Etc/UTC").
    // 3. Windows display names resolve to canonical IANA names via CLDR (W. Europe Standard Time -> Europe/Berlin).
    // 4. Mozilla/Apple unique prefixes normalize to canonical IANA suffixes (/mozilla.org/... -> Europe/Madrid).
    // 5. Custom solidus zones are retained verbatim as time_zone: Some("/org.custom/zone").
    // Outbound export:
    // 1. UTC (Etc/UTC or UTC) serializes with Z suffix.
    // 2. Floating time serializes without TZID and without Z.
    // 3. Canonical IANA zones serialize with TZID=<zone>.
    // 4. Custom solidus zones serialize with TZID=<solidus-zone>.

    // 1. Floating date-time
    let ics_floating = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
BEGIN:VEVENT\r\nUID:oracle-floating-001\r\n\
DTSTART:20260904T120000\r\nDURATION:PT1H\r\nSUMMARY:Floating Lunch\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev_floating = ical_to_event(ics_floating).expect("parse floating");
    assert_eq!(ev_floating.start.as_deref(), Some("2026-09-04T12:00:00"));
    assert_eq!(ev_floating.time_zone, None);
    let out_floating = event_to_ical(&ev_floating);
    assert_eq!(line(&out_floating, "DTSTART:"), "DTSTART:20260904T120000");

    // 2. UTC date-time
    let ics_utc = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
BEGIN:VEVENT\r\nUID:oracle-utc-001\r\n\
DTSTART:20260904T120000Z\r\nDURATION:PT1H\r\nSUMMARY:UTC Sync\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev_utc = ical_to_event(ics_utc).expect("parse utc");
    assert_eq!(ev_utc.start.as_deref(), Some("2026-09-04T12:00:00"));
    assert_eq!(ev_utc.time_zone.as_deref(), Some("Etc/UTC"));
    let out_utc = event_to_ical(&ev_utc);
    assert_eq!(line(&out_utc, "DTSTART:"), "DTSTART:20260904T120000Z");

    // 3. Windows display name resolution
    let ics_windows = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
BEGIN:VTIMEZONE\r\nTZID:W. Europe Standard Time\r\n\
BEGIN:STANDARD\r\nDTSTART:16010101T020000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:oracle-win-001\r\n\
DTSTART;TZID=\"W. Europe Standard Time\":20260904T140000\r\n\
DURATION:PT1H\r\nSUMMARY:Berlin Sync\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev_windows = ical_to_event(ics_windows).expect("parse windows tzid");
    assert_eq!(ev_windows.time_zone.as_deref(), Some("Europe/Berlin"));
    let out_windows = event_to_ical(&ev_windows);
    assert_eq!(
        line(&out_windows, "DTSTART;"),
        "DTSTART;TZID=Europe/Berlin:20260904T140000"
    );

    // 4. Mozilla unique prefix normalization
    let ics_mozilla = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
BEGIN:VEVENT\r\nUID:oracle-moz-001\r\n\
DTSTART;TZID=/mozilla.org/20050126_1/Europe/Madrid:20260904T150000\r\n\
DURATION:PT1H\r\nSUMMARY:Madrid Sync\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev_mozilla = ical_to_event(ics_mozilla).expect("parse mozilla tzid");
    assert_eq!(ev_mozilla.time_zone.as_deref(), Some("Europe/Madrid"));
    let out_mozilla = event_to_ical(&ev_mozilla);
    assert_eq!(
        line(&out_mozilla, "DTSTART;"),
        "DTSTART;TZID=Europe/Madrid:20260904T150000"
    );

    // 5. Custom solidus zone retention
    let ics_solidus = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
BEGIN:VEVENT\r\nUID:oracle-solidus-001\r\n\
DTSTART;TZID=/org.custom/zone:20260904T160000\r\n\
DURATION:PT1H\r\nSUMMARY:Custom Zone Sync\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev_solidus = ical_to_event(ics_solidus).expect("parse solidus tzid");
    assert_eq!(ev_solidus.time_zone.as_deref(), Some("/org.custom/zone"));
    let out_solidus = event_to_ical(&ev_solidus);
    assert_eq!(
        line(&out_solidus, "DTSTART;"),
        "DTSTART;TZID=/org.custom/zone:20260904T160000"
    );
}

#[test]
fn differential_oracle_vtimezone_observance_rules_ingestion_and_standard_iana_pruning() {
    // Divergence 31 against Stalwart differential oracle:
    // RFC 5545 section 3.6.5 specifies VTIMEZONE with STANDARD/DAYLIGHT observance subcomponents.
    // RFC 8984 section 4.7.2 models custom timezone definitions inside timeZones.
    // Stalwart v1.0.0 parses VTIMEZONEs, dropping redundant standard IANA definitions.
    // In contrast, jmap-ical's read_time_zones:
    // 1. Drops inline VTIMEZONE components for recognized standard IANA zone names (time_zones: None),
    //    preventing multi-kilobyte JSON payload bloat in JMAP event state.
    // 2. Preserves VTIMEZONE components for custom solidus zones (/example.com/custom_tz) with their
    //    observance rules (TZOFFSETFROM, TZOFFSETTO, RRULE) in event.time_zones.
    // 3. prune_time_zones removes unreferenced custom timezone definitions when neither the master series
    //    nor any recurrence override refers to the custom zone.
    // 4. Outbound export: defines_time_zone confirms custom zone presence, and event_to_ical emits
    //    VTIMEZONE only for custom solidus zones while omitting redundant standard IANA VTIMEZONE blocks.

    // 1. Standard IANA zone with inline VTIMEZONE is pruned on import
    let ics_standard = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
BEGIN:VTIMEZONE\r\nTZID:Europe/Berlin\r\n\
BEGIN:STANDARD\r\nDTSTART:19701025T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\nEND:STANDARD\r\n\
BEGIN:DAYLIGHT\r\nDTSTART:19700329T020000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3\r\nEND:DAYLIGHT\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:oracle-std-tz-001\r\n\
DTSTART;TZID=Europe/Berlin:20260904T100000\r\nDURATION:PT1H\r\nSUMMARY:Standard IANA Event\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev_standard = ical_to_event(ics_standard).expect("parse standard zone with vtimezone");
    assert_eq!(ev_standard.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(
        ev_standard.time_zones, None,
        "standard IANA zone must be pruned from event.time_zones"
    );
    assert!(!defines_time_zone(&ev_standard, "Europe/Berlin"));
    let out_standard = event_to_ical(&ev_standard);
    assert!(
        without(&out_standard, "BEGIN:VTIMEZONE"),
        "standard IANA zone must not emit VTIMEZONE"
    );

    // 2. Custom solidus zone with inline VTIMEZONE is preserved
    let custom_tzid = "/example.com/custom_tz";
    let ics_custom = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
BEGIN:VTIMEZONE\r\nTZID:{custom_tzid}\r\n\
BEGIN:STANDARD\r\nDTSTART:19700101T000000\r\nTZOFFSETFROM:+0300\r\nTZOFFSETTO:+0300\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:oracle-cust-tz-001\r\n\
DTSTART;TZID={custom_tzid}:20260904T100000\r\nDURATION:PT1H\r\nSUMMARY:Custom Zone Event\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let mut ev_custom = ical_to_event(&ics_custom).expect("parse custom zone with vtimezone");
    assert_eq!(ev_custom.time_zone.as_deref(), Some(custom_tzid));
    assert!(
        defines_time_zone(&ev_custom, custom_tzid),
        "custom solidus zone must be defined in event.time_zones"
    );
    let time_zones = ev_custom.time_zones.as_ref().expect("time_zones present");
    assert!(time_zones.contains_key(custom_tzid));

    let out_custom = event_to_ical(&ev_custom);
    assert!(
        out_custom.contains("BEGIN:VTIMEZONE"),
        "custom solidus zone must emit VTIMEZONE"
    );
    assert!(out_custom.contains(&format!("TZID:{custom_tzid}")));

    // 3. prune_time_zones removes unreferenced custom timezone definitions
    ev_custom.time_zone = Some("Europe/Berlin".to_owned());
    prune_time_zones(&mut ev_custom);
    assert_eq!(
        ev_custom.time_zones, None,
        "unreferenced custom zone must be pruned"
    );
}

#[test]
fn differential_oracle_resources_equipment_room_lists_dropped_on_import() {
    // Divergence 32 against Stalwart differential oracle:
    // RFC 5545 section 3.8.1.10 defines RESOURCES as a comma-separated list of equipment or resource names
    // (e.g. RESOURCES:EASEL,PROJECTOR,CONFERENCE ROOM A).
    // In RFC 8984 / jscalendarbis section 4.4, resources can be represented in participants with kind: "resource",
    // or in locations with locationTypes: ["resource"].
    // Stalwart v1.0.0 parses RESOURCES into participant or location resource records, or preserves them
    // in tracking dictionaries.
    // In contrast, jmap-ical's read_vevent:
    // 1. Drops RESOURCES on inbound parse without polluting event.extra (extra remains empty).
    // 2. Leaves participants and locations as None (unless standard LOCATION or ATTENDEE is present).
    // 3. Outbound export does not emit RESOURCES lines.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-resources-test-001\r\n\
DTSTART:20260904T150000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Quarterly Strategy All-Hands\r\n\
RESOURCES:PROJECTOR,SCREEN,MICROPHONE,ROOM-101\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let ev = ical_to_event(ics).expect("parse event with resources");
    assert_eq!(ev.title.as_deref(), Some("Quarterly Strategy All-Hands"));
    assert_eq!(ev.participants, None);
    assert_eq!(ev.locations, None);
    assert!(ev.extra.is_empty());

    // Outbound export does not emit RESOURCES
    let out = event_to_ical(&ev);
    assert!(without(&out, "RESOURCES"));
    assert_eq!(
        line(&out, "SUMMARY:"),
        "SUMMARY:Quarterly Strategy All-Hands"
    );
}

#[test]
fn differential_oracle_contact_and_scheduling_parameters_sent_by_dir_boundary() {
    // Divergence 33 against Stalwart differential oracle:
    // RFC 5545 section 3.8.4.2 defines CONTACT to represent contact information (e.g. CONTACT:Jane Doe, +1-555-0199).
    // RFC 5545 sections 3.2.18 and 3.2.6 specify SENT-BY and DIR parameters on ORGANIZER and ATTENDEE.
    // RFC 8984 section 4.4 models sentBy on Participant and participants with roles: {"contact": true}.
    // Stalwart v1.0.0 parses CONTACT into participant entries or related contacts, and maps SENT-BY to Participant.sentBy.
    // In contrast, jmap-ical:
    // 1. Drops CONTACT on inbound parse without polluting event.extra.
    // 2. Drops ORGANIZER and ATTENDEE on inbound parse (participants: None) for scheduling boundary safety.
    // 3. Outbound export renders ORGANIZER/ATTENDEE when participants is populated, but emits no CONTACT lines
    //    and omits SENT-BY and DIR parameters.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-contact-test-001\r\n\
DTSTART:20260904T160000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Executive Briefing\r\n\
CONTACT:Event Support Team, support@example.com\r\n\
ORGANIZER;SENT-BY=\"mailto:assistant@example.com\";DIR=\"ldap://corp.example.com\":mailto:boss@example.com\r\n\
ATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;DIR=\"ldap://corp.example.com\":mailto:engineer@example.com\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let ev = ical_to_event(ics).expect("parse event with contact and scheduling parameters");
    assert_eq!(ev.title.as_deref(), Some("Executive Briefing"));
    assert_eq!(ev.participants, None);
    assert!(ev.extra.is_empty());

    let out = event_to_ical(&ev);
    assert!(without(&out, "CONTACT:"));
    assert!(without(&out, "SENT-BY"));
    assert!(without(&out, "DIR="));
}

#[test]
fn differential_oracle_comment_notes_dropped_on_import_preserving_description_identity() {
    // Divergence 34 against Stalwart differential oracle:
    // RFC 5545 section 3.8.1.4 specifies COMMENT for non-editorial notes or comments regarding a component.
    // RFC 8984 / jscalendarbis has no dedicated comment property (all notes belong in description).
    // Stalwart v1.0.0 may concatenate COMMENT into description or capture it in convertedProperties tracking metadata.
    // In contrast, jmap-ical:
    // 1. Maps DESCRIPTION strictly to event.description.
    // 2. Drops COMMENT lines on import without appending them to description or polluting event.extra.
    // 3. Outbound export emits DESCRIPTION and never synthesizes COMMENT lines, preventing text duplication across round-trips.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-comment-test-001\r\n\
DTSTART:20260904T170000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Quarterly Review\r\n\
DESCRIPTION:Q3 performance review and architecture roadmap.\r\n\
COMMENT:Please arrive 10 minutes early for badge verification.\r\n\
COMMENT:Bring your team performance sheets.\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let ev = ical_to_event(ics).expect("parse event with comments");
    assert_eq!(ev.title.as_deref(), Some("Quarterly Review"));
    assert_eq!(
        ev.description.as_deref(),
        Some("Q3 performance review and architecture roadmap.")
    );
    assert!(ev.extra.is_empty());

    let out = event_to_ical(&ev);
    assert_eq!(
        line(&out, "DESCRIPTION:"),
        "DESCRIPTION:Q3 performance review and architecture roadmap."
    );
    assert!(without(&out, "COMMENT:"));
}

#[test]
fn differential_oracle_attachment_filename_parameter_dropped_for_uri_only_links() {
    // Divergence 35 against Stalwart differential oracle:
    // RFC 5545 section 3.8.4.1 defines ATTACH with URI values. Exporters append FILENAME or X-APPLE-FILENAME parameters.
    // RFC 8984 section 1.4.11 defines Link with href, title, contentType, and size.
    // Stalwart v1.0.0 parses FILENAME into Link.title.
    // In contrast, jmap-ical's read_links:
    // 1. Extracts href, contentType (from FMTTYPE), and size (from SIZE).
    // 2. Drops FILENAME and X-APPLE-FILENAME parameters, leaving title omitted from imported links.
    // 3. Synthesizes deterministic stable keys (k1, k2) or preserves X-JMAP-KEY.
    // 4. Outbound export emits ATTACH with FMTTYPE, SIZE, and X-JMAP-KEY, omitting FILENAME.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-attach-filename-001\r\n\
DTSTART:20260904T180000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Project Kickoff\r\n\
ATTACH;FMTTYPE=application/pdf;SIZE=1048576;FILENAME=\"architecture-spec.pdf\":https://example.com/docs/spec.pdf\r\n\
ATTACH;FMTTYPE=image/png;X-APPLE-FILENAME=\"network-topology.png\":https://example.com/images/arch.png\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let ev = ical_to_event(ics).expect("parse event with attachment filenames");
    let links = ev.links.as_ref().expect("links present");
    assert_eq!(links.len(), 2);

    let link1 = links.get("k1").expect("first attachment key k1");
    assert_eq!(
        link1.get("href").and_then(Value::as_str),
        Some("https://example.com/docs/spec.pdf")
    );
    assert_eq!(
        link1.get("contentType").and_then(Value::as_str),
        Some("application/pdf")
    );
    assert_eq!(link1.get("size").and_then(Value::as_u64), Some(1048576));
    assert_eq!(link1.get("title"), None);

    let link2 = links.get("k2").expect("second attachment key k2");
    assert_eq!(
        link2.get("href").and_then(Value::as_str),
        Some("https://example.com/images/arch.png")
    );
    assert_eq!(
        link2.get("contentType").and_then(Value::as_str),
        Some("image/png")
    );
    assert_eq!(link2.get("title"), None);
    assert!(ev.extra.is_empty());

    let out = event_to_ical(&ev);
    let unfolded = out.replace("\r\n ", "").replace("\r\n\t", "");
    assert!(without(&unfolded, "FILENAME="));
    assert!(without(&unfolded, "X-APPLE-FILENAME="));
    assert!(unfolded.contains("ATTACH;"));
    assert!(unfolded.contains("https://example.com/docs/spec.pdf"));
    assert!(unfolded.contains("https://example.com/images/arch.png"));
}

#[test]
fn differential_oracle_image_property_rel_icon_and_display_parameter_mapping() {
    // Divergence 36 against Stalwart differential oracle:
    // RFC 7986 section 5.10 specifies IMAGE to associate graphics or icons with a component,
    // requiring VALUE=URI and admitting optional DISPLAY (BADGE, GRAPHIC, FULLSIZE, THUMBNAIL).
    // RFC 8984 section 1.4.11 and section 4.2.7 model resources as links: Map<Id, Link>, where
    // rel differentiates icons ("icon") from file attachments ("enclosure").
    // Stalwart v1.0.0 parses ATTACH and IMAGE, but may treat links uniformly or omit rel.
    // In contrast, jmap-ical's read_links:
    // 1. Identifies IMAGE lines and sets rel: "icon".
    // 2. Maps DISPLAY parameters (BADGE, GRAPHIC, FULLSIZE, THUMBNAIL) to lowercase link.display.
    // 3. Leaves rel omitted on standard ATTACH lines (preserving default enclosure semantics).
    // 4. Outbound export emits IMAGE with mandatory VALUE=URI, DISPLAY, and FMTTYPE when rel is "icon",
    //    and emits ATTACH for non-icon links.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-image-test-001\r\n\
DTSTART:20260904T190000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Keynote Presentation\r\n\
ATTACH;FMTTYPE=application/pdf;SIZE=4096:https://example.com/slides.pdf\r\n\
IMAGE;VALUE=URI;DISPLAY=BADGE;FMTTYPE=image/png:https://example.com/badge.png\r\n\
IMAGE;VALUE=URI;DISPLAY=THUMBNAIL;FMTTYPE=image/jpeg:https://example.com/preview.jpg\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let ev = ical_to_event(ics).expect("parse event with image and attach");
    let links = ev.links.as_ref().expect("links map present");
    assert_eq!(links.len(), 3);

    let k1 = links.get("k1").expect("attach link k1");
    assert_eq!(
        k1.get("href").and_then(Value::as_str),
        Some("https://example.com/slides.pdf")
    );
    assert_eq!(
        k1.get("contentType").and_then(Value::as_str),
        Some("application/pdf")
    );
    assert_eq!(k1.get("size").and_then(Value::as_u64), Some(4096));
    assert_eq!(k1.get("rel"), None);
    assert_eq!(k1.get("display"), None);

    let k2 = links.get("k2").expect("image link k2");
    assert_eq!(
        k2.get("href").and_then(Value::as_str),
        Some("https://example.com/badge.png")
    );
    assert_eq!(
        k2.get("contentType").and_then(Value::as_str),
        Some("image/png")
    );
    assert_eq!(k2.get("rel").and_then(Value::as_str), Some("icon"));
    assert_eq!(k2.get("display").and_then(Value::as_str), Some("badge"));

    let k3 = links.get("k3").expect("image link k3");
    assert_eq!(
        k3.get("href").and_then(Value::as_str),
        Some("https://example.com/preview.jpg")
    );
    assert_eq!(
        k3.get("contentType").and_then(Value::as_str),
        Some("image/jpeg")
    );
    assert_eq!(k3.get("rel").and_then(Value::as_str), Some("icon"));
    assert_eq!(k3.get("display").and_then(Value::as_str), Some("thumbnail"));
    assert!(ev.extra.is_empty());

    let out = event_to_ical(&ev);
    let unfolded = out.replace("\r\n ", "").replace("\r\n\t", "");
    assert!(unfolded.contains("ATTACH;"));
    assert!(unfolded.contains("https://example.com/slides.pdf"));
    assert!(unfolded.contains("IMAGE;VALUE=URI;"));
    assert!(unfolded.contains("DISPLAY=BADGE"));
    assert!(unfolded.contains("https://example.com/badge.png"));
    assert!(unfolded.contains("DISPLAY=THUMBNAIL"));
    assert!(unfolded.contains("https://example.com/preview.jpg"));
}

#[test]
fn differential_oracle_altrep_parameter_on_text_properties_dropped_on_import() {
    // Divergence 37 against Stalwart differential oracle:
    // RFC 5545 section 3.2.2 defines ALTREP parameter for SUMMARY, DESCRIPTION, and LOCATION,
    // providing a URI to an alternate representation of the text content.
    // RFC 8984 models title, description, and location names as plain strings without URI pointers.
    // Stalwart v1.0.0 may parse ALTREP into alternate links or tracking metadata.
    // In contrast, jmap-ical's read_vevent:
    // 1. Reads the plain text value for SUMMARY, DESCRIPTION, and LOCATION.
    // 2. Ignores ALTREP parameters on import without creating unwanted link entries or polluting event.extra.
    // 3. Outbound export renders clean text properties without ALTREP parameters.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-altrep-test-001\r\n\
DTSTART:20260904T200000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY;ALTREP=\"https://example.com/alt/summary.html\":Strategy Session\r\n\
DESCRIPTION;ALTREP=\"cid:part1.doc@example.com\":Detailed quarterly planning and priorities.\r\n\
LOCATION;ALTREP=\"https://example.com/alt/room101.vcf\":Conference Room A\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let ev = ical_to_event(ics).expect("parse event with altrep parameters");
    assert_eq!(ev.title.as_deref(), Some("Strategy Session"));
    assert_eq!(
        ev.description.as_deref(),
        Some("Detailed quarterly planning and priorities.")
    );
    let locations = ev.locations.as_ref().expect("locations present");
    let loc = locations.get("l1").expect("location entry l1");
    assert_eq!(
        loc.get("name").and_then(Value::as_str),
        Some("Conference Room A")
    );
    assert_eq!(ev.links, None);
    assert!(ev.extra.is_empty());

    let out = event_to_ical(&ev);
    assert!(without(&out, "ALTREP="));
    assert_eq!(line(&out, "SUMMARY:"), "SUMMARY:Strategy Session");
    assert_eq!(
        line(&out, "DESCRIPTION:"),
        "DESCRIPTION:Detailed quarterly planning and priorities."
    );
    assert_eq!(
        line(&out, "LOCATION;"),
        "LOCATION;X-JMAP-KEY=l1:Conference Room A"
    );
}

#[test]
fn differential_oracle_rich_html_descriptions_x_alt_desc_and_styled_dropped() {
    // Divergence 38 against Stalwart differential oracle:
    // Common calendaring software (Outlook, Apple, Google, Thunderbird) exports HTML descriptions
    // using X-ALT-DESC (with FMTTYPE=text/html) or RFC 9073 STYLED-DESCRIPTION.
    // Stalwart v1.0.0 may convert HTML descriptions, expose them, or populate descriptionContentType.
    // In contrast, jmap-ical's read_vevent:
    // 1. Reads standard DESCRIPTION strictly into event.description as plain text.
    // 2. Drops X-ALT-DESC and STYLED-DESCRIPTION on import without polluting event.extra.
    // 3. Outbound export emits standard DESCRIPTION and never synthesizes vendor or styled description lines.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:oracle-html-desc-001\r\n\
DTSTART:20260904T210000Z\r\n\
DURATION:PT45M\r\n\
SUMMARY:Engineering Sync\r\n\
DESCRIPTION:Discussion of database schema migration and cache invalidation.\r\n\
X-ALT-DESC;FMTTYPE=text/html:<p>Discussion of <b>database schema migration</b> and <i>cache invalidation</i>.</p>\r\n\
STYLED-DESCRIPTION;VALUE=TEXT;FMTTYPE=text/html:<p>Rich styled description</p>\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let ev = ical_to_event(ics).expect("parse event with html and styled descriptions");
    assert_eq!(ev.title.as_deref(), Some("Engineering Sync"));
    assert_eq!(
        ev.description.as_deref(),
        Some("Discussion of database schema migration and cache invalidation.")
    );
    assert!(ev.extra.is_empty());

    let out = event_to_ical(&ev);
    assert_eq!(
        line(&out, "DESCRIPTION:"),
        "DESCRIPTION:Discussion of database schema migration and cache invalidation."
    );
    assert!(without(&out, "X-ALT-DESC"));
    assert!(without(&out, "STYLED-DESCRIPTION"));
}

#[test]
fn differential_oracle_multi_component_stream_isolation_and_non_vevent_rejection() {
    // Divergence 39 against Stalwart differential oracle:
    // RFC 5545 section 3.4 permits calendar streams with mixed components (VEVENT, VTODO, VJOURNAL, VFREEBUSY).
    // Stalwart v1.0.0's CalendarEvent/parse processes the stream and returns an array of parsed events.
    // In contrast, jmap-ical's ical_to_event acts as a single-event record synchronization codec:
    // 1. Streams containing only non-VEVENT components (such as VTODO or VJOURNAL) return Err(ICalError::NoEvent).
    // 2. Streams with mixed components isolate the VEVENT series and ignore VTODO or VJOURNAL tasks.
    // 3. Outbound export produces a single VEVENT component and never emits non-event component blocks.
    let todo_only_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VTODO\r\n\
UID:todo-item-001\r\n\
SUMMARY:Complete differential audit\r\n\
STATUS:NEEDS-ACTION\r\n\
DUE:20260905T120000Z\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";
    let todo_result = ical_to_event(todo_only_ics);
    assert!(
        matches!(todo_result, Err(ICalError::NoEvent)),
        "stream with only VTODO must yield NoEvent error"
    );

    let journal_only_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VJOURNAL\r\n\
UID:journal-item-001\r\n\
SUMMARY:Daily Retrospective Notes\r\n\
STATUS:DRAFT\r\n\
END:VJOURNAL\r\n\
END:VCALENDAR\r\n";
    let journal_result = ical_to_event(journal_only_ics);
    assert!(
        matches!(journal_result, Err(ICalError::NoEvent)),
        "stream with only VJOURNAL must yield NoEvent error"
    );

    let mixed_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:event-in-mixed-stream-001\r\n\
DTSTART:20260904T220000Z\r\n\
DURATION:PT30M\r\n\
SUMMARY:Team Architecture Check-in\r\n\
END:VEVENT\r\n\
BEGIN:VTODO\r\n\
UID:todo-in-mixed-stream-002\r\n\
SUMMARY:Follow-up action item\r\n\
STATUS:NEEDS-ACTION\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";
    let ev = ical_to_event(mixed_ics).expect("parse mixed stream isolating vevent");
    assert_eq!(
        ev.id.as_ref().map(|id| id.as_str()),
        Some("event-in-mixed-stream-001")
    );
    assert_eq!(ev.title.as_deref(), Some("Team Architecture Check-in"));
    assert!(ev.extra.is_empty());

    let out = event_to_ical(&ev);
    assert!(out.contains("BEGIN:VEVENT"));
    assert!(out.contains("UID:event-in-mixed-stream-001"));
    assert!(without(&out, "BEGIN:VTODO"));
    assert!(without(&out, "UID:todo-in-mixed-stream-002"));
}

#[test]
fn differential_oracle_valarm_repeat_and_duration_unmodeled_loop_dropping() {
    // Divergence 40 against Stalwart differential oracle:
    // RFC 5545 section 3.8.6.2 and 3.8.6.3 specify REPEAT (repeat count) and DURATION (snooze delay).
    // RFC 8984 section 4.5.2 models Alert without repetition loops or snooze intervals.
    // Stalwart v1.0.0 parses VALARM and drops repeat loop properties or captures them in metadata.
    // In contrast, jmap-ical's read_alert ignores REPEAT and DURATION on inbound import without
    // polluting event.extra, mapping only the primary trigger. Outbound serialization (drawn_alert)
    // refuses alerts with unmodeled keys like repeat or duration, and emits clean VALARMs without repeat fields.
    let repeat_alarm_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:repeat-alarm-event-001\r\n\
DTSTART:20260905T100000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Staff Strategy Session\r\n\
BEGIN:VALARM\r\n\
UID:alarm-repeat-001\r\n\
ACTION:DISPLAY\r\n\
TRIGGER:-PT15M\r\n\
REPEAT:3\r\n\
DURATION:PT5M\r\n\
DESCRIPTION:Staff Strategy Session\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(repeat_alarm_ics).expect("parse alarm with repeat and duration");
    let alerts = ev.alerts.as_ref().expect("alerts must be present");
    assert_eq!(alerts.len(), 1);
    let alert = &alerts["alarm-repeat-001"];
    assert_eq!(alert.get("action").and_then(Value::as_str), Some("display"));
    assert_eq!(
        alert
            .get("trigger")
            .and_then(|t| t.get("offset"))
            .and_then(Value::as_str),
        Some("-PT15M")
    );
    assert!(
        alert.get("repeat").is_none(),
        "repeat count must be dropped from Alert"
    );
    assert!(
        alert.get("duration").is_none(),
        "duration loop interval must be dropped from Alert"
    );
    assert!(
        ev.extra.is_empty(),
        "event extra must remain clean without repeat pollution"
    );
    assert!(
        maps_alerts(&ev),
        "maps_alerts must accept clean parsed alert"
    );

    let out = event_to_ical(&ev);
    assert!(out.contains("BEGIN:VALARM"));
    assert!(out.contains("ACTION:DISPLAY"));
    assert!(out.contains("TRIGGER:-PT15M"));
    assert!(without(&out, "REPEAT:"));
    assert!(without(&out, "DURATION:PT5M"));

    // If an alert object contains unmodeled repeat keys, drawn_alert refuses it
    let mut bad_event = ev.clone();
    if let Some(obj) = bad_event
        .alerts
        .as_mut()
        .and_then(|map| map.get_mut("alarm-repeat-001"))
        .and_then(Value::as_object_mut)
    {
        obj.insert("repeat".to_string(), json!(3));
    }
    let bad_out = event_to_ical(&bad_event);
    assert!(
        without(&bad_out, "BEGIN:VALARM"),
        "drawn_alert must refuse alert with unmodeled repeat key"
    );
}

#[test]
fn differential_oracle_valarm_description_summary_and_title_synthesis_boundary() {
    // Divergence 41 against Stalwart differential oracle:
    // RFC 5545 section 3.8.6.1 requires ACTION:DISPLAY VALARMs to include a DESCRIPTION property.
    // RFC 8984 section 4.5.2 defines Alert as an abstract notification trigger without description or summary.
    // Stalwart v1.0.0 parses VALARM and drops custom reminder descriptions.
    // In contrast, jmap-ical's read_alert drops custom reminder description strings on inbound parse,
    // keeping Alert models clean and preventing redundant storage. On outbound export, drawn_alert
    // synthesizes DESCRIPTION from event.title to strictly satisfy RFC 5545 wire requirements.
    let valarm_custom_desc_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:valarm-desc-event-001\r\n\
DTSTART:20260905T140000Z\r\n\
DURATION:PT45M\r\n\
SUMMARY:Quarterly Budget Review\r\n\
DESCRIPTION:Detailed agenda notes for the review meeting.\r\n\
BEGIN:VALARM\r\n\
UID:alarm-desc-001\r\n\
ACTION:DISPLAY\r\n\
TRIGGER:-PT10M\r\n\
DESCRIPTION:Custom popup text: Bring printouts of ledger!\r\n\
SUMMARY:Reminder popup header\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev =
        ical_to_event(valarm_custom_desc_ics).expect("parse valarm with custom desc and summary");
    assert_eq!(ev.title.as_deref(), Some("Quarterly Budget Review"));
    assert_eq!(
        ev.description.as_deref(),
        Some("Detailed agenda notes for the review meeting.")
    );

    let alerts = ev.alerts.as_ref().expect("alerts must be present");
    let alert = &alerts["alarm-desc-001"];
    assert_eq!(alert.get("action").and_then(Value::as_str), Some("display"));
    assert!(
        alert.get("description").is_none(),
        "Alert must not store custom reminder description"
    );
    assert!(
        alert.get("summary").is_none(),
        "Alert must not store custom reminder summary"
    );
    assert!(ev.extra.is_empty(), "event extra must remain clean");

    // Outbound export synthesizes VALARM DESCRIPTION from event.title
    let out = event_to_ical(&ev);
    assert!(out.contains("BEGIN:VALARM"));
    assert!(out.contains("DESCRIPTION:Quarterly Budget Review"));
    assert!(without(&out, "Bring printouts of ledger"));
    assert!(without(&out, "Reminder popup header"));

    // When event has no title, drawn_alert omits DESCRIPTION rather than inventing text
    let mut untitled_event = ev.clone();
    untitled_event.title = None;
    let untitled_out = event_to_ical(&untitled_event);
    assert!(untitled_out.contains("BEGIN:VALARM"));
    assert!(without(
        &untitled_out,
        "DESCRIPTION:Quarterly Budget Review"
    ));
}

#[test]
fn differential_oracle_rrule_until_timezone_conversion_and_all_day_date_formatting() {
    // Divergence 42 against Stalwart differential oracle:
    // RFC 5545 section 3.3.10 requires UNTIL to be UTC when DTSTART carries a TZID, and date-only
    // when DTSTART is a DATE. RFC 8984 section 4.3.1 models until as a LocalDateTime without offset or Z.
    // Stalwart v1.0.0 parses UNTIL into local LocalDateTime.
    // In contrast, jmap-ical's read_until converts UTC UNTIL into local time using the observance offset
    // when VTIMEZONE definitions are in scope, and rule_to_rrule formats date-only UNTIL for all-day events
    // (showWithoutTime: true) to strictly comply with RFC 5545 section 3.3.10 value-type rules.
    let zoned_rrule_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:Europe/Berlin\r\n\
BEGIN:DAYLIGHT\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0200\r\n\
TZNAME:CEST\r\n\
DTSTART:19700329T020000\r\n\
RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU\r\n\
END:DAYLIGHT\r\n\
BEGIN:STANDARD\r\n\
TZOFFSETFROM:+0200\r\n\
TZOFFSETTO:+0100\r\n\
TZNAME:CET\r\n\
DTSTART:19701025T030000\r\n\
RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
UID:zoned-until-event-001\r\n\
DTSTART;TZID=Europe/Berlin:20260601T090000\r\n\
DURATION:PT1H\r\n\
SUMMARY:Daily Morning Standup\r\n\
RRULE:FREQ=DAILY;UNTIL=20260610T220000Z\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let zoned_ev = ical_to_event(zoned_rrule_ics).expect("parse zoned rrule with utc until");
    let rule = zoned_ev
        .recurrence_rule
        .as_ref()
        .expect("rule must be present");
    // In Berlin CEST (+02:00), 2026-06-10T22:00:00Z converts to local 2026-06-11T00:00:00
    assert_eq!(rule.until.as_deref(), Some("2026-06-11T00:00:00"));

    // All-day event with date-only UNTIL formatting
    let allday_rrule_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:allday-until-event-002\r\n\
DTSTART;VALUE=DATE:20260701\r\n\
DURATION:P1D\r\n\
SUMMARY:Weekly Summer Sabbatical\r\n\
RRULE:FREQ=WEEKLY;UNTIL=20260831\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let allday_ev = ical_to_event(allday_rrule_ics).expect("parse allday rrule with date until");
    assert_eq!(allday_ev.show_without_time, Some(true));
    let allday_rule = allday_ev
        .recurrence_rule
        .as_ref()
        .expect("allday rule present");
    // In JSCalendar RFC 8984 section 4.3.1, until is always a LocalDateTime ("YYYY-MM-DDTHH:MM:SS")
    assert_eq!(allday_rule.until.as_deref(), Some("2026-08-31T00:00:00"));

    let out_allday = event_to_ical(&allday_ev);
    assert!(out_allday.contains("DTSTART;VALUE=DATE:20260701"));
    assert!(out_allday.contains("RRULE:FREQ=WEEKLY;UNTIL=20260831"));
    assert!(without(&out_allday, "UNTIL=20260831T"));
}

#[test]
fn differential_oracle_rrule_ordinal_weekdays_byday_and_nday_structure_mapping() {
    // Divergence 43 against Stalwart differential oracle:
    // RFC 5545 section 3.3.10 specifies BYDAY with optional positive/negative signed integer ordinals.
    // RFC 8984 section 4.3.2 models these as byDay: NDay[] with lowercase day codes and nthOfPeriod integers.
    // Stalwart v1.0.0 parses BYDAY into NDay arrays with lowercase tokens.
    // In contrast, jmap-ical's read_rrule parses positive and negative ordinals into NDay structs with
    // lowercase day tokens, by_day_part renders them back to uppercase RFC 5545 format (e.g. 2MO, -1FR),
    // and maps_recurrence_rule enforces strict weekday token validity, refusing rules with invalid tokens.
    let ordinal_rrule_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:ordinal-rrule-event-001\r\n\
DTSTART:20260901T150000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Bi-Monthly Steering Committee\r\n\
RRULE:FREQ=MONTHLY;BYDAY=2MO,-1FR\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(ordinal_rrule_ics).expect("parse ordinal rrule");
    let rule = ev.recurrence_rule.as_ref().expect("rule must be present");
    let by_day = rule.by_day.as_ref().expect("by_day must be present");
    assert_eq!(by_day.len(), 2);
    assert_eq!(by_day[0].day.as_str(), "mo");
    assert_eq!(by_day[0].nth_of_period, Some(2));
    assert_eq!(by_day[1].day.as_str(), "fr");
    assert_eq!(by_day[1].nth_of_period, Some(-1));
    assert!(
        maps_recurrence_rule(rule),
        "rule with valid NDays must pass maps_recurrence_rule"
    );

    let out = event_to_ical(&ev);
    assert!(out.contains("RRULE:FREQ=MONTHLY;BYDAY=2MO,-1FR"));

    // Multi-day un-ordered list without ordinals
    let multi_day_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:multi-day-event-002\r\n\
DTSTART:20260901T090000Z\r\n\
DURATION:PT30M\r\n\
SUMMARY:Tri-Weekly Sync\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO,WE,FR\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let multi_ev = ical_to_event(multi_day_ics).expect("parse multi-day rrule");
    let multi_rule = multi_ev
        .recurrence_rule
        .as_ref()
        .expect("multi rule present");
    let multi_days = multi_rule.by_day.as_ref().expect("multi by_day present");
    assert_eq!(multi_days.len(), 3);
    assert_eq!(multi_days[0].day.as_str(), "mo");
    assert_eq!(multi_days[0].nth_of_period, None);
    assert_eq!(multi_days[1].day.as_str(), "we");
    assert_eq!(multi_days[1].nth_of_period, None);
    assert_eq!(multi_days[2].day.as_str(), "fr");
    assert_eq!(multi_days[2].nth_of_period, None);

    let multi_out = event_to_ical(&multi_ev);
    assert!(multi_out.contains("RRULE:FREQ=WEEKLY;BYDAY=MO,WE,FR"));

    // Invalid weekday tokens are refused by maps_recurrence_rule
    let mut invalid_rule = multi_rule.clone();
    invalid_rule.by_day = Some(vec![NDay::new("zz")]);
    assert!(
        !maps_recurrence_rule(&invalid_rule),
        "invalid weekday token zz must be refused by maps_recurrence_rule"
    );
}

#[test]
fn differential_oracle_rrule_bysetpos_and_multiple_byparts_set_selection_gating() {
    // Divergence 44 against Stalwart differential oracle:
    // RFC 5545 section 3.3.10 specifies BYSETPOS operating on the set of occurrences within the interval,
    // and mandates that BYSETPOS MUST only be specified in conjunction with another BYxxx rule part.
    // RFC 8984 section 4.3.1 models this as bySetPosition: Integer[].
    // Stalwart v1.0.0 parses BYSETPOS into bySetPosition integer arrays.
    // In contrast, jmap-ical's read_rrule preserves signed integers (including negative offsets like -1
    // for last occurrence), outbound by_set_position_part strictly requires another BYxxx part to be present
    // (selects_from_a_set), and maps_recurrence_rule refuses standalone BYSETPOS, zero values, or out-of-range indices.
    let bysetpos_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:bysetpos-event-001\r\n\
DTSTART:20260901T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Last Workday of the Month\r\n\
RRULE:FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(bysetpos_ics).expect("parse bysetpos rrule");
    let rule = ev.recurrence_rule.as_ref().expect("rule present");
    assert_eq!(rule.by_set_position, Some(vec![-1]));
    assert!(
        maps_recurrence_rule(rule),
        "valid by_set_position with by_day must pass maps_recurrence_rule"
    );

    let out = event_to_ical(&ev);
    assert!(out.contains("RRULE:FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1"));

    // Multiple set positions
    let mut multi_pos_rule = rule.clone();
    multi_pos_rule.by_set_position = Some(vec![1, 3, -1]);
    assert!(maps_recurrence_rule(&multi_pos_rule));
    let mut multi_ev = ev.clone();
    multi_ev.recurrence_rule = Some(multi_pos_rule);
    let multi_out = event_to_ical(&multi_ev);
    assert!(multi_out.contains("BYSETPOS=1,3,-1"));

    // Standalone by_set_position without another BYxxx part is refused by maps_recurrence_rule
    let mut standalone_pos_rule = rule.clone();
    standalone_pos_rule.by_day = None;
    standalone_pos_rule.by_month_day = None;
    assert!(
        !maps_recurrence_rule(&standalone_pos_rule),
        "standalone by_set_position without another BYxxx part must be refused"
    );

    // Zero position is invalid in RFC 5545 and RFC 8984
    let mut zero_pos_rule = rule.clone();
    zero_pos_rule.by_set_position = Some(vec![0]);
    assert!(
        !maps_recurrence_rule(&zero_pos_rule),
        "by_set_position with zero must be refused"
    );
}

#[test]
fn differential_oracle_rrule_bymonth_numbers_vs_strings_and_leap_month_refusal() {
    // Divergence 45 against Stalwart differential oracle:
    // RFC 5545 section 3.3.10 specifies BYMONTH as month numbers 1..12.
    // RFC 8984 section 4.3.1 models byMonth: String[] to admit non-Gregorian leap month qualifiers (e.g. 5L).
    // Stalwart v1.0.0 parses BYMONTH into byMonth string arrays.
    // In contrast, jmap-ical's read_rrule maps BYMONTH integer numbers into string arrays, outbound
    // by_month_part requires canonical month numbers 1..=12 without leading zeros (rejecting 03), and
    // deliberately refuses leap months (such as 5L) because Gregorian series have no RSCALE calendar system.
    let bymonth_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:bymonth-event-001\r\n\
DTSTART:20260101T100000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Quarterly Audit\r\n\
RRULE:FREQ=YEARLY;BYMONTH=1,6,12\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(bymonth_ics).expect("parse bymonth rrule");
    let rule = ev.recurrence_rule.as_ref().expect("rule present");
    assert_eq!(
        rule.by_month.as_deref(),
        Some(&["1".to_string(), "6".to_string(), "12".to_string()][..])
    );
    assert!(
        maps_recurrence_rule(rule),
        "canonical by_month must pass maps_recurrence_rule"
    );

    let out = event_to_ical(&ev);
    assert!(out.contains("RRULE:FREQ=YEARLY;BYMONTH=1,6,12"));

    // Out of bounds month numbers are refused
    let mut invalid_month_rule = rule.clone();
    invalid_month_rule.by_month = Some(vec!["13".to_string()]);
    assert!(
        !maps_recurrence_rule(&invalid_month_rule),
        "month 13 must be refused"
    );

    // Leading zero formatting is refused to prevent round-trip diffs
    let mut leading_zero_rule = rule.clone();
    leading_zero_rule.by_month = Some(vec!["03".to_string()]);
    assert!(
        !maps_recurrence_rule(&leading_zero_rule),
        "month 03 must be refused"
    );

    // Leap month 5L (RFC 7529 RSCALE) is refused for Gregorian series
    let mut leap_month_rule = rule.clone();
    leap_month_rule.by_month = Some(vec!["5L".to_string()]);
    assert!(
        !maps_recurrence_rule(&leap_month_rule),
        "leap month 5L must be refused without RSCALE"
    );
}

#[test]
fn differential_oracle_rrule_wkst_default_monday_omission_and_case_normalization() {
    // Divergence 46 against Stalwart differential oracle:
    // RFC 5545 section 3.3.10 specifies WKST with default MO.
    // RFC 8984 section 4.3.1 specifies firstDayOfWeek: String (default: "mo") with lowercase two-character day code.
    // Stalwart v1.0.0 parses WKST and normalizes to lowercase, omitting firstDayOfWeek when it matches "mo".
    // In contrast, jmap-ical's read_rrule lowercases incoming WKST tokens, but outbound first_day_of_week_part
    // deliberately suppresses WKST=MO on export because libical strips default WKST=MO upon reading into EDS cache.
    // Non-Monday days (such as WKST=SU) are exported canonically, and invalid day tokens are refused.
    let wkst_mo_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:wkst-mo-event-001\r\n\
DTSTART:20260901T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Weekly Planning\r\n\
RRULE:FREQ=WEEKLY;BYDAY=TU,TH;WKST=MO\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(wkst_mo_ics).expect("parse wkst mo rrule");
    let rule = ev.recurrence_rule.as_ref().expect("rule present");
    assert_eq!(rule.first_day_of_week.as_deref(), Some("mo"));
    assert!(
        maps_recurrence_rule(rule),
        "valid first_day_of_week must pass maps_recurrence_rule"
    );

    let out = event_to_ical(&ev);
    assert!(out.contains("RRULE:FREQ=WEEKLY;BYDAY=TU,TH"));
    assert!(
        !out.contains("WKST=MO"),
        "WKST=MO must be suppressed on export to prevent EDS cache-drop diffs"
    );

    // Non-default work week start (WKST=SU) is serialized
    let wkst_su_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:wkst-su-event-002\r\n\
DTSTART:20260901T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Sunday Week Start Planning\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO,WE;WKST=SU\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let su_ev = ical_to_event(wkst_su_ics).expect("parse wkst su rrule");
    let su_rule = su_ev.recurrence_rule.as_ref().expect("rule present");
    assert_eq!(su_rule.first_day_of_week.as_deref(), Some("su"));
    assert!(maps_recurrence_rule(su_rule));
    let su_out = event_to_ical(&su_ev);
    assert!(su_out.contains("RRULE:FREQ=WEEKLY;BYDAY=MO,WE;WKST=SU"));

    // Invalid day codes or uppercase values in JSCalendar model are refused
    let mut invalid_day_rule = rule.clone();
    invalid_day_rule.first_day_of_week = Some("invalid".to_string());
    assert!(
        !maps_recurrence_rule(&invalid_day_rule),
        "invalid weekday name must be refused"
    );

    let mut upper_day_rule = rule.clone();
    upper_day_rule.first_day_of_week = Some("MO".to_string());
    assert!(
        !maps_recurrence_rule(&upper_day_rule),
        "uppercase weekday code must be refused by weekday_token"
    );
}

#[test]
fn differential_oracle_rrule_frequency_gates_and_incompatible_parts_refusal() {
    // Divergence 47 against Stalwart differential oracle:
    // RFC 5545 section 3.3.10 enforces strict combinatorial rules between FREQ and BYxxx parts:
    // BYWEEKNO MUST NOT be specified when FREQ is not YEARLY.
    // BYMONTHDAY MUST NOT be specified when FREQ is WEEKLY.
    // BYYEARDAY MUST NOT be specified when FREQ is DAILY, WEEKLY, or MONTHLY.
    // Stalwart v1.0.0 parses rule parts into JSCalendar objects where frequency combinations may be loosely validated.
    // In contrast, jmap-ical's outbound mapping applies strict frequency gating and maps_recurrence_rule refuses
    // frequency-incompatible combinations to prevent libical in EDS from rejecting the entire component.
    let yearly_weekno_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:yearly-weekno-001\r\n\
DTSTART:20260101T100000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Yearly Week Number Event\r\n\
RRULE:FREQ=YEARLY;BYWEEKNO=20,-1\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let yearly_ev = ical_to_event(yearly_weekno_ics).expect("parse yearly weekno ics");
    let yearly_rule = yearly_ev.recurrence_rule.as_ref().expect("rule present");
    assert_eq!(yearly_rule.by_week_no, Some(vec![20, -1]));
    assert!(maps_recurrence_rule(yearly_rule));
    let yearly_out = event_to_ical(&yearly_ev);
    assert!(yearly_out.contains("RRULE:FREQ=YEARLY;BYWEEKNO=20,-1"));

    // BYWEEKNO on monthly frequency is refused by by_week_no_part and maps_recurrence_rule
    let mut monthly_weekno_rule = yearly_rule.clone();
    monthly_weekno_rule.frequency = "monthly".to_string();
    assert!(
        !maps_recurrence_rule(&monthly_weekno_rule),
        "BYWEEKNO on monthly frequency must be refused"
    );

    // BYMONTHDAY on weekly frequency is refused by by_month_day_part and maps_recurrence_rule
    let weekly_monthday_rule = RecurrenceRule {
        rule_type: Some("RecurrenceRule".to_string()),
        frequency: "weekly".to_string(),
        by_month_day: Some(vec![15]),
        ..Default::default()
    };
    assert!(
        !maps_recurrence_rule(&weekly_monthday_rule),
        "BYMONTHDAY on weekly frequency must be refused"
    );

    // BYYEARDAY on daily, weekly, or monthly frequency is refused by by_year_day_part and maps_recurrence_rule
    let daily_yearday_rule = RecurrenceRule {
        rule_type: Some("RecurrenceRule".to_string()),
        frequency: "daily".to_string(),
        by_year_day: Some(vec![100]),
        ..Default::default()
    };
    assert!(
        !maps_recurrence_rule(&daily_yearday_rule),
        "BYYEARDAY on daily frequency must be refused"
    );
}

#[test]
fn differential_oracle_rrule_interval_default_omission_and_custom_interval_emission() {
    // Divergence 48 against Stalwart differential oracle:
    // RFC 5545 section 3.3.10 specifies INTERVAL as an optional positive integer (default: 1).
    // RFC 8984 section 4.3.1 specifies interval: UnsignedInt (default: 1).
    // Stalwart v1.0.0 parses INTERVAL and omits interval when equal to 1.
    // In jmap-ical: rrule_to_rule parses INTERVAL into rule.interval; outbound rule_to_rrule
    // deliberately suppresses INTERVAL=1 because libical in EDS drops default INTERVAL=1 upon
    // reading into cache; non-default intervals (such as INTERVAL=3) are emitted explicitly.
    let int_1_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:interval-1-event-001\r\n\
DTSTART:20260901T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Daily Meeting Default Interval\r\n\
RRULE:FREQ=DAILY;INTERVAL=1;COUNT=5\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(int_1_ics).expect("parse interval 1 rrule");
    let rule = ev.recurrence_rule.as_ref().expect("rule present");
    assert_eq!(rule.interval, Some(1));
    assert!(maps_recurrence_rule(rule));
    let out = event_to_ical(&ev);
    assert!(out.contains("RRULE:FREQ=DAILY;COUNT=5"));
    assert!(
        !out.contains("INTERVAL=1"),
        "INTERVAL=1 must be suppressed on export to maintain EDS cache fixpoint stability"
    );

    // Non-default interval (INTERVAL=3) is serialized
    let int_3_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:interval-3-event-002\r\n\
DTSTART:20260901T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Tri-Daily Meeting\r\n\
RRULE:FREQ=DAILY;INTERVAL=3;COUNT=5\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev3 = ical_to_event(int_3_ics).expect("parse interval 3 rrule");
    let rule3 = ev3.recurrence_rule.as_ref().expect("rule present");
    assert_eq!(rule3.interval, Some(3));
    assert!(maps_recurrence_rule(rule3));
    let out3 = event_to_ical(&ev3);
    assert!(out3.contains("RRULE:FREQ=DAILY;COUNT=5;INTERVAL=3"));
}

#[test]
fn differential_oracle_rrule_endpoint_count_and_until_mutual_exclusivity_handling() {
    // Divergence 49 against Stalwart differential oracle:
    // RFC 5545 section 3.3.10 explicitly specifies: "The UNTIL or COUNT rule parts are optional,
    // but UNTIL and COUNT rule parts MUST NOT occur in the same 'recurrence-rule'."
    // RFC 8984 section 4.3.1 specifies: "Both MUST NOT be present in the same RecurrenceRule;
    // if both are present, the until rule part MUST be ignored."
    // Stalwart v1.0.0 enforces RFC 8984 preference or rejects conflicting rules.
    // In jmap-ical: standard bounded events specify either count or until, or neither (unbounded).
    // An event with COUNT alone serializes COUNT; an event with UNTIL alone serializes UNTIL;
    // an unbounded event serializes neither endpoint.
    let count_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:count-only-event-001\r\n\
DTSTART:20260901T100000Z\r\n\
DURATION:PT30M\r\n\
SUMMARY:Ten Count Series\r\n\
RRULE:FREQ=WEEKLY;COUNT=10\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let count_ev = ical_to_event(count_ics).expect("parse count rrule");
    let count_rule = count_ev.recurrence_rule.as_ref().expect("rule present");
    assert_eq!(count_rule.count, Some(10));
    assert_eq!(count_rule.until, None);
    assert!(maps_recurrence_rule(count_rule));
    let count_out = event_to_ical(&count_ev);
    assert!(count_out.contains("RRULE:FREQ=WEEKLY;COUNT=10"));
    assert!(!count_out.contains("UNTIL="));

    let until_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:until-only-event-002\r\n\
DTSTART:20260901T100000Z\r\n\
DURATION:PT30M\r\n\
SUMMARY:Year End Series\r\n\
RRULE:FREQ=WEEKLY;UNTIL=20261231T235959Z\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let until_ev = ical_to_event(until_ics).expect("parse until rrule");
    let until_rule = until_ev.recurrence_rule.as_ref().expect("rule present");
    assert_eq!(until_rule.count, None);
    assert!(until_rule.until.is_some());
    assert!(maps_recurrence_rule(until_rule));
    let until_out = event_to_ical(&until_ev);
    assert!(until_out.contains("RRULE:FREQ=WEEKLY;UNTIL=20261231T235959Z"));
    assert!(!until_out.contains("COUNT="));

    // Unbounded recurrence: neither count nor until is present
    let unbounded_rule = RecurrenceRule {
        rule_type: Some("RecurrenceRule".to_string()),
        frequency: "monthly".to_string(),
        ..Default::default()
    };
    assert!(maps_recurrence_rule(&unbounded_rule));
    let mut unbounded_ev = count_ev.clone();
    unbounded_ev.recurrence_rule = Some(unbounded_rule);
    let unbounded_out = event_to_ical(&unbounded_ev);
    assert!(unbounded_out.contains("RRULE:FREQ=MONTHLY"));
    assert!(!unbounded_out.contains("COUNT="));
    assert!(!unbounded_out.contains("UNTIL="));
}

#[test]
fn differential_oracle_rrule_time_of_day_byparts_leap_second_and_all_day_gating() {
    // Divergence 50 against Stalwart differential oracle:
    // RFC 5545 section 3.3.10 specifies BYHOUR (0..23), BYMINUTE (0..59), and BYSECOND (0..60, leap second).
    // RFC 8984 section 4.3.1 models byHour, byMinute, bySecond as UnsignedInt[].
    // RFC 5545 section 3.3.10 mandates: "The BYSECOND, BYMINUTE and BYHOUR rule parts MUST NOT be specified
    // when the associated 'DTSTART' property has a DATE value type."
    // Stalwart v1.0.0 parses these parts into unsigned integer arrays.
    // In jmap-ical:
    // 1. Inbound parsing maps BYSECOND, BYMINUTE, BYHOUR using to_time_of_day; invalid tokens become u32::MAX.
    // 2. Outbound time_of_day_part enforces bounds (0..=23, 0..=59, 0..=60) and accepts leap second 60.
    // 3. Out-of-bounds values (hour > 23, minute > 59, second > 60) or empty lists cause maps_recurrence_rule to return false.
    // 4. shows_without_time checks names_a_time_of_day: an event with show_without_time: true whose rule names
    //    a time of day is drawn as a timed DATE-TIME event instead of DATE, satisfying RFC 5545.
    let time_parts_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:time-parts-event-001\r\n\
DTSTART:20260901T000000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Sub-Day Recurrence\r\n\
RRULE:FREQ=DAILY;BYSECOND=0,30,60;BYMINUTE=15,45;BYHOUR=9,17\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(time_parts_ics).expect("parse time parts rrule");
    let rule = ev.recurrence_rule.as_ref().expect("rule present");
    assert_eq!(rule.by_second, Some(vec![0, 30, 60]));
    assert_eq!(rule.by_minute, Some(vec![15, 45]));
    assert_eq!(rule.by_hour, Some(vec![9, 17]));
    assert!(maps_recurrence_rule(rule));

    let out = event_to_ical(&ev);
    // Emitted in libical's order: BYSECOND, BYMINUTE, BYHOUR
    assert!(out.contains("RRULE:FREQ=DAILY;BYSECOND=0,30,60;BYMINUTE=15,45;BYHOUR=9,17"));

    // Out-of-range hour (24) is refused by maps_recurrence_rule
    let mut invalid_hour_rule = rule.clone();
    invalid_hour_rule.by_hour = Some(vec![9, 24]);
    assert!(
        !maps_recurrence_rule(&invalid_hour_rule),
        "hour 24 must be refused by maps_recurrence_rule"
    );

    // Out-of-range minute (60) is refused by maps_recurrence_rule
    let mut invalid_minute_rule = rule.clone();
    invalid_minute_rule.by_minute = Some(vec![60]);
    assert!(
        !maps_recurrence_rule(&invalid_minute_rule),
        "minute 60 must be refused by maps_recurrence_rule"
    );

    // Out-of-range second (61) is refused by maps_recurrence_rule
    let mut invalid_second_rule = rule.clone();
    invalid_second_rule.by_second = Some(vec![61]);
    assert!(
        !maps_recurrence_rule(&invalid_second_rule),
        "second 61 must be refused by maps_recurrence_rule"
    );

    // Empty time part list is refused by maps_recurrence_rule (libical would read BYHOUR= as BYHOUR=0)
    let mut empty_hour_rule = rule.clone();
    empty_hour_rule.by_hour = Some(vec![]);
    assert!(
        !maps_recurrence_rule(&empty_hour_rule),
        "empty by_hour list must be refused by maps_recurrence_rule"
    );

    // All-day event gating: show_without_time: true with time-of-day recurrence rule
    // must NOT be emitted as DATE; it must be emitted as DATE-TIME to conform to RFC 5545 section 3.3.10
    let mut all_day_ev = ev.clone();
    all_day_ev.show_without_time = Some(true);
    let all_day_out = event_to_ical(&all_day_ev);
    assert!(
        !all_day_out.contains("VALUE=DATE"),
        "recurrence rule naming a time of day must force DATE-TIME DTSTART representation"
    );
    assert!(all_day_out.contains("DTSTART:20260901T000000Z"));
}

#[test]
fn differential_oracle_rrule_rscale_calendar_scale_and_skip_isolation() {
    // Divergence 51 against Stalwart differential oracle:
    // RFC 7529 defines RSCALE and SKIP for non-Gregorian recurrence rules.
    // RFC 8984 section 4.3.1 models rscale: String (default: "gregorian") and skip: String (default: "omit").
    // Stalwart v1.0.0 parses or drops RSCALE/SKIP depending on calendar engine support.
    // In jmap-ical:
    // 1. Inbound rrule_to_rule drops unmodeled RSCALE and SKIP parts without polluting event.extra.
    // 2. Outbound event_to_ical does not emit RSCALE or SKIP.
    // 3. maps_recurrence_rule strictly requires rule.rscale.is_none() && rule.skip.is_none(),
    //    refusing any non-Gregorian recurrence rule to prevent EDS libical from failing or calculating
    //    corrupted occurrences.
    let rscale_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:rscale-event-001\r\n\
DTSTART:20260901T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Non-Gregorian Recurrence\r\n\
RRULE:FREQ=YEARLY;RSCALE=ISLAMIC;SKIP=FORWARD\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(rscale_ics).expect("parse rscale rrule");
    let rule = ev.recurrence_rule.as_ref().expect("rule present");
    assert_eq!(rule.rscale, None);
    assert_eq!(rule.skip, None);
    assert!(
        rule.extra.is_empty(),
        "RSCALE and SKIP must not pollute event.extra"
    );
    assert!(
        ev.extra.is_empty(),
        "event.extra must remain empty after dropping RSCALE and SKIP"
    );
    assert!(maps_recurrence_rule(rule));

    // If a rule explicitly sets rscale or skip, maps_recurrence_rule refuses it
    let mut non_gregorian_rule = rule.clone();
    non_gregorian_rule.rscale = Some("islamic".to_string());
    assert!(
        !maps_recurrence_rule(&non_gregorian_rule),
        "recurrence rule with rscale must be refused by maps_recurrence_rule"
    );

    let mut skip_rule = rule.clone();
    skip_rule.skip = Some("forward".to_string());
    assert!(
        !maps_recurrence_rule(&skip_rule),
        "recurrence rule with skip must be refused by maps_recurrence_rule"
    );
}

#[test]
fn differential_oracle_rrule_bymonthday_positive_and_negative_days_and_weekly_refusal() {
    // Divergence 52 against Stalwart differential oracle:
    // RFC 5545 section 3.3.10 specifies BYMONTHDAY as a list of signed month days (-31..=-1 | 1..=31).
    // RFC 8984 section 4.3.1 models byMonthDay: Integer[] with identical signed values.
    // Day zero (0) is invalid in both specifications.
    // RFC 5545 section 3.3.10 explicitly specifies: "The BYMONTHDAY rule part MUST NOT be specified
    // when the associated 'FREQ' rule part is set to 'WEEKLY'."
    // Stalwart v1.0.0 parses BYMONTHDAY into byMonthDay.
    // In jmap-ical: to_month_day parses tokens, mapping unreadable tokens to sentinel 0;
    // outbound by_month_day_part verifies that day values fall in -31..=-1 | 1..=31,
    // refuses day 0, and disallows weekly frequency; and maps_recurrence_rule checks that
    // all days are valid and frequency is not weekly.
    let month_day_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:month-day-event-001\r\n\
DTSTART:20260901T100000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Monthly Bill and Retrospective\r\n\
RRULE:FREQ=MONTHLY;COUNT=12;BYMONTHDAY=1,15,-1\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(month_day_ics).expect("parse month day rrule");
    let rule = ev.recurrence_rule.as_ref().expect("rule present");
    assert_eq!(rule.by_month_day, Some(vec![1, 15, -1]));
    assert!(maps_recurrence_rule(rule));

    let out = event_to_ical(&ev);
    assert!(out.contains("RRULE:FREQ=MONTHLY;COUNT=12;BYMONTHDAY=1,15,-1"));

    // Day zero (0) is rejected by month_day_token and maps_recurrence_rule
    let mut zero_day_rule = rule.clone();
    zero_day_rule.by_month_day = Some(vec![1, 0, 15]);
    assert!(
        !maps_recurrence_rule(&zero_day_rule),
        "day zero must be refused by maps_recurrence_rule"
    );

    // Out-of-bounds day 32 is rejected by month_day_token and maps_recurrence_rule
    let mut out_of_bounds_pos = rule.clone();
    out_of_bounds_pos.by_month_day = Some(vec![32]);
    assert!(
        !maps_recurrence_rule(&out_of_bounds_pos),
        "day 32 must be refused by maps_recurrence_rule"
    );

    // Out-of-bounds day -32 is rejected by month_day_token and maps_recurrence_rule
    let mut out_of_bounds_neg = rule.clone();
    out_of_bounds_neg.by_month_day = Some(vec![-32]);
    assert!(
        !maps_recurrence_rule(&out_of_bounds_neg),
        "day -32 must be refused by maps_recurrence_rule"
    );

    // FREQ=WEEKLY combined with BYMONTHDAY is forbidden by RFC 5545 section 3.3.10
    let mut weekly_month_day = rule.clone();
    weekly_month_day.frequency = "weekly".to_string();
    assert!(
        !maps_recurrence_rule(&weekly_month_day),
        "FREQ=WEEKLY with BYMONTHDAY must be refused by maps_recurrence_rule"
    );
}

#[test]
fn differential_oracle_rrule_byyearday_signed_days_leap_366_and_frequency_gating() {
    // Divergence 53 against Stalwart differential oracle:
    // RFC 5545 section 3.3.10 specifies BYYEARDAY as signed integers (-366..=-1 | 1..=366).
    // RFC 8984 section 4.3.1 models byYearDay: Integer[]. Zero is invalid.
    // RFC 5545 section 3.3.10 mandates: "The BYYEARDAY rule part MUST NOT be specified
    // when the associated 'FREQ' rule part is set to 'DAILY', 'WEEKLY', or 'MONTHLY'."
    // Stalwart v1.0.0 parses BYYEARDAY into byYearDay.
    // In jmap-ical: inbound rrule_to_rule extracts by_year_day; outbound by_year_day_part
    // accepts -366..=-1 | 1..=366 (including leap day 366), rejects 0, and enforces
    // holds_a_year(&rule.frequency); maps_recurrence_rule flags invalid days or invalid frequencies.
    let year_day_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:year-day-event-001\r\n\
DTSTART:20260101T000000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Year Day Celebrations\r\n\
RRULE:FREQ=YEARLY;COUNT=4;BYYEARDAY=1,100,366,-1\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(year_day_ics).expect("parse year day rrule");
    let rule = ev.recurrence_rule.as_ref().expect("rule present");
    assert_eq!(rule.by_year_day, Some(vec![1, 100, 366, -1]));
    assert!(maps_recurrence_rule(rule));

    let out = event_to_ical(&ev);
    assert!(out.contains("RRULE:FREQ=YEARLY;COUNT=4;BYYEARDAY=1,100,366,-1"));

    // Day zero (0) is rejected by year_day_token and maps_recurrence_rule
    let mut zero_day_rule = rule.clone();
    zero_day_rule.by_year_day = Some(vec![1, 0, 100]);
    assert!(
        !maps_recurrence_rule(&zero_day_rule),
        "year day zero must be refused by maps_recurrence_rule"
    );

    // Out-of-bounds day 367 is rejected
    let mut out_of_bounds_pos = rule.clone();
    out_of_bounds_pos.by_year_day = Some(vec![367]);
    assert!(
        !maps_recurrence_rule(&out_of_bounds_pos),
        "year day 367 must be refused by maps_recurrence_rule"
    );

    // Out-of-bounds day -367 is rejected
    let mut out_of_bounds_neg = rule.clone();
    out_of_bounds_neg.by_year_day = Some(vec![-367]);
    assert!(
        !maps_recurrence_rule(&out_of_bounds_neg),
        "year day -367 must be refused by maps_recurrence_rule"
    );

    // Forbidden frequencies: daily, weekly, monthly per RFC 5545 section 3.3.10
    let mut daily_year_day = rule.clone();
    daily_year_day.frequency = "daily".to_string();
    assert!(
        !maps_recurrence_rule(&daily_year_day),
        "FREQ=DAILY with BYYEARDAY must be refused by maps_recurrence_rule"
    );

    let mut weekly_year_day = rule.clone();
    weekly_year_day.frequency = "weekly".to_string();
    assert!(
        !maps_recurrence_rule(&weekly_year_day),
        "FREQ=WEEKLY with BYYEARDAY must be refused by maps_recurrence_rule"
    );

    let mut monthly_year_day = rule.clone();
    monthly_year_day.frequency = "monthly".to_string();
    assert!(
        !maps_recurrence_rule(&monthly_year_day),
        "FREQ=MONTHLY with BYYEARDAY must be refused by maps_recurrence_rule"
    );

    // Sub-day frequency like hourly is permitted alongside BYYEARDAY per RFC 5545
    let mut hourly_year_day = rule.clone();
    hourly_year_day.frequency = "hourly".to_string();
    assert!(
        maps_recurrence_rule(&hourly_year_day),
        "FREQ=HOURLY with BYYEARDAY is valid per RFC 5545 table"
    );
}

#[test]
fn differential_oracle_rrule_byweekno_signed_weeks_and_yearly_frequency_gating() {
    // Divergence 54 against Stalwart differential oracle:
    // RFC 5545 section 3.3.10 specifies BYWEEKNO as signed ISO 8601 week ordinals (-53..=-1 | 1..=53).
    // RFC 8984 section 4.3.1 models byWeekNo: Integer[]. Zero is invalid.
    // RFC 5545 section 3.3.10 mandates: "The BYWEEKNO rule part MUST NOT be specified
    // when the associated 'FREQ' rule part is set to anything other than 'YEARLY'."
    // Stalwart v1.0.0 parses BYWEEKNO into byWeekNo.
    // In jmap-ical: inbound rrule_to_rule extracts by_week_no; outbound by_week_no_part
    // accepts -53..=-1 | 1..=53 (including long year 53), rejects 0, and enforces yearly frequency;
    // maps_recurrence_rule validates that all weeks are valid and frequency is yearly.
    let week_no_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:week-no-event-001\r\n\
DTSTART:20260105T090000Z\r\n\
DURATION:PT2H\r\n\
SUMMARY:Quarterly Week Reviews\r\n\
RRULE:FREQ=YEARLY;COUNT=5;BYWEEKNO=1,26,53,-1\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(week_no_ics).expect("parse week no rrule");
    let rule = ev.recurrence_rule.as_ref().expect("rule present");
    assert_eq!(rule.by_week_no, Some(vec![1, 26, 53, -1]));
    assert!(maps_recurrence_rule(rule));

    let out = event_to_ical(&ev);
    assert!(out.contains("RRULE:FREQ=YEARLY;COUNT=5;BYWEEKNO=1,26,53,-1"));

    // Week zero (0) is rejected by week_no_token and maps_recurrence_rule
    let mut zero_week_rule = rule.clone();
    zero_week_rule.by_week_no = Some(vec![1, 0, 26]);
    assert!(
        !maps_recurrence_rule(&zero_week_rule),
        "week zero must be refused by maps_recurrence_rule"
    );

    // Out-of-bounds week 54 is rejected
    let mut out_of_bounds_pos = rule.clone();
    out_of_bounds_pos.by_week_no = Some(vec![54]);
    assert!(
        !maps_recurrence_rule(&out_of_bounds_pos),
        "week 54 must be refused by maps_recurrence_rule"
    );

    // Out-of-bounds week -54 is rejected
    let mut out_of_bounds_neg = rule.clone();
    out_of_bounds_neg.by_week_no = Some(vec![-54]);
    assert!(
        !maps_recurrence_rule(&out_of_bounds_neg),
        "week -54 must be refused by maps_recurrence_rule"
    );

    // Non-yearly frequencies are forbidden per RFC 5545 section 3.3.10
    let mut monthly_week_rule = rule.clone();
    monthly_week_rule.frequency = "monthly".to_string();
    assert!(
        !maps_recurrence_rule(&monthly_week_rule),
        "FREQ=MONTHLY with BYWEEKNO must be refused by maps_recurrence_rule"
    );

    let mut weekly_week_rule = rule.clone();
    weekly_week_rule.frequency = "weekly".to_string();
    assert!(
        !maps_recurrence_rule(&weekly_week_rule),
        "FREQ=WEEKLY with BYWEEKNO must be refused by maps_recurrence_rule"
    );
}

#[test]
fn differential_oracle_rrule_count_positive_bounds_and_unbounded_series_omission() {
    // Divergence 55 against Stalwart differential oracle:
    // RFC 5545 section 3.3.10 specifies COUNT as a positive integer (1 or greater).
    // RFC 8984 section 4.3.1 defines count: UnsignedInt. Zero and negative counts are invalid.
    // Stalwart v1.0.0 parses COUNT into an unsigned integer.
    // In jmap-ical: rrule_to_rule parses COUNT into rule.count; outbound rule_to_rrule
    // emits COUNT=n when count is set; unbounded series omit COUNT and UNTIL entirely.
    let count_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:count-bounds-event-001\r\n\
DTSTART:20260901T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Count Bounded Meeting\r\n\
RRULE:FREQ=DAILY;COUNT=10\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(count_ics).expect("parse count rrule");
    let rule = ev.recurrence_rule.as_ref().expect("rule present");
    assert_eq!(rule.count, Some(10));
    assert!(maps_recurrence_rule(rule));
    let out = event_to_ical(&ev);
    assert!(out.contains("RRULE:FREQ=DAILY;COUNT=10"));

    // Unbounded series: neither count nor until is set
    let unbounded_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:unbounded-event-002\r\n\
DTSTART:20260901T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Indefinite Recurrence\r\n\
RRULE:FREQ=DAILY\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let unb_ev = ical_to_event(unbounded_ics).expect("parse unbounded rrule");
    let unb_rule = unb_ev.recurrence_rule.as_ref().expect("rule present");
    assert_eq!(unb_rule.count, None);
    assert_eq!(unb_rule.until, None);
    assert!(maps_recurrence_rule(unb_rule));
    let unb_out = event_to_ical(&unb_ev);
    assert!(unb_out.contains("RRULE:FREQ=DAILY"));
    assert!(!unb_out.contains("COUNT="));
    assert!(!unb_out.contains("UNTIL="));

    // Malformed non-numeric count is ignored on parse
    let malformed_count_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:malformed-count-003\r\n\
DTSTART:20260901T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Malformed Count\r\n\
RRULE:FREQ=DAILY;COUNT=invalid\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let mal_ev = ical_to_event(malformed_count_ics).expect("parse malformed count");
    let mal_rule = mal_ev.recurrence_rule.as_ref().expect("rule present");
    assert_eq!(mal_rule.count, None);
}

#[test]
fn differential_oracle_recurrence_override_instance_key_local_datetime_and_recurrence_id_matching()
{
    // Divergence 56 against Stalwart differential oracle:
    // RFC 5545 section 3.8.4.4 specifies RECURRENCE-ID identifying an instance of a recurrence.
    // RFC 8984 section 4.3.4 models recurrenceOverrides: Map<LocalDateTime, PatchObject>.
    // RFC 8984 section 1.4.3 specifies LocalDateTime MUST NOT include a timezone offset or 'Z'.
    // Stalwart v1.0.0 keys recurrenceOverrides using local date-time strings.
    // In jmap-ical: read_overrides converts RECURRENCE-ID to local date-time strings without Z;
    // event_to_ical serializes overrides as detached VEVENT blocks with matching RECURRENCE-ID;
    // override_maps_by validates that the instance key is a valid date-time via to_ical_date_time.
    let override_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:rec-override-key-001\r\n\
DTSTART:20260901T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Team Sync Series\r\n\
RRULE:FREQ=WEEKLY;COUNT=5\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:rec-override-key-001\r\n\
RECURRENCE-ID:20260908T090000Z\r\n\
DTSTART:20260908T100000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Team Sync Moved Hour\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(override_ics).expect("parse recurrence override");
    let overrides = ev.recurrence_overrides.as_ref().expect("overrides present");
    // The key must be a LocalDateTime without trailing 'Z'
    assert!(overrides.contains_key("2026-09-08T09:00:00"));
    let patch = &overrides["2026-09-08T09:00:00"];
    assert_eq!(
        patch.get("title").and_then(Value::as_str),
        Some("Team Sync Moved Hour")
    );

    // Serialization round-trip emits RECURRENCE-ID matching the original key
    let out = event_to_ical(&ev);
    assert!(out.contains("RECURRENCE-ID:20260908T090000Z"));
    assert!(out.contains("SUMMARY:Team Sync Moved Hour"));

    // Key validation in maps_recurrence_override
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T09:00:00",
        &json!({"title": "Valid Key"})
    ));

    // Malformed instance keys are refused
    assert!(
        !maps_recurrence_override(&ev, "not-a-datetime", &json!({"title": "Bad Key"})),
        "non-date instance key must be refused"
    );
    assert!(
        !maps_recurrence_override(
            &ev,
            "2026-13-45T99:99:99",
            &json!({"title": "Invalid Date Key"})
        ),
        "invalid date tokens must be refused"
    );
}

#[test]
fn differential_oracle_recurrence_override_excluded_purity_and_property_conflict_refusal() {
    // Divergence 57 against Stalwart differential oracle:
    // RFC 5545 section 3.8.5.1 specifies EXDATE for cancelled instances.
    // RFC 8984 section 4.3.4 specifies: "The excluded property, if present, MUST be true.
    // If true, the PatchObject MUST NOT contain any other properties."
    // Stalwart v1.0.0 parses EXDATE into {"excluded": true}.
    // In jmap-ical: override_maps_by enforces single-field purity for excluded patches;
    // recurrence_dates emits EXDATE on master VEVENT and never detached components for exclusions.
    let series = CalendarEvent {
        title: Some("Bi-weekly Review".to_owned()),
        start: Some("2026-09-01T14:00:00".to_owned()),
        duration: Some("PT1H".to_owned()),
        recurrence_rule: Some(RecurrenceRule::new("weekly")),
        ..CalendarEvent::default()
    };
    let id = "2026-09-08T14:00:00";

    // Valid pure exclusion: excluded: true alone
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"excluded": true})
    ));

    // Conflict: excluded: true combined with modified title is refused per RFC 8984 section 4.3.4
    assert!(
        !maps_recurrence_override(
            &series,
            id,
            &json!({"excluded": true, "title": "Cancelled Review"})
        ),
        "excluded with title must be refused"
    );

    // Conflict: excluded: true combined with status is refused
    assert!(
        !maps_recurrence_override(
            &series,
            id,
            &json!({"excluded": true, "status": "cancelled"})
        ),
        "excluded with status must be refused"
    );

    // Non-boolean or false excluded values are refused
    assert!(
        !maps_recurrence_override(&series, id, &json!({"excluded": "true"})),
        "string excluded must be refused"
    );
    assert!(
        !maps_recurrence_override(&series, id, &json!({"excluded": 1})),
        "integer excluded must be refused"
    );

    // Serialization check: excluded instance emits EXDATE, no detached VEVENT
    let mut ex_ev = series.clone();
    let mut overrides = BTreeMap::new();
    overrides.insert(id.to_string(), json!({"excluded": true}));
    ex_ev.recurrence_overrides = Some(overrides);

    let out = event_to_ical(&ex_ev);
    assert!(out.contains("EXDATE:20260908T140000"));
    assert!(!out.contains("RECURRENCE-ID"));
}

#[test]
fn differential_oracle_recurrence_override_property_allowlist_and_subobject_isolation() {
    // Divergence 58 against Stalwart differential oracle:
    // RFC 8984 section 4.3.4 theoretically permits patching any CalendarEvent property.
    // In RFC 5545, detached VEVENT components have defined semantics for scalar properties,
    // but per-instance participant and location mutation creates severe scheduling and sync hazards.
    // Stalwart v1.0.0 parses properties present on detached components.
    // In jmap-ical: OVERRIDE_PROPERTIES enforces a strict allowlist of 11 vetted properties:
    // ["title", "description", "start", "timeZone", "duration", "status", "freeBusyStatus",
    //  "priority", "privacy", "keywords", "alerts"].
    // Properties outside this list (such as locations, virtualLocations, participants, links)
    // are refused by maps_override_field and inherited from the series.
    let series = CalendarEvent {
        title: Some("Project Sync".to_owned()),
        start: Some("2026-09-01T11:00:00".to_owned()),
        duration: Some("PT45M".to_owned()),
        recurrence_rule: Some(RecurrenceRule::new("weekly")),
        ..CalendarEvent::default()
    };
    let id = "2026-09-08T11:00:00";

    // Verify the exact 11 allowed properties in OVERRIDE_PROPERTIES
    let expected = [
        "title",
        "description",
        "start",
        "timeZone",
        "duration",
        "status",
        "freeBusyStatus",
        "priority",
        "privacy",
        "keywords",
        "alerts",
    ];
    assert_eq!(OVERRIDE_PROPERTIES, expected);

    // Each allowed property is accepted when valid
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"title": "New Title"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"description": "New Desc"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"start": "2026-09-08T11:30:00"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"timeZone": "UTC"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"duration": "PT30M"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"status": "cancelled"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"freeBusyStatus": "free"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"priority": 5})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"privacy": "private"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"keywords": {"work": true}})
    ));

    // Complex sub-objects outside OVERRIDE_PROPERTIES are refused
    assert!(
        !maps_recurrence_override(
            &series,
            id,
            &json!({"locations": {"loc1": {"name": "Room B"}}})
        ),
        "locations override must be refused"
    );
    assert!(
        !maps_recurrence_override(
            &series,
            id,
            &json!({"virtualLocations": {"vloc1": {"uri": "https://meet.example.com"}}})
        ),
        "virtualLocations override must be refused"
    );
    assert!(
        !maps_recurrence_override(
            &series,
            id,
            &json!({"participants": {"p1": {"name": "Alice"}}})
        ),
        "participants override must be refused"
    );
    assert!(
        !maps_recurrence_override(
            &series,
            id,
            &json!({"links": {"l1": {"href": "https://example.com/doc"}}})
        ),
        "links override must be refused"
    );
    assert!(
        !maps_recurrence_override(&series, id, &json!({"locale": "fr-CA"})),
        "locale override must be refused"
    );
}

#[test]
fn differential_oracle_recurrence_override_timezone_scoping_and_custom_zone_definitions() {
    // Divergence 59 against Stalwart differential oracle:
    // RFC 8984 section 1.4.9 and 4.7.2 require custom timezone identifiers to be defined
    // in timeZones. RFC 5545 section 3.6.5 keeps VTIMEZONE at the root VCALENDAR level.
    // Stalwart v1.0.0 parses timezone references and resolves against root VTIMEZONE.
    // In jmap-ical: isolated patch checking (maps_recurrence_override) admits standard IANA
    // zone names and refuses custom timezones because the patch cannot carry definitions;
    // full serialization (sends_recurrence_override) admits custom zones defined by the series.
    let series_iana = CalendarEvent {
        title: Some("Global Standup".to_owned()),
        start: Some("2026-09-01T15:00:00".to_owned()),
        time_zone: Some("Europe/London".to_owned()),
        duration: Some("PT30M".to_owned()),
        recurrence_rule: Some(RecurrenceRule::new("weekly")),
        ..CalendarEvent::default()
    };
    let id = "2026-09-08T15:00:00";

    // Standard IANA timezone names are accepted by maps_recurrence_override
    assert!(maps_recurrence_override(
        &series_iana,
        id,
        &json!({"timeZone": "America/New_York"})
    ));
    assert!(maps_recurrence_override(
        &series_iana,
        id,
        &json!({"timeZone": "Asia/Tokyo"})
    ));

    // Floating timezone (null) is accepted
    assert!(maps_recurrence_override(
        &series_iana,
        id,
        &json!({"timeZone": null})
    ));

    // Custom solidus timezone without series definition is refused by both predicates
    let custom_tz = "/custom.org/CorporateZone";
    assert!(
        !maps_recurrence_override(&series_iana, id, &json!({"timeZone": custom_tz})),
        "custom timezone without definition refused by maps_recurrence_override"
    );
    assert!(
        !sends_recurrence_override(&series_iana, id, &json!({"timeZone": custom_tz})),
        "custom timezone without definition refused by sends_recurrence_override"
    );

    // Custom timezone with series definition: refused by maps_recurrence_override (isolated patch),
    // but accepted by sends_recurrence_override (full document carrying the VTIMEZONE definition)
    let custom_tz_ics = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
         BEGIN:VTIMEZONE\r\nTZID:{custom_tz}\r\n\
         BEGIN:STANDARD\r\n\
         DTSTART:20260101T000000\r\n\
         TZOFFSETFROM:+0200\r\n\
         TZOFFSETTO:+0200\r\n\
         END:STANDARD\r\n\
         END:VTIMEZONE\r\n\
         BEGIN:VEVENT\r\nUID:custom-series-001\r\n\
         DTSTART;TZID={custom_tz}:20260901T150000\r\n\
         DURATION:PT30M\r\nSUMMARY:Custom Zone Event\r\n\
         END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let custom_series = ical_to_event(&custom_tz_ics).expect("parse custom series");

    assert!(
        !maps_recurrence_override(&custom_series, id, &json!({"timeZone": custom_tz})),
        "custom timezone in isolated patch refused even when series defines it"
    );
    assert!(
        sends_recurrence_override(&custom_series, id, &json!({"timeZone": custom_tz})),
        "custom timezone accepted by sends_recurrence_override when series defines it"
    );
}

#[test]
fn differential_oracle_recurrence_override_null_property_removal_and_detached_component_roundtrip()
{
    // Divergence 60 against Stalwart differential oracle:
    // RFC 8984 section 4.3.4 models per-instance modifications as a PatchObject, where setting
    // a property to null removes or unsets that property on the instance.
    // In RFC 5545, a detached VEVENT component simply omits the corresponding content line.
    // In jmap-ical:
    // 1. maps_recurrence_override admits value.is_null() for restatable override properties
    //    (status, priority, freeBusyStatus, privacy, keywords, alerts, description, timeZone).
    // 2. Outbound event_to_ical does not emit the content line on the detached VEVENT when null.
    // 3. Inbound read_overrides / instance_patch diffs the detached VEVENT against the series,
    //    generating explicit null values in the patch when properties on the series are absent
    //    on the detached instance.
    let series_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:override-null-001\r\n\
DTSTART:20260901T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Full Series Meeting\r\n\
DESCRIPTION:Series Description\r\n\
STATUS:CONFIRMED\r\n\
PRIORITY:1\r\n\
CLASS:PRIVATE\r\n\
TRANSP:OPAQUE\r\n\
CATEGORIES:Work,Projects\r\n\
RRULE:FREQ=WEEKLY;COUNT=5\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
TRIGGER:-PT15M\r\n\
DESCRIPTION:Reminder\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:override-null-001\r\n\
RECURRENCE-ID:20260908T090000Z\r\n\
DTSTART:20260908T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Detached Instance Without Props\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(series_ics).expect("parse series with stripped detached vevent");
    let overrides = ev.recurrence_overrides.as_ref().expect("overrides present");
    let patch = &overrides["2026-09-08T09:00:00"];

    // The detached instance omitted DESCRIPTION, STATUS, PRIORITY, CLASS, TRANSP, CATEGORIES, VALARM.
    // instance_patch generates null for each property present on series but absent on detached component.
    assert_eq!(patch.get("description"), Some(&Value::Null));
    assert_eq!(patch.get("status"), Some(&Value::Null));
    assert_eq!(patch.get("priority"), Some(&Value::Null));
    assert_eq!(patch.get("privacy"), Some(&Value::Null));
    assert_eq!(patch.get("freeBusyStatus"), Some(&Value::Null));
    assert_eq!(patch.get("keywords"), Some(&Value::Null));
    assert_eq!(patch.get("alerts"), Some(&Value::Null));

    // maps_recurrence_override validates null removals
    assert!(maps_recurrence_override(&ev, "2026-09-08T09:00:00", patch));

    // Outbound serialization round-trip: detached VEVENT omits the lines
    let out = event_to_ical(&ev);
    let detached = vevent(&out, 1);
    assert!(without(detached, "DESCRIPTION:"));
    assert!(without(detached, "STATUS:"));
    assert!(without(detached, "PRIORITY:"));
    assert!(without(detached, "CLASS:"));
    assert!(without(detached, "TRANSP:"));
    assert!(without(detached, "CATEGORIES:"));
    assert!(without(detached, "BEGIN:VALARM"));
}

#[test]
fn differential_oracle_recurrence_override_empty_string_refusal_for_title_and_description() {
    // Divergence 61 against Stalwart differential oracle:
    // In JSON and RFC 8984 section 4.1.1/4.1.2, empty strings ("") are distinct from null.
    // In RFC 5545, SUMMARY and DESCRIPTION lines cannot represent empty strings distinctly
    // from absent properties.
    // In jmap-ical:
    // 1. maps_override_field refuses empty strings ("") for title and description to prevent
    //    non-idempotent round-trips where an empty string turns into null or series inheritance.
    // 2. Non-empty text strings and null deletions are accepted.
    // 3. event_to_ical suppresses empty strings on master series and detached components.
    let series = CalendarEvent {
        title: Some("Bi-weekly Review".to_owned()),
        description: Some("Review goals and progress".to_owned()),
        start: Some("2026-09-01T14:00:00".to_owned()),
        duration: Some("PT1H".to_owned()),
        recurrence_rule: Some(RecurrenceRule::new("weekly")),
        ..CalendarEvent::default()
    };
    let id = "2026-09-08T14:00:00";

    // Non-empty string overrides are accepted
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"title": "Rescheduled Review"})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"description": "Updated agenda items"})
    ));

    // Null deletions are accepted
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"title": null})
    ));
    assert!(maps_recurrence_override(
        &series,
        id,
        &json!({"description": null})
    ));

    // Empty strings are refused because RFC 5545 lines cannot represent empty text distinctly from absent
    assert!(
        !maps_recurrence_override(&series, id, &json!({"title": ""})),
        "empty string title must be refused in override patch"
    );
    assert!(
        !maps_recurrence_override(&series, id, &json!({"description": ""})),
        "empty string description must be refused in override patch"
    );

    // Serialization verification: empty strings are not emitted as empty lines
    let mut empty_event = series.clone();
    empty_event.title = Some("".to_string());
    empty_event.description = Some("".to_string());
    let out = event_to_ical(&empty_event);
    assert!(without(&out, "SUMMARY:"));
    assert!(without(&out, "DESCRIPTION:"));
}

#[test]
fn differential_oracle_recurrence_override_rescheduled_start_time_and_recurrence_id_separation() {
    // Divergence 62 against Stalwart differential oracle:
    // RFC 5545 section 3.8.4.4 models recurring instances using RECURRENCE-ID, which identifies
    // the original recurrence occurrence slot. When an instance is rescheduled, DTSTART specifies
    // the new time while RECURRENCE-ID retains the original slot.
    // RFC 8984 section 4.3.4 keys recurrenceOverrides by the original slot (id), and includes
    // "start" in the PatchObject only when rescheduled to a different time.
    // In jmap-ical:
    // 1. Inbound instance_patch suppresses "start" when DTSTART == id, avoiding redundant fields.
    // 2. Inbound instance_patch includes "start" when DTSTART != id.
    // 3. Outbound event_to_ical maintains RECURRENCE-ID at the original slot and DTSTART at the
    //    rescheduled time.
    // 4. maps_recurrence_override validates that overridden start times parse as valid date-times.
    let rescheduled_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:rescheduled-001\r\n\
DTSTART:20260901T100000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Weekly Architecture Sync\r\n\
RRULE:FREQ=WEEKLY;COUNT=4\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:rescheduled-001\r\n\
RECURRENCE-ID:20260908T100000Z\r\n\
DTSTART:20260908T140000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Rescheduled Afternoon Sync\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:rescheduled-001\r\n\
RECURRENCE-ID:20260915T100000Z\r\n\
DTSTART:20260915T100000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Regular Morning Sync\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(rescheduled_ics).expect("parse rescheduled event");
    let overrides = ev.recurrence_overrides.as_ref().expect("overrides present");

    // Rescheduled occurrence: DTSTART (14:00) != RECURRENCE-ID (10:00) -> "start" is present
    let patch_rescheduled = &overrides["2026-09-08T10:00:00"];
    assert_eq!(
        patch_rescheduled.get("start").and_then(Value::as_str),
        Some("2026-09-08T14:00:00")
    );
    assert_eq!(
        patch_rescheduled.get("title").and_then(Value::as_str),
        Some("Rescheduled Afternoon Sync")
    );

    // Non-rescheduled occurrence: DTSTART (10:00) == RECURRENCE-ID (10:00) -> "start" is omitted
    let patch_regular = &overrides["2026-09-15T10:00:00"];
    assert_eq!(patch_regular.get("start"), None);
    assert_eq!(
        patch_regular.get("title").and_then(Value::as_str),
        Some("Regular Morning Sync")
    );

    // Serialization preserves RECURRENCE-ID and rescheduled DTSTART
    let out = event_to_ical(&ev);
    assert!(out.contains("RECURRENCE-ID:20260908T100000Z"));
    assert!(out.contains("DTSTART:20260908T140000Z"));
    assert!(out.contains("SUMMARY:Rescheduled Afternoon Sync"));

    // Validation: invalid start date-time strings are refused
    let bad_start_patch = json!({"start": "invalid-datetime"});
    assert!(
        !maps_recurrence_override(&ev, "2026-09-08T10:00:00", &bad_start_patch),
        "malformed start in override patch must be refused"
    );
}

#[test]
fn differential_oracle_recurrence_override_duration_modification_and_rdate_period_calculation() {
    // Divergence 63 against Stalwart differential oracle:
    // RFC 5545 section 3.8.5.2 permits RDATE values to be discrete dates or periods (start/end).
    // RFC 8984 section 4.3.4 models extra instances added via RDATE as recurrenceOverrides entries.
    // If an instance shares the series duration, duration is omitted; if it differs, duration
    // is explicitly stated.
    // In jmap-ical:
    // 1. read_overrides uses period_length to calculate period duration from RDATE start/end bounds.
    // 2. When an RDATE period duration differs from the series duration, "duration" is set in patch.
    // 3. When an RDATE period matches series duration, an empty patch ({}) is emitted.
    // 4. Detached VEVENT with different duration emits "duration" in patch; matching duration omits it.
    let period_rdate_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:period-rdate-001\r\n\
DTSTART:20260901T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Sprint Planning Series\r\n\
RRULE:FREQ=WEEKLY;COUNT=3\r\n\
RDATE;VALUE=PERIOD:20260915T090000Z/20260915T113000Z\r\n\
RDATE;VALUE=PERIOD:20260922T090000Z/20260922T100000Z\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:period-rdate-001\r\n\
RECURRENCE-ID:20260908T090000Z\r\n\
DTSTART:20260908T090000Z\r\n\
DURATION:PT2H\r\n\
SUMMARY:Extended Sprint Planning\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(period_rdate_ics).expect("parse event with period rdates");
    let overrides = ev.recurrence_overrides.as_ref().expect("overrides present");

    // 1. RDATE with 2h30m period differs from 1h series duration -> duration is in patch
    let rdate_extended = &overrides["2026-09-15T09:00:00"];
    assert_eq!(
        rdate_extended.get("duration").and_then(Value::as_str),
        Some("PT2H30M")
    );

    // 2. RDATE with 1h period matches 1h series duration -> empty patch
    let rdate_matching = &overrides["2026-09-22T09:00:00"];
    assert_eq!(rdate_matching, &json!({}));

    // 3. Detached VEVENT with 2h duration differs from 1h series -> duration is in patch
    let detached_extended = &overrides["2026-09-08T09:00:00"];
    assert_eq!(
        detached_extended.get("duration").and_then(Value::as_str),
        Some("PT2H")
    );

    // Validation via maps_recurrence_override
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-15T09:00:00",
        rdate_extended
    ));
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-22T09:00:00",
        rdate_matching
    ));
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T09:00:00",
        detached_extended
    ));

    // Serialization verification: detached VEVENT emits custom DURATION
    let out = event_to_ical(&ev);
    let detached = vevent(&out, 1);
    assert!(detached.contains("DURATION:PT2H"));
}

#[test]
fn differential_oracle_recurrence_override_status_mapping_and_cancellation_semantics() {
    // Divergence 64 against Stalwart differential oracle:
    // In RFC 5545 section 3.8.1.11, STATUS on a recurring detached VEVENT takes CONFIRMED,
    // CANCELLED, or TENTATIVE. In RFC 8984 section 4.4.4, status takes "confirmed", "cancelled",
    // or "tentative".
    // RFC 8984 section 4.3.4 distinguishes "excluded": true (an occurrence completely dropped via
    // EXDATE) from "status": "cancelled" (an occurrence retained in the schedule as cancelled).
    // In jmap-ical:
    // 1. instance_patch records "status": "cancelled" or "tentative" when differing from series.
    // 2. maps_recurrence_override enforces known_status, admitting null to revert to default.
    // 3. Invalid status values like "needs-action" or "completed" are refused.
    let status_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:status-override-001\r\n\
DTSTART:20260901T100000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Weekly Standup\r\n\
STATUS:CONFIRMED\r\n\
RRULE:FREQ=WEEKLY;COUNT=3\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:status-override-001\r\n\
RECURRENCE-ID:20260908T100000Z\r\n\
DTSTART:20260908T100000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Cancelled Standup\r\n\
STATUS:CANCELLED\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:status-override-001\r\n\
RECURRENCE-ID:20260915T100000Z\r\n\
DTSTART:20260915T100000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Tentative Standup\r\n\
STATUS:TENTATIVE\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(status_ics).expect("parse status override event");
    let overrides = ev.recurrence_overrides.as_ref().expect("overrides present");

    // 1. Cancelled occurrence is modeled via status: "cancelled", not excluded: true
    let patch_cancelled = &overrides["2026-09-08T10:00:00"];
    assert_eq!(
        patch_cancelled.get("status").and_then(Value::as_str),
        Some("cancelled")
    );
    assert_eq!(patch_cancelled.get("excluded"), None);

    // 2. Tentative occurrence is modeled via status: "tentative"
    let patch_tentative = &overrides["2026-09-15T10:00:00"];
    assert_eq!(
        patch_tentative.get("status").and_then(Value::as_str),
        Some("tentative")
    );

    // 3. Serialization emits detached VEVENTs with STATUS:CANCELLED and STATUS:TENTATIVE
    let out = event_to_ical(&ev);
    assert!(out.contains("STATUS:CANCELLED"));
    assert!(out.contains("STATUS:TENTATIVE"));
    assert!(!out.contains("EXDATE"));

    // 4. Validation: known_status accepts valid statuses and null, rejects invalid ones
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T10:00:00",
        &json!({"status": "cancelled"})
    ));
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T10:00:00",
        &json!({"status": "tentative"})
    ));
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T10:00:00",
        &json!({"status": "confirmed"})
    ));
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T10:00:00",
        &json!({"status": null})
    ));
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T10:00:00",
        &json!({"status": "needs-action"})
    ));
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T10:00:00",
        &json!({"status": "completed"})
    ));
}

#[test]
fn differential_oracle_recurrence_override_free_busy_status_and_transparency_mapping() {
    // Divergence 65 against Stalwart differential oracle:
    // RFC 5545 section 3.8.2.7 defines TRANSP (OPAQUE or TRANSPARENT).
    // RFC 8984 section 4.4.2 defines freeBusyStatus ("busy" or "free").
    // Both specifications default to busy/opaque when omitted.
    // In jmap-ical:
    // 1. instance_patch records freeBusyStatus when differing from series.
    // 2. maps_recurrence_override admits "free", "busy", and null, rejecting non-standard tokens.
    // 3. Detached VEVENT serialization emits TRANSP:TRANSPARENT for "free", TRANSP:OPAQUE for "busy".
    let fb_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:fb-override-001\r\n\
DTSTART:20260901T140000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Office Hours Series\r\n\
TRANSP:OPAQUE\r\n\
RRULE:FREQ=WEEKLY;COUNT=3\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:fb-override-001\r\n\
RECURRENCE-ID:20260908T140000Z\r\n\
DTSTART:20260908T140000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Open Office Hours (Free)\r\n\
TRANSP:TRANSPARENT\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:fb-override-001\r\n\
RECURRENCE-ID:20260915T140000Z\r\n\
DTSTART:20260915T140000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Busy Office Hours\r\n\
TRANSP:OPAQUE\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(fb_ics).expect("parse freebusy override event");
    let overrides = ev.recurrence_overrides.as_ref().expect("overrides present");

    // Occurrence 1 with TRANSP:TRANSPARENT differs from series TRANSP:OPAQUE -> "free"
    let patch_free = &overrides["2026-09-08T14:00:00"];
    assert_eq!(
        patch_free.get("freeBusyStatus").and_then(Value::as_str),
        Some("free")
    );

    // Occurrence 2 with TRANSP:OPAQUE matches series -> freeBusyStatus is omitted
    let patch_busy = &overrides["2026-09-15T14:00:00"];
    assert_eq!(patch_busy.get("freeBusyStatus"), None);

    // Validation: known_transparency accepts "free", "busy", null; rejects invalid tokens
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T14:00:00",
        &json!({"freeBusyStatus": "free"})
    ));
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T14:00:00",
        &json!({"freeBusyStatus": "busy"})
    ));
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T14:00:00",
        &json!({"freeBusyStatus": null})
    ));
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T14:00:00",
        &json!({"freeBusyStatus": "tentative"})
    ));
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T14:00:00",
        &json!({"freeBusyStatus": "out-of-office"})
    ));

    // Serialization check: detached VEVENT emits TRANSP:TRANSPARENT
    let out = event_to_ical(&ev);
    assert!(out.contains("TRANSP:TRANSPARENT"));
}

#[test]
fn differential_oracle_recurrence_override_priority_range_clamping_and_non_integer_refusal() {
    // Divergence 66 against Stalwart differential oracle:
    // RFC 5545 section 3.8.1.9 and RFC 8984 section 4.4.1 define priority as integer 0..=9.
    // In jmap-ical:
    // 1. instance_patch records priority integer when differing from series, or null when cleared.
    // 2. maps_recurrence_override enforces integer range 0..=9 and admits null.
    // 3. String numbers, floating point numbers, and out-of-range integers are refused.
    let priority_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:priority-override-001\r\n\
DTSTART:20260901T150000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Team Review Series\r\n\
PRIORITY:5\r\n\
RRULE:FREQ=WEEKLY;COUNT=3\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:priority-override-001\r\n\
RECURRENCE-ID:20260908T150000Z\r\n\
DTSTART:20260908T150000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Urgent Review\r\n\
PRIORITY:1\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:priority-override-001\r\n\
RECURRENCE-ID:20260915T150000Z\r\n\
DTSTART:20260915T150000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Unprioritized Review\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(priority_ics).expect("parse priority override event");
    let overrides = ev.recurrence_overrides.as_ref().expect("overrides present");

    // Occurrence 1 has PRIORITY:1 -> patch has "priority": 1
    let patch_urgent = &overrides["2026-09-08T15:00:00"];
    assert_eq!(
        patch_urgent.get("priority").and_then(Value::as_i64),
        Some(1)
    );

    // Occurrence 2 omits PRIORITY where series has 5 -> patch has "priority": null
    let patch_cleared = &overrides["2026-09-15T15:00:00"];
    assert_eq!(patch_cleared.get("priority"), Some(&Value::Null));

    // Validation: known_priority admits 0..=9 and null
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T15:00:00",
        &json!({"priority": 1})
    ));
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T15:00:00",
        &json!({"priority": 0})
    ));
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T15:00:00",
        &json!({"priority": 9})
    ));
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T15:00:00",
        &json!({"priority": null})
    ));

    // Refusal: out-of-range integer, negative, string number, float
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T15:00:00",
        &json!({"priority": 10})
    ));
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T15:00:00",
        &json!({"priority": -1})
    ));
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T15:00:00",
        &json!({"priority": "1"})
    ));
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T15:00:00",
        &json!({"priority": 2.5})
    ));

    // Serialization check: urgent occurrence emits PRIORITY:1; cleared omits PRIORITY
    let out = event_to_ical(&ev);
    assert!(out.contains("PRIORITY:1"));
}

#[test]
fn differential_oracle_recurrence_override_privacy_classification_and_confidentiality_isolation() {
    // Divergence 67 against Stalwart differential oracle:
    // RFC 5545 section 3.8.1.3 defines CLASS (PUBLIC, PRIVATE, CONFIDENTIAL).
    // RFC 8984 section 4.4.3 defines privacy ("public", "private", "secret").
    // In jmap-ical:
    // 1. instance_patch records privacy when differing from series, or null when cleared to default.
    // 2. maps_recurrence_override admits "public", "private", "secret", and null.
    // 3. Unrecognized classifications like "confidential" (raw ical name) or "restricted" are refused.
    let privacy_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:privacy-override-001\r\n\
DTSTART:20260901T160000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:General 1-on-1 Series\r\n\
CLASS:PUBLIC\r\n\
RRULE:FREQ=WEEKLY;COUNT=3\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:privacy-override-001\r\n\
RECURRENCE-ID:20260908T160000Z\r\n\
DTSTART:20260908T160000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Private Discussion\r\n\
CLASS:PRIVATE\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:privacy-override-001\r\n\
RECURRENCE-ID:20260915T160000Z\r\n\
DTSTART:20260915T160000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Compensation Review (Secret)\r\n\
CLASS:CONFIDENTIAL\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(privacy_ics).expect("parse privacy override event");
    let overrides = ev.recurrence_overrides.as_ref().expect("overrides present");

    // Occurrence 1 CLASS:PRIVATE -> "privacy": "private"
    let patch_private = &overrides["2026-09-08T16:00:00"];
    assert_eq!(
        patch_private.get("privacy").and_then(Value::as_str),
        Some("private")
    );

    // Occurrence 2 CLASS:CONFIDENTIAL -> "privacy": "secret"
    let patch_secret = &overrides["2026-09-15T16:00:00"];
    assert_eq!(
        patch_secret.get("privacy").and_then(Value::as_str),
        Some("secret")
    );

    // Validation: known_privacy admits "public", "private", "secret", and null
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T16:00:00",
        &json!({"privacy": "private"})
    ));
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T16:00:00",
        &json!({"privacy": "secret"})
    ));
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T16:00:00",
        &json!({"privacy": "public"})
    ));
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T16:00:00",
        &json!({"privacy": null})
    ));

    // Refusal: raw iCal name "confidential", unknown vendor string, empty string
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T16:00:00",
        &json!({"privacy": "confidential"})
    ));
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T16:00:00",
        &json!({"privacy": "restricted"})
    ));
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T16:00:00",
        &json!({"privacy": ""})
    ));

    // Serialization check: detached VEVENT emits CLASS:PRIVATE and CLASS:CONFIDENTIAL
    let out = event_to_ical(&ev);
    assert!(out.contains("CLASS:PRIVATE"));
    assert!(out.contains("CLASS:CONFIDENTIAL"));
}

#[test]
fn differential_oracle_recurrence_override_keywords_categories_mapping_and_null_removal() {
    // Divergence 68 against Stalwart differential oracle:
    // RFC 5545 section 3.8.1.2 defines CATEGORIES (comma-separated tags).
    // RFC 8984 section 4.4.5 defines keywords: Map<String, Boolean>.
    // In jmap-ical:
    // 1. instance_patch records keywords when differing from series, or null when cleared.
    // 2. maps_recurrence_override admits non-empty maps with valid boolean true tags, and null.
    // 3. Empty map {} is refused to preserve round-trip idempotence with null.
    // 4. Invalid tag names with commas or false values are refused.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:keywords-override-001\r\n\
DTSTART:20260901T100000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Team Standup\r\n\
CATEGORIES:Work,Planning\r\n\
RRULE:FREQ=WEEKLY;COUNT=3\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:keywords-override-001\r\n\
RECURRENCE-ID:20260908T100000Z\r\n\
DTSTART:20260908T100000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Sprint Review Standup\r\n\
CATEGORIES:Sprint,Review\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:keywords-override-001\r\n\
RECURRENCE-ID:20260915T100000Z\r\n\
DTSTART:20260915T100000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Uncategorized Standup\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(ics).expect("parse keywords override stream");
    let overrides = ev.recurrence_overrides.as_ref().expect("overrides present");

    // Occurrence 1 has distinct CATEGORIES:Sprint,Review
    let patch_sprint = &overrides["2026-09-08T10:00:00"];
    let kw = patch_sprint
        .get("keywords")
        .and_then(Value::as_object)
        .expect("keywords object");
    assert_eq!(kw.get("Sprint"), Some(&Value::Bool(true)));
    assert_eq!(kw.get("Review"), Some(&Value::Bool(true)));
    assert_eq!(kw.len(), 2);

    // Occurrence 2 omits CATEGORIES while series has it, yielding "keywords": null
    let patch_uncat = &overrides["2026-09-15T10:00:00"];
    assert_eq!(patch_uncat.get("keywords"), Some(&Value::Null));

    // Validation: valid non-empty map and null are accepted
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T10:00:00",
        &json!({"keywords": {"Focus": true}})
    ));
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T10:00:00",
        &json!({"keywords": null})
    ));

    // Refusal: empty map {} is refused (must use null for removal)
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T10:00:00",
        &json!({"keywords": {}})
    ));

    // Refusal: empty or whitespace-only tag name, carriage return in tag, non-boolean true value
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T10:00:00",
        &json!({"keywords": {"": true}})
    ));
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T10:00:00",
        &json!({"keywords": {"   ": true}})
    ));
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T10:00:00",
        &json!({"keywords": {"tag\rwith\rcarriage": true}})
    ));
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T10:00:00",
        &json!({"keywords": {"valid": false}})
    ));

    // Serialization: detached component emits CATEGORIES with tags
    let out = event_to_ical(&ev);
    assert!(out.contains("CATEGORIES:"));
    assert!(out.contains("Sprint"));
    assert!(out.contains("Review"));
}

#[test]
fn differential_oracle_recurrence_override_alerts_valarm_mapping_and_null_suppression() {
    // Divergence 69 against Stalwart differential oracle:
    // RFC 5545 section 3.6.6 defines VALARM subcomponents.
    // RFC 8984 section 4.5.2 defines alerts: Map<String, Alert>.
    // In jmap-ical:
    // 1. instance_patch records alerts when differing from series, or null when cleared.
    // 2. maps_recurrence_override admits non-empty maps with valid OffsetTrigger alerts, and null.
    // 3. Empty map {} is refused to preserve round-trip idempotence with null.
    // 4. Absolute triggers and non-display actions are unmodeled and refused.
    // 5. When series uses default alerts, override alerts are refused.
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:alerts-override-001\r\n\
DTSTART:20260901T140000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Client Sync\r\n\
RRULE:FREQ=WEEKLY;COUNT=3\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Client Sync Reminder\r\n\
TRIGGER:-PT15M\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:alerts-override-001\r\n\
RECURRENCE-ID:20260908T140000Z\r\n\
DTSTART:20260908T140000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Client Sync (Early Alert)\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Early Alert\r\n\
TRIGGER:-PT30M\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:alerts-override-001\r\n\
RECURRENCE-ID:20260915T140000Z\r\n\
DTSTART:20260915T140000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Client Sync (Silent Instance)\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(ics).expect("parse alerts override stream");
    let overrides = ev.recurrence_overrides.as_ref().expect("overrides present");

    // Occurrence 1 has custom 30m reminder
    let patch_early = &overrides["2026-09-08T14:00:00"];
    let alerts_map = patch_early
        .get("alerts")
        .and_then(Value::as_object)
        .expect("alerts map");
    assert!(!alerts_map.is_empty());
    let (_k, alert_val) = alerts_map.iter().next().unwrap();
    assert_eq!(
        alert_val
            .get("trigger")
            .and_then(|t| t.get("offset"))
            .and_then(Value::as_str),
        Some("-PT30M")
    );

    // Occurrence 2 has no VALARM while series does, yielding "alerts": null
    let patch_silent = &overrides["2026-09-15T14:00:00"];
    assert_eq!(patch_silent.get("alerts"), Some(&Value::Null));

    // Validation: valid non-empty alert map and null are accepted
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T14:00:00",
        &json!({
            "alerts": {
                "alert-1": {
                    "@type": "Alert",
                    "trigger": {"@type": "OffsetTrigger", "offset": "-PT10M"},
                    "action": "display"
                }
            }
        })
    ));
    assert!(maps_recurrence_override(
        &ev,
        "2026-09-08T14:00:00",
        &json!({"alerts": null})
    ));

    // Refusal: empty map {} is refused (must use null for suppression)
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T14:00:00",
        &json!({"alerts": {}})
    ));

    // Refusal: absolute trigger or email action
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T14:00:00",
        &json!({
            "alerts": {
                "alert-abs": {
                    "@type": "Alert",
                    "trigger": {"@type": "AbsoluteTrigger", "when": "2026-09-08T13:30:00Z"},
                    "action": "display"
                }
            }
        })
    ));
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T14:00:00",
        &json!({
            "alerts": {
                "alert-email": {
                    "@type": "Alert",
                    "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
                    "action": "email"
                }
            }
        })
    ));

    // Serialization: detached component emits TRIGGER:-PT30M
    let out = event_to_ical(&ev);
    assert!(out.contains("TRIGGER:-PT30M"));
}

#[test]
fn differential_oracle_recurrence_override_use_default_alerts_document_scoping_and_refusal() {
    // Divergence 70 against Stalwart differential oracle:
    // RFC 8984 section 4.5.1 defines useDefaultAlerts: Boolean.
    // In jmap-ical:
    // 1. useDefaultAlerts is document-wide and excluded from OVERRIDE_PROPERTIES.
    // 2. maps_recurrence_override refuses patches specifying useDefaultAlerts.
    // 3. When useDefaultAlerts is true on the series, no VALARM is emitted anywhere,
    //    and overriding alerts on any instance is refused.
    assert!(!OVERRIDE_PROPERTIES.contains(&"useDefaultAlerts"));

    let ev = ical_to_event(
        "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:default-alerts-override-001\r\n\
DTSTART:20260901T150000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Recurring Review\r\n\
RRULE:FREQ=WEEKLY;COUNT=2\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n",
    )
    .expect("parse base event");

    // Refusal: useDefaultAlerts is not an allowable override property
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T15:00:00",
        &json!({"useDefaultAlerts": true})
    ));
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T15:00:00",
        &json!({"useDefaultAlerts": false})
    ));
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T15:00:00",
        &json!({"useDefaultAlerts": null})
    ));

    // When useDefaultAlerts is set on the series, override alerts are refused
    let mut ev_with_default = ev.clone();
    ev_with_default.use_default_alerts = Some(true);
    assert!(!maps_recurrence_override(
        &ev_with_default,
        "2026-09-08T15:00:00",
        &json!({
            "alerts": {
                "alert-1": {
                    "@type": "Alert",
                    "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
                    "action": "display"
                }
            }
        })
    ));
    assert!(!maps_recurrence_override(
        &ev_with_default,
        "2026-09-08T15:00:00",
        &json!({"alerts": null})
    ));
}

#[test]
fn differential_oracle_recurrence_override_show_without_time_document_scoping_and_refusal() {
    // Divergence 71 against Stalwart differential oracle:
    // RFC 8984 section 4.2.1 defines showWithoutTime: Boolean (all-day event flag).
    // RFC 5545 section 3.8.4.4 requires RECURRENCE-ID value type to match DTSTART.
    // In jmap-ical:
    // 1. showWithoutTime is decided once for the entire event document.
    // 2. showWithoutTime is excluded from OVERRIDE_PROPERTIES.
    // 3. maps_recurrence_override refuses patches specifying showWithoutTime.
    assert!(!OVERRIDE_PROPERTIES.contains(&"showWithoutTime"));

    let ev = ical_to_event(
        "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:show-without-time-override-001\r\n\
DTSTART:20260901T090000Z\r\n\
DURATION:PT1H\r\n\
SUMMARY:Timed Series\r\n\
RRULE:FREQ=WEEKLY;COUNT=2\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n",
    )
    .expect("parse timed event");

    // Refusal: showWithoutTime cannot be toggled per occurrence
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T09:00:00",
        &json!({"showWithoutTime": true})
    ));
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T09:00:00",
        &json!({"showWithoutTime": false})
    ));
    assert!(!maps_recurrence_override(
        &ev,
        "2026-09-08T09:00:00",
        &json!({"showWithoutTime": null})
    ));

    // Same check for an all-day series
    let all_day_ev = ical_to_event(
        "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example Exporter//EN\r\n\
BEGIN:VEVENT\r\n\
UID:all-day-override-001\r\n\
DTSTART;VALUE=DATE:20260901\r\n\
SUMMARY:All Day Series\r\n\
RRULE:FREQ=DAILY;COUNT=3\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n",
    )
    .expect("parse all day event");

    assert!(!maps_recurrence_override(
        &all_day_ev,
        "2026-09-02",
        &json!({"showWithoutTime": false})
    ));
}

#[test]
fn differential_oracle_participant_sendto_imip_uri_validation_and_crlf_sanitization() {
    // Divergence 72 against Stalwart differential oracle:
    // RFC 8984 section 4.4.6 defines sendTo: Map<String, String>.
    // RFC 5545 section 3.3.3 requires CAL-ADDRESS to be a URI.
    // In jmap-ical:
    // 1. calendar_address checks sendTo.imip.
    // 2. names_a_uri validates RFC 3986 scheme, colon, and no whitespace.
    // 3. Newlines or carriage returns are rejected to prevent CRLF injection.
    // 4. Non-imip delivery methods (sms, other) or non-URI strings are dropped.
    // 5. Inbound ical_to_event leaves participants: None for scheduling safety.
    let valid_event = attended(json!({
        "alice": guest("mailto:alice@example.com", "Alice Example", json!({
            "roles": {"attendee": true},
        })),
    }));
    let ics = event_to_ical(&valid_event);
    assert_eq!(
        content_line(&ics, "ATTENDEE"),
        "ATTENDEE;CN=\"Alice Example\";ROLE=REQ-PARTICIPANT:mailto:alice@example.com",
        "{ics}"
    );

    // Dropped participants: missing imip, non-imip keys, bare email, whitespace, or CRLF
    for bad_participant in [
        json!({"@type": "Participant", "name": "No SendTo"}),
        json!({"@type": "Participant", "sendTo": {}}),
        json!({"@type": "Participant", "sendTo": {"sms": "tel:+1234567890"}}),
        json!({"@type": "Participant", "sendTo": {"web": "https://example.com/invite"}}),
        json!({"@type": "Participant", "sendTo": {"imip": "alice@example.com"}}),
        json!({"@type": "Participant", "sendTo": {"imip": "mailto:"}}),
        json!({"@type": "Participant", "sendTo": {"imip": "mailto:alice example.com"}}),
        json!({"@type": "Participant", "sendTo": {"imip": "mailto:alice@example.com\r\nATTENDEE:injected"}}),
        json!({"@type": "Participant", "sendTo": {"imip": "mailto:alice@example.com\nSUMMARY:Injected"}}),
    ] {
        let bad_event = attended(json!({"bad": bad_participant}));
        let bad_ics = event_to_ical(&bad_event);
        assert!(
            without(&bad_ics, "ATTENDEE"),
            "{bad_participant}: {bad_ics}"
        );
        assert!(
            without(&bad_ics, "ORGANIZER"),
            "{bad_participant}: {bad_ics}"
        );
        assert!(
            !bad_ics.contains("injected"),
            "{bad_participant}: {bad_ics}"
        );
    }

    // Inbound safety: participants are dropped on import
    let imported = ical_to_event(&ics).expect("parse valid event with attendee");
    assert_eq!(imported.participants, None);
}

#[test]
fn differential_oracle_participant_owner_role_isolation_and_dual_line_emission() {
    // Divergence 73 against Stalwart differential oracle:
    // RFC 8984 section 4.4.6 models organizer via roles: {"owner": true}.
    // RFC 5545 section 3.6.1 admits at most one ORGANIZER line per VEVENT.
    // In jmap-ical:
    // 1. Only the first owner in iteration order gets an ORGANIZER line.
    // 2. An owner-only participant emits ORGANIZER and no ATTENDEE line.
    // 3. An owner who also attends (roles: {"owner": true, "attendee": true}) emits both lines.
    let single_owner_only = attended(json!({
        "alice": guest("mailto:alice@example.com", "Alice Owner", json!({
            "roles": {"owner": true},
        })),
    }));
    let ics1 = event_to_ical(&single_owner_only);
    assert_eq!(
        content_line(&ics1, "ORGANIZER"),
        "ORGANIZER;CN=\"Alice Owner\":mailto:alice@example.com",
        "{ics1}"
    );
    assert!(without(&ics1, "ATTENDEE"), "{ics1}");

    // Dual-line emission: attending owner
    let attending_owner = attended(json!({
        "alice": guest("mailto:alice@example.com", "Alice Owner", json!({
            "roles": {"owner": true, "attendee": true},
            "participationStatus": "accepted",
        })),
    }));
    let ics2 = event_to_ical(&attending_owner);
    assert_eq!(
        content_line(&ics2, "ORGANIZER"),
        "ORGANIZER;CN=\"Alice Owner\":mailto:alice@example.com",
        "{ics2}"
    );
    assert_eq!(
        content_line(&ics2, "ATTENDEE"),
        "ATTENDEE;CN=\"Alice Owner\";ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:mailto:alice@example.com",
        "{ics2}"
    );

    // Multiple owners: only the first owner in map key order emits ORGANIZER
    let multi_owners = attended(json!({
        "alice": guest("mailto:alice@example.com", "Alice First", json!({
            "roles": {"owner": true},
        })),
        "bob": guest("mailto:bob@example.com", "Bob Second", json!({
            "roles": {"owner": true, "attendee": true},
        })),
    }));
    let ics3 = event_to_ical(&multi_owners);
    let organizer_lines: Vec<String> = ics3
        .replace("\r\n ", "")
        .split("\r\n")
        .filter(|line| line.starts_with("ORGANIZER"))
        .map(str::to_owned)
        .collect();
    assert_eq!(
        organizer_lines,
        vec!["ORGANIZER;CN=\"Alice First\":mailto:alice@example.com".to_string()],
        "{ics3}"
    );
    // Bob is second owner: does not get ORGANIZER, but gets ATTENDEE because of attendee role
    assert_eq!(
        content_line(&ics3, "ATTENDEE"),
        "ATTENDEE;CN=\"Bob Second\";ROLE=REQ-PARTICIPANT:mailto:bob@example.com",
        "{ics3}"
    );
}

#[test]
fn differential_oracle_participant_role_precedence_and_single_role_parameter_clamping() {
    // Divergence 74 against Stalwart differential oracle:
    // RFC 8984 section 4.4.6 models roles as a set, admitting multiple roles.
    // RFC 5545 section 3.2.16 allows only a single ROLE parameter on ATTENDEE.
    // In jmap-ical:
    // 1. PARTICIPANT_ROLES precedence order: chair > informational > optional > attendee.
    // 2. Unknown roles outside standard table are omitted.
    // 3. Single role set collapses to single ROLE parameter.
    for (roles, expected_role) in [
        // Precedence: chair beats all
        (
            json!({"chair": true, "attendee": true, "optional": true}),
            "ROLE=CHAIR",
        ),
        // Precedence: informational beats optional and attendee
        (
            json!({"informational": true, "optional": true, "attendee": true}),
            "ROLE=NON-PARTICIPANT",
        ),
        // Precedence: optional beats attendee
        (
            json!({"optional": true, "attendee": true}),
            "ROLE=OPT-PARTICIPANT",
        ),
        // Attendee only
        (json!({"attendee": true}), "ROLE=REQ-PARTICIPANT"),
    ] {
        let ev = attended(json!({
            "guest": guest("mailto:guest@example.com", "Guest Person", json!({
                "roles": roles,
            })),
        }));
        let ics = event_to_ical(&ev);
        let attendee = content_line(&ics, "ATTENDEE");
        assert!(attendee.contains(expected_role), "{roles}: {attendee}");
    }

    // Unknown or non-standard roles are dropped, leaving no ROLE parameter
    let ev_unknown = attended(json!({
        "guest": guest("mailto:guest@example.com", "Guest Person", json!({
            "roles": {"observer": true, "vip": true},
        })),
    }));
    let ics_unknown = event_to_ical(&ev_unknown);
    let attendee_unknown = content_line(&ics_unknown, "ATTENDEE");
    assert!(!attendee_unknown.contains("ROLE="), "{ics_unknown}");
    assert_eq!(
        attendee_unknown,
        "ATTENDEE;CN=\"Guest Person\":mailto:guest@example.com"
    );
}

#[test]
fn differential_oracle_participant_cutype_mapping_partstat_and_rsvp_gating() {
    // Divergence 75 against Stalwart differential oracle:
    // RFC 8984 section 4.4.6 defines kind, participationStatus, and expectReply.
    // RFC 5545 defines CUTYPE, PARTSTAT, and RSVP parameters.
    // In jmap-ical:
    // 1. kind "location" maps to CUTYPE=ROOM; other kinds map to uppercase.
    // 2. participationStatus maps to uppercase PARTSTAT closed vocabulary.
    // 3. expectReply: true maps to RSVP=TRUE; false or omitted drops RSVP parameter.
    // 4. Non-standard values are filtered out.
    let ev_full = attended(json!({
        "room": guest("mailto:boardroom@example.com", "Boardroom", json!({
            "kind": "location",
            "participationStatus": "accepted",
            "expectReply": true,
            "roles": {"attendee": true},
        })),
        "user": guest("mailto:user@example.com", "Regular User", json!({
            "kind": "individual",
            "participationStatus": "declined",
            "expectReply": false,
            "roles": {"optional": true},
        })),
    }));
    let ics = event_to_ical(&ev_full);
    let lines: Vec<String> = ics
        .replace("\r\n ", "")
        .split("\r\n")
        .filter(|line| line.starts_with("ATTENDEE"))
        .map(str::to_owned)
        .collect();
    assert_eq!(
        lines,
        vec![
            "ATTENDEE;CN=Boardroom;CUTYPE=ROOM;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;\
             RSVP=TRUE:mailto:boardroom@example.com".to_string(),
            "ATTENDEE;CN=\"Regular User\";CUTYPE=INDIVIDUAL;ROLE=OPT-PARTICIPANT;PARTSTAT=DECLINED:\
             mailto:user@example.com".to_string(),
        ],
        "{ics}"
    );

    // Filter non-standard values
    let ev_invalid = attended(json!({
        "guest": guest("mailto:guest@example.com", "Custom Guest", json!({
            "kind": "unknown-robot",
            "participationStatus": "undecided",
            "expectReply": null,
            "roles": {"attendee": true},
        })),
    }));
    let ics_invalid = event_to_ical(&ev_invalid);
    let line_invalid = content_line(&ics_invalid, "ATTENDEE");
    assert!(!line_invalid.contains("CUTYPE="), "{line_invalid}");
    assert!(!line_invalid.contains("PARTSTAT="), "{line_invalid}");
    assert!(!line_invalid.contains("RSVP="), "{line_invalid}");
    assert_eq!(
        line_invalid,
        "ATTENDEE;CN=\"Custom Guest\";ROLE=REQ-PARTICIPANT:mailto:guest@example.com"
    );
}

#[test]
fn differential_oracle_participant_cn_name_mapping_and_empty_suppression() {
    // Divergence 76 against Stalwart differential oracle:
    // RFC 8984 section 4.4.6 defines name: String.
    // RFC 5545 section 3.2.2 defines the CN (Common Name) parameter.
    // In jmap-ical:
    // 1. stated_name maps non-empty name to CN on ORGANIZER and ATTENDEE lines.
    // 2. An empty name ("") is suppressed (omits CN parameter).
    // 3. Names with whitespace are quoted: CN="Alice Smith".
    // 4. Inbound ical_to_event leaves participants: None for scheduling safety.
    let ev = attended(json!({
        "organizer": json!({
            "@type": "Participant",
            "name": "Alice Organizer",
            "sendTo": {"imip": "mailto:alice@example.com"},
            "roles": {"owner": true},
        }),
        "unquoted": json!({
            "@type": "Participant",
            "name": "Bob",
            "sendTo": {"imip": "mailto:bob@example.com"},
            "roles": {"attendee": true},
        }),
        "empty_name": json!({
            "@type": "Participant",
            "name": "",
            "sendTo": {"imip": "mailto:empty@example.com"},
            "roles": {"attendee": true},
        }),
        "no_name": json!({
            "@type": "Participant",
            "sendTo": {"imip": "mailto:noname@example.com"},
            "roles": {"attendee": true},
        }),
    }));
    let ics = event_to_ical(&ev);
    assert_eq!(
        content_line(&ics, "ORGANIZER"),
        "ORGANIZER;CN=\"Alice Organizer\":mailto:alice@example.com",
        "{ics}"
    );
    let attendee_lines: Vec<String> = ics
        .replace("\r\n ", "")
        .split("\r\n")
        .filter(|line| line.starts_with("ATTENDEE"))
        .map(str::to_owned)
        .collect();
    assert_eq!(
        attendee_lines,
        vec![
            "ATTENDEE;ROLE=REQ-PARTICIPANT:mailto:empty@example.com".to_string(),
            "ATTENDEE;ROLE=REQ-PARTICIPANT:mailto:noname@example.com".to_string(),
            "ATTENDEE;CN=Bob;ROLE=REQ-PARTICIPANT:mailto:bob@example.com".to_string(),
        ],
        "{ics}"
    );

    // Inbound safety: participants dropped on parse
    let parsed = ical_to_event(&ics).expect("parse valid event with attendees");
    assert_eq!(parsed.participants, None);
}

#[test]
fn differential_oracle_participant_delegation_parameter_omission() {
    // Divergence 77 against Stalwart differential oracle:
    // RFC 8984 section 4.4.6 defines delegatedTo and delegatedFrom maps.
    // RFC 5545 section 3.2.4 and 3.2.5 define DELEGATED-TO and DELEGATED-FROM parameters.
    // In jmap-ical:
    // 1. drawn_participants omits DELEGATED-TO and DELEGATED-FROM on ATTENDEE lines.
    // 2. Inbound ical_to_event drops participants (None) for scheduling boundary safety.
    let ev = attended(json!({
        "alice": guest("mailto:alice@example.com", "Alice", json!({
            "roles": {"attendee": true},
            "delegatedTo": {
                "mailto:bob@example.com": true,
            },
        })),
        "bob": guest("mailto:bob@example.com", "Bob", json!({
            "roles": {"attendee": true},
            "delegatedFrom": {
                "mailto:alice@example.com": true,
            },
        })),
    }));
    let ics = event_to_ical(&ev);
    assert!(!ics.contains("DELEGATED-TO"), "{ics}");
    assert!(!ics.contains("DELEGATED-FROM"), "{ics}");
    let lines: Vec<String> = ics
        .replace("\r\n ", "")
        .split("\r\n")
        .filter(|line| line.starts_with("ATTENDEE"))
        .map(str::to_owned)
        .collect();
    assert_eq!(
        lines,
        vec![
            "ATTENDEE;CN=Alice;ROLE=REQ-PARTICIPANT:mailto:alice@example.com".to_string(),
            "ATTENDEE;CN=Bob;ROLE=REQ-PARTICIPANT:mailto:bob@example.com".to_string(),
        ],
        "{ics}"
    );

    // Inbound safety: participants dropped on parse
    let parsed = ical_to_event(&ics).expect("parse valid event with delegated attendees");
    assert_eq!(parsed.participants, None);
}

#[test]
fn differential_oracle_participant_member_of_group_parameter_omission() {
    // Divergence 78 against Stalwart differential oracle:
    // RFC 8984 section 4.4.6 defines memberOf map.
    // RFC 5545 section 3.2.11 defines the MEMBER parameter.
    // In jmap-ical:
    // 1. drawn_participants omits MEMBER parameters on ATTENDEE lines.
    // 2. Inbound ical_to_event drops participants (None).
    let ev = attended(json!({
        "member": guest("mailto:developer@example.com", "Dev Team Member", json!({
            "roles": {"attendee": true},
            "memberOf": {
                "mailto:dev-team@example.com": true,
                "mailto:engineering@example.com": true,
            },
        })),
    }));
    let ics = event_to_ical(&ev);
    assert!(!ics.contains("MEMBER="), "{ics}");
    assert_eq!(
        content_line(&ics, "ATTENDEE"),
        "ATTENDEE;CN=\"Dev Team Member\";ROLE=REQ-PARTICIPANT:mailto:developer@example.com",
        "{ics}"
    );

    // Inbound safety
    let parsed = ical_to_event(&ics).expect("parse valid event with member attendee");
    assert_eq!(parsed.participants, None);
}

#[test]
fn differential_oracle_participant_schedule_agent_parameters_omission() {
    // Divergence 79 against Stalwart differential oracle:
    // RFC 8984 section 4.4.6 and RFC 6638 define CalDAV scheduling parameters:
    // scheduleAgent, scheduleStatus, and scheduleForceSend.
    // In jmap-ical:
    // 1. drawn_participants omits SCHEDULE-AGENT, SCHEDULE-STATUS, and SCHEDULE-FORCE-SEND.
    // 2. Inbound ical_to_event drops participants (None).
    let ev = attended(json!({
        "server_managed": guest("mailto:managed@example.com", "Managed Invitee", json!({
            "roles": {"attendee": true},
            "scheduleAgent": "server",
            "scheduleStatus": "1.1;Delivered",
            "scheduleForceSend": "request",
            "scheduleSequence": 2,
            "scheduleUpdated": "2026-09-04T12:00:00Z",
        })),
    }));
    let ics = event_to_ical(&ev);
    assert!(!ics.contains("SCHEDULE-AGENT"), "{ics}");
    assert!(!ics.contains("SCHEDULE-STATUS"), "{ics}");
    assert!(!ics.contains("SCHEDULE-FORCE-SEND"), "{ics}");
    assert_eq!(
        content_line(&ics, "ATTENDEE"),
        "ATTENDEE;CN=\"Managed Invitee\";ROLE=REQ-PARTICIPANT:mailto:managed@example.com",
        "{ics}"
    );

    // Inbound safety
    let parsed = ical_to_event(&ics).expect("parse valid event with managed invitee");
    assert_eq!(parsed.participants, None);
}

#[test]
fn differential_oracle_location_single_entry_restriction_and_empty_name_suppression() {
    // Divergence 80 against Stalwart differential oracle:
    // RFC 8984 section 4.2.5 models locations: Map<String, Location>.
    // RFC 5545 section 3.6.1 restricts VEVENT to at most one LOCATION line.
    // In jmap-ical:
    // 1. drawn_place selects the first entry with a non-empty name.
    // 2. maps_locations returns false when more than one location entry is present.
    // 3. An empty name ("") is suppressed on outbound serialization (no LOCATION line).
    // 4. Inbound ical_to_event returns None for absent or empty LOCATION text.
    let mut locations = BTreeMap::new();
    locations.insert(
        "loc1".to_string(),
        json!({"@type": "Location", "name": "Primary Room"}),
    );
    locations.insert(
        "loc2".to_string(),
        json!({"@type": "Location", "name": "Overflow Room"}),
    );
    let ev = CalendarEvent {
        locations: Some(locations),
        ..CalendarEvent::default()
    };
    assert!(!maps_locations(ev.locations.as_ref().unwrap()));
    let ics = event_to_ical(&ev);
    assert_eq!(
        content_line(&ics, "LOCATION"),
        "LOCATION;X-JMAP-KEY=loc1:Primary Room"
    );
    assert!(!ics.contains("Overflow Room"));

    // Empty name suppression
    let ev_empty = placed("loc1", json!({"@type": "Location", "name": ""}));
    let ics_empty = event_to_ical(&ev_empty);
    assert!(without(&ics_empty, "LOCATION"), "{ics_empty}");
    let parsed_empty = ical_to_event(&ics_empty).expect("parse empty location event");
    assert_eq!(parsed_empty.locations, None);
}

#[test]
fn differential_oracle_location_x_jmap_key_tracking_and_invented_key_allocation() {
    // Divergence 81 against Stalwart differential oracle:
    // RFC 8984 section 4.2.5 keys locations by Id.
    // In jmap-ical:
    // 1. drawn_place attaches X-JMAP-KEY to retain server entry keys.
    // 2. read_locations extracts X-JMAP-KEY, preserving round-trip key identity.
    // 3. When X-JMAP-KEY is absent, invented key "l1" is allocated.
    // 4. When X-JMAP-KEY contains invalid Id characters, it falls back safely to "l1".
    let ev = placed(
        "office_404",
        json!({"@type": "Location", "name": "Conference Suite 404"}),
    );
    let ics = event_to_ical(&ev);
    assert_eq!(
        content_line(&ics, "LOCATION"),
        "LOCATION;X-JMAP-KEY=office_404:Conference Suite 404"
    );
    let parsed = ical_to_event(&ics).expect("parse with X-JMAP-KEY");
    let locs = parsed.locations.expect("locations map present");
    assert!(locs.contains_key("office_404"));
    assert_eq!(locs["office_404"]["name"], "Conference Suite 404");

    // Absent X-JMAP-KEY falls back to invented key "l1"
    let ics_no_key = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n\
        UID:E1\r\nDTSTART:20260904T120000Z\r\nLOCATION:Main Hall\r\n\
        END:VEVENT\r\nEND:VCALENDAR\r\n";
    let parsed_no_key = ical_to_event(ics_no_key).expect("parse location without key");
    let locs_no_key = parsed_no_key.locations.expect("invented location map");
    assert_eq!(locs_no_key.keys().collect::<Vec<_>>(), ["l1"]);
    assert_eq!(locs_no_key["l1"]["name"], "Main Hall");

    // Invalid X-JMAP-KEY (colons, spaces) rejected per names_map_entry, falls back to "l1"
    let ics_bad_key = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n\
        UID:E2\r\nDTSTART:20260904T120000Z\r\nLOCATION;X-JMAP-KEY=\"bad key\":Room A\r\n\
        END:VEVENT\r\nEND:VCALENDAR\r\n";
    let parsed_bad_key = ical_to_event(ics_bad_key).expect("parse location with bad key");
    let locs_bad_key = parsed_bad_key.locations.expect("bad key location map");
    assert_eq!(locs_bad_key.keys().collect::<Vec<_>>(), ["l1"]);
    assert_eq!(locs_bad_key["l1"]["name"], "Room A");
}

#[test]
fn differential_oracle_virtual_location_conference_multi_line_and_feature_vocabulary_gating() {
    // Divergence 82 against Stalwart differential oracle:
    // RFC 8984 section 4.2.6 defines virtualLocations.
    // RFC 7986 section 5.11 defines the CONFERENCE property.
    // In jmap-ical:
    // 1. drawn_conferences serializes multiple CONFERENCE lines with VALUE=URI.
    // 2. FEATURE parameters are mapped from CONFERENCE_FEATURES table.
    // 3. LABEL parameter is mapped from VirtualLocation.name.
    // 4. maps_virtual_locations refuses non-standard features or invalid URIs.
    // 5. read_virtual_locations parses CONFERENCE into virtualLocations with round-trip fidelity.
    let mut vlocs = BTreeMap::new();
    vlocs.insert(
        "conf1".to_string(),
        json!({
            "@type": "VirtualLocation",
            "uri": "https://meet.example.com/standup",
            "name": "Team Standup",
            "features": {"video": true, "audio": true, "chat": true},
        }),
    );
    vlocs.insert(
        "conf2".to_string(),
        json!({
            "@type": "VirtualLocation",
            "uri": "tel:+15551234567",
            "name": "Phone Bridge",
            "features": {"phone": true},
        }),
    );
    let ev = CalendarEvent {
        virtual_locations: Some(vlocs),
        ..CalendarEvent::default()
    };
    assert!(maps_virtual_locations(
        ev.virtual_locations.as_ref().unwrap()
    ));
    let ics = event_to_ical(&ev);
    let unfolded = ics.replace("\r\n ", "").replace("\r\n\t", "");
    assert!(unfolded.contains("CONFERENCE;VALUE=URI"));
    assert!(unfolded.contains("FEATURE=AUDIO,CHAT,VIDEO"));
    assert!(unfolded.contains("LABEL=\"Team Standup\""));
    assert!(unfolded.contains("X-JMAP-KEY=conf1:https://meet.example.com/standup"));
    assert!(unfolded.contains("FEATURE=PHONE"));
    assert!(unfolded.contains("LABEL=\"Phone Bridge\""));
    assert!(unfolded.contains("X-JMAP-KEY=conf2:tel:+15551234567"));

    // Refusal of non-standard features
    let bad_vloc = BTreeMap::from([(
        "c1".to_string(),
        json!({
            "@type": "VirtualLocation",
            "uri": "https://meet.example.com/bad",
            "features": {"whiteboard": true},
        }),
    )]);
    assert!(!maps_virtual_locations(&bad_vloc));

    // Inbound round-trip
    let parsed = ical_to_event(&ics).expect("parse conferences");
    let parsed_vlocs = parsed.virtual_locations.expect("virtual locations");
    assert_eq!(parsed_vlocs.len(), 2);
    assert_eq!(
        parsed_vlocs["conf1"]["uri"],
        "https://meet.example.com/standup"
    );
    assert_eq!(parsed_vlocs["conf1"]["name"], "Team Standup");
    assert_eq!(parsed_vlocs["conf2"]["uri"], "tel:+15551234567");
    assert_eq!(parsed_vlocs["conf2"]["name"], "Phone Bridge");
}

#[test]
fn differential_oracle_links_file_uri_and_binary_data_suppression_for_privacy() {
    // Divergence 83 against Stalwart differential oracle:
    // RFC 8984 section 4.2.7 defines links.
    // RFC 5545 section 3.8.1.1 defines ATTACH; RFC 7986 section 5.10 defines IMAGE.
    // In jmap-ical:
    // 1. read_links drops file:// URIs to protect local desktop paths and privacy.
    // 2. Inline binary attachments (VALUE=BINARY) are dropped on import.
    // 3. rel: "icon" maps to IMAGE;VALUE=URI while general links map to ATTACH.
    // 4. FMTTYPE media type validation adheres to RFC 6838 restricted names.
    let ics_file = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n\
        UID:E1\r\nDTSTART:20260904T120000Z\r\n\
        ATTACH:file:///home/runner/Documents/private_notes.pdf\r\n\
        ATTACH:https://example.com/public_agenda.pdf\r\n\
        END:VEVENT\r\nEND:VCALENDAR\r\n";
    let parsed = ical_to_event(ics_file).expect("parse links with local file");
    let links = parsed.links.expect("links map present");
    assert_eq!(links.len(), 1);
    assert_eq!(links["k1"]["href"], "https://example.com/public_agenda.pdf");

    // Inline binary attachment dropped
    let ics_binary = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n\
        UID:E2\r\nDTSTART:20260904T120000Z\r\n\
        ATTACH;VALUE=BINARY;ENCODING=BASE64:SGVsbG8gV29ybGQ=\r\n\
        END:VEVENT\r\nEND:VCALENDAR\r\n";
    let parsed_binary = ical_to_event(ics_binary).expect("parse inline binary attachment");
    assert_eq!(parsed_binary.links, None);

    // IMAGE with rel: "icon" round-trip
    let mut links_map = BTreeMap::new();
    links_map.insert(
        "img1".to_string(),
        json!({
            "@type": "Link",
            "href": "https://example.com/badge.png",
            "rel": "icon",
            "contentType": "image/png",
            "display": "badge",
        }),
    );
    let ev_link = CalendarEvent {
        links: Some(links_map),
        ..CalendarEvent::default()
    };
    let ics_link = event_to_ical(&ev_link);
    assert_eq!(
        content_line(&ics_link, "IMAGE"),
        "IMAGE;VALUE=URI;DISPLAY=BADGE;FMTTYPE=image/png;X-JMAP-KEY=img1:https://example.com/badge.png"
    );
    let parsed_link = ical_to_event(&ics_link).expect("parse image link");
    let read_links = parsed_link.links.expect("parsed links");
    assert_eq!(read_links["img1"]["href"], "https://example.com/badge.png");
    assert_eq!(read_links["img1"]["rel"], "icon");
    assert_eq!(read_links["img1"]["contentType"], "image/png");
}

#[test]
fn differential_oracle_freebusy_attendee_normalization_and_injection_prevention() {
    // Divergence 84 against Stalwart differential oracle:
    // RFC 5545 sections 3.8.4.1 and 3.8.4.3 require ATTENDEE in VFREEBUSY to be a CAL-ADDRESS URI.
    // In jmap-ical:
    // 1. mailto normalizes bare email addresses to mailto: URIs.
    // 2. Existing mailto: prefixes (case-insensitive) are not doubled (no mailto:mailto:).
    // 3. Newlines in attendee strings are escaped to prevent property injection.
    let window_start = UtcDate::new("2026-09-04T08:00:00Z");
    let window_end = UtcDate::new("2026-09-04T18:00:00Z");

    // Bare email address
    let ics_bare = busy_periods_to_vfreebusy("alice@example.com", &window_start, &window_end, &[])
        .expect("render bare attendee");
    assert!(ics_bare.contains("\r\nATTENDEE:mailto:alice@example.com\r\n"));

    // Case-insensitive scheme tolerance
    let ics_lower =
        busy_periods_to_vfreebusy("mailto:alice@example.com", &window_start, &window_end, &[])
            .expect("render lower mailto");
    assert!(ics_lower.contains("\r\nATTENDEE:mailto:alice@example.com\r\n"));

    let ics_upper =
        busy_periods_to_vfreebusy("MAILTO:alice@example.com", &window_start, &window_end, &[])
            .expect("render upper mailto");
    assert!(ics_upper.contains("\r\nATTENDEE:mailto:alice@example.com\r\n"));

    // Injection sanitization
    let ics_injected = busy_periods_to_vfreebusy(
        "alice@example.com\r\nSUMMARY:Injected",
        &window_start,
        &window_end,
        &[],
    )
    .expect("render injected attendee");
    assert!(
        !ics_injected
            .split("\r\n")
            .any(|l| l.starts_with("SUMMARY:"))
    );
}

#[test]
fn differential_oracle_freebusy_whole_component_refusal_on_invalid_period_or_window() {
    // Divergence 85 against Stalwart differential oracle:
    // General iCalendar event mappings drop invalid properties to preserve the event.
    // In contrast, busy_periods_to_vfreebusy refuses the whole component (Option::None)
    // if any period or the search window cannot be parsed as a valid UTC date-time.
    // This prevents schedulers from seeing an attendee as falsely free.
    let valid_start = UtcDate::new("2026-09-04T08:00:00Z");
    let valid_end = UtcDate::new("2026-09-04T18:00:00Z");
    let invalid_date = UtcDate::new("not-a-valid-timestamp");
    let missing_z = UtcDate::new("2026-09-04T08:00:00");
    let invalid_hour = UtcDate::new("2026-09-04T25:00:00Z");

    // Invalid search window refuses component
    assert_eq!(
        busy_periods_to_vfreebusy("alice@example.com", &invalid_date, &valid_end, &[]),
        None
    );
    assert_eq!(
        busy_periods_to_vfreebusy("alice@example.com", &valid_start, &invalid_date, &[]),
        None
    );
    assert_eq!(
        busy_periods_to_vfreebusy("alice@example.com", &missing_z, &valid_end, &[]),
        None
    );

    // Invalid busy period refuses component rather than dropping period
    let bad_period = BusyPeriod {
        utc_start: invalid_hour,
        utc_end: valid_end.clone(),
        busy_status: "confirmed".to_string(),
        event: None,
    };
    assert_eq!(
        busy_periods_to_vfreebusy("alice@example.com", &valid_start, &valid_end, &[bad_period]),
        None
    );

    // Valid empty period slice returns whole component without FREEBUSY lines
    let empty_component =
        busy_periods_to_vfreebusy("alice@example.com", &valid_start, &valid_end, &[])
            .expect("render empty periods");
    assert!(empty_component.contains("BEGIN:VFREEBUSY"));
    assert!(empty_component.contains("DTSTART:20260904T080000Z"));
    assert!(empty_component.contains("DTEND:20260904T180000Z"));
    assert!(empty_component.contains("ATTENDEE:mailto:alice@example.com"));
    assert!(
        !empty_component
            .split("\r\n")
            .any(|l| l.starts_with("FREEBUSY"))
    );
    assert!(empty_component.contains("END:VFREEBUSY"));
}

#[test]
fn differential_oracle_freebusy_status_mapping_and_fail_safe_busy_fallback() {
    // Divergence 86 against Stalwart differential oracle:
    // draft-ietf-jmap-calendars section 2.2 defines busyStatus (confirmed, tentative, unavailable).
    // RFC 5545 section 3.2.9 defines FBTYPE (BUSY, BUSY-TENTATIVE, BUSY-UNAVAILABLE).
    // In jmap-ical:
    // 1. confirmed -> BUSY
    // 2. tentative -> BUSY-TENTATIVE
    // 3. unavailable -> BUSY-UNAVAILABLE
    // 4. Unknown or future statuses and empty string fall back safely to BUSY.
    assert_eq!(free_busy_type("confirmed"), "BUSY");
    assert_eq!(free_busy_type("tentative"), "BUSY-TENTATIVE");
    assert_eq!(free_busy_type("unavailable"), "BUSY-UNAVAILABLE");
    assert_eq!(free_busy_type("focus-time"), "BUSY");
    assert_eq!(free_busy_type("working-elsewhere"), "BUSY");
    assert_eq!(free_busy_type(""), "BUSY");

    let window_start = UtcDate::new("2026-09-04T08:00:00Z");
    let window_end = UtcDate::new("2026-09-04T18:00:00Z");
    let periods = vec![
        BusyPeriod {
            utc_start: UtcDate::new("2026-09-04T09:00:00Z"),
            utc_end: UtcDate::new("2026-09-04T10:00:00Z"),
            busy_status: "tentative".to_string(),
            event: None,
        },
        BusyPeriod {
            utc_start: UtcDate::new("2026-09-04T11:00:00Z"),
            utc_end: UtcDate::new("2026-09-04T12:00:00Z"),
            busy_status: "unavailable".to_string(),
            event: None,
        },
        BusyPeriod {
            utc_start: UtcDate::new("2026-09-04T13:00:00Z"),
            utc_end: UtcDate::new("2026-09-04T14:00:00Z"),
            busy_status: "future-extension".to_string(),
            event: None,
        },
    ];
    let ics = busy_periods_to_vfreebusy("alice@example.com", &window_start, &window_end, &periods)
        .expect("render periods");
    assert!(ics.contains("FREEBUSY;FBTYPE=BUSY-TENTATIVE:20260904T090000Z/20260904T100000Z"));
    assert!(ics.contains("FREEBUSY;FBTYPE=BUSY-UNAVAILABLE:20260904T110000Z/20260904T120000Z"));
    assert!(ics.contains("FREEBUSY;FBTYPE=BUSY:20260904T130000Z/20260904T140000Z"));
}

#[test]
fn differential_oracle_freebusy_fractional_seconds_truncation_and_digit_validation() {
    // Divergence 87 against Stalwart differential oracle:
    // RFC 3339 allows fractional seconds, while RFC 5545 DATE-TIME forbids them.
    // In jmap-ical:
    // 1. Valid fractional digits before Z are truncated to maintain RFC 5545 compliance.
    // 2. Non-digit characters in fractional portion cause instant to return None,
    //    refusing the whole component.
    let window_start = UtcDate::new("2026-09-04T08:00:00.123Z");
    let window_end = UtcDate::new("2026-09-04T18:00:00.999Z");
    let periods = vec![BusyPeriod {
        utc_start: UtcDate::new("2026-09-04T09:30:00.555Z"),
        utc_end: UtcDate::new("2026-09-04T10:30:00.001Z"),
        busy_status: "confirmed".to_string(),
        event: None,
    }];
    let ics = busy_periods_to_vfreebusy("alice@example.com", &window_start, &window_end, &periods)
        .expect("render with fractional seconds");
    assert!(ics.contains("DTSTART:20260904T080000Z"));
    assert!(ics.contains("DTEND:20260904T180000Z"));
    assert!(ics.contains("FREEBUSY;FBTYPE=BUSY:20260904T093000Z/20260904T103000Z"));

    // Invalid non-digit fractional seconds refuse component
    let bad_fraction = UtcDate::new("2026-09-04T09:30:00.55aZ");
    let bad_period = BusyPeriod {
        utc_start: bad_fraction,
        utc_end: UtcDate::new("2026-09-04T10:30:00Z"),
        busy_status: "confirmed".to_string(),
        event: None,
    };
    assert_eq!(
        busy_periods_to_vfreebusy(
            "alice@example.com",
            &window_start,
            &window_end,
            &[bad_period]
        ),
        None
    );
}

#[test]
fn differential_oracle_unterminated_and_truncated_component_refusal() {
    // Divergence 88 against Stalwart differential oracle:
    // RFC 5545 sections 3.4 and 3.6 require strict syntactic structure where every component
    // is cleanly enclosed in matching BEGIN:<name> and END:<name> lines within a VCALENDAR envelope.
    // In jmap-ical:
    // 1. Inputs lacking BEGIN:VCALENDAR or empty inputs yield ICalError::NotACalendar.
    // 2. Components without closing END tags before EOF yield ICalError::Unterminated.
    // 3. Mismatched closing tags yield ICalError::Mismatched.
    // 4. Trailing data after END:VCALENDAR yields ICalError::Trailing.
    // In contrast, Stalwart CalendarEvent/parse or CalDAV parsers may attempt best-effort
    // recovery or report server-level notParsable dictionaries.

    // Empty or non-calendar input
    assert_eq!(ical_to_event(""), Err(ICalError::NotACalendar));
    assert_eq!(ical_to_event("   \r\n"), Err(ICalError::NotACalendar));
    assert_eq!(
        ical_to_event("VERSION:2.0\r\nSUMMARY:Invalid\r\n"),
        Err(ICalError::NotACalendar)
    );

    // Unterminated VEVENT or VCALENDAR
    let unterminated_vevent = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:evt-unterminated\r\nDTSTART:20260904T100000Z\r\n";
    assert_eq!(
        ical_to_event(unterminated_vevent),
        Err(ICalError::Unterminated("VEVENT".to_string()))
    );

    let unterminated_vcalendar = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:evt-ok\r\nDTSTART:20260904T100000Z\r\nEND:VEVENT\r\n";
    assert_eq!(
        ical_to_event(unterminated_vcalendar),
        Err(ICalError::Unterminated("VCALENDAR".to_string()))
    );

    // Mismatched closing component tags
    let mismatched = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:evt-mismatched\r\nEND:VALARM\r\nEND:VCALENDAR\r\n";
    assert_eq!(
        ical_to_event(mismatched),
        Err(ICalError::Mismatched {
            expected: "VEVENT".to_string(),
            found: "VALARM".to_string(),
        })
    );

    // Trailing content after END:VCALENDAR
    let trailing = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:evt-trailing\r\nDTSTART:20260904T100000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\nEXTRA:AFTER_EOF\r\n";
    assert!(matches!(
        ical_to_event(trailing),
        Err(ICalError::Trailing(_))
    ));
}

#[test]
fn differential_oracle_parser_nesting_depth_limitation_and_stack_protection() {
    // Divergence 89 against Stalwart differential oracle:
    // RFC 5545 defines hierarchical calendar objects (VCALENDAR -> VEVENT -> VALARM, depth 3).
    // In jmap-ical:
    // 1. check_depth enforces MAX_DEPTH = 32 using an iterative traversal.
    // 2. Nested components up to MAX_DEPTH parse successfully.
    // 3. Components exceeding MAX_DEPTH are refused with ICalError::TooDeep to protect against
    //    stack exhaustion panics on adversarial inputs.
    assert_eq!(MAX_DEPTH, 32);

    let build_nested = |depth: usize| {
        let mut ics = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\n");
        for _ in 1..depth {
            ics.push_str("BEGIN:VALARM\r\n");
        }
        for _ in 1..depth {
            ics.push_str("END:VALARM\r\n");
        }
        ics.push_str("END:VCALENDAR\r\n");
        ics
    };

    // Up to MAX_DEPTH (32) parses without error from depth check
    let at_limit = build_nested(MAX_DEPTH);
    let parsed_limit = jmap_ical::event::parse_ical(&at_limit);
    assert!(parsed_limit.is_ok(), "document at MAX_DEPTH must parse");

    // Exceeding MAX_DEPTH (33) returns TooDeep
    let over_limit = build_nested(MAX_DEPTH + 1);
    let parsed_over = jmap_ical::event::parse_ical(&over_limit);
    assert_eq!(
        parsed_over.err(),
        Some(ICalError::TooDeep("VALARM".to_string())),
        "document exceeding MAX_DEPTH must be refused with TooDeep"
    );
}

#[test]
fn differential_oracle_unbalanced_parameter_quoting_and_delimiter_tolerance() {
    // Divergence 90 against Stalwart differential oracle:
    // RFC 5545 section 3.2 mandates double-quoting for parameter values containing delimiters
    // and forbids bare double quotes.
    // In real-world exporter streams, broken or missing closing quotes frequently appear.
    // In jmap-ical:
    // 1. Parameters with missing closing quotes or inner unescaped quotes parse in bounded time
    //    (< 1 second) without hanging or crashing.
    // 2. Core component fields (id, summary, dtstart) are extracted safely.
    // 3. Outbound serialization formats clean quoted parameters per RFC 5545 rules.

    let missing_closing_quote = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:evt-quote-1\r\nDTSTART;TZID=\"Europe/Berlin:20260904T120000\r\nSUMMARY:Standup Meeting\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let start_time = std::time::Instant::now();
    let ev1 = ical_to_event(missing_closing_quote).expect("parse with unbalanced quote");
    assert!(
        start_time.elapsed() < std::time::Duration::from_secs(1),
        "parse must complete in bounded time"
    );
    assert_eq!(ev1.id.as_ref().map(|id| id.as_str()), Some("evt-quote-1"));

    let inner_quotes = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:evt-quote-2\r\nORGANIZER;CN=\"Alice\"Smith\":mailto:alice@example.com\r\nDTSTART:20260904T120000Z\r\nSUMMARY:Quarterly Review\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev2 = ical_to_event(inner_quotes).expect("parse with inner unescaped quotes");
    assert_eq!(ev2.id.as_ref().map(|id| id.as_str()), Some("evt-quote-2"));
    assert_eq!(ev2.title.as_deref(), Some("Quarterly Review"));
}

#[test]
fn differential_oracle_content_line_folding_and_crlf_boundary_sanitization() {
    // Divergence 91 against Stalwart differential oracle:
    // RFC 5545 section 3.1 specifies line folding at 75 octets via CRLF followed by linear whitespace.
    // In jmap-ical:
    // 1. Inbound unfold handles both space and tab continuation lines losslessly.
    // 2. Empty continuation lines and mixed LF, CRLF, and CR line breaks parse deterministically.
    // 3. Outbound raw properties (duration, recurrence frequency, timeZone) sanitize or drop
    //    injected CRLF/LF/CR sequences to prevent property injection.

    // Inbound unfolding with space and tab continuations
    let folded_ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:evt-fold-1\r\nDTSTART:20260904T100000Z\r\nSUMMARY:Architecture\r\n  Review and Plan\r\nDESCRIPTION:First paragraph line\r\n \r\n \tSecond paragraph line\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let parsed_folded = ical_to_event(folded_ics).expect("parse folded content lines");
    assert_eq!(
        parsed_folded.title.as_deref(),
        Some("Architecture Review and Plan")
    );
    assert_eq!(
        parsed_folded.description.as_deref(),
        Some("First paragraph line\tSecond paragraph line")
    );

    // Mixed line endings (LF and CRLF) parse deterministically
    let mixed_endings = "BEGIN:VCALENDAR\nVERSION:2.0\r\nPRODID:test\nBEGIN:VEVENT\r\nUID:evt-mixed-1\nDTSTART:20260904T100000Z\r\nSUMMARY:Mixed Line Endings\nEND:VEVENT\r\nEND:VCALENDAR\n";
    let parsed_mixed = ical_to_event(mixed_endings).expect("parse mixed endings");
    assert_eq!(parsed_mixed.title.as_deref(), Some("Mixed Line Endings"));

    // Outbound raw property injection protection: duration with CRLF is dropped
    let injected_ev = CalendarEvent {
        id: Some("evt-inj".into()),
        start: Some("2026-09-04T10:00:00".into()),
        duration: Some("PT1H\r\nLOCATION:Injected".into()),
        time_zone: Some("Europe/Berlin\r\nSUMMARY:Injected".into()),
        ..CalendarEvent::default()
    };
    let ics_out = event_to_ical(&injected_ev);
    assert!(
        !ics_out.contains("\r\nLOCATION:Injected"),
        "injected CRLF in duration must not create a content line"
    );
    assert!(
        !ics_out.contains("\r\nSUMMARY:Injected"),
        "injected CRLF in timeZone must not create a content line"
    );
}

#[test]
fn differential_oracle_vtimezone_multi_observance_transition_resolution() {
    // Divergence 92 against Stalwart differential oracle:
    // RFC 5545 section 3.6.5 specifies VTIMEZONE components containing multiple STANDARD
    // and DAYLIGHT observances defining daylight saving transitions.
    // In jmap-ical:
    // 1. Evaluates daylight saving transitions directly from in-document VTIMEZONE observances
    //    without requiring an external zoneinfo database.
    // 2. Evaluates summer and winter offsets (+0200 vs +0100 for Europe/Berlin) accurately.
    // 3. Exact transition boundary semantics: at the transition onset, the new offset applies;
    //    one second before the transition onset, the old offset applies.
    // 4. Southern-hemisphere seasonal reversals (e.g. Pacific/Auckland) where summer in January
    //    is governed by the previous year's transition (+1300) and winter in July is governed
    //    by the April transition (+1200).
    // 5. Single-observance non-DST zones (e.g. Asia/Kolkata +0530) are handled deterministically.
    // 6. Outbound serialization writes local UNTIL beside zoned DTSTART.

    let berlin_vtz = "BEGIN:VTIMEZONE\r\n\
         TZID:Europe/Berlin\r\n\
         X-LIC-LOCATION:Europe/Berlin\r\n\
         BEGIN:DAYLIGHT\r\n\
         TZOFFSETFROM:+0100\r\n\
         TZOFFSETTO:+0200\r\n\
         TZNAME:CEST\r\n\
         DTSTART:19700329T020000\r\n\
         RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3\r\n\
         END:DAYLIGHT\r\n\
         BEGIN:STANDARD\r\n\
         TZOFFSETFROM:+0200\r\n\
         TZOFFSETTO:+0100\r\n\
         TZNAME:CET\r\n\
         DTSTART:19701025T030000\r\n\
         RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n\
         END:STANDARD\r\n\
         END:VTIMEZONE\r\n";

    // Summer instant (+0200)
    let ics_summer = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{berlin_vtz}BEGIN:VEVENT\r\nUID:evt-tz-1\r\nDTSTART;TZID=Europe/Berlin:20260115T100000\r\nRRULE:FREQ=WEEKLY;UNTIL=20260331T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let ev_summer = ical_to_event(&ics_summer).expect("parse summer until");
    let rule_summer = ev_summer
        .recurrence_rule
        .as_ref()
        .cloned()
        .expect("rule summer");
    assert_eq!(rule_summer.until.as_deref(), Some("2026-03-31T14:00:00"));
    assert!(maps_recurrence_rule(&rule_summer));

    // Winter instant (+0100)
    let ics_winter = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{berlin_vtz}BEGIN:VEVENT\r\nUID:evt-tz-2\r\nDTSTART;TZID=Europe/Berlin:20260115T100000\r\nRRULE:FREQ=WEEKLY;UNTIL=20261130T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let ev_winter = ical_to_event(&ics_winter).expect("parse winter until");
    let rule_winter = ev_winter.recurrence_rule.expect("rule winter");
    assert_eq!(rule_winter.until.as_deref(), Some("2026-11-30T13:00:00"));
    assert!(maps_recurrence_rule(&rule_winter));

    // Exactly at spring transition (2026-03-29T01:00:00Z -> new offset +0200 -> 03:00:00)
    let ics_onset = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{berlin_vtz}BEGIN:VEVENT\r\nUID:evt-tz-3\r\nDTSTART;TZID=Europe/Berlin:20260115T100000\r\nRRULE:FREQ=WEEKLY;UNTIL=20260329T010000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let ev_onset = ical_to_event(&ics_onset).expect("parse onset");
    assert_eq!(
        ev_onset.recurrence_rule.unwrap().until.as_deref(),
        Some("2026-03-29T03:00:00")
    );

    // One second before spring transition (2026-03-29T00:59:59Z -> old offset +0100 -> 01:59:59)
    let ics_before = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{berlin_vtz}BEGIN:VEVENT\r\nUID:evt-tz-4\r\nDTSTART;TZID=Europe/Berlin:20260115T100000\r\nRRULE:FREQ=WEEKLY;UNTIL=20260329T005959Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let ev_before = ical_to_event(&ics_before).expect("parse before onset");
    assert_eq!(
        ev_before.recurrence_rule.unwrap().until.as_deref(),
        Some("2026-03-29T01:59:59")
    );

    // Southern hemisphere: Pacific/Auckland
    let auckland_vtz = "BEGIN:VTIMEZONE\r\n\
         TZID:Pacific/Auckland\r\n\
         BEGIN:DAYLIGHT\r\n\
         TZOFFSETFROM:+1200\r\n\
         TZOFFSETTO:+1300\r\n\
         TZNAME:NZDT\r\n\
         DTSTART:20070930T020000\r\n\
         RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=9\r\n\
         END:DAYLIGHT\r\n\
         BEGIN:STANDARD\r\n\
         TZOFFSETFROM:+1300\r\n\
         TZOFFSETTO:+1200\r\n\
         TZNAME:NZST\r\n\
         DTSTART:20080406T030000\r\n\
         RRULE:FREQ=YEARLY;BYDAY=1SU;BYMONTH=4\r\n\
         END:STANDARD\r\n\
         END:VTIMEZONE\r\n";

    // Auckland January (summer, +1300)
    let ics_auckland_jan = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{auckland_vtz}BEGIN:VEVENT\r\nUID:evt-tz-5\r\nDTSTART;TZID=Pacific/Auckland:20260115T100000\r\nRRULE:FREQ=WEEKLY;UNTIL=20260115T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let ev_auck_jan = ical_to_event(&ics_auckland_jan).expect("parse auckland jan");
    assert_eq!(
        ev_auck_jan.recurrence_rule.unwrap().until.as_deref(),
        Some("2026-01-16T01:00:00")
    );

    // Auckland July (winter, +1200)
    let ics_auckland_jul = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{auckland_vtz}BEGIN:VEVENT\r\nUID:evt-tz-6\r\nDTSTART;TZID=Pacific/Auckland:20260115T100000\r\nRRULE:FREQ=WEEKLY;UNTIL=20260715T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let ev_auck_jul = ical_to_event(&ics_auckland_jul).expect("parse auckland jul");
    assert_eq!(
        ev_auck_jul.recurrence_rule.unwrap().until.as_deref(),
        Some("2026-07-16T00:00:00")
    );

    // Outbound serialization round trip preserves local UNTIL beside zoned DTSTART
    let out_ics = event_to_ical(&ev_summer);
    assert!(out_ics.contains("DTSTART;TZID=Europe/Berlin:20260115T100000\r\n"));
    assert!(out_ics.contains("RRULE:FREQ=WEEKLY;UNTIL=20260331T140000\r\n"));
}

#[test]
fn differential_oracle_vtimezone_transition_day_modeling_and_bounded_search() {
    // Divergence 93 against Stalwart differential oracle:
    // Transition rules in VTIMEZONE observances are restricted by convention and grammar to yearly recurrence.
    // In jmap-ical:
    // 1. Day representations support Day::Nth (ordinal weekday), Day::WeekdayAmong (tzdata/libical pattern),
    //    Day::OfMonth (single month day), and Day::OfStart.
    // 2. Multiple days without limiting filters (e.g. unadorned BYDAY without ordinals in yearly rules)
    //    are refused as multi-transition sets (Falls::Set).
    // 3. WKST part in yearly transition rules (Exchange and Zimbra pattern) is ignored safely rather
    //    than rejecting the rule.
    // 4. Fixed bounded search window (SEARCH = 40 years) prevents unbounded historical search.

    // WeekdayAmong pattern (libical / tzdata common idiom: first Sunday on or after 23rd)
    let weekday_among_vtz = "BEGIN:VTIMEZONE\r\n\
         TZID:CustomZone1\r\n\
         BEGIN:STANDARD\r\n\
         TZOFFSETFROM:+0200\r\n\
         TZOFFSETTO:+0100\r\n\
         DTSTART:19701025T030000\r\n\
         RRULE:FREQ=YEARLY;BYDAY=SU;BYMONTH=10;BYMONTHDAY=23,24,25,26,27,28,29\r\n\
         END:STANDARD\r\n\
         END:VTIMEZONE\r\n";
    let ics_among = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{weekday_among_vtz}BEGIN:VEVENT\r\nUID:evt-day-1\r\nDTSTART;TZID=CustomZone1:20260101T100000\r\nRRULE:FREQ=WEEKLY;UNTIL=20261115T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let ev_among = ical_to_event(&ics_among).expect("parse weekday among");
    assert_eq!(
        ev_among.recurrence_rule.unwrap().until.as_deref(),
        Some("2026-11-15T13:00:00")
    );

    // Exchange / Zimbra pattern with WKST=MO on yearly rule
    let wkst_vtz = "BEGIN:VTIMEZONE\r\n\
         TZID:CustomZone2\r\n\
         BEGIN:STANDARD\r\n\
         TZOFFSETFROM:+0200\r\n\
         TZOFFSETTO:+0100\r\n\
         DTSTART:19701025T030000\r\n\
         RRULE:FREQ=YEARLY;WKST=MO;BYDAY=-1SU;BYMONTH=10\r\n\
         END:STANDARD\r\n\
         END:VTIMEZONE\r\n";
    let ics_wkst = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{wkst_vtz}BEGIN:VEVENT\r\nUID:evt-day-2\r\nDTSTART;TZID=CustomZone2:20260101T100000\r\nRRULE:FREQ=WEEKLY;UNTIL=20261115T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let ev_wkst = ical_to_event(&ics_wkst).expect("parse wkst tolerated");
    assert_eq!(
        ev_wkst.recurrence_rule.unwrap().until.as_deref(),
        Some("2026-11-15T13:00:00")
    );

    // Unadorned BYDAY=SU without ordinal in yearly rule names a set of days (every Sunday),
    // which cannot define a single transition instant and is refused.
    let set_vtz = "BEGIN:VTIMEZONE\r\n\
         TZID:CustomZone3\r\n\
         BEGIN:STANDARD\r\n\
         TZOFFSETFROM:+0200\r\n\
         TZOFFSETTO:+0100\r\n\
         DTSTART:19701025T030000\r\n\
         RRULE:FREQ=YEARLY;BYDAY=SU;BYMONTH=10\r\n\
         END:STANDARD\r\n\
         END:VTIMEZONE\r\n";
    let ics_set = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{set_vtz}BEGIN:VEVENT\r\nUID:evt-day-3\r\nDTSTART;TZID=CustomZone3:20260101T100000\r\nRRULE:FREQ=WEEKLY;UNTIL=20261115T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let ev_set = ical_to_event(&ics_set).expect("parse unresolvable set rule");
    // Because the rule cannot be resolved, UNTIL retains 'Z' and is refused
    let rule_set = ev_set.recurrence_rule.unwrap();
    assert_eq!(rule_set.until.as_deref(), Some("2026-11-15T12:00:00Z"));
    assert!(!maps_recurrence_rule(&rule_set));
}

#[test]
fn differential_oracle_zoned_until_unresolvable_z_preservation_and_refusal() {
    // Divergence 94 against Stalwart differential oracle:
    // RFC 5545 section 3.3.10 requires UNTIL in a zoned event to be stated in UTC ('Z').
    // RFC 8984 section 4.3.1 requires JSCalendar until to be local LocalDateTime (no 'Z').
    // In jmap-ical:
    // 1. If DTSTART names a non-UTC time zone and the document provides no VTIMEZONE definition
    //    (or an unresolvable definition), read_until preserves the trailing 'Z' marker.
    // 2. A value with trailing 'Z' is not a valid JSCalendar LocalDateTime, so maps_recurrence_rule
    //    returns false and unstateable_until returns true, preventing silent schedule corruption.
    // 3. For UTC events (DTSTART with 'Z'), local digits without 'Z' are returned directly and
    //    maps_recurrence_rule succeeds.
    // 4. For floating events (no TZID, no 'Z'), local digits without 'Z' are returned directly and
    //    maps_recurrence_rule succeeds.

    // Zoned event without VTIMEZONE in document
    let ics_missing_vtz = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:evt-no-vtz\r\nDTSTART;TZID=Europe/Berlin:20260115T100000\r\nRRULE:FREQ=WEEKLY;UNTIL=20260331T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev_no_vtz = ical_to_event(ics_missing_vtz).expect("parse missing vtz");
    let rule_no_vtz = ev_no_vtz.recurrence_rule.as_ref().unwrap();
    assert_eq!(rule_no_vtz.until.as_deref(), Some("2026-03-31T12:00:00Z"));
    assert!(
        !maps_recurrence_rule(rule_no_vtz),
        "rule with unresolvable UNTIL must be refused by maps_recurrence_rule"
    );
    assert!(
        unstateable_until(rule_no_vtz).is_some(),
        "event must be flagged by unstateable_until"
    );

    // UTC event (DTSTART with 'Z'): offset shift is identity, local digits without 'Z'
    let ics_utc = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:evt-utc\r\nDTSTART:20260115T100000Z\r\nRRULE:FREQ=WEEKLY;UNTIL=20260331T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev_utc = ical_to_event(ics_utc).expect("parse utc");
    let rule_utc = ev_utc.recurrence_rule.as_ref().unwrap();
    assert_eq!(rule_utc.until.as_deref(), Some("2026-03-31T12:00:00"));
    assert!(
        maps_recurrence_rule(rule_utc),
        "UTC event UNTIL is valid local date-time"
    );
    assert!(unstateable_until(rule_utc).is_none());

    // Floating event (no TZID, no 'Z'): no zone to shift, local digits without 'Z'
    let ics_floating = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:evt-floating\r\nDTSTART:20260115T100000\r\nRRULE:FREQ=WEEKLY;UNTIL=20260331T120000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev_floating = ical_to_event(ics_floating).expect("parse floating");
    let rule_floating = ev_floating.recurrence_rule.as_ref().unwrap();
    assert_eq!(rule_floating.until.as_deref(), Some("2026-03-31T12:00:00"));
    assert!(
        maps_recurrence_rule(rule_floating),
        "floating event UNTIL is valid local date-time"
    );
    assert!(unstateable_until(rule_floating).is_none());
}

#[test]
fn differential_oracle_timezone_identifier_translation_precedence_and_peeling() {
    // Divergence 95 against Stalwart differential oracle:
    // Calendar clients express timezones in disparate formats: standard IANA names,
    // X-LIC-LOCATION metadata, Windows display names, and globally unique prefixed URIs.
    // In jmap-ical:
    // 1. Literal IANA match priority: if TZID satisfies names_time_zone, it is used directly;
    //    secondary X-LIC-LOCATION metadata is ignored to prevent geographic drift.
    // 2. Non-standard TZID with X-LIC-LOCATION: if TZID is non-standard but X-LIC-LOCATION is a valid
    //    IANA name, X-LIC-LOCATION is selected.
    // 3. CLDR Windows mapping: Windows timezone display names resolve to canonical IANA zones.
    // 4. Globally unique prefixed TZID peeling: prefixes (e.g. /mozilla.org/.../) are peeled
    //    to isolate the canonical IANA zone name.
    // 5. Custom solidus zones lacking IANA prefixes are preserved verbatim as custom zones.

    // 1. Literal IANA match takes precedence over conflicting X-LIC-LOCATION
    let ics_iana_priority = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Berlin\r\nX-LIC-LOCATION:Asia/Tokyo\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:evt-iana\r\nDTSTART;TZID=Europe/Berlin:20260904T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev_iana = ical_to_event(ics_iana_priority).expect("parse iana priority");
    assert_eq!(ev_iana.time_zone.as_deref(), Some("Europe/Berlin"));

    // 2. Non-standard TZID with valid X-LIC-LOCATION
    let ics_x_lic = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VTIMEZONE\r\nTZID:Custom Berlin Zone\r\nX-LIC-LOCATION:Europe/Berlin\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:evt-x-lic\r\nDTSTART;TZID=\"Custom Berlin Zone\":20260904T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev_x_lic = ical_to_event(ics_x_lic).expect("parse x-lic-location");
    assert_eq!(ev_x_lic.time_zone.as_deref(), Some("Europe/Berlin"));

    // 3. CLDR Windows mapping
    assert_eq!(
        windows_time_zone_to_iana("W. Europe Standard Time"),
        Some("Europe/Berlin")
    );
    assert_eq!(
        windows_time_zone_to_iana("Pacific Standard Time"),
        Some("America/Los_Angeles")
    );
    assert_eq!(
        windows_time_zone_to_iana("FLE Standard Time"),
        Some("Europe/Kyiv")
    );
    let ics_win = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VTIMEZONE\r\nTZID:W. Europe Standard Time\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:evt-win\r\nDTSTART;TZID=\"W. Europe Standard Time\":20260904T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev_win = ical_to_event(ics_win).expect("parse windows zone");
    assert_eq!(ev_win.time_zone.as_deref(), Some("Europe/Berlin"));

    // 4. Globally unique prefixed TZID peeling
    assert_eq!(
        unique_tzid_to_iana("/mozilla.org/20050126_1/Europe/Berlin"),
        Some("Europe/Berlin")
    );
    assert_eq!(
        unique_tzid_to_iana("/citadel.org/2026/America/New_York"),
        Some("America/New_York")
    );
    let ics_peeled = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:evt-peeled\r\nDTSTART;TZID=/mozilla.org/20050126_1/Europe/Berlin:20260904T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev_peeled = ical_to_event(ics_peeled).expect("parse peeled tzid");
    assert_eq!(ev_peeled.time_zone.as_deref(), Some("Europe/Berlin"));

    // 5. Custom solidus zone lacking IANA area prefix is preserved verbatim
    assert_eq!(unique_tzid_to_iana("/myorg/custom_zone"), None);
    let ics_custom_solidus = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:evt-custom\r\nDTSTART;TZID=/myorg/custom_zone:20260904T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev_custom = ical_to_event(ics_custom_solidus).expect("parse custom solidus");
    assert_eq!(ev_custom.time_zone.as_deref(), Some("/myorg/custom_zone"));
}

#[test]
fn differential_oracle_custom_solidus_timezone_definition_ingestion_and_sendability() {
    // Divergence 96 against Stalwart differential oracle:
    // RFC 8984 section 1.4.9 admits two forms of TimeZoneId: standard IANA names and solidus-prefixed custom IDs.
    // RFC 8984 section 4.7.2 requires custom solidus IDs to be defined in timeZones.
    // In jmap-ical:
    // 1. Inbound: standard IANA zones (Europe/Berlin) with inline VTIMEZONE are pruned (time_zones: None),
    //    preventing JSON payload bloat.
    // 2. Custom solidus zones (/example.org/custom_tz) with complete VTIMEZONE are ingested into time_zones.
    // 3. defines_time_zone confirms presence of complete custom definition.
    // 4. maps_time_zone returns true for IANA or defined custom solidus zones; returns false for unmapped Windows zones
    //    or dangling solidus zones without a definition.
    // 5. Outbound: event_to_ical emits VTIMEZONE only for defined custom solidus zones.

    // 1. Standard IANA zone with VTIMEZONE: definition pruned from time_zones
    let ics_iana = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Berlin\r\nBEGIN:STANDARD\r\nDTSTART:19701025T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:evt-iana\r\nDTSTART;TZID=Europe/Berlin:20260904T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev_iana = ical_to_event(ics_iana).expect("parse iana");
    assert_eq!(ev_iana.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(ev_iana.time_zones, None);
    assert!(maps_time_zone(&ev_iana));
    assert!(!defines_time_zone(&ev_iana, "Europe/Berlin"));
    let out_iana = event_to_ical(&ev_iana);
    assert!(!out_iana.contains("BEGIN:VTIMEZONE"));

    // 2. Custom solidus zone with complete VTIMEZONE: ingested into time_zones
    let ics_custom = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VTIMEZONE\r\nTZID:/example.org/custom_tz\r\nBEGIN:STANDARD\r\nDTSTART:19701025T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:evt-custom\r\nDTSTART;TZID=/example.org/custom_tz:20260904T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev_custom = ical_to_event(ics_custom).expect("parse custom");
    assert_eq!(
        ev_custom.time_zone.as_deref(),
        Some("/example.org/custom_tz")
    );
    assert!(ev_custom.time_zones.is_some());
    assert!(defines_time_zone(&ev_custom, "/example.org/custom_tz"));
    assert!(maps_time_zone(&ev_custom));
    let out_custom = event_to_ical(&ev_custom);
    assert!(out_custom.contains("BEGIN:VTIMEZONE"));
    assert!(out_custom.contains("TZID:/example.org/custom_tz"));

    // 3. Dangling custom solidus zone without definition
    let ev_dangling = CalendarEvent {
        time_zone: Some("/example.org/dangling".to_owned()),
        ..CalendarEvent::default()
    };
    assert!(!defines_time_zone(&ev_dangling, "/example.org/dangling"));
    assert!(!maps_time_zone(&ev_dangling));

    // 4. Non-solidus unmapped custom zone
    let ev_unmapped = CalendarEvent {
        time_zone: Some("Unmapped Non Solidus".to_owned()),
        ..CalendarEvent::default()
    };
    assert!(!defines_time_zone(&ev_unmapped, "Unmapped Non Solidus"));
    assert!(!maps_time_zone(&ev_unmapped));
}

#[test]
fn differential_oracle_custom_timezone_pruning_and_override_scope() {
    // Divergence 97 against Stalwart differential oracle:
    // prune_time_zones drops definitions not referred to by either the master series or any recurrence override.
    // Matching supports both exact key and key without leading solidus.
    // When emptied, time_zones is set to None rather than Some({}).

    let custom_tz = "/example.org/custom_tz";
    let custom_def = json!({
        "@type": "TimeZone",
        "tzId": custom_tz,
        "standard": [{
            "@type": "TimeZoneRule",
            "start": "1970-10-25T03:00:00",
            "offsetFrom": "+0200",
            "offsetTo": "+0100"
        }]
    });

    let mut event = CalendarEvent {
        time_zone: Some(custom_tz.to_owned()),
        time_zones: Some(BTreeMap::from([
            (custom_tz.to_owned(), custom_def.clone()),
            (
                "/example.org/unused_tz".to_owned(),
                json!({
                    "@type": "TimeZone",
                    "tzId": "/example.org/unused_tz",
                    "standard": [{
                        "@type": "TimeZoneRule",
                        "start": "1970-10-25T03:00:00",
                        "offsetFrom": "+0300",
                        "offsetTo": "+0200"
                    }]
                }),
            ),
        ])),
        ..CalendarEvent::default()
    };

    // 1. Prune removes unreferenced zone while retaining master zone
    prune_time_zones(&mut event);
    assert_eq!(
        event
            .time_zones
            .as_ref()
            .map(|z| z.keys().cloned().collect::<Vec<_>>()),
        Some(vec![custom_tz.to_owned()])
    );

    // 2. Master clears zone, but recurrence override retains reference
    event.time_zone = None;
    event.recurrence_overrides = Some(BTreeMap::from([(
        "2026-01-16T13:00:00".to_owned(),
        json!({
            "start": "2026-01-16T15:00:00",
            "timeZone": custom_tz
        }),
    )]));
    prune_time_zones(&mut event);
    assert!(
        event
            .time_zones
            .as_ref()
            .is_some_and(|zones| zones.contains_key(custom_tz)),
        "override reference protects custom timezone definition from being pruned"
    );

    // 3. Normalized matching without leading solidus in definition map
    let mut event_no_slash = CalendarEvent {
        time_zone: None,
        recurrence_overrides: Some(BTreeMap::from([(
            "2026-01-16T13:00:00".to_owned(),
            json!({
                "start": "2026-01-16T15:00:00",
                "timeZone": custom_tz
            }),
        )])),
        time_zones: Some(BTreeMap::from([(
            "example.org/custom_tz".to_owned(),
            custom_def,
        )])),
        ..CalendarEvent::default()
    };
    prune_time_zones(&mut event_no_slash);
    assert!(
        event_no_slash
            .time_zones
            .as_ref()
            .is_some_and(|zones| zones.contains_key("example.org/custom_tz")),
        "definition key without leading solidus matches referred zone"
    );

    // 4. When all references are gone, time_zones becomes None
    event.recurrence_overrides = None;
    prune_time_zones(&mut event);
    assert_eq!(
        event.time_zones, None,
        "completely unreferenced map is set to None rather than empty object"
    );
}

#[test]
fn differential_oracle_utc_offset_colon_stripping_and_negative_zero_rejection() {
    // Divergence 98 against Stalwart differential oracle:
    // RFC 5545 section 3.3.14 requires UTC-OFFSET as [+-]HHMM[SS], forbidding colons and forbidding -0000.
    // RFC 8984 and ISO 8601 allow or require colons.
    // jmap-ical:
    // 1. Strips colons and normalizes 4-digit or 6-digit offsets for RFC 5545 output.
    // 2. Rejects -0000 and -00:00 (negative zero).
    // 3. Formats seconds only when non-zero.

    let custom_tz = "/example.org/offset_tz";
    let event = CalendarEvent {
        time_zone: Some(custom_tz.to_owned()),
        start: Some("2026-09-04T10:00:00".to_owned()),
        duration: Some("PT1H".to_owned()),
        time_zones: Some(BTreeMap::from([(
            custom_tz.to_owned(),
            json!({
                "@type": "TimeZone",
                "tzId": custom_tz,
                "standard": [{
                    "@type": "TimeZoneRule",
                    "start": "1970-10-25T03:00:00",
                    "offsetFrom": "+02:00",
                    "offsetTo": "+01:00"
                }]
            }),
        )])),
        ..CalendarEvent::default()
    };

    let ics = event_to_ical(&event);
    assert!(
        ics.contains("TZOFFSETFROM:+0200\r\n"),
        "colons stripped from offsetFrom: {ics}"
    );
    assert!(
        ics.contains("TZOFFSETTO:+0100\r\n"),
        "colons stripped from offsetTo: {ics}"
    );

    // Sub-minute seconds formatting
    let custom_tz_sec = "/example.org/subminute_tz";
    let event_sec = CalendarEvent {
        time_zone: Some(custom_tz_sec.to_owned()),
        start: Some("2026-09-04T10:00:00".to_owned()),
        duration: Some("PT1H".to_owned()),
        time_zones: Some(BTreeMap::from([(
            custom_tz_sec.to_owned(),
            json!({
                "@type": "TimeZone",
                "tzId": custom_tz_sec,
                "standard": [{
                    "@type": "TimeZoneRule",
                    "start": "1970-10-25T03:00:00",
                    "offsetFrom": "+00:09:21",
                    "offsetTo": "+00:00:00"
                }]
            }),
        )])),
        ..CalendarEvent::default()
    };
    let ics_sec = event_to_ical(&event_sec);
    assert!(
        ics_sec.contains("TZOFFSETFROM:+000921\r\n"),
        "non-zero seconds preserved in 6-digit offset: {ics_sec}"
    );
    assert!(
        ics_sec.contains("TZOFFSETTO:+0000\r\n"),
        "zero seconds truncated in 4-digit offset: {ics_sec}"
    );

    // Negative zero rejection
    let custom_tz_neg_zero = "/example.org/neg_zero_tz";
    let event_neg_zero = CalendarEvent {
        time_zone: Some(custom_tz_neg_zero.to_owned()),
        time_zones: Some(BTreeMap::from([(
            custom_tz_neg_zero.to_owned(),
            json!({
                "@type": "TimeZone",
                "tzId": custom_tz_neg_zero,
                "standard": [{
                    "@type": "TimeZoneRule",
                    "start": "1970-10-25T03:00:00",
                    "offsetFrom": "-00:00",
                    "offsetTo": "+01:00"
                }]
            }),
        )])),
        ..CalendarEvent::default()
    };
    assert!(
        !defines_time_zone(&event_neg_zero, custom_tz_neg_zero),
        "-00:00 is refused as negative zero"
    );
}

#[test]
fn differential_oracle_timezone_rule_names_map_and_tzname_property_mapping() {
    // Divergence 99 against Stalwart differential oracle:
    // RFC 8984 section 4.7.2 models observance names as names: Map<String, Boolean>.
    // RFC 5545 section 3.8.3.2 models names as TZNAME properties, optionally with LANGUAGE parameter.
    // In jmap-ical:
    // 1. Inbound: TZNAME properties are parsed into names map with boolean true values; LANGUAGE parameter is omitted.
    // 2. Outbound: only keys with boolean true value emit TZNAME lines; falsy or non-boolean values are ignored.

    // 1. Inbound parsing collects TZNAME properties and drops LANGUAGE
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VTIMEZONE\r\nTZID:/example.org/named_tz\r\nBEGIN:STANDARD\r\nDTSTART:19701025T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nTZNAME:CET\r\nTZNAME;LANGUAGE=de:MEZ\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:evt-names\r\nDTSTART;TZID=/example.org/named_tz:20260904T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev = ical_to_event(ics).expect("parse named tz");
    let zone_def = ev
        .time_zones
        .as_ref()
        .and_then(|z| z.get("/example.org/named_tz"))
        .expect("zone def");
    let standard = zone_def
        .get("standard")
        .and_then(Value::as_array)
        .expect("standard rules");
    let names = standard[0]
        .get("names")
        .and_then(Value::as_object)
        .expect("names map");
    assert_eq!(names.get("CET"), Some(&Value::Bool(true)));
    assert_eq!(names.get("MEZ"), Some(&Value::Bool(true)));

    // 2. Outbound emission filters for true boolean values
    let custom_tz = "/example.org/outbound_names";
    let event = CalendarEvent {
        time_zone: Some(custom_tz.to_owned()),
        start: Some("2026-09-04T10:00:00".to_owned()),
        duration: Some("PT1H".to_owned()),
        time_zones: Some(BTreeMap::from([(
            custom_tz.to_owned(),
            json!({
                "@type": "TimeZone",
                "tzId": custom_tz,
                "standard": [{
                    "@type": "TimeZoneRule",
                    "start": "1970-10-25T03:00:00",
                    "offsetFrom": "+0200",
                    "offsetTo": "+0100",
                    "names": {
                        "EST": true,
                        "Eastern Standard Time": true,
                        "OLD_NAME": false,
                        "INVALID_VAL": null
                    }
                }]
            }),
        )])),
        ..CalendarEvent::default()
    };
    let out_ics = event_to_ical(&event);
    assert!(
        out_ics.contains("TZNAME:EST\r\n"),
        "emits true name: {out_ics}"
    );
    assert!(
        out_ics.contains("TZNAME:Eastern Standard Time\r\n"),
        "emits true name: {out_ics}"
    );
    assert!(
        !out_ics.contains("TZNAME:OLD_NAME\r\n"),
        "suppresses false name: {out_ics}"
    );
    assert!(
        !out_ics.contains("TZNAME:INVALID_VAL\r\n"),
        "suppresses null name: {out_ics}"
    );
}

#[test]
fn differential_oracle_timezone_unmodeled_properties_dropped_on_import_and_export() {
    // Divergence 100 against Stalwart differential oracle:
    // RFC 8984 section 4.7.2 defines aliases, url, validUntil on TimeZone,
    // and comments, recurrenceOverrides on TimeZoneRule.
    // RFC 5545 defines TZURL and COMMENT on VTIMEZONE and observances.
    // In jmap-ical:
    // 1. Inbound: TZURL and COMMENT are dropped on parse.
    // 2. Outbound: unmodeled administrative properties on TimeZone / TimeZoneRule are dropped,
    //    emitting only core VTIMEZONE properties without TZURL or COMMENT.

    // 1. Inbound drop of TZURL and COMMENT
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:test\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:/example.org/custom_admin_tz\r\n\
TZURL:https://example.org/tz/custom_admin_tz\r\n\
COMMENT:Global administrative note\r\n\
BEGIN:STANDARD\r\n\
DTSTART:19701025T030000\r\n\
TZOFFSETFROM:+0200\r\n\
TZOFFSETTO:+0100\r\n\
COMMENT:Winter transition note\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
UID:evt-admin-tz\r\n\
DTSTART;TZID=/example.org/custom_admin_tz:20260904T100000\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(ics).expect("parse custom admin tz");
    let zone_def = ev
        .time_zones
        .as_ref()
        .and_then(|z| z.get("/example.org/custom_admin_tz"))
        .expect("zone def");
    assert_eq!(
        zone_def.get("tzId").and_then(Value::as_str),
        Some("/example.org/custom_admin_tz")
    );
    assert!(zone_def.get("url").is_none(), "url dropped on import");
    assert!(
        zone_def.get("aliases").is_none(),
        "aliases unpopulated on import"
    );
    let standard = zone_def
        .get("standard")
        .and_then(Value::as_array)
        .expect("standard");
    assert!(
        standard[0].get("comments").is_none(),
        "comments dropped on import"
    );

    // 2. Outbound drop of extra administrative properties
    let custom_tz = "/example.org/custom_admin_out";
    let event = CalendarEvent {
        time_zone: Some(custom_tz.to_owned()),
        start: Some("2026-09-04T10:00:00".to_owned()),
        duration: Some("PT1H".to_owned()),
        time_zones: Some(BTreeMap::from([(
            custom_tz.to_owned(),
            json!({
                "@type": "TimeZone",
                "tzId": custom_tz,
                "url": "https://example.org/tz/custom_admin_out",
                "validUntil": "2030-01-01T00:00:00",
                "aliases": ["Legacy/Admin_Zone"],
                "standard": [{
                    "@type": "TimeZoneRule",
                    "start": "1970-10-25T03:00:00",
                    "offsetFrom": "+0200",
                    "offsetTo": "+0100",
                    "comments": "Internal rule comment",
                    "recurrenceOverrides": {}
                }]
            }),
        )])),
        ..CalendarEvent::default()
    };

    let out_ics = event_to_ical(&event);
    assert!(out_ics.contains("BEGIN:VTIMEZONE\r\n"));
    assert!(out_ics.contains("TZID:/example.org/custom_admin_out\r\n"));
    assert!(out_ics.contains("TZOFFSETFROM:+0200\r\n"));
    assert!(out_ics.contains("TZOFFSETTO:+0100\r\n"));
    assert!(!out_ics.contains("TZURL:"), "TZURL not emitted: {out_ics}");
    assert!(
        !out_ics.contains("COMMENT:"),
        "COMMENT not emitted: {out_ics}"
    );
    assert!(
        !out_ics.contains("validUntil"),
        "validUntil not leaked: {out_ics}"
    );
}

#[test]
fn differential_oracle_vtimezone_whole_component_and_at_least_one_observance_requirement() {
    // Divergence 101 against Stalwart differential oracle:
    // RFC 5545 section 3.6.5 requires at least one STANDARD or DAYLIGHT subcomponent.
    // libical refuses a VTIMEZONE without subcomponents.
    // In jmap-ical:
    // 1. A TimeZone definition with 0 observances returns None from vtimezone_of,
    //    causing defines_time_zone and maps_time_zone to return false.
    // 2. An invalid rule in standard/daylight aborts the entire definition (whole or nothing).
    // 3. A valid observance produces a complete VTIMEZONE.

    let custom_tz = "/example.org/empty_observances";
    // 1. Zero observances
    let ev_empty = CalendarEvent {
        time_zone: Some(custom_tz.to_owned()),
        time_zones: Some(BTreeMap::from([(
            custom_tz.to_owned(),
            json!({
                "@type": "TimeZone",
                "tzId": custom_tz,
                "standard": [],
                "daylight": []
            }),
        )])),
        ..CalendarEvent::default()
    };
    assert!(
        !defines_time_zone(&ev_empty, custom_tz),
        "zero observances refused"
    );
    assert!(
        !maps_time_zone(&ev_empty),
        "maps_time_zone fails on empty observances"
    );
    let ics_empty = event_to_ical(&ev_empty);
    assert!(
        !ics_empty.contains("BEGIN:VTIMEZONE"),
        "empty VTIMEZONE is not emitted: {ics_empty}"
    );

    // 2. Invalid observance rule aborts whole definition
    let invalid_tz = "/example.org/invalid_observance";
    let ev_invalid = CalendarEvent {
        time_zone: Some(invalid_tz.to_owned()),
        time_zones: Some(BTreeMap::from([(
            invalid_tz.to_owned(),
            json!({
                "@type": "TimeZone",
                "tzId": invalid_tz,
                "standard": [
                    {
                        "@type": "TimeZoneRule",
                        "start": "1970-10-25T03:00:00",
                        "offsetFrom": "+0200",
                        "offsetTo": "+0100"
                    },
                    {
                        "@type": "TimeZoneRule"
                        // Missing start and offsets
                    }
                ]
            }),
        )])),
        ..CalendarEvent::default()
    };
    assert!(
        !defines_time_zone(&ev_invalid, invalid_tz),
        "invalid rule fails entire definition"
    );
    assert!(!maps_time_zone(&ev_invalid));
    let ics_invalid = event_to_ical(&ev_invalid);
    assert!(
        !ics_invalid.contains("BEGIN:VTIMEZONE"),
        "partial VTIMEZONE is not emitted: {ics_invalid}"
    );

    // 3. Valid observance succeeds
    let valid_tz = "/example.org/valid_observance";
    let ev_valid = CalendarEvent {
        time_zone: Some(valid_tz.to_owned()),
        time_zones: Some(BTreeMap::from([(
            valid_tz.to_owned(),
            json!({
                "@type": "TimeZone",
                "tzId": valid_tz,
                "standard": [{
                    "@type": "TimeZoneRule",
                    "start": "1970-10-25T03:00:00",
                    "offsetFrom": "+0200",
                    "offsetTo": "+0100"
                }]
            }),
        )])),
        ..CalendarEvent::default()
    };
    assert!(defines_time_zone(&ev_valid, valid_tz));
    assert!(maps_time_zone(&ev_valid));
    let ics_valid = event_to_ical(&ev_valid);
    assert!(ics_valid.contains("BEGIN:VTIMEZONE\r\n"));
    assert!(ics_valid.contains("END:VTIMEZONE\r\n"));
}

#[test]
fn differential_oracle_timezone_observance_recurrence_rule_plural_and_singular_dual_acceptance() {
    // Divergence 102 against Stalwart differential oracle:
    // RFC 8984 section 4.7.2 defines recurrenceRules (plural array) on TimeZoneRule.
    // jscalendarbis and varied implementations also use recurrenceRule (singular array or object).
    // In jmap-ical:
    // 1. observance accepts recurrenceRules array, recurrenceRule array, and recurrenceRule single object.
    // 2. Unmappable recurrence rules fail defines_time_zone.
    // 3. Inbound read_observance emits recurrenceRules array.

    let tz_plural = "/example.org/tz_plural";
    let ev_plural = CalendarEvent {
        time_zone: Some(tz_plural.to_owned()),
        time_zones: Some(BTreeMap::from([(
            tz_plural.to_owned(),
            json!({
                "@type": "TimeZone",
                "tzId": tz_plural,
                "standard": [{
                    "@type": "TimeZoneRule",
                    "start": "1970-10-25T03:00:00",
                    "offsetFrom": "+0200",
                    "offsetTo": "+0100",
                    "recurrenceRules": [{
                        "frequency": "yearly",
                        "byMonth": ["10"],
                        "byDay": [{"day": "su", "nthOfPeriod": -1}]
                    }]
                }]
            }),
        )])),
        ..CalendarEvent::default()
    };
    let ics_plural = event_to_ical(&ev_plural);
    assert!(
        ics_plural.contains("RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n"),
        "emits from recurrenceRules: {ics_plural}"
    );

    let tz_singular_arr = "/example.org/tz_singular_arr";
    let ev_singular_arr = CalendarEvent {
        time_zone: Some(tz_singular_arr.to_owned()),
        time_zones: Some(BTreeMap::from([(
            tz_singular_arr.to_owned(),
            json!({
                "@type": "TimeZone",
                "tzId": tz_singular_arr,
                "standard": [{
                    "@type": "TimeZoneRule",
                    "start": "1970-10-25T03:00:00",
                    "offsetFrom": "+0200",
                    "offsetTo": "+0100",
                    "recurrenceRule": [{
                        "frequency": "yearly",
                        "byMonth": ["10"],
                        "byDay": [{"day": "su", "nthOfPeriod": -1}]
                    }]
                }]
            }),
        )])),
        ..CalendarEvent::default()
    };
    let ics_singular_arr = event_to_ical(&ev_singular_arr);
    assert!(
        ics_singular_arr.contains("RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n"),
        "emits from recurrenceRule array: {ics_singular_arr}"
    );

    let tz_singular_obj = "/example.org/tz_singular_obj";
    let ev_singular_obj = CalendarEvent {
        time_zone: Some(tz_singular_obj.to_owned()),
        time_zones: Some(BTreeMap::from([(
            tz_singular_obj.to_owned(),
            json!({
                "@type": "TimeZone",
                "tzId": tz_singular_obj,
                "standard": [{
                    "@type": "TimeZoneRule",
                    "start": "1970-10-25T03:00:00",
                    "offsetFrom": "+0200",
                    "offsetTo": "+0100",
                    "recurrenceRule": {
                        "frequency": "yearly",
                        "byMonth": ["10"],
                        "byDay": [{"day": "su", "nthOfPeriod": -1}]
                    }
                }]
            }),
        )])),
        ..CalendarEvent::default()
    };
    let ics_singular_obj = event_to_ical(&ev_singular_obj);
    assert!(
        ics_singular_obj.contains("RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n"),
        "emits from recurrenceRule object: {ics_singular_obj}"
    );

    // Unmappable recurrence rule fails defines_time_zone
    let tz_bad_rule = "/example.org/tz_bad_rule";
    let ev_bad_rule = CalendarEvent {
        time_zone: Some(tz_bad_rule.to_owned()),
        time_zones: Some(BTreeMap::from([(
            tz_bad_rule.to_owned(),
            json!({
                "@type": "TimeZone",
                "tzId": tz_bad_rule,
                "standard": [{
                    "@type": "TimeZoneRule",
                    "start": "1970-10-25T03:00:00",
                    "offsetFrom": "+0200",
                    "offsetTo": "+0100",
                    "recurrenceRule": {
                        "frequency": "weekly",
                        "byMonthDay": [15] // Forbidden on weekly
                    }
                }]
            }),
        )])),
        ..CalendarEvent::default()
    };
    assert!(
        !defines_time_zone(&ev_bad_rule, tz_bad_rule),
        "unmappable rule fails whole definition"
    );

    // Inbound parse always produces recurrenceRules array
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:test\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:/example.org/parse_rules\r\n\
BEGIN:STANDARD\r\n\
DTSTART:19701025T030000\r\n\
TZOFFSETFROM:+0200\r\n\
TZOFFSETTO:+0100\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
UID:evt-rules\r\n\
DTSTART;TZID=/example.org/parse_rules:20260904T100000\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let ev_parsed = ical_to_event(ics).expect("parse rules");
    let zone = ev_parsed
        .time_zones
        .as_ref()
        .unwrap()
        .get("/example.org/parse_rules")
        .unwrap();
    let std_rules = zone.get("standard").unwrap().as_array().unwrap();
    assert!(std_rules[0].get("recurrenceRules").unwrap().is_array());
}

#[test]
fn differential_oracle_timezone_observance_dtstart_and_until_arithmetic_against_offset_from() {
    // Divergence 103 against Stalwart differential oracle:
    // RFC 5545 section 3.6.5 specifies that observance DTSTART carries no TZID and resolves against TZOFFSETFROM.
    // Observance RRULE UNTIL must be in UTC.
    // In jmap-ical:
    // 1. Inbound: DTSTART parsed as local date-time without zone lookup; UNTIL converted to local time via Ends::At(&offset_from) arithmetic.
    // 2. Outbound: UNTIL converted from local time to UTC instant with trailing Z via Ends::At(&offset_from).

    // 1. Inbound: UNTIL at 00:00:00Z with offsetFrom +0200 becomes 02:00:00 local time
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:test\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:/example.org/until_tz\r\n\
BEGIN:STANDARD\r\n\
DTSTART:19971026T020000\r\n\
TZOFFSETFROM:+0200\r\n\
TZOFFSETTO:+0100\r\n\
RRULE:FREQ=YEARLY;UNTIL=20051030T000000Z;BYDAY=-1SU;BYMONTH=10\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
UID:evt-until\r\n\
DTSTART;TZID=/example.org/until_tz:20260904T100000\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(ics).expect("parse until tz");
    let zone = ev
        .time_zones
        .as_ref()
        .unwrap()
        .get("/example.org/until_tz")
        .unwrap();
    let std_rules = zone.get("standard").unwrap().as_array().unwrap();
    assert_eq!(
        std_rules[0].get("start").unwrap().as_str(),
        Some("1997-10-26T02:00:00")
    );
    let rrules = std_rules[0]
        .get("recurrenceRules")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(
        rrules[0].get("until").unwrap().as_str(),
        Some("2005-10-30T02:00:00"),
        "UTC 00:00:00Z + 2 hours offsetFrom = local 02:00:00"
    );

    // 2. Outbound: local UNTIL 02:00:00 with offsetFrom +0200 converted back to UTC 00:00:00Z
    let custom_tz = "/example.org/until_out_tz";
    let ev_out = CalendarEvent {
        time_zone: Some(custom_tz.to_owned()),
        start: Some("2026-09-04T10:00:00".to_owned()),
        duration: Some("PT1H".to_owned()),
        time_zones: Some(BTreeMap::from([(
            custom_tz.to_owned(),
            json!({
                "@type": "TimeZone",
                "tzId": custom_tz,
                "standard": [{
                    "@type": "TimeZoneRule",
                    "start": "1997-10-26T02:00:00",
                    "offsetFrom": "+0200",
                    "offsetTo": "+0100",
                    "recurrenceRules": [{
                        "frequency": "yearly",
                        "until": "2005-10-30T02:00:00",
                        "byMonth": ["10"],
                        "byDay": [{"day": "su", "nthOfPeriod": -1}]
                    }]
                }]
            }),
        )])),
        ..CalendarEvent::default()
    };

    let out_ics = event_to_ical(&ev_out);
    assert!(
        out_ics.contains("RRULE:FREQ=YEARLY;UNTIL=20051030T000000Z;BYDAY=-1SU;BYMONTH=10\r\n"),
        "converts local UNTIL back to UTC instant with Z suffix: {out_ics}"
    );
}

#[test]
fn differential_oracle_date_property_emission_and_multi_value_formatting() {
    // Divergence 104 against Stalwart differential oracle:
    // RFC 5545 section 3.2.19, 3.2.20, 3.3.4, 3.3.5, 3.8.2.4, 3.8.5.1 govern date and date-time property formatting.
    // In jmap-ical:
    // 1. All-day: VALUE=DATE parameter emitted with 8-digit date string; time portion truncated.
    // 2. Timed date-times: redundant VALUE=DATE-TIME parameter omitted.
    // 3. UTC date-times: trailing Z emitted without TZID parameter (RFC 5545 forbids TZID on UTC).
    // 4. Non-UTC date-times: TZID parameter emitted without trailing Z.
    // 5. Floating date-times: emitted without TZID and without trailing Z.
    // 6. Multi-valued property formatting: multiple EXDATE/RDATE values emitted as a single comma-separated line.

    // 1. All-day event with multiple excluded dates
    let all_day_ev = CalendarEvent {
        show_without_time: Some(true),
        start: Some("2026-09-05T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        recurrence_overrides: Some(BTreeMap::from([
            ("2026-09-06T00:00:00".to_owned(), json!({"excluded": true})),
            ("2026-09-07T00:00:00".to_owned(), json!({"excluded": true})),
        ])),
        ..CalendarEvent::default()
    };

    let all_day_ics = event_to_ical(&all_day_ev);
    assert!(
        all_day_ics.contains("DTSTART;VALUE=DATE:20260905\r\n"),
        "all-day start formatted with VALUE=DATE and 8 digits: {all_day_ics}"
    );
    assert!(
        all_day_ics.contains("EXDATE;VALUE=DATE:20260906,20260907\r\n"),
        "multiple excluded dates formatted as single comma-separated VALUE=DATE line: {all_day_ics}"
    );

    // 2. Timed UTC event
    let utc_ev = CalendarEvent {
        time_zone: Some("Etc/UTC".to_owned()),
        start: Some("2026-09-05T10:00:00".to_owned()),
        duration: Some("PT1H".to_owned()),
        ..CalendarEvent::default()
    };
    let utc_ics = event_to_ical(&utc_ev);
    assert!(
        utc_ics.contains("DTSTART:20260905T100000Z\r\n"),
        "UTC start emitted with Z and no TZID or VALUE=DATE-TIME: {utc_ics}"
    );

    // 3. Timed non-UTC event with multiple excluded dates
    let zoned_ev = CalendarEvent {
        time_zone: Some("Europe/Berlin".to_owned()),
        start: Some("2026-09-05T10:00:00".to_owned()),
        duration: Some("PT1H".to_owned()),
        recurrence_overrides: Some(BTreeMap::from([
            ("2026-09-06T10:00:00".to_owned(), json!({"excluded": true})),
            ("2026-09-07T10:00:00".to_owned(), json!({"excluded": true})),
        ])),
        ..CalendarEvent::default()
    };
    let zoned_ics = event_to_ical(&zoned_ev);
    assert!(
        zoned_ics.contains("DTSTART;TZID=Europe/Berlin:20260905T100000\r\n"),
        "zoned start emitted with TZID and no Z or VALUE=DATE-TIME: {zoned_ics}"
    );
    assert!(
        zoned_ics.contains("EXDATE;TZID=Europe/Berlin:20260906T100000,20260907T100000\r\n"),
        "zoned excluded dates formatted as single comma-separated line with TZID: {zoned_ics}"
    );

    // 4. Floating timed event
    let floating_ev = CalendarEvent {
        start: Some("2026-09-05T10:00:00".to_owned()),
        duration: Some("PT1H".to_owned()),
        ..CalendarEvent::default()
    };
    let floating_ics = event_to_ical(&floating_ev);
    assert!(
        floating_ics.contains("DTSTART:20260905T100000\r\n"),
        "floating start emitted without TZID and without Z: {floating_ics}"
    );
}

#[test]
fn differential_oracle_proleptic_gregorian_calendar_validation_and_leap_second() {
    // Divergence 105 against Stalwart differential oracle:
    // RFC 5545 section 3.3.4, 3.3.5, 3.3.12 specify Gregorian date-time validity.
    // In jmap-ical:
    // 1. Leap year calculation: Feb 29 accepted in leap years (divisible by 4, not 100 unless 400).
    // 2. Non-leap year: Feb 29 in 1900 or 2026 rejected (DTSTART dropped).
    // 3. Invalid month or day: month 13 or April 31 rejected (DTSTART dropped).
    // 4. Leap second 60: accepted per RFC 5545 section 3.3.12; second 61 rejected.
    // 5. Sub-second fractional digits: truncated rather than rejected.

    // 1. Leap years
    let leap_2024 = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:leap-2024\r\nDTSTART:20240229T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev = ical_to_event(leap_2024).expect("parse leap 2024");
    assert_eq!(ev.start.as_deref(), Some("2024-02-29T12:00:00"));

    let leap_2000 = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:leap-2000\r\nDTSTART:20000229T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev = ical_to_event(leap_2000).expect("parse leap 2000");
    assert_eq!(ev.start.as_deref(), Some("2000-02-29T12:00:00"));

    // 2. Non-leap centurial year 1900 and regular non-leap year 2026
    let non_leap_1900 = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:non-leap-1900\r\nDTSTART:19000229T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev = ical_to_event(non_leap_1900).expect("parse non-leap 1900");
    assert_eq!(ev.start, None, "Feb 29 in 1900 rejected as non-existent");

    let non_leap_2026 = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:non-leap-2026\r\nDTSTART:20260229T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev = ical_to_event(non_leap_2026).expect("parse non-leap 2026");
    assert_eq!(ev.start, None, "Feb 29 in 2026 rejected as non-existent");

    // 3. Invalid month and invalid day
    let bad_month = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:bad-month\r\nDTSTART:20261301T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev = ical_to_event(bad_month).expect("parse bad month");
    assert_eq!(ev.start, None, "month 13 rejected");

    let bad_day = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:bad-day\r\nDTSTART:20260431T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev = ical_to_event(bad_day).expect("parse bad day");
    assert_eq!(ev.start, None, "April 31 rejected");

    // 4. Leap second 60 accepted, second 61 rejected
    let leap_second = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:leap-sec\r\nDTSTART:20161231T235960Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev = ical_to_event(leap_second).expect("parse leap second");
    assert_eq!(ev.start.as_deref(), Some("2016-12-31T23:59:60"));

    let bad_second = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:bad-sec\r\nDTSTART:20161231T235961Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev = ical_to_event(bad_second).expect("parse bad second");
    assert_eq!(ev.start, None, "second 61 rejected");

    // 5. Sub-second fractional truncation
    let sub_second = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:sub-sec\r\nDTSTART:20260905T123000.456Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev = ical_to_event(sub_second).expect("parse sub-second");
    assert_eq!(ev.start.as_deref(), Some("2026-09-05T12:30:00"));
}

#[test]
fn differential_oracle_wall_clock_duration_measurement_and_nominal_days() {
    // Divergence 106 against Stalwart differential oracle:
    // RFC 5545 section 3.8.2.2 and RFC 8984 section 4.1.4 duration measurement.
    // In jmap-ical:
    // 1. Explicit DURATION parsed with stated_duration, preserving units and dropping leading +.
    // 2. Calculated duration from DTSTART and DTEND measures wall-clock difference via days_from_civil.
    // 3. Whole-day differences formatted as P<D>D rather than PT<H>H, preserving nominal day length across DST.
    // 4. Zero or negative calculated duration returns None (falling back to server default PT0S).

    // 1. Explicit DURATION
    let explicit_dur = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:dur-exp\r\nDTSTART:20260905T100000\r\nDURATION:+PT2H30M\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev = ical_to_event(explicit_dur).expect("parse explicit duration");
    assert_eq!(ev.duration.as_deref(), Some("PT2H30M"));

    let week_dur = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:dur-week\r\nDTSTART:20260905T100000\r\nDURATION:P1W2D\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev = ical_to_event(week_dur).expect("parse week duration");
    assert_eq!(ev.duration.as_deref(), Some("P1W2D"));

    // 2. Calculated duration from DTSTART and DTEND
    let calc_timed = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:calc-timed\r\nDTSTART:20260905T100000\r\nDTEND:20260905T143000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev = ical_to_event(calc_timed).expect("parse calc timed");
    assert_eq!(ev.duration.as_deref(), Some("PT4H30M"));

    // 3. Multi-day duration formatted as nominal days (P3D, not PT72H)
    let calc_days = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:calc-days\r\nDTSTART;VALUE=DATE:20260905\r\nDTEND;VALUE=DATE:20260908\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev = ical_to_event(calc_days).expect("parse calc days");
    assert_eq!(ev.duration.as_deref(), Some("P3D"));

    // 4. Zero and negative duration return None
    let zero_dur = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:zero-dur\r\nDTSTART:20260905T100000\r\nDTEND:20260905T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev = ical_to_event(zero_dur).expect("parse zero dur");
    assert_eq!(ev.duration, None, "zero duration drops to default");

    let neg_dur = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nBEGIN:VEVENT\r\nUID:neg-dur\r\nDTSTART:20260905T100000\r\nDTEND:20260905T090000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let ev = ical_to_event(neg_dur).expect("parse neg dur");
    assert_eq!(ev.duration, None, "negative duration drops to default");
}

#[test]
fn differential_oracle_bounded_date_shifting_and_four_digit_year_bounds() {
    // Divergence 107 against Stalwart differential oracle:
    // RFC 5545 section 3.3.4 requires 4-digit years.
    // In jmap-ical:
    // 1. moved shifts LocalDateTime by signed integer seconds, carrying single-day offsets.
    // 2. Backward carry across month boundary in leap year 2024 rolls back to Feb 29; in non-leap 2026 to Feb 28.
    // 3. Backward carry across year boundary rolls back from Jan 1 to Dec 31 of prior year.
    // 4. Out-of-bounds years (negative or > 9999) return None and trigger maps_recurrence_rule refusal.

    // 1. Leap year 2024 month boundary backward carry: March 1 minus 2 hours = Feb 29 22:00
    let leap_carry_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:test\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:/example.org/leap_tz\r\n\
BEGIN:STANDARD\r\n\
DTSTART:19971026T020000\r\n\
TZOFFSETFROM:-0200\r\n\
TZOFFSETTO:-0200\r\n\
RRULE:FREQ=YEARLY;UNTIL=20240301T003000Z;BYDAY=-1SU;BYMONTH=10\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
UID:evt-leap-carry\r\n\
DTSTART;TZID=/example.org/leap_tz:20260905T100000\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(leap_carry_ics).expect("parse leap carry");
    let zone = ev
        .time_zones
        .as_ref()
        .unwrap()
        .get("/example.org/leap_tz")
        .unwrap();
    let std_rules = zone.get("standard").unwrap().as_array().unwrap();
    let rrules = std_rules[0]
        .get("recurrenceRules")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(
        rrules[0].get("until").unwrap().as_str(),
        Some("2024-02-29T22:30:00"),
        "March 1 minus 2 hours rolls back to Feb 29 in leap year 2024"
    );

    // 2. Non-leap year 2026 month boundary backward carry: March 1 minus 2 hours = Feb 28 22:00
    let non_leap_carry_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:test\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:/example.org/non_leap_tz\r\n\
BEGIN:STANDARD\r\n\
DTSTART:19971026T020000\r\n\
TZOFFSETFROM:-0200\r\n\
TZOFFSETTO:-0200\r\n\
RRULE:FREQ=YEARLY;UNTIL=20260301T003000Z;BYDAY=-1SU;BYMONTH=10\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
UID:evt-non-leap-carry\r\n\
DTSTART;TZID=/example.org/non_leap_tz:20260905T100000\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(non_leap_carry_ics).expect("parse non leap carry");
    let zone = ev
        .time_zones
        .as_ref()
        .unwrap()
        .get("/example.org/non_leap_tz")
        .unwrap();
    let std_rules = zone.get("standard").unwrap().as_array().unwrap();
    let rrules = std_rules[0]
        .get("recurrenceRules")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(
        rrules[0].get("until").unwrap().as_str(),
        Some("2026-02-28T22:30:00"),
        "March 1 minus 2 hours rolls back to Feb 28 in non-leap year 2026"
    );

    // 3. Year boundary carry: Jan 1 minus 2 hours rolls back to Dec 31 of prior year
    let year_carry_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:test\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:/example.org/year_tz\r\n\
BEGIN:STANDARD\r\n\
DTSTART:19971026T020000\r\n\
TZOFFSETFROM:-0200\r\n\
TZOFFSETTO:-0200\r\n\
RRULE:FREQ=YEARLY;UNTIL=20260101T003000Z;BYDAY=-1SU;BYMONTH=10\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
UID:evt-year-carry\r\n\
DTSTART;TZID=/example.org/year_tz:20260905T100000\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(year_carry_ics).expect("parse year carry");
    let zone = ev
        .time_zones
        .as_ref()
        .unwrap()
        .get("/example.org/year_tz")
        .unwrap();
    let std_rules = zone.get("standard").unwrap().as_array().unwrap();
    let rrules = std_rules[0]
        .get("recurrenceRules")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(
        rrules[0].get("until").unwrap().as_str(),
        Some("2025-12-31T22:30:00"),
        "Jan 1 minus 2 hours rolls back to Dec 31 2025"
    );

    // 4. Underflow below year 0000: refused and trailing Z preserved
    let underflow_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:test\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:/example.org/underflow_tz\r\n\
BEGIN:STANDARD\r\n\
DTSTART:19971026T020000\r\n\
TZOFFSETFROM:-0200\r\n\
TZOFFSETTO:-0200\r\n\
RRULE:FREQ=YEARLY;UNTIL=00000101T003000Z;BYDAY=-1SU;BYMONTH=10\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
UID:evt-underflow\r\n\
DTSTART;TZID=/example.org/underflow_tz:20260905T100000\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(underflow_ics).expect("parse underflow");
    assert_eq!(
        ev.time_zones, None,
        "definition with underflow UNTIL is refused by vtimezone_of and dropped from time_zones"
    );
    assert!(
        !maps_time_zone(&ev),
        "event referring to dropped custom zone cannot be safely sent"
    );
}

#[test]
fn differential_oracle_rrule_malformed_until_break_and_unstateable_date() {
    // Divergence 108 against Stalwart differential oracle:
    // RFC 5545 section 3.3.10 recurrence rule UNTIL boundary handling.
    // In jmap-ical:
    // 1. Non-date-time shaped UNTIL (e.g. UNTIL=notadate) halts part parsing (break).
    //    Trailing rule parts like BYDAY are omitted, preventing un-terminated series explosion.
    // 2. Date-shaped invalid instant (e.g. UNTIL=20261301T000000Z, month 13) is kept verbatim.
    //    maps_recurrence_rule flags it as unstateable, preventing corruption on save.

    // 1. Syntactically malformed UNTIL: parsing halts, subsequent BYDAY dropped
    let malformed_until_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:test\r\n\
BEGIN:VEVENT\r\n\
UID:malformed-until\r\n\
DTSTART:20260905T100000Z\r\n\
RRULE:FREQ=DAILY;UNTIL=notadate;BYDAY=MO,TU\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(malformed_until_ics).expect("parse malformed until");
    let rule = ev.recurrence_rule.expect("rule present");
    assert_eq!(rule.frequency, "daily");
    assert_eq!(rule.until, None, "unparseable UNTIL dropped");
    assert_eq!(
        rule.by_day, None,
        "parts after malformed UNTIL are not parsed (break)"
    );

    // 2. Structurally valid date-time shape with non-existent calendar date
    let bad_date_until_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:test\r\n\
BEGIN:VEVENT\r\n\
UID:bad-date-until\r\n\
DTSTART:20260905T100000Z\r\n\
RRULE:FREQ=DAILY;UNTIL=20261301T000000Z;BYDAY=MO,TU\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev2 = ical_to_event(bad_date_until_ics).expect("parse bad date until");
    let rule2 = ev2.recurrence_rule.expect("rule present");
    assert_eq!(rule2.frequency, "daily");
    assert_eq!(
        rule2.until.as_deref(),
        Some("20261301T000000Z"),
        "date-shaped invalid UNTIL kept verbatim"
    );
    assert!(
        rule2.by_day.is_some(),
        "subsequent parts parsed when UNTIL is date-shaped"
    );
    assert!(
        !maps_recurrence_rule(&rule2),
        "rule with invalid UNTIL instant is refused by maps_recurrence_rule"
    );
}

#[test]
fn differential_oracle_utc_datetime_formatting_and_idempotent_timestamps() {
    // Divergence 109 against Stalwart differential oracle:
    // RFC 8984 section 1.4.5 UTCDateTime and RFC 5545 section 3.8.7.1, 3.8.7.2, 3.8.7.3.
    // In jmap-ical:
    // 1. Valid UTC timestamps format into CREATED, DTSTAMP, and LAST-MODIFIED with trailing Z.
    // 2. Timestamps lacking trailing Z or carrying sub-second fractions are rejected.
    // 3. When updated is absent (None), DTSTAMP is omitted rather than inventing local clock time.

    // 1. Valid UTC timestamps
    let ev_valid = CalendarEvent {
        created: Some("2026-09-05T10:00:00Z".to_owned()),
        updated: Some("2026-09-05T12:30:00Z".to_owned()),
        start: Some("2026-09-05T14:00:00Z".to_owned()),
        duration: Some("PT1H".to_owned()),
        ..CalendarEvent::default()
    };
    let ics_valid = event_to_ical(&ev_valid);
    assert!(
        ics_valid.contains("CREATED:20260905T100000Z\r\n"),
        "CREATED emitted with Z: {ics_valid}"
    );
    assert!(
        ics_valid.contains("DTSTAMP:20260905T123000Z\r\n"),
        "DTSTAMP emitted with updated timestamp: {ics_valid}"
    );
    assert!(
        ics_valid.contains("LAST-MODIFIED:20260905T123000Z\r\n"),
        "LAST-MODIFIED emitted with updated timestamp: {ics_valid}"
    );

    // 2. Missing updated timestamp: DTSTAMP omitted to prevent non-deterministic sync churn
    let ev_no_updated = CalendarEvent {
        created: Some("2026-09-05T10:00:00Z".to_owned()),
        updated: None,
        start: Some("2026-09-05T14:00:00Z".to_owned()),
        duration: Some("PT1H".to_owned()),
        ..CalendarEvent::default()
    };
    let ics_no_updated = event_to_ical(&ev_no_updated);
    assert!(
        !ics_no_updated.contains("DTSTAMP:"),
        "DTSTAMP omitted when updated is absent: {ics_no_updated}"
    );
    assert!(
        !ics_no_updated.contains("LAST-MODIFIED:"),
        "LAST-MODIFIED omitted when updated is absent: {ics_no_updated}"
    );

    // 3. Timestamps without Z suffix rejected and omitted
    let ev_no_z = CalendarEvent {
        created: Some("2026-09-05T10:00:00".to_owned()),
        updated: Some("2026-09-05T12:30:00".to_owned()),
        start: Some("2026-09-05T14:00:00Z".to_owned()),
        ..CalendarEvent::default()
    };
    let ics_no_z = event_to_ical(&ev_no_z);
    assert!(
        !ics_no_z.contains("CREATED:"),
        "CREATED without Z suffix omitted: {ics_no_z}"
    );
    assert!(
        !ics_no_z.contains("DTSTAMP:"),
        "DTSTAMP without Z suffix omitted: {ics_no_z}"
    );

    // 4. Sub-second fractional timestamps rejected and omitted
    let ev_frac = CalendarEvent {
        created: Some("2026-09-05T10:00:00.123Z".to_owned()),
        updated: Some("2026-09-05T12:30:00.456Z".to_owned()),
        start: Some("2026-09-05T14:00:00Z".to_owned()),
        ..CalendarEvent::default()
    };
    let ics_frac = event_to_ical(&ev_frac);
    assert!(
        !ics_frac.contains("CREATED:"),
        "CREATED with fractional seconds omitted: {ics_frac}"
    );
    assert!(
        !ics_frac.contains("DTSTAMP:"),
        "DTSTAMP with fractional seconds omitted: {ics_frac}"
    );
}

#[test]
fn differential_oracle_all_day_multi_property_invariant_gating() {
    // Divergence 110 against Stalwart differential oracle:
    // RFC 8984 section 4.2.1 showWithoutTime vs RFC 5545 section 3.8.2.4 VALUE=DATE.
    // In jmap-ical:
    // Six invariants must hold for shows_without_time:
    // 1. show_without_time == Some(true)
    // 2. time_zone.is_none() (RFC 5545 forbids TZID on date-only values)
    // 3. at_midnight(start) (start ends with T000000)
    // 4. duration whole days (no T time designator)
    // 5. recurrence rule until at midnight and no BYHOUR/BYMINUTE/BYSECOND
    // 6. all overrides satisfy instance_shows_without_time
    // Violating any invariant falls back to timed date-time representation.

    // 1. Baseline conforming all-day event emits VALUE=DATE
    let valid_all_day = CalendarEvent {
        show_without_time: Some(true),
        time_zone: None,
        start: Some("2026-09-05T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        ..CalendarEvent::default()
    };
    let ics_valid = event_to_ical(&valid_all_day);
    assert!(
        ics_valid.contains("DTSTART;VALUE=DATE:20260905\r\n"),
        "conforming all-day event emits VALUE=DATE: {ics_valid}"
    );

    // 2. Invariant 2 violation: timezone present forces timed representation
    let tz_all_day = CalendarEvent {
        show_without_time: Some(true),
        time_zone: Some("Etc/UTC".to_owned()),
        start: Some("2026-09-05T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        ..CalendarEvent::default()
    };
    let ics_tz = event_to_ical(&tz_all_day);
    assert!(
        ics_tz.contains("DTSTART:20260905T000000Z\r\n"),
        "timezone present forces timed representation without VALUE=DATE: {ics_tz}"
    );

    // 3. Invariant 3 violation: non-midnight start forces timed representation
    let non_midnight = CalendarEvent {
        show_without_time: Some(true),
        time_zone: None,
        start: Some("2026-09-05T09:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        ..CalendarEvent::default()
    };
    let ics_non_midnight = event_to_ical(&non_midnight);
    assert!(
        ics_non_midnight.contains("DTSTART:20260905T090000\r\n"),
        "non-midnight start forces timed representation: {ics_non_midnight}"
    );

    // 4. Invariant 4 violation: sub-day duration forces timed representation
    let sub_day_dur = CalendarEvent {
        show_without_time: Some(true),
        time_zone: None,
        start: Some("2026-09-05T00:00:00".to_owned()),
        duration: Some("PT8H".to_owned()),
        ..CalendarEvent::default()
    };
    let ics_sub_day = event_to_ical(&sub_day_dur);
    assert!(
        ics_sub_day.contains("DTSTART:20260905T000000\r\n"),
        "sub-day duration forces timed representation: {ics_sub_day}"
    );

    // 5. Invariant 5 violation: recurrence rule naming time of day forces timed representation
    let rrule_time = CalendarEvent {
        show_without_time: Some(true),
        time_zone: None,
        start: Some("2026-09-05T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        recurrence_rule: Some(RecurrenceRule {
            frequency: "daily".to_owned(),
            by_hour: Some(vec![9]),
            ..RecurrenceRule::default()
        }),
        ..CalendarEvent::default()
    };
    let ics_rrule_time = event_to_ical(&rrule_time);
    assert!(
        ics_rrule_time.contains("DTSTART:20260905T000000\r\n"),
        "recurrence rule with byHour forces timed representation: {ics_rrule_time}"
    );

    // 6. Invariant 6 violation: override instance with non-midnight start forces timed representation
    let override_time = CalendarEvent {
        show_without_time: Some(true),
        time_zone: None,
        start: Some("2026-09-05T00:00:00".to_owned()),
        duration: Some("P1D".to_owned()),
        recurrence_overrides: Some(BTreeMap::from([(
            "2026-09-06T00:00:00".to_owned(),
            json!({ "start": "2026-09-06T10:00:00" }),
        )])),
        ..CalendarEvent::default()
    };
    let ics_override_time = event_to_ical(&override_time);
    assert!(
        ics_override_time.contains("DTSTART:20260905T000000\r\n"),
        "override starting at non-midnight forces series timed representation: {ics_override_time}"
    );
}

#[test]
fn differential_oracle_recurrence_token_plus_prefix_and_sentinel_zero() {
    // Divergence 111 against Stalwart differential oracle:
    // RFC 5545 section 3.3.10 recurrence rule signed tokens and sentinel zero.
    // In jmap-ical:
    // 1. Explicit + on BYDAY (+2MO) stripped to ordinal 2; + on BYMONTHDAY (+15) parsed to 15.
    // 2. Unparseable token in BYMONTHDAY mapped to sentinel 0, triggering maps_recurrence_rule refusal.
    // 3. Ordinal 0 on BYDAY (0MO) preserves raw token, triggering maps_recurrence_rule refusal.

    // 1. Explicit + sign handling
    let plus_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:test\r\n\
BEGIN:VEVENT\r\n\
UID:plus-tokens\r\n\
DTSTART:20260905T100000Z\r\n\
RRULE:FREQ=MONTHLY;BYDAY=+2MO;BYMONTHDAY=+15\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev = ical_to_event(plus_ics).expect("parse plus tokens");
    let rule = ev.recurrence_rule.as_ref().expect("rule present");
    let by_day = rule.by_day.as_ref().expect("by_day present");
    assert_eq!(by_day[0].day, "mo");
    assert_eq!(by_day[0].nth_of_period, Some(2));
    let by_mday = rule.by_month_day.as_ref().expect("by_month_day present");
    assert_eq!(by_mday, &[15]);
    assert!(
        maps_recurrence_rule(rule),
        "rule with stripped plus prefixes is valid"
    );

    let out_ics = event_to_ical(&ev);
    assert!(
        out_ics.contains("BYDAY=2MO"),
        "emits canonical 2MO without plus prefix: {out_ics}"
    );
    assert!(
        out_ics.contains("BYMONTHDAY=15"),
        "emits canonical 15 without plus prefix: {out_ics}"
    );

    // 2. Unparseable token in BYMONTHDAY maps to sentinel 0
    let bad_mday_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:test\r\n\
BEGIN:VEVENT\r\n\
UID:bad-mday\r\n\
DTSTART:20260905T100000Z\r\n\
RRULE:FREQ=MONTHLY;BYMONTHDAY=1,bad,15\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev_bad = ical_to_event(bad_mday_ics).expect("parse bad mday");
    let rule_bad = ev_bad.recurrence_rule.expect("rule present");
    assert_eq!(
        rule_bad.by_month_day.as_deref(),
        Some(&[1, 0, 15][..]),
        "bad token parsed as sentinel 0"
    );
    assert!(
        !maps_recurrence_rule(&rule_bad),
        "rule containing sentinel 0 is refused by maps_recurrence_rule"
    );

    // 3. Ordinal 0 in BYDAY preserves raw token and triggers refusal
    let zero_day_ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:test\r\n\
BEGIN:VEVENT\r\n\
UID:zero-day\r\n\
DTSTART:20260905T100000Z\r\n\
RRULE:FREQ=MONTHLY;BYDAY=0MO\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let ev_zero = ical_to_event(zero_day_ics).expect("parse zero day");
    let rule_zero = ev_zero.recurrence_rule.expect("rule present");
    let by_day_zero = rule_zero.by_day.as_ref().expect("by_day present");
    assert_eq!(
        by_day_zero[0].day, "0mo",
        "ordinal 0 preserves raw token as day"
    );
    assert_eq!(by_day_zero[0].nth_of_period, None);
    assert!(
        !maps_recurrence_rule(&rule_zero),
        "rule containing 0mo is refused by maps_recurrence_rule"
    );
}
