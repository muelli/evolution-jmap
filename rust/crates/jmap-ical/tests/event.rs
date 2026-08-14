// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSCalendar `CalendarEvent` ↔ iCalendar `VEVENT`, the minimal property set
//! the calendar backend needs: UID, SUMMARY, DESCRIPTION, DTSTART (+timeZone,
//! or as a date for showWithoutTime), DURATION, STATUS, LOCATION, RRULE, and
//! the instances named one at a time by an EXDATE, an RDATE, or a component of
//! their own carrying a RECURRENCE-ID.

use jmap_ical::{
    ICalError, event_to_ical, ical_to_event, maps_alerts, maps_keyword, maps_locations,
    maps_recurrence_override, maps_recurrence_rule, maps_virtual_locations, names_time_zone,
};
use jmap_proto::calendars::{CalendarEvent, NDay, RecurrenceRule};
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
    event.recurrence_rules = Some(vec![RecurrenceRule {
        frequency: "weekly".to_owned(),
        ..RecurrenceRule::default()
    }]);
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
    event.recurrence_rules = Some(vec![RecurrenceRule {
        frequency: "weekly".to_owned(),
        ..RecurrenceRule::default()
    }]);
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

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rules
        .unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].frequency, "weekly");
    assert_eq!(rules[0].interval, Some(2));
    assert_eq!(rules[0].count, Some(10));
    assert_eq!(rules[0].rule_type.as_deref(), Some("RecurrenceRule"));
}

#[test]
fn an_interval_of_one_is_left_implicit() {
    let event = CalendarEvent {
        recurrence_rules: Some(vec![RecurrenceRule {
            interval: Some(1),
            ..RecurrenceRule::new("daily")
        }]),
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
        recurrence_rules: Some(vec![RecurrenceRule {
            until: Some("2026-12-31T09:00:00".to_owned()),
            ..RecurrenceRule::new("monthly")
        }]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=MONTHLY;UNTIL=20261231T090000Z"
    );

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rules
        .unwrap();
    assert_eq!(rules[0].until.as_deref(), Some("2026-12-31T09:00:00"));
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
        recurrence_rules: Some(vec![RecurrenceRule {
            until: Some("2026-12-31T00:00:00".to_owned()),
            ..RecurrenceRule::new("weekly")
        }]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "RRULE:"), "RRULE:FREQ=WEEKLY;UNTIL=20261231");

    let read_back = ical_to_event(&ics).expect("parse");
    let rules = read_back.recurrence_rules.expect("a rule came back");
    assert_eq!(rules[0].until.as_deref(), Some("2026-12-31T00:00:00"));
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
        recurrence_rules: Some(vec![RecurrenceRule {
            until: Some("2026-12-31T09:00:00".to_owned()),
            ..RecurrenceRule::new("weekly")
        }]),
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
    // all is the smaller lie, and the save path is told so: recurrenceRules is
    // patched only when every rule the server holds survives the trip.
    for until in ["2026-13-31T09:00:00", "whenever", "2026-02-30T09:00:00"] {
        let rule = RecurrenceRule {
            until: Some(until.to_owned()),
            ..RecurrenceRule::new("weekly")
        };
        assert!(!maps_recurrence_rule(&rule), "{until}");

        let event = CalendarEvent {
            recurrence_rules: Some(vec![rule]),
            ..CalendarEvent::default()
        };
        assert!(without(&event_to_ical(&event), "RRULE"), "{until}");
    }

    // A rule with no frequency has no RRULE spelling at all, and never had one.
    assert!(!maps_recurrence_rule(&RecurrenceRule::new("")));
}

