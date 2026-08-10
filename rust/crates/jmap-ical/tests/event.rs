// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSCalendar `CalendarEvent` ↔ iCalendar `VEVENT`, the minimal property set
//! the calendar backend needs: UID, SUMMARY, DESCRIPTION, DTSTART (+timeZone,
//! or as a date for showWithoutTime), DURATION, STATUS, RRULE, and the
//! instances named one at a time by an EXDATE, an RDATE, or a component of
//! their own carrying a RECURRENCE-ID.

use jmap_ical::{
    ICalError, event_to_ical, ical_to_event, maps_recurrence_override, maps_recurrence_rule,
    names_time_zone,
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
    // byMinute & friends ride in `extra` and do not survive the trip through
    // iCalendar, so the save path must not patch recurrenceRules for them.
    let mut rule = RecurrenceRule::new("monthly");
    rule.extra.insert("byMinute".to_owned(), json!([15, 45]));
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
fn the_hours_of_the_day_are_written_before_every_other_part() {
    // Every modeled part at once, in the order libical writes them — and this
    // one goes *first*, ahead of `BYDAY`, unlike every part added before it.
    // Measured in `jmap-backend-cal/tests/marshal.rs`: a rule that went out in
    // another order comes back out of EDS's own cache reordered and compares
    // unequal to itself, which the save path reads as an edit.
    let rule = RecurrenceRule {
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
        "RRULE:FREQ=YEARLY;BYHOUR=9;BYDAY=WE;BYMONTHDAY=15;BYYEARDAY=100;\
         BYWEEKNO=20;BYMONTH=3;BYSETPOS=2;WKST=SU"
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
    assert!(maps_recurrence_override("2026-01-29T13:00:00", &patch));

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
    assert!(maps_recurrence_override("2026-01-29T13:00:00", &patch));

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
    assert!(maps_recurrence_override("2026-01-29T13:00:00", &patch));

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
        assert!(
            !maps_recurrence_override("2026-01-29T13:00:00", &patch),
            "{patch}"
        );

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
    assert!(maps_recurrence_override(
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
    assert!(!maps_recurrence_override("2026-01-29T13:00:00", &patch));

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
        json!({"start": "2026-02-30T13:00:00"}),
    ] {
        assert!(
            !maps_recurrence_override("2026-01-29T13:00:00", &patch),
            "{patch}"
        );

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
    assert!(!maps_recurrence_override("2026-01-29T13:00:00", &patch));

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
        assert!(
            !maps_recurrence_override(id, &json!({"excluded": true})),
            "{id}"
        );

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
        json!({"status": null, "duration": null}),
    ] {
        assert!(
            maps_recurrence_override("2026-01-29T13:00:00", &patch),
            "{patch}"
        );
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
            !maps_recurrence_override("2026-01-29T13:00:00", &json!({"duration": value})),
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
            maps_recurrence_override("2026-01-29T13:00:00", &json!({"duration": value})),
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
        "BEGIN:VALARM\r\n",
        "ACTION:DISPLAY\r\n",
        "TRIGGER:-PT15M\r\n",
        "END:VALARM\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    assert_eq!(event.title.as_deref(), Some("Dentist"));
    // An unmapped property is a property we never write back, not a parse
    // failure: an event that loses its alarm still opens.
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
