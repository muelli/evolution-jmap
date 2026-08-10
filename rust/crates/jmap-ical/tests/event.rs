// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSCalendar `CalendarEvent` ↔ iCalendar `VEVENT`, the minimal property set
//! the calendar backend needs: UID, SUMMARY, DESCRIPTION, DTSTART (+timeZone),
//! DURATION, STATUS, RRULE.

use jmap_ical::{ICalError, event_to_ical, ical_to_event, maps_recurrence_rule};
use jmap_proto::calendars::{CalendarEvent, RecurrenceRule};
use serde_json::json;

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

fn without(ics: &str, prefix: &str) -> bool {
    !ics.split("\r\n").any(|line| line.starts_with(prefix))
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
fn a_date_only_dtstart_is_read_as_midnight() {
    // Evolution writes VALUE=DATE for an all-day event. showWithoutTime is not
    // modeled yet, but dropping the start entirely would leave an event with
    // no time at all.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:E7\r\n",
        "DTSTART;VALUE=DATE:20260115\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = ical_to_event(ics).expect("parse");
    assert_eq!(event.start.as_deref(), Some("2026-01-15T00:00:00"));
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
    let ics = event_to_ical(&fixture_event());
    assert_eq!(
        line(&ics, "RRULE:"),
        "RRULE:FREQ=WEEKLY;INTERVAL=2;COUNT=10"
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
    // byDay & friends ride in `extra` and do not survive the trip through
    // iCalendar, so the save path must not patch recurrenceRules for them.
    let mut rule = RecurrenceRule::new("weekly");
    rule.extra
        .insert("byDay".to_owned(), json!([{"day": "mo"}]));
    assert!(!maps_recurrence_rule(&rule));
    assert!(maps_recurrence_rule(&RecurrenceRule::new("weekly")));
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
