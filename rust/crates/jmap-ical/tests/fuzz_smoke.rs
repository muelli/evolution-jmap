// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Smoke and regression tests for roundtrip fuzzer.
//!
//! Asserts that parsing hostile streams never panics.
//! Verifies multi-stage fixed point stability (docs/ICAL-MAPPING.md Section 6).

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

#[test]
fn data_uri_in_conference_or_attach_reaches_fixed_point_stability() {
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:test-data-uri-conference\r\nDTSTART:20261110T000000Z\r\nCONFERENCE;VALUE=-CALDAV-CTAG:data:,ctag-42\r\nATTACH;VALUE=-CUSTOM:data:text/plain,hello\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
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
fn raw_backslashes_in_uri_reaches_fixed_point_stability() {
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:test-backslash-uri\r\nDTSTART:20261110T000000Z\r\nORGANIZER;CN=\"Release Lead\":mailto:release-lead@gnome\\\\\\\\.example\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
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
fn control_characters_in_uri_reaches_fixed_point_stability() {
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:test-control-char-uri\r\nDTSTART:20261110T000000Z\r\nORGANIZER;CN=\"Prof. Alicia Vaenc\":mailto:alicia.v\x17ance@example.com\r\nATTENDEE;CN=\"Prof. Alicia Vance\":mailto:alicia.vance@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
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
fn categories_with_tabs_or_whitespace_reaches_fixed_point_stability() {
    let bytes: &[u8] = &[
        66, 69, 71, 73, 78, 58, 86, 67, 65, 76, 69, 78, 68, 65, 82, 10, 80, 82, 79, 68, 73, 68, 58,
        45, 47, 47, 0, 0, 0, 0, 0, 63, 97, 32, 67, 97, 108, 101, 110, 100, 78, 69, 10, 66, 69, 71,
        73, 78, 58, 86, 69, 86, 69, 78, 84, 10, 85, 73, 68, 58, 100, 67, 76, 65, 83, 58, 48, 10,
        67, 65, 84, 69, 71, 79, 82, 73, 69, 83, 58, 58, 107, 105, 110, 100, 9, 9, 9, 9, 9, 9, 9, 9,
        9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
        9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 65, 82, 77,
        10, 88, 1, 0, 0, 32, 78, 69, 82, 65, 84, 73, 79, 78, 58, 53, 10, 69, 78, 68, 58, 86, 69,
        86, 69, 78, 84, 10, 69, 78, 68, 58, 86, 67, 65, 76, 69, 78, 68, 65, 82, 10,
    ];
    let text = std::str::from_utf8(bytes).expect("Valid UTF-8");
    let event1 = ical_to_event(text).expect("Pass 1 import succeeds");
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
fn trigger_duration_without_time_prefix_reaches_fixed_point_stability() {
    let ical = "BEGIN:VCALENDAR\n\
VERSION:2.0\n\
PRODID:-//evolution-jmap//Differential Harness Custom Timezone Fixture//EN\n\
CALSCALE:GREGORIAN\n\
BEGIN:VEVENT\n\
UID:custom-tz-sync-2026-c001\n\
DTSTAMP:20260928T120000Z\n\
DTSTART:20261015T140000Z\n\
SUMMARY:Technical Sync\n\
BEGIN:VALARM\n\
ACTION:DISPLAY\n\
TRIGGER;VALUE=DURATIONT:-P15M\n\
DESCRIPTION:Reminder\n\
END:VALARM\n\
END:VEVENT\n\
END:VCALENDAR\n";
    let event1 = ical_to_event(ical).expect("Pass 1 import succeeds");
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
fn participant_cn_with_trailing_tab_reaches_fixed_point_stability() {
    let ical = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//evolution-jmap//Differential Harness DST Spanning Fixture//EN\r\n\
BEGIN:VEVENT\r\n\
UID:dst-transition-spanning-2026-c001\r\n\
DTSTART:20261025T010000\r\n\
SUMMARY:Overnight Database\r\n\
ORGANIZER;CN=\"Art       \t\":mailto:arthur.dent@earth.example\r\n\
ATTENDEE;CN=Art\t;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT:mailto:arthur.dent@earth.example\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let event1 = ical_to_event(ical).expect("Pass 1 import succeeds");
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
fn tzid_with_tabs_reaches_fixed_point_stability() {
    let ical = format!(
        "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//evolution-jmap//Differential Harness Fixture//EN\r\n\
BEGIN:VEVENT\r\n\
UID:tzid-tabs-c001\r\n\
DTSTART;TZID=America/N{}ew_York:20260915T100000\r\n\
SUMMARY:TZID with tabs\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n",
        "\t".repeat(74)
    );
    let event1 = ical_to_event(&ical).expect("Pass 1 import succeeds");
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
fn fuzz_ical_roundtrip_extended_helpers_never_panic() {
    let test_inputs: &[&str] = &[
        include_str!("fixtures/apple_calendar_export.ics"),
        include_str!("fixtures/evolution_calendar_export.ics"),
        "NOT ICAL DATA AT ALL",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:sample-tz\r\nDTSTART;TZID=/custom/zone:20261108T100000\r\nRRULE:FREQ=DAILY;UNTIL=20261115T100000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    ];

    for text in test_inputs {
        let _ = jmap_ical::unique_tzid_to_iana(text);
        let _ = jmap_ical::windows_time_zone_to_iana(text);
        let _ = jmap_ical::resolve_canonical_time_zone(text);
        let _ = jmap_ical::names_time_zone(text);
        let _ = jmap_ical::free_busy_type(text);

        if let Ok(event1) = ical_to_event(text) {
            let _ = jmap_ical::maps_alerts(&event1);
            let _ = jmap_ical::maps_time_zone(&event1);

            if let Some(ref tzid) = event1.time_zone {
                let _ = jmap_ical::unique_tzid_to_iana(tzid);
                let _ = jmap_ical::windows_time_zone_to_iana(tzid);
                let _ = jmap_ical::resolve_canonical_time_zone(tzid);
                let _ = jmap_ical::time_zone_definition(&event1, tzid);
                let _ = jmap_ical::names_time_zone(tzid);
                let _ = jmap_ical::defines_time_zone(&event1, tzid);
            }

            if let Some(ref rule) = event1.recurrence_rule {
                let _ = jmap_ical::unstateable_until(rule);
                let _ = jmap_ical::maps_recurrence_rule(rule);
            }

            if let Some(ref locations) = event1.locations {
                let _ = jmap_ical::maps_locations(locations);
            }

            if let Some(ref vlocations) = event1.virtual_locations {
                let _ = jmap_ical::maps_virtual_locations(vlocations);
            }

            if let Some(ref overrides) = event1.recurrence_overrides {
                for (recurrence_id, patch) in overrides {
                    let _ = jmap_ical::maps_recurrence_override(&event1, recurrence_id, patch);
                    let _ = jmap_ical::sends_recurrence_override(&event1, recurrence_id, patch);
                }
            }

            let mut pruned = event1.clone();
            jmap_ical::prune_time_zones(&mut pruned);
        }
    }
}

#[test]
fn fuzz_ical_import_extended_helpers_never_panic() {
    let test_inputs: &[&str] = &[
        include_str!("fixtures/apple_calendar_export.ics"),
        include_str!("fixtures/evolution_calendar_export.ics"),
        "NOT ICAL DATA AT ALL",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:sample-tz\r\nDTSTART;TZID=/custom/zone:20261108T100000\r\nRRULE:FREQ=DAILY;UNTIL=20261115T100000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    ];

    for text in test_inputs {
        let _ = jmap_ical::unique_tzid_to_iana(text);
        let _ = jmap_ical::windows_time_zone_to_iana(text);
        let _ = jmap_ical::resolve_canonical_time_zone(text);
        let _ = jmap_ical::names_time_zone(text);
        let _ = jmap_ical::free_busy_type(text);

        let date = jmap_proto::state::UtcDate::new(*text);
        let period = jmap_proto::principals::BusyPeriod::new(date.clone(), date.clone(), *text);
        let _ = jmap_ical::busy_periods_to_vfreebusy(text, &date, &date, &[period]);

        if let Ok(event) = ical_to_event(text) {
            let ical2 = event_to_ical(&event);
            let _ = ical_to_event(&ical2);

            let _ = jmap_ical::scheduling_ical(&event, "REQUEST", None);
            let _ = jmap_ical::scheduling_ical(&event, "REPLY", None);
            let _ = jmap_ical::scheduling_ical(&event, "CANCEL", None);
            let _ = jmap_ical::scheduling_ical(&event, text, None);
            if let Some(ref overrides) = event.recurrence_overrides {
                for recurrence_id in overrides.keys() {
                    let _ = jmap_ical::scheduling_ical(&event, "REQUEST", Some(recurrence_id));
                    let _ = jmap_ical::scheduling_ical(&event, "REPLY", Some(recurrence_id));
                    let _ = jmap_ical::scheduling_ical(&event, "CANCEL", Some(recurrence_id));
                }
            }

            let _ = jmap_ical::maps_alerts(&event);
            let _ = jmap_ical::maps_time_zone(&event);

            if let Some(ref tzid) = event.time_zone {
                let _ = jmap_ical::unique_tzid_to_iana(tzid);
                let _ = jmap_ical::windows_time_zone_to_iana(tzid);
                let _ = jmap_ical::resolve_canonical_time_zone(tzid);
                let _ = jmap_ical::time_zone_definition(&event, tzid);
                let _ = jmap_ical::names_time_zone(tzid);
                let _ = jmap_ical::defines_time_zone(&event, tzid);
            }

            if let Some(ref rule) = event.recurrence_rule {
                let _ = jmap_ical::unstateable_until(rule);
                let _ = jmap_ical::maps_recurrence_rule(rule);
            }

            if let Some(ref locations) = event.locations {
                let _ = jmap_ical::maps_locations(locations);
            }

            if let Some(ref vlocations) = event.virtual_locations {
                let _ = jmap_ical::maps_virtual_locations(vlocations);
            }

            if let Some(ref overrides) = event.recurrence_overrides {
                for (recurrence_id, patch) in overrides {
                    let _ = jmap_ical::maps_recurrence_override(&event, recurrence_id, patch);
                    let _ = jmap_ical::sends_recurrence_override(&event, recurrence_id, patch);
                }
            }

            let mut pruned = event.clone();
            jmap_ical::prune_time_zones(&mut pruned);
            let _ = event_to_ical(&pruned);
        }
    }
}
