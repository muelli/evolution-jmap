// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the mapping from `BusyPeriod` to `VFREEBUSY`.
//!
//! Verifies the `VFREEBUSY` components returned to Evolution's meeting scheduler.
//! `ECalMetaBackend`, CalDAV, and Microsoft 365 backends all answer `get_free_busy_sync`
//! with a bare `VFREEBUSY` component rendered to an iCalendar string.

use jmap_ical::{busy_periods_to_vfreebusy, free_busy_type};
use jmap_proto::principals::BusyPeriod;
use jmap_proto::state::UtcDate;

fn period(start: &str, end: &str, status: &str) -> BusyPeriod {
    BusyPeriod::new(UtcDate::new(start), UtcDate::new(end), status)
}

fn render(attendee: &str, periods: &[BusyPeriod]) -> Option<String> {
    busy_periods_to_vfreebusy(
        attendee,
        &UtcDate::new("2026-08-19T08:00:00Z"),
        &UtcDate::new("2026-08-19T18:00:00Z"),
        periods,
    )
}

fn lines(ics: &str) -> Vec<String> {
    ics.split("\r\n")
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_busy_period_becomes_a_freebusy_line_inside_the_requested_window() {
    let ics = render(
        "bob@example.com",
        &[period(
            "2026-08-19T09:00:00Z",
            "2026-08-19T10:30:00Z",
            "confirmed",
        )],
    )
    .expect("renders");

    assert_eq!(
        lines(&ics),
        vec![
            "BEGIN:VFREEBUSY",
            "DTSTART:20260819T080000Z",
            "DTEND:20260819T180000Z",
            "ATTENDEE:mailto:bob@example.com",
            "FREEBUSY;FBTYPE=BUSY:20260819T090000Z/20260819T103000Z",
            "END:VFREEBUSY",
        ],
    );
}

/// Preserves the order of periods returned by `Principal/getAvailability`.
#[test]
fn every_period_gets_its_own_line_in_the_order_given() {
    let ics = render(
        "bob@example.com",
        &[
            period("2026-08-19T09:00:00Z", "2026-08-19T10:00:00Z", "confirmed"),
            period("2026-08-19T14:00:00Z", "2026-08-19T15:00:00Z", "tentative"),
        ],
    )
    .expect("renders");

    let freebusy: Vec<String> = lines(&ics)
        .into_iter()
        .filter(|line| line.starts_with("FREEBUSY"))
        .collect();
    assert_eq!(
        freebusy,
        vec![
            "FREEBUSY;FBTYPE=BUSY:20260819T090000Z/20260819T100000Z",
            "FREEBUSY;FBTYPE=BUSY-TENTATIVE:20260819T140000Z/20260819T150000Z",
        ],
    );
}

/// An attendee with no busy periods still receives a `VFREEBUSY` component.
///
/// An empty list indicates no busy time in the query window.
#[test]
fn an_attendee_with_no_busy_periods_still_gets_a_component() {
    let ics = render("bob@example.com", &[]).expect("renders");

    assert_eq!(
        lines(&ics),
        vec![
            "BEGIN:VFREEBUSY",
            "DTSTART:20260819T080000Z",
            "DTEND:20260819T180000Z",
            "ATTENDEE:mailto:bob@example.com",
            "END:VFREEBUSY",
        ],
    );
}

/// Ensures addresses that already contain `mailto:` are not prefixed again.
#[test]
fn an_attendee_that_already_names_a_scheme_is_not_prefixed_twice() {
    for attendee in ["mailto:bob@example.com", "MAILTO:bob@example.com"] {
        let ics = render(attendee, &[]).expect("renders");
        assert!(
            ics.contains("\r\nATTENDEE:mailto:bob@example.com\r\n"),
            "{attendee} rendered as {ics}",
        );
    }
}

#[test]
fn the_draft_busy_statuses_map_onto_rfc_5545_fbtypes() {
    assert_eq!(free_busy_type("confirmed"), "BUSY");
    assert_eq!(free_busy_type("tentative"), "BUSY-TENTATIVE");
    assert_eq!(free_busy_type("unavailable"), "BUSY-UNAVAILABLE");
}

/// Unknown busy statuses are treated as busy.
#[test]
fn an_unknown_busy_status_is_still_busy() {
    assert_eq!(free_busy_type("something-draft-28-invented"), "BUSY");
    assert_eq!(free_busy_type(""), "BUSY");
}

/// Truncates fractional seconds to match RFC 5545 DATE-TIME format.
#[test]
fn a_fractional_second_is_truncated_not_rejected() {
    let ics = render(
        "bob@example.com",
        &[period(
            "2026-08-19T09:00:00.512Z",
            "2026-08-19T10:00:00.999Z",
            "confirmed",
        )],
    )
    .expect("renders");

    assert!(
        ics.contains("FREEBUSY;FBTYPE=BUSY:20260819T090000Z/20260819T100000Z"),
        "{ics}",
    );
}

/// Unreadable periods cause the entire component to be rejected.
#[test]
fn a_period_that_cannot_be_read_refuses_the_whole_component() {
    for (start, end) in [
        ("not-a-date", "2026-08-19T10:00:00Z"),
        ("2026-08-19T09:00:00Z", "not-a-date"),
        // UTCDate requires a trailing Z.
        ("2026-08-19T09:00:00", "2026-08-19T10:00:00Z"),
        // Out-of-range dates are rejected.
        ("2026-13-19T09:00:00Z", "2026-08-19T10:00:00Z"),
        ("2026-08-19T25:00:00Z", "2026-08-19T10:00:00Z"),
    ] {
        assert_eq!(
            render("bob@example.com", &[period(start, end, "confirmed")]),
            None,
            "{start}/{end} should have been refused",
        );
    }
}

#[test]
fn a_window_that_cannot_be_read_refuses_the_whole_component() {
    assert_eq!(
        busy_periods_to_vfreebusy(
            "bob@example.com",
            &UtcDate::new("whenever"),
            &UtcDate::new("2026-08-19T18:00:00Z"),
            &[],
        ),
        None,
    );
    assert_eq!(
        busy_periods_to_vfreebusy(
            "bob@example.com",
            &UtcDate::new("2026-08-19T08:00:00Z"),
            &UtcDate::new("whenever"),
            &[],
        ),
        None,
    );
}

/// Ensures attendee strings cannot inject additional iCalendar properties.
#[test]
fn an_attendee_cannot_inject_a_property_of_its_own() {
    let ics = render("bob@example.com\r\nSUMMARY:injected", &[]).expect("renders");

    let lines = lines(&ics);
    assert_eq!(
        lines.len(),
        5,
        "the newline was written through, not escaped: {ics}",
    );
    assert!(
        lines.iter().all(|line| !line.starts_with("SUMMARY:")),
        "{ics}",
    );
}