#[test]
fn a_rule_with_unmodeled_parts_is_flagged_rather_than_silently_narrowed() {
    // `rscale` & friends ride in `extra` and do not survive the trip through
    // iCalendar, so the save path must not patch recurrenceRules for them. (It
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
        recurrence_rules: Some(vec![RecurrenceRule {
            by_day: Some(vec![NDay::new("mo"), NDay::new("th")]),
            count: Some(6),
            ..RecurrenceRule::new("weekly")
        }]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=WEEKLY;COUNT=6;BYDAY=MO,TH"
    );

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rules
        .unwrap();
    assert_eq!(
        rules[0].by_day.as_deref(),
        Some(&[NDay::new("mo"), NDay::new("th")][..])
    );
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules[0]));
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
        recurrence_rules: Some(vec![RecurrenceRule {
            by_day: Some(days.clone()),
            ..RecurrenceRule::new("monthly")
        }]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "RRULE:"), "RRULE:FREQ=MONTHLY;BYDAY=2WE,-1FR");

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rules
        .unwrap();
    assert_eq!(rules[0].by_day.as_deref(), Some(&days[..]));
    assert!(maps_recurrence_rule(&rules[0]));
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
        .recurrence_rules
        .expect("a rule came back");
    assert_eq!(
        rules[0].by_day.as_deref(),
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
            recurrence_rules: Some(vec![rule]),
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
            recurrence_rules: Some(vec![rule]),
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
        recurrence_rules: Some(vec![RecurrenceRule {
            by_month_day: Some(vec![15, -1]),
            count: Some(6),
            ..RecurrenceRule::new("monthly")
        }]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=MONTHLY;COUNT=6;BYMONTHDAY=15,-1"
    );

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rules
        .unwrap();
    assert_eq!(rules[0].by_month_day.as_deref(), Some(&[15, -1][..]));
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules[0]));
}

#[test]
fn the_days_of_the_month_are_written_after_the_days_of_the_week() {
    // Both parts at once, in the order libical writes them, so that a rule read
    // back out of EDS's own cache compares equal to the one that went in.
    let event = CalendarEvent {
        recurrence_rules: Some(vec![RecurrenceRule {
            by_day: Some(vec![NDay::new("we")]),
            by_month_day: Some(vec![15]),
            ..RecurrenceRule::new("yearly")
        }]),
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
    let rules = ical_to_event(ics).expect("parse").recurrence_rules.unwrap();
    assert_eq!(rules[0].by_month_day.as_deref(), Some(&[1, -31][..]));
    assert!(maps_recurrence_rule(&rules[0]));
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
        recurrence_rules: Some(vec![rule]),
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
            recurrence_rules: Some(vec![rule]),
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
    // below this crate and cannot be seen from here — see docs/NIGHT-LOG.md.
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
    let rules = event.recurrence_rules.as_deref().unwrap();
    assert_eq!(rules[0].by_month_day.as_deref(), Some(&[15, 32][..]));
    assert!(!maps_recurrence_rule(&rules[0]));
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
        recurrence_rules: Some(vec![RecurrenceRule {
            by_year_day: Some(vec![1, -1]),
            count: Some(4),
            ..RecurrenceRule::new("yearly")
        }]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=YEARLY;COUNT=4;BYYEARDAY=1,-1"
    );

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rules
        .unwrap();
    assert_eq!(rules[0].by_year_day.as_deref(), Some(&[1, -1][..]));
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules[0]));
}

#[test]
fn the_days_of_the_year_are_written_after_the_days_of_the_month() {
    // Every modeled part at once, in the order libical writes them —
    // `BYYEARDAY` between `BYMONTHDAY` and `BYMONTH` — so that a rule read back
    // out of EDS's own cache compares equal to the one that went in.
    let event = CalendarEvent {
        recurrence_rules: Some(vec![RecurrenceRule {
            by_day: Some(vec![NDay::new("we")]),
            by_month_day: Some(vec![15]),
            by_year_day: Some(vec![100]),
            by_month: Some(vec!["3".to_owned()]),
            ..RecurrenceRule::new("yearly")
        }]),
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
    let rules = event.recurrence_rules.as_deref().unwrap();
    assert_eq!(rules[0].by_year_day.as_deref(), Some(&[1, -366][..]));
    assert!(maps_recurrence_rule(&rules[0]));
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
            recurrence_rules: Some(vec![rule]),
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
            recurrence_rules: Some(vec![RecurrenceRule {
                by_year_day: Some(vec![100]),
                ..RecurrenceRule::new(frequency)
            }]),
            ..CalendarEvent::default()
        };
        let ics = event_to_ical(&event);
        assert!(
            line(&ics, "RRULE:").ends_with(";BYYEARDAY=100"),
            "{frequency}"
        );
        let rules = ical_to_event(&ics)
            .expect("parse")
            .recurrence_rules
            .unwrap();
        assert!(maps_recurrence_rule(&rules[0]), "{frequency}");
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
            recurrence_rules: Some(vec![rule]),
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
    let rules = event.recurrence_rules.as_deref().unwrap();
    assert_eq!(rules[0].by_year_day.as_deref(), Some(&[100, 367][..]));
    assert!(!maps_recurrence_rule(&rules[0]));
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
        recurrence_rules: Some(vec![RecurrenceRule {
            by_month: Some(vec!["3".to_owned(), "9".to_owned()]),
            count: Some(4),
            ..RecurrenceRule::new("yearly")
        }]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=YEARLY;COUNT=4;BYMONTH=3,9"
    );

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rules
        .unwrap();
    assert_eq!(
        rules[0].by_month.as_deref(),
        Some(&["3".to_owned(), "9".to_owned()][..])
    );
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules[0]));
}

