/*
 * SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

use jmap_ical::event::parse_ical;

fn get_observances(
    cal: &calcard::icalendar::ICalendar,
) -> Vec<&calcard::icalendar::ICalendarComponent> {
    let vtz = cal
        .components
        .iter()
        .find(|c| c.component_type.as_str().eq_ignore_ascii_case("VTIMEZONE"))
        .expect("vtimezone component");
    vtz.component_ids
        .iter()
        .filter_map(|id| cal.components.get(*id as usize))
        .collect()
}

#[test]
fn vtimezone_rdate_subtracts_tzoffsetfrom_for_utc_onset() {
    // RFC 5545 §3.6.5 and §3.8.5.2: In a VTIMEZONE observance with a non-zero
    // TZOFFSETFROM (+0200 = 7200s), discrete RDATE transition dates represent
    // local time in the prior offset and must subtract `from` to convert to UTC.
    let ics = "\
BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Test//EN\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:Custom/RdateFromOffset\r\n\
BEGIN:STANDARD\r\n\
DTSTART:20200101T000000\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0100\r\n\
END:STANDARD\r\n\
BEGIN:DAYLIGHT\r\n\
DTSTART:20300101T000000\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0200\r\n\
RDATE:20261025T030000\r\n\
END:DAYLIGHT\r\n\
END:VTIMEZONE\r\n\
END:VCALENDAR\r\n";

    let cal = parse_ical(ics).expect("parse ics");
    let observances = get_observances(&cal);

    // Discrete RDATE 20261025T030000 with TZOFFSETFROM:+0100 (3600s) onsets at:
    // 2026-10-25T03:00:00 - 01:00:00 = 2026-10-25T02:00:00 UTC.
    // At 2026-10-25T01:59:59 (before onset), in force offset is STANDARD (+0100 = 3600).
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-10-25T01:59:59"),
        Some(3600)
    );
    // At 2026-10-25T02:00:00 (at/after onset), in force offset is DAYLIGHT (+0200 = 7200).
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-10-25T02:00:00"),
        Some(7200)
    );
}

#[test]
fn vtimezone_rrule_local_until_subtracts_tzoffsetfrom_for_utc_onset() {
    // RFC 5545 §3.6.5: An UNTIL parameter in a VTIMEZONE observance RRULE stated
    // as local time (without a trailing 'Z') is dated in the local time of the zone
    // and must subtract TZOFFSETFROM to convert to a UTC bound.
    let ics = "\
BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Test//EN\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:Custom/LocalUntil\r\n\
BEGIN:STANDARD\r\n\
DTSTART:20200101T000000\r\n\
TZOFFSETFROM:+0200\r\n\
TZOFFSETTO:+0200\r\n\
END:STANDARD\r\n\
BEGIN:DAYLIGHT\r\n\
DTSTART:20200329T020000\r\n\
TZOFFSETFROM:+0200\r\n\
TZOFFSETTO:+0300\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3;UNTIL=20260329T020000\r\n\
END:DAYLIGHT\r\n\
BEGIN:STANDARD\r\n\
DTSTART:20201025T030000\r\n\
TZOFFSETFROM:+0300\r\n\
TZOFFSETTO:+0200\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
END:VCALENDAR\r\n";

    let cal = parse_ical(ics).expect("parse ics");
    let observances = get_observances(&cal);

    // On 2026-03-29, DTSTART is at local 02:00:00.
    // With TZOFFSETFROM:+0200 (7200s), UTC onset is 02:00:00 - 02:00:00 = 00:00:00 UTC.
    // UNTIL is 20260329T020000 local, which converts to 00:00:00 UTC with (- from).
    // The onset 00:00:00 UTC is <= UNTIL 00:00:00 UTC, so the 2026 occurrence is included,
    // shifting offset to TZOFFSETTO:+0300 (10800s).
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-03-29T00:00:00"),
        Some(10800)
    );

    // When UNTIL is 20260329T015959 local, UTC bound is 23:59:59 (prior day).
    // Onset 00:00:00 UTC > UNTIL bound, so 2026 transition is excluded.
    let ics_expired = ics.replace("UNTIL=20260329T020000", "UNTIL=20260329T015959");
    let cal_exp = parse_ical(&ics_expired).expect("parse ics");
    let obs_exp = get_observances(&cal_exp);
    // At 2026-03-29T00:00:00, 2026 transition did not happen; falls back to winter standard (+0200 = 7200).
    assert_eq!(
        jmap_ical::event::zone_offset_at(&obs_exp, "2026-03-29T00:00:00"),
        Some(7200)
    );
}

#[test]
fn vtimezone_rrule_count_bounds_last_transition_year() {
    // RFC 5545 §3.3.10: COUNT in a yearly recurrence rule specifies the total number
    // of occurrences, bounding the last year to `start_year + count - 1`.
    let ics = "\
BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Test//EN\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:Custom/CountBound\r\n\
BEGIN:STANDARD\r\n\
DTSTART:20200101T000000\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0100\r\n\
END:STANDARD\r\n\
BEGIN:DAYLIGHT\r\n\
DTSTART:20200329T020000\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0200\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3;COUNT=2\r\n\
END:DAYLIGHT\r\n\
BEGIN:STANDARD\r\n\
DTSTART:20201025T030000\r\n\
TZOFFSETFROM:+0200\r\n\
TZOFFSETTO:+0100\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10;COUNT=2\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
END:VCALENDAR\r\n";

    let cal = parse_ical(ics).expect("parse ics");
    let observances = get_observances(&cal);

    // COUNT=2 from 2020 means occurrences in 2020 and 2021 only.
    // In summer 2021 (within count), DAYLIGHT (+0200 = 7200) is in force.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2021-06-15T12:00:00"),
        Some(7200)
    );
    // In autumn 2021 (October transition), STANDARD (+0100 = 3600) is in force.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2021-11-15T12:00:00"),
        Some(3600)
    );
    // In summer 2022 (after count expired), no new DAYLIGHT transition occurs in 2022.
    // The last transition in force remains the autumn 2021 STANDARD (+0100 = 3600).
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2022-06-15T12:00:00"),
        Some(3600)
    );
}

#[test]
fn vtimezone_rule_onsets_search_horizon_boundary() {
    // When a recurrence rule's transitions fall outside the 40-year search window
    // (earliest = start_year.max(last - SEARCH + 1)), onsets cannot be established back
    // to start_year and the transition is refused (returning None).
    let ics = "\
BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Test//EN\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:Custom/OldHorizon\r\n\
BEGIN:STANDARD\r\n\
DTSTART:19800101T000000\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0100\r\n\
END:STANDARD\r\n\
BEGIN:DAYLIGHT\r\n\
DTSTART:19860228T020000\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0200\r\n\
RRULE:FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30\r\n\
END:DAYLIGHT\r\n\
END:VTIMEZONE\r\n\
END:VCALENDAR\r\n";

    let cal = parse_ical(ics).expect("parse ics");
    let observances = get_observances(&cal);

    // Target year 2026 is 40 years after 1986. With SEARCH=40, earliest search year is:
    // 2026 - 40 + 1 = 1987.
    // Since earliest (1987) > start_year (1986) and February 30 never occurs, onsets is empty
    // and earliest != start_year, so rule_onsets returns None, refusing the zone.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-06-15T12:00:00"),
        None
    );
}

#[test]
fn vtimezone_restated_time_of_day_preserves_minutes_and_seconds() {
    // RFC 5545 §3.6.5: An observance transition with a DTSTART specifying non-zero
    // minutes and seconds (e.g. 02:30:45) preserves those fields when the RRULE does
    // not specify BYMINUTE or BYSECOND.
    let ics = "\
BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Test//EN\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:Custom/MinuteSecond\r\n\
BEGIN:STANDARD\r\n\
DTSTART:20200101T000000\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0100\r\n\
END:STANDARD\r\n\
BEGIN:DAYLIGHT\r\n\
DTSTART:20200329T023045\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0200\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3\r\n\
END:DAYLIGHT\r\n\
BEGIN:STANDARD\r\n\
DTSTART:20201025T030000\r\n\
TZOFFSETFROM:+0200\r\n\
TZOFFSETTO:+0100\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
END:VCALENDAR\r\n";

    let cal = parse_ical(ics).expect("parse ics");
    let observances = get_observances(&cal);

    // In 2026, transition is on March 29 at local 02:30:45.
    // UTC onset is 02:30:45 - 01:00:00 = 01:30:45 UTC.
    // 1 second before transition (01:30:44 UTC): STANDARD (+0100 = 3600)
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-03-29T01:30:44"),
        Some(3600)
    );
    // 1 second after transition (01:30:46 UTC): DAYLIGHT (+0200 = 7200)
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-03-29T01:30:46"),
        Some(7200)
    );
}

#[test]
fn vtimezone_byday_positive_ordinal_second_sunday() {
    // RFC 5545 §3.3.10: Positive ordinal on BYDAY (e.g. 2SU) evaluates to the 2nd Sunday
    // of the month, using formula `1 + (wanted - weekday).rem_euclid(7) + 7 * (nth - 1)`.
    let ics = "\
BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Test//EN\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:Custom/SecondSunday\r\n\
BEGIN:STANDARD\r\n\
DTSTART:20200101T000000\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0100\r\n\
END:STANDARD\r\n\
BEGIN:DAYLIGHT\r\n\
DTSTART:20200308T020000\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0200\r\n\
RRULE:FREQ=YEARLY;BYDAY=2SU;BYMONTH=3\r\n\
END:DAYLIGHT\r\n\
BEGIN:STANDARD\r\n\
DTSTART:20201101T020000\r\n\
TZOFFSETFROM:+0200\r\n\
TZOFFSETTO:+0100\r\n\
RRULE:FREQ=YEARLY;BYDAY=1SU;BYMONTH=11\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
END:VCALENDAR\r\n";

    let cal = parse_ical(ics).expect("parse ics");
    let observances = get_observances(&cal);

    // In March 2026:
    // March 1 is Sunday (1st Sunday).
    // March 8 is 2nd Sunday (2SU).
    // Transition onset is March 8 at 02:00:00 local (01:00:00 UTC).
    // On March 5 (between 1st Sunday and 2nd Sunday), transition has NOT occurred yet.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-03-05T12:00:00"),
        Some(3600)
    );
    // On March 9 (after 2nd Sunday), DAYLIGHT (+0200 = 7200) is in force.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-03-09T12:00:00"),
        Some(7200)
    );
}

#[test]
fn vtimezone_parts_adds_seconds_to_target_instant() {
    // Target instant second parsing must add seconds, not subtract them.
    let ics = "\
BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Test//EN\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:Custom/PartsSeconds\r\n\
BEGIN:STANDARD\r\n\
DTSTART:20200101T000000\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0100\r\n\
END:STANDARD\r\n\
BEGIN:DAYLIGHT\r\n\
DTSTART:20200329T020000\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0200\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3\r\n\
END:DAYLIGHT\r\n\
END:VTIMEZONE\r\n\
END:VCALENDAR\r\n";

    let cal = parse_ical(ics).expect("parse ics");
    let observances = get_observances(&cal);

    // In 2026, transition is on March 29 at 02:00:00 local = 01:00:00 UTC.
    // Querying with seconds at 01:00:15 UTC (15s after transition):
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-03-29T01:00:15"),
        Some(7200)
    );
}

#[test]
fn vtimezone_first_observance_document_order_tie_break() {
    // When two observances share an identical earliest transition onset,
    // document order is preserved by using `<` rather than `<=` when tracking `first`.
    let ics = "\
BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Test//EN\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:Custom/CoincidentOnset\r\n\
BEGIN:STANDARD\r\n\
DTSTART:20200101T010000\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0100\r\n\
END:STANDARD\r\n\
BEGIN:DAYLIGHT\r\n\
DTSTART:20200101T020000\r\n\
TZOFFSETFROM:+0200\r\n\
TZOFFSETTO:+0200\r\n\
END:DAYLIGHT\r\n\
END:VTIMEZONE\r\n\
END:VCALENDAR\r\n";

    let cal = parse_ical(ics).expect("parse ics");
    let observances = get_observances(&cal);

    // Both observances have UTC onset at 2020-01-01T00:00:00 UTC:
    // Standard: 01:00:00 - 01:00 = 00:00:00 UTC.
    // Daylight: 02:00:00 - 02:00 = 00:00:00 UTC.
    // For a query prior to all transitions (2019-01-01), the first observance in
    // document order (STANDARD, +0100 = 3600) is preserved.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2019-01-01T00:00:00"),
        Some(3600)
    );
}
