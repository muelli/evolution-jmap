/*
 * SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

use jmap_proto::calendars::CalendarEvent;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn fold_overlong_lines_exact_boundary_and_continuation_budget() {
    // RFC 5545 Section 3.1: lines should not exceed 75 octets.
    // Lines are split by CRLF followed by a space.

    // Line 1 is 80 bytes and needs folding.
    // Line 2 is 75 bytes and must not fold.
    let line1 = format!("SUMMARY:{}", "A".repeat(72));
    let line2 = format!("DESCRIPTION:{}", "B".repeat(63));
    let input = format!("{line1}\r\n{line2}");
    let folded = jmap_ical::event::fold_overlong_lines(input);
    let lines: Vec<&str> = folded.split("\r\n").collect();

    // Line 1 folds into 75 bytes followed by a 6-byte continuation line.
    assert_eq!(lines[0].len(), 75);
    assert_eq!(lines[0], &line1[..75]);
    assert_eq!(lines[1], format!(" {}", &line1[75..]));
    assert_eq!(lines[1].len(), 6);

    // Line 2 remains one line of 75 bytes.
    assert_eq!(lines[2], line2);
    assert_eq!(lines[2].len(), 75);
    assert_eq!(lines.len(), 3);

    // Multi-continuation line fold with 200 octets.
    // Line 1 has 75 octets, line 2 has 75 octets, line 3 has remaining octets.
    let long_line = format!("NOTE:{}", "C".repeat(195));
    let folded_long = jmap_ical::event::fold_overlong_lines(long_line);
    let long_lines: Vec<&str> = folded_long.split("\r\n").collect();

    assert_eq!(long_lines[0].len(), 75);
    assert_eq!(long_lines[1].len(), 75);
    assert!(long_lines[1].starts_with(' '));
    assert_eq!(long_lines[2].len(), 1 + (200 - 75 - 74));
    assert!(long_lines[2].starts_with(' '));
    assert_eq!(long_lines.len(), 3);
}

#[test]
fn modified_instance_inherits_use_default_alerts_suppressing_alarms() {
    // RFC 8984 Section 4.5.1: `useDefaultAlerts` on the series suppresses instance `VALARM`.
    let mut alerts = BTreeMap::new();
    alerts.insert(
        "alert1".to_string(),
        json!({
            "@type": "Alert",
            "trigger": {
                "@type": "OffsetTrigger",
                "offset": "-PT15M",
                "relativeTo": "start"
            },
            "action": "display"
        }),
    );

    let mut overrides = BTreeMap::new();
    overrides.insert(
        "2026-06-02T10:00:00".to_string(),
        json!({
            "title": "Modified Instance"
        }),
    );

    let event = {
        let mut event_out = CalendarEvent::default();
        event_out.id = Some("ev1".into());
        event_out.uid = Some("uid1".into());
        event_out.title = Some("Series".into());
        event_out.start = Some("2026-06-01T10:00:00".into());
        event_out.duration = Some("PT1H".into());
        event_out.use_default_alerts = Some(true);
        event_out.alerts = Some(alerts);
        event_out.recurrence_overrides = Some(overrides);
        event_out
    };

    let ics = jmap_ical::event::event_to_ical(&event);
    assert!(
        !ics.contains("BEGIN:VALARM"),
        "VALARM must be suppressed when use_default_alerts is true:\n{ics}"
    );
}

#[test]
fn unfold_folded_component_structure_delimiters() {
    // RFC 5545 Section 3.1: content lines may be folded across delimiters.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVE\r\n",
        " NT\r\n",
        "UID:folded-end\r\n",
        "DTSTART:20260101T100000\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event = jmap_ical::event::ical_to_event(ics).expect("parse with folded BEGIN line");
    assert_eq!(event.id.as_ref().map(|id| id.as_str()), Some("folded-end"));
}

#[test]
fn read_definition_refuses_vtimezone_with_no_observances() {
    // RFC 5545 Section 3.6.5: `VTIMEZONE` requires `TZID` and at least one observance.
    let ics = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:/CustomEmptyZone\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:empty-zone-test\r\n",
        "DTSTART;TZID=/CustomEmptyZone:20260601T100000\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let calendar = jmap_ical::event::parse_ical(ics).expect("parse");
    let vtimezone = calendar
        .components
        .iter()
        .find(|c| c.component_type.as_str().eq_ignore_ascii_case("VTIMEZONE"))
        .expect("vtimezone");
    assert_eq!(
        jmap_ical::event::read_definition(vtimezone, &calendar.components),
        None,
        "read_definition must return None when VTIMEZONE has no observances"
    );
}

#[test]
fn read_vevent_resolves_vtimezone_by_location_name_for_until_offset() {
    // RFC 5545 Section 3.6.5: resolve `VTIMEZONE` by `X-LIC-LOCATION` for `UNTIL` adjustment.
    let ics_berlin = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:/CustomZoneBerlin\r\n",
        "X-LIC-LOCATION:Europe/Berlin\r\n",
        "BEGIN:DAYLIGHT\r\n",
        "TZOFFSETFROM:+0100\r\n",
        "TZOFFSETTO:+0200\r\n",
        "TZNAME:CEST\r\n",
        "DTSTART:19700329T020000\r\n",
        "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3\r\n",
        "END:DAYLIGHT\r\n",
        "BEGIN:STANDARD\r\n",
        "TZOFFSETFROM:+0200\r\n",
        "TZOFFSETTO:+0100\r\n",
        "TZNAME:CET\r\n",
        "DTSTART:19701025T030000\r\n",
        "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n",
        "END:STANDARD\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:until-offset-test-1\r\n",
        "DTSTART;TZID=/freeassociation.sourceforge.net/Europe/Berlin:20260115T130000\r\n",
        "RRULE:FREQ=WEEKLY;UNTIL=20260331T120000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event1 = jmap_ical::event::ical_to_event(ics_berlin).expect("parse berlin");
    let rule1 = event1.recurrence_rule.as_ref().expect("rule exists");
    assert_eq!(
        rule1.until.as_deref(),
        Some("2026-03-31T14:00:00"),
        "UNTIL must be adjusted by observance offset for alias zone"
    );

    // Match `DTSTART` by property `TZID` with multiple timezones present.
    let ics_multi = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:/CustomZoneTokyo\r\n",
        "X-LIC-LOCATION:Asia/Tokyo\r\n",
        "BEGIN:STANDARD\r\n",
        "TZOFFSETFROM:+0900\r\n",
        "TZOFFSETTO:+0900\r\n",
        "TZNAME:JST\r\n",
        "DTSTART:19700101T000000\r\n",
        "END:STANDARD\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:/CustomZoneBerlin\r\n",
        "X-LIC-LOCATION:Europe/Berlin\r\n",
        "BEGIN:DAYLIGHT\r\n",
        "TZOFFSETFROM:+0100\r\n",
        "TZOFFSETTO:+0200\r\n",
        "TZNAME:CEST\r\n",
        "DTSTART:19700329T020000\r\n",
        "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3\r\n",
        "END:DAYLIGHT\r\n",
        "BEGIN:STANDARD\r\n",
        "TZOFFSETFROM:+0200\r\n",
        "TZOFFSETTO:+0100\r\n",
        "TZNAME:CET\r\n",
        "DTSTART:19701025T030000\r\n",
        "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n",
        "END:STANDARD\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:until-offset-test-2\r\n",
        "DTSTART;TZID=Europe/Berlin:20260115T130000Z\r\n",
        "RRULE:FREQ=WEEKLY;UNTIL=20260331T120000Z\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let event2 = jmap_ical::event::ical_to_event(ics_multi).expect("parse multi");
    let rule2 = event2.recurrence_rule.as_ref().expect("rule exists");
    assert_eq!(
        rule2.until.as_deref(),
        Some("2026-03-31T14:00:00"),
        "UNTIL must match Europe/Berlin via tzid when time_zone is UTC"
    );
}

#[test]
fn date_time_digits_rejects_overlong_date_components() {
    // RFC 5545 Section 3.3.4: dates require exactly 8 digits (`YYYYMMDD`).
    assert_eq!(
        jmap_ical::event::date_time_digits("202601015T120000"),
        None,
        "9-digit date must be rejected"
    );
    assert_eq!(
        jmap_ical::event::to_local_date_time("202601015T120000"),
        None,
        "9-digit date must not convert to local date-time"
    );
}