#[test]
fn the_months_are_written_after_the_days_of_the_month() {
    // All three parts at once, in the order libical and calcard both write them
    // — `BYMONTH` last — so that a rule read back out of EDS's own cache
    // compares equal to the one that went in.
    let event = CalendarEvent {
        recurrence_rules: Some(vec![RecurrenceRule {
            by_day: Some(vec![NDay::new("we")]),
            by_month_day: Some(vec![15]),
            by_month: Some(vec!["3".to_owned()]),
            ..RecurrenceRule::new("yearly")
        }]),
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
            recurrence_rules: Some(vec![RecurrenceRule {
                by_month: Some(vec!["1".to_owned()]),
                ..RecurrenceRule::new(frequency)
            }]),
            ..CalendarEvent::default()
        };
        let ics = event_to_ical(&event);
        assert!(line(&ics, "RRULE:").ends_with(";BYMONTH=1"), "{frequency}");
        let rules = ical_to_event(&ics)
            .expect("parse")
            .recurrence_rules
            .unwrap();
        assert!(maps_recurrence_rule(&rules[0]), "{frequency}");
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
    let rules = event.recurrence_rules.as_deref().unwrap();
    assert_eq!(
        rules[0].by_month.as_deref(),
        Some(&["3".to_owned(), "12".to_owned()][..])
    );
    assert!(maps_recurrence_rule(&rules[0]));
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
            recurrence_rules: Some(vec![rule]),
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
        recurrence_rules: Some(vec![rule]),
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
        let rules = event.recurrence_rules.as_deref().unwrap();
        assert!(!maps_recurrence_rule(&rules[0]), "{value}");
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
        recurrence_rules: Some(vec![RecurrenceRule {
            interval: Some(2),
            by_day: Some(vec![NDay::new("tu")]),
            first_day_of_week: Some("su".to_owned()),
            ..RecurrenceRule::new("weekly")
        }]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=TU;WKST=SU"
    );

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rules
        .unwrap();
    assert_eq!(rules[0].first_day_of_week.as_deref(), Some("su"));
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules[0]));
}

#[test]
fn the_day_the_week_starts_on_is_written_last() {
    // Every modeled part at once, in the order libical and calcard both write
    // them — `WKST` after `BYMONTH`, last of all — so that a rule read back out
    // of EDS's own cache compares equal to the one that went in.
    let event = CalendarEvent {
        recurrence_rules: Some(vec![RecurrenceRule {
            by_day: Some(vec![NDay::new("we")]),
            by_month_day: Some(vec![15]),
            by_year_day: Some(vec![100]),
            by_month: Some(vec!["3".to_owned()]),
            first_day_of_week: Some("su".to_owned()),
            ..RecurrenceRule::new("yearly")
        }]),
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
        recurrence_rules: Some(vec![rule]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "RRULE:"), "RRULE:FREQ=WEEKLY");
    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rules
        .unwrap();
    assert_eq!(rules[0].first_day_of_week, None);
}

