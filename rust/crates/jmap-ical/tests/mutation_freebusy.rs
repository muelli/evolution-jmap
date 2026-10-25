// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use jmap_ical::error::ICalError;
use jmap_ical::freebusy::{busy_periods_to_vfreebusy, free_busy_type, instant, mailto};
use jmap_proto::principals::BusyPeriod;
use jmap_proto::state::UtcDate;

#[test]
fn instant_truncates_fractional_seconds_with_z() {
    let date = UtcDate::new("2026-08-19T09:00:00.123456Z");
    assert_eq!(instant(&date), Some("20260819T090000Z".to_string()));
}

#[test]
fn instant_rejects_empty_fraction_or_missing_digits() {
    assert_eq!(instant(&UtcDate::new("2026-08-19T09:00:00.")), None);
    assert_eq!(instant(&UtcDate::new("2026-08-19T09:00:00.Z")), None);
}

#[test]
fn instant_rejects_non_digit_fractional_characters() {
    assert_eq!(instant(&UtcDate::new("2026-08-19T09:00:00.12aZ")), None);
    assert_eq!(instant(&UtcDate::new("2026-08-19T09:00:00.12-Z")), None);
}

#[test]
fn instant_rejects_fraction_without_z_designator() {
    assert_eq!(instant(&UtcDate::new("2026-08-19T09:00:00.123")), None);
    assert_eq!(
        instant(&UtcDate::new("2026-08-19T09:00:00.123+00:00")),
        None
    );
}

#[test]
fn mailto_handles_schemes_case_insensitively_and_bare_addresses() {
    assert_eq!(mailto("bob@example.com"), "mailto:bob@example.com");
    assert_eq!(mailto("mailto:bob@example.com"), "mailto:bob@example.com");
    assert_eq!(mailto("MAILTO:bob@example.com"), "mailto:bob@example.com");
    assert_eq!(mailto("Mailto:bob@example.com"), "mailto:bob@example.com");
    assert_eq!(mailto(""), "mailto:");
}

#[test]
fn free_busy_type_maps_known_statuses_and_defaults_unknowns_to_busy() {
    assert_eq!(free_busy_type("tentative"), "BUSY-TENTATIVE");
    assert_eq!(free_busy_type("unavailable"), "BUSY-UNAVAILABLE");
    assert_eq!(free_busy_type("confirmed"), "BUSY");
    assert_eq!(free_busy_type("other"), "BUSY");
    assert_eq!(free_busy_type(""), "BUSY");
}

#[test]
fn busy_periods_to_vfreebusy_renders_component_with_periods() {
    let window_start = UtcDate::new("2026-08-19T08:00:00Z");
    let window_end = UtcDate::new("2026-08-19T18:00:00Z");
    let periods = vec![
        BusyPeriod::new(
            UtcDate::new("2026-08-19T09:00:00Z"),
            UtcDate::new("2026-08-19T10:00:00Z"),
            "tentative",
        ),
        BusyPeriod::new(
            UtcDate::new("2026-08-19T11:00:00Z"),
            UtcDate::new("2026-08-19T12:00:00Z"),
            "unavailable",
        ),
    ];

    let ics = busy_periods_to_vfreebusy("user@example.com", &window_start, &window_end, &periods)
        .expect("renders vfreebusy");

    assert!(ics.contains("BEGIN:VFREEBUSY\r\n"));
    assert!(ics.contains("DTSTART:20260819T080000Z\r\n"));
    assert!(ics.contains("DTEND:20260819T180000Z\r\n"));
    assert!(ics.contains("ATTENDEE:mailto:user@example.com\r\n"));
    assert!(ics.contains("FREEBUSY;FBTYPE=BUSY-TENTATIVE:20260819T090000Z/20260819T100000Z\r\n"));
    assert!(ics.contains("FREEBUSY;FBTYPE=BUSY-UNAVAILABLE:20260819T110000Z/20260819T120000Z\r\n"));
    assert!(ics.contains("END:VFREEBUSY\r\n"));
}

#[test]
fn busy_periods_to_vfreebusy_returns_none_on_invalid_period() {
    let window_start = UtcDate::new("2026-08-19T08:00:00Z");
    let window_end = UtcDate::new("2026-08-19T18:00:00Z");
    let periods = vec![BusyPeriod::new(
        UtcDate::new("invalid-start"),
        UtcDate::new("2026-08-19T10:00:00Z"),
        "confirmed",
    )];

    assert_eq!(
        busy_periods_to_vfreebusy("user@example.com", &window_start, &window_end, &periods),
        None
    );
}

#[test]
fn ical_error_display_and_error_trait_impl() {
    use std::error::Error;

    let err = ICalError::NotACalendar;
    assert_eq!(
        err.to_string(),
        "not an iCalendar object: missing BEGIN:VCALENDAR"
    );
    assert!(err.source().is_none());

    let err = ICalError::Unterminated("VTIMEZONE".to_string());
    assert_eq!(
        err.to_string(),
        "truncated iCalendar: missing END:VTIMEZONE"
    );

    let err = ICalError::Mismatched {
        expected: "STANDARD".to_string(),
        found: "DAYLIGHT".to_string(),
    };
    assert_eq!(
        err.to_string(),
        "END:DAYLIGHT closes nothing; END:STANDARD was due"
    );

    let err = ICalError::Trailing("GARBAGE".to_string());
    assert_eq!(err.to_string(), "content after END:VCALENDAR: GARBAGE");

    let err = ICalError::TooDeep("COMPONENT".to_string());
    assert_eq!(
        err.to_string(),
        "iCalendar components nested more than 32 deep at BEGIN:COMPONENT"
    );

    let err = ICalError::NoEvent;
    assert_eq!(err.to_string(), "iCalendar object contains no VEVENT");
}
