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
    // RFC 5545 3.6.5: Discrete RDATE dates represent local time in the prior offset.
    // Subtract `from` to convert to UTC.
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

    // At 2026-10-25T01:59:59 (before onset), STANDARD offset (+0100 = 3600s) applies.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-10-25T01:59:59"),
        Some(3600)
    );
    // At 2026-10-25T02:00:00 (at onset), DAYLIGHT offset (+0200 = 7200s) applies.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-10-25T02:00:00"),
        Some(7200)
    );
}

#[test]
fn vtimezone_rrule_local_until_subtracts_tzoffsetfrom_for_utc_onset() {
    // RFC 5545 3.6.5: A local UNTIL parameter subtracts TZOFFSETFROM to convert to UTC.
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

    // Onset 00:00:00 UTC matches UNTIL, so the 2026 occurrence shifts offset to +0300.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-03-29T00:00:00"),
        Some(10800)
    );

    // Onset exceeds UNTIL bound, so 2026 transition is excluded.
    let ics_expired = ics.replace("UNTIL=20260329T020000", "UNTIL=20260329T015959");
    let cal_exp = parse_ical(&ics_expired).expect("parse ics");
    let obs_exp = get_observances(&cal_exp);
    // Without the 2026 transition, winter standard (+0200) applies.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&obs_exp, "2026-03-29T00:00:00"),
        Some(7200)
    );
}

#[test]
fn vtimezone_rrule_count_bounds_last_transition_year() {
    // RFC 5545 3.3.10: COUNT bounds the last recurrence year to start_year + count - 1.
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

    // In summer 2021 (within count), DAYLIGHT (+0200) applies.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2021-06-15T12:00:00"),
        Some(7200)
    );
    // In autumn 2021, STANDARD (+0100) applies.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2021-11-15T12:00:00"),
        Some(3600)
    );
    // In summer 2022 (count expired), autumn 2021 STANDARD (+0100) remains in force.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2022-06-15T12:00:00"),
        Some(3600)
    );
}

#[test]
fn vtimezone_rule_onsets_search_horizon_boundary() {
    // Transitions outside the 40-year search window return None.
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

    // Earliest search year exceeds start_year with no valid onsets, returning None.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-06-15T12:00:00"),
        None
    );
}

#[test]
fn vtimezone_restated_time_of_day_preserves_minutes_and_seconds() {
    // RFC 5545 3.6.5: DTSTART minutes and seconds are preserved when RRULE omits them.
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

    // One second before transition (01:30:44 UTC): STANDARD applies.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-03-29T01:30:44"),
        Some(3600)
    );
    // One second after transition (01:30:46 UTC): DAYLIGHT applies.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-03-29T01:30:46"),
        Some(7200)
    );
}

#[test]
fn vtimezone_byday_positive_ordinal_second_sunday() {
    // RFC 5545 3.3.10: A positive BYDAY ordinal evaluates to the nth weekday of the month.
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

    // On March 5 (before 2nd Sunday), the transition has not occurred.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-03-05T12:00:00"),
        Some(3600)
    );
    // On March 9 (after 2nd Sunday), DAYLIGHT applies.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-03-09T12:00:00"),
        Some(7200)
    );
}

#[test]
fn vtimezone_parts_adds_seconds_to_target_instant() {
    // Target instant parsing must add seconds, not subtract them.
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

    // Query at 01:00:15 UTC is 15 seconds after the transition.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2026-03-29T01:00:15"),
        Some(7200)
    );
}

#[test]
fn vtimezone_first_observance_document_order_tie_break() {
    // When two observances share an onset, document order resolves the tie.
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

    // For a query before all transitions, the first observance in document order applies.
    assert_eq!(
        jmap_ical::event::zone_offset_at(&observances, "2019-01-01T00:00:00"),
        Some(3600)
    );
}
