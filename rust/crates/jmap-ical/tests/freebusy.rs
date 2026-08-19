// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `BusyPeriod` → `VFREEBUSY`, the free/busy half of the calendar mapping.
//!
//! The acceptance suite for what Evolution's meeting scheduler is handed when
//! it asks a JMAP calendar for an attendee's free/busy. The shape is not ours
//! to invent: `ECalMetaBackend`, the CalDAV backend and the Microsoft 365
//! backend all answer `get_free_busy_sync` with a bare `VFREEBUSY` component
//! rendered to an iCalendar string, and the assertions below pin this crate to
//! the same one.

use jmap_ical::{busy_periods_to_vfreebusy, free_busy_type};
use jmap_proto::principals::BusyPeriod;
use jmap_proto::state::UtcDate;

fn period(start: &str, end: &str, status: &str) -> BusyPeriod {
    BusyPeriod {
        utc_start: UtcDate::new(start),
        utc_end: UtcDate::new(end),
        busy_status: status.to_owned(),
        event: None,
    }
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

/// Order is the server's. `Principal/getAvailability` returns the periods it
/// has already merged and split; re-sorting here would only hide a server that
/// did not.
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

/// An attendee with nothing on is not the same as an attendee we know nothing
/// about, and the difference is what a scheduler needs: a component with a
/// window and no `FREEBUSY` line states the first. `ECalMetaBackend`'s own
/// implementation emits exactly that for an empty cache.
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

/// The three backends in the EDS tree all build the `ATTENDEE` by prepending
/// `mailto:` to the string EDS handed them, so a bare address is what the
/// vfunc's `users` list holds. Tolerating the prefix anyway costs one check
/// and avoids `mailto:mailto:`.
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

/// `getAvailability` answers with busy periods only — there is no "free" in
/// its vocabulary — so a status this mapping does not know is still a period
/// the attendee is unavailable for. Reporting it as anything but busy would
/// let a later draft's new value book a meeting over someone's calendar.
#[test]
fn an_unknown_busy_status_is_still_busy() {
    assert_eq!(free_busy_type("something-draft-28-invented"), "BUSY");
    assert_eq!(free_busy_type(""), "BUSY");
}

/// RFC 3339 allows a fractional second and RFC 5545's DATE-TIME does not, so
/// the fraction is dropped rather than the period.
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

/// The safety rule of this mapping, and the reason it answers with an
/// `Option`: a period that cannot be read is **not** dropped. Dropping it
/// would render the attendee free for a time they are busy, and a scheduler
/// acts on that — it books the slot. Refusing the whole component instead
/// leaves the attendee's row blank, which is the true statement.
#[test]
fn a_period_that_cannot_be_read_refuses_the_whole_component() {
    for (start, end) in [
        ("not-a-date", "2026-08-19T10:00:00Z"),
        ("2026-08-19T09:00:00Z", "not-a-date"),
        // No `Z`: a JMAP UTCDate is always UTC, and a local time here would be
        // a different instant with no way to tell which.
        ("2026-08-19T09:00:00", "2026-08-19T10:00:00Z"),
        // Digits in the right places are not an instant.
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

/// A component this crate emits has to stay one component however hostile the
/// attendee string is: a raw newline in it, written through, would end the
/// `ATTENDEE` line and let the rest be read as properties of its own. The
/// protection is the escaping the writer already applies to a value — this
/// pins it, because nothing else in the free/busy path would notice it going
/// away, and `users` reaches the vfunc from outside this process.
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
