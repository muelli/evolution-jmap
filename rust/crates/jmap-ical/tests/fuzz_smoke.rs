// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Smoke and regression tests for jmap-ical roundtrip fuzzer.
//!
//! Asserts that parsing hostile or malformed iCalendar streams never panics,
//! and verifies multi-stage fixed point stability (docs/ICAL-MAPPING.md Section 6).

use jmap_ical::{event_to_ical, ical_to_event};

#[test]
fn hostile_ical_bytes_never_panic() {
    let hostile_inputs: &[&str] = &[
        "",
        "BEGIN:VCALENDAR",
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n",
        "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n",
        "NOT ICAL DATA AT ALL",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nDTSTART:INVALID\r\nEND:VEVENT\r\nEND:VCALENDAR",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nRRULE:FREQ=SECONDLY;COUNT=-5\r\nEND:VEVENT\r\nEND:VCALENDAR",
    ];

    for input in hostile_inputs {
        if let Ok(event1) = ical_to_event(input) {
            let ical2 = event_to_ical(&event1);
            if let Ok(event2) = ical_to_event(&ical2) {
                let ical3 = event_to_ical(&event2);
                assert_eq!(ical2, ical3);
            }
        }
    }
}

#[test]
fn fixture_seeds_reach_fixed_point_stability() {
    let fixtures: &[&str] = &[
        include_str!("fixtures/apple_calendar_export.ics"),
        include_str!("fixtures/evolution_calendar_export.ics"),
        include_str!("fixtures/google_calendar_export.ics"),
        include_str!("fixtures/nextcloud_calendar_export.ics"),
        include_str!("fixtures/thunderbird_calendar_export.ics"),
    ];

    for ics in fixtures {
        let event1 = ical_to_event(ics).expect("Pass 1 import succeeds on fixture");
        let ical2 = event_to_ical(&event1);
        let event2 = ical_to_event(&ical2).expect("Pass 2 re-import succeeds");
        let ical3 = event_to_ical(&event2);
        let event3 = ical_to_event(&ical3).expect("Pass 3 re-import succeeds");

        assert_eq!(
            ical2, ical3,
            "Export_2 and Export_3 must be byte-identical fixed point"
        );
        assert_eq!(
            event2, event3,
            "Event_2 and Event_3 must be identical fixed point"
        );
    }
}

#[test]
fn duration_with_leading_zeros_reaches_fixed_point_stability() {
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:test-leading-zero\r\nDTSTART:20261012T093000Z\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER;VALUE=X-CUSTOM:-PT07M\r\nDESCRIPTION:Reminder\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let event1 = ical_to_event(ics).expect("Pass 1 import succeeds");
    let ical2 = event_to_ical(&event1);
    let event2 = ical_to_event(&ical2).expect("Pass 2 re-import succeeds");
    let ical3 = event_to_ical(&event2);
    let event3 = ical_to_event(&ical3).expect("Pass 3 re-import succeeds");

    assert_eq!(
        ical2, ical3,
        "Export_2 and Export_3 must be byte-identical fixed point"
    );
    assert_eq!(
        event2, event3,
        "Event_2 and Event_3 must be identical fixed point"
    );
}

#[test]
fn unmappable_rrule_frequency_reaches_fixed_point_stability() {
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:test-rrule-freq\r\nDTSTART:20261012T093000Z\r\nRRULE:FREQ=FICE\\\\,TTENDEE\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let event1 = ical_to_event(ics).expect("Pass 1 import succeeds");
    let ical2 = event_to_ical(&event1);
    let event2 = ical_to_event(&ical2).expect("Pass 2 re-import succeeds");
    let ical3 = event_to_ical(&event2);
    let event3 = ical_to_event(&ical3).expect("Pass 3 re-import succeeds");

    assert_eq!(
        ical2, ical3,
        "Export_2 and Export_3 must be byte-identical fixed point"
    );
    assert_eq!(
        event2, event3,
        "Event_2 and Event_3 must be identical fixed point"
    );
}

#[test]
fn zero_duration_trigger_with_custom_param_reaches_fixed_point_stability() {
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:test-zero-dur\r\nDTSTART:20261012T093000Z\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER;VALUE=X-CUSTOM:-PT0H\r\nDESCRIPTION:Reminder\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let event1 = ical_to_event(ics).expect("Pass 1 import succeeds");
    let ical2 = event_to_ical(&event1);
    let event2 = ical_to_event(&ical2).expect("Pass 2 re-import succeeds");
    let ical3 = event_to_ical(&event2);
    let event3 = ical_to_event(&ical3).expect("Pass 3 re-import succeeds");

    assert_eq!(
        ical2, ical3,
        "Export_2 and Export_3 must be byte-identical fixed point"
    );
    assert_eq!(
        event2, event3,
        "Event_2 and Event_3 must be identical fixed point"
    );
}