#[test]
fn the_day_the_week_starts_on_is_carried_at_any_frequency() {
    // RFC 5545 §3.3.10 does not exclude `WKST` at any frequency — it says only
    // where the part is *significant*, which is a reader's business rather than a
    // writer's — and libical keeps it beside every one. So there is no frequency
    // gate: the day the server named is carried as it came.
    for frequency in ["daily", "weekly", "monthly", "yearly"] {
        let event = CalendarEvent {
            recurrence_rules: Some(vec![RecurrenceRule {
                first_day_of_week: Some("su".to_owned()),
                ..RecurrenceRule::new(frequency)
            }]),
            ..CalendarEvent::default()
        };
        let ics = event_to_ical(&event);
        assert!(line(&ics, "RRULE:").ends_with(";WKST=SU"), "{frequency}");
        let rules = ical_to_event(&ics)
            .expect("parse")
            .recurrence_rules
            .unwrap();
        assert_eq!(
            rules[0].first_day_of_week.as_deref(),
            Some("su"),
            "{frequency}"
        );
        assert!(maps_recurrence_rule(&rules[0]), "{frequency}");
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
            recurrence_rules: Some(vec![RecurrenceRule {
                first_day_of_week: Some(day.to_owned()),
                ..RecurrenceRule::new("weekly")
            }]),
            ..CalendarEvent::default()
        };
        let ics = event_to_ical(&event);
        assert_eq!(
            line(&ics, "RRULE:"),
            format!("RRULE:FREQ=WEEKLY;WKST={token}")
        );
        let rules = ical_to_event(&ics)
            .expect("parse")
            .recurrence_rules
            .unwrap();
        assert_eq!(rules[0].first_day_of_week.as_deref(), Some(day), "{day}");
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
    let rules = event.recurrence_rules.as_deref().unwrap();
    assert_eq!(rules[0].first_day_of_week.as_deref(), Some("sa"));
    assert!(maps_recurrence_rule(&rules[0]));
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
            recurrence_rules: Some(vec![rule]),
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
        recurrence_rules: Some(vec![RecurrenceRule {
            by_week_no: Some(vec![1, -1]),
            first_day_of_week: Some("su".to_owned()),
            count: Some(4),
            ..RecurrenceRule::new("yearly")
        }]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=YEARLY;COUNT=4;BYWEEKNO=1,-1;WKST=SU"
    );

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rules
        .unwrap();
    assert_eq!(rules[0].by_week_no.as_deref(), Some(&[1, -1][..]));
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules[0]));
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
        recurrence_rules: Some(vec![rule.clone()]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        content_line(&ics, "RRULE:"),
        "RRULE:FREQ=YEARLY;BYDAY=WE;BYMONTHDAY=15;BYYEARDAY=100;BYWEEKNO=20;BYMONTH=3;WKST=SU"
    );

    // And the fold survives the trip back: every part arrives as it left, so a
    // save comparing the two sees no edit.
    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rules
        .unwrap();
    assert_eq!(rules[0], rule);
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
    let rules = event.recurrence_rules.as_deref().unwrap();
    assert_eq!(rules[0].by_week_no.as_deref(), Some(&[1, -53][..]));
    assert!(maps_recurrence_rule(&rules[0]));
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
            recurrence_rules: Some(vec![rule]),
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
            recurrence_rules: Some(vec![rule]),
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
    let rules = event.recurrence_rules.as_deref().unwrap();
    assert_eq!(rules[0].by_week_no.as_deref(), Some(&[20, 54][..]));
    assert!(!maps_recurrence_rule(&rules[0]));
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
        recurrence_rules: Some(vec![RecurrenceRule {
            by_day: Some(vec![NDay::new("fr")]),
            by_set_position: Some(vec![-1]),
            count: Some(4),
            ..RecurrenceRule::new("monthly")
        }]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=MONTHLY;COUNT=4;BYDAY=FR;BYSETPOS=-1"
    );

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rules
        .unwrap();
    assert_eq!(rules[0].by_set_position.as_deref(), Some(&[-1][..]));
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules[0]));
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
        recurrence_rules: Some(vec![rule.clone()]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        content_line(&ics, "RRULE:"),
        "RRULE:FREQ=YEARLY;BYDAY=WE;BYMONTHDAY=15;BYYEARDAY=100;BYWEEKNO=20;\
         BYMONTH=3;BYSETPOS=2;WKST=SU"
    );

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rules
        .unwrap();
    assert_eq!(rules[0], rule);
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
    let rules = event.recurrence_rules.as_deref().unwrap();
    assert_eq!(rules[0].by_set_position.as_deref(), Some(&[1, -1][..]));
    assert!(maps_recurrence_rule(&rules[0]));
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
            recurrence_rules: Some(vec![rule.clone()]),
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
            recurrence_rules: Some(vec![rule]),
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
    let rules = event.recurrence_rules.as_deref().unwrap();
    assert_eq!(rules[0].by_set_position.as_deref(), Some(&[1, 367][..]));
    assert!(!maps_recurrence_rule(&rules[0]));
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
        recurrence_rules: Some(vec![rule.clone()]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "RRULE:"), "RRULE:FREQ=DAILY;BYHOUR=9,17");

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rules
        .unwrap();
    assert_eq!(rules[0].by_hour.as_deref(), Some(&[9, 17][..]));
    assert_eq!(rules[0], rule);
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules[0]));
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
        recurrence_rules: Some(vec![rule.clone()]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        content_line(&ics, "RRULE:"),
        "RRULE:FREQ=YEARLY;BYSECOND=0;BYMINUTE=30;BYHOUR=9;BYDAY=WE;BYMONTHDAY=15;\
         BYYEARDAY=100;BYWEEKNO=20;BYMONTH=3;BYSETPOS=2;WKST=SU"
    );

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rules
        .unwrap();
    assert_eq!(rules[0], rule);
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
    let rules = event.recurrence_rules.as_deref().unwrap();
    assert_eq!(rules[0].by_hour.as_deref(), Some(&[9, 17][..]));
    assert!(maps_recurrence_rule(&rules[0]));
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
            recurrence_rules: Some(vec![rule]),
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
    let rules = event.recurrence_rules.as_deref().unwrap();
    assert!(!maps_recurrence_rule(&rules[0]));
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
        recurrence_rules: Some(vec![RecurrenceRule {
            by_hour: Some(vec![9]),
            ..RecurrenceRule::new("daily")
        }]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "DTSTART"), "DTSTART:20260115T000000");
    assert_eq!(line(&ics, "RRULE:"), "RRULE:FREQ=DAILY;BYHOUR=9");

    // And the save path compares against this same rendering, so the flag lost
    // here is not read back as the user having cleared it.
    let read_back = ical_to_event(&ics).expect("parse");
    assert_eq!(read_back.show_without_time, None);
    assert_eq!(read_back.recurrence_rules, event.recurrence_rules);
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
        recurrence_rules: Some(vec![RecurrenceRule {
            by_hour: Some(vec![24]),
            ..RecurrenceRule::new("daily")
        }]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "DTSTART"), "DTSTART;VALUE=DATE:20260115");
    assert_eq!(line(&ics, "RRULE:"), "RRULE:FREQ=DAILY");
    assert!(!maps_recurrence_rule(
        &event.recurrence_rules.as_ref().unwrap()[0]
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
        recurrence_rules: Some(vec![rule.clone()]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=HOURLY;BYSECOND=0;BYMINUTE=0,30"
    );

    let rules = ical_to_event(&ics)
        .expect("parse")
        .recurrence_rules
        .unwrap();
    assert_eq!(rules[0].by_minute.as_deref(), Some(&[0, 30][..]));
    assert_eq!(rules[0].by_second.as_deref(), Some(&[0][..]));
    assert_eq!(rules[0], rule);
    // Which is what tells the save path it may write the property back.
    assert!(maps_recurrence_rule(&rules[0]));
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
    let rules = event.recurrence_rules.as_deref().unwrap();
    assert_eq!(rules[0].by_minute.as_deref(), Some(&[15, 45][..]));
    assert_eq!(rules[0].by_second.as_deref(), Some(&[0, 30][..]));
    assert!(maps_recurrence_rule(&rules[0]));
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
        recurrence_rules: Some(vec![leap]),
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
            recurrence_rules: Some(vec![rule]),
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
    let rules = event.recurrence_rules.as_deref().unwrap();
    assert_eq!(rules[0].by_minute.as_deref(), Some(&[30, 60][..]));
    assert_eq!(rules[0].by_second.as_deref(), Some(&[15, 61][..]));
    assert!(!maps_recurrence_rule(&rules[0]));
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
        recurrence_rules: Some(vec![RecurrenceRule {
            by_minute: Some(vec![30]),
            by_second: Some(vec![15]),
            ..RecurrenceRule::new("daily")
        }]),
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
    assert_eq!(read_back.recurrence_rules, event.recurrence_rules);
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
        recurrence_rules: Some(vec![RecurrenceRule {
            by_minute: Some(vec![60]),
            ..RecurrenceRule::new("daily")
        }]),
        ..CalendarEvent::default()
    };
    let ics = event_to_ical(&event);
    assert_eq!(line(&ics, "DTSTART"), "DTSTART;VALUE=DATE:20260115");
    assert_eq!(line(&ics, "RRULE:"), "RRULE:FREQ=DAILY");
    assert!(!maps_recurrence_rule(
        &event.recurrence_rules.as_ref().unwrap()[0]
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
        recurrence_rules: Some(vec![RecurrenceRule::new("weekly")]),
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
        recurrence_rules: Some(vec![RecurrenceRule::new("weekly")]),
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
        recurrence_rules: Some(vec![RecurrenceRule::new("weekly")]),
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
        recurrence_rules: Some(vec![RecurrenceRule::new("weekly")]),
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
        recurrence_rules: Some(vec![RecurrenceRule::new("weekly")]),
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
        recurrence_rules: Some(vec![RecurrenceRule::new("weekly")]),
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
        vtimezone("W. Europe Standard Time", "W. Europe Standard Time"),
    ] {
        let ics = zoned("W. Europe Standard Time", &vtimezone);

        let event = ical_to_event(&ics).expect("parse");

        assert_eq!(
            event.time_zone.as_deref(),
            Some("W. Europe Standard Time"),
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
/// save path leave it alone: [`names_time_zone`] refuses a custom identifier, so
/// `patch::diff` never sends one — the zone stays the server's, definition and
/// all.
#[test]
fn a_custom_zone_is_read_back_as_the_identifier_it_was_drawn_with() {
    let ics = event_to_ical(&defining(CUSTOM_TZID, json!({CUSTOM_TZID: custom_zone()})));

    let back = ical_to_event(&ics).expect("parse");

    assert_eq!(back.time_zone.as_deref(), Some(CUSTOM_TZID));
    assert_eq!(back.start.as_deref(), Some("2026-01-15T13:00:00"));
    // The definitions are the server's own and no save writes them back, so
    // nothing reads them off the document either — see `read_vevent`.
    assert_eq!(back.time_zones, None);
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
        recurrence_rules: Some(vec![RecurrenceRule {
            frequency: "daily".to_owned(),
            ..RecurrenceRule::default()
        }]),
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
            "ATTENDEE;CN=Bob Example;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:\
             mailto:bob@example.com",
            "ATTENDEE;CN=Carol Example;ROLE=OPT-PARTICIPANT;PARTSTAT=NEEDS-ACTION;\
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
        "ORGANIZER;CN=Alice Example:mailto:alice@example.com",
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
        "ATTENDEE;CN=Bob Example;ROLE=REQ-PARTICIPANT:mailto:bob@example.com",
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
        "ORGANIZER;CN=Alice Example:mailto:alice@example.com",
        "{ics}"
    );
    assert_eq!(
        content_line(&ics, "ATTENDEE"),
        "ATTENDEE;CN=Alice Example;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:\
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
            format!("ATTENDEE;CN=Room 1;CUTYPE={cutype}:mailto:room-1@example.com"),
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
        "ATTENDEE;CN=Bob Example:mailto:bob@example.com",
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
        "ATTENDEE;CN=Bob Example;ROLE=REQ-PARTICIPANT:mailto:bob@example.com",
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
            "CONFERENCE;VALUE=URI;FEATURE=AUDIO,VIDEO;LABEL=Team room;X-JMAP-KEY=v1:\
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
            "CONFERENCE;VALUE=URI;FEATURE=SCREEN;LABEL=Team room;X-JMAP-KEY=v1:https://meet.example.com/sprint"
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
        ["CONFERENCE;VALUE=URI;LABEL=Team room;X-JMAP-KEY=v1:https://meet.example.com/sprint"],
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
        "ORGANIZER;CN=Alice Owner:mailto:alice@example.com",
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
            "ATTENDEE;CN=Alice Owner;ROLE=CHAIR;PARTSTAT=ACCEPTED:mailto:alice@example.com",
            "ATTENDEE;CN=Bob Engineer;ROLE=REQ-PARTICIPANT;PARTSTAT=DECLINED:mailto:bob@example.com",
            "ATTENDEE;CN=Carol Observer;ROLE=NON-PARTICIPANT;PARTSTAT=TENTATIVE:mailto:carol@example.com",
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
