// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What offset from UTC a `VTIMEZONE` puts in force at a given instant.
//!
//! The one question [`event`](crate::event) has to answer that iCalendar
//! itself does not: RFC 5545 §3.3.10 states a recurrence's `UNTIL` as a UTC
//! instant whenever `DTSTART` names a zone, while RFC 8984 §4.3.3 states it as
//! a local time in that zone, and converting between them needs the offset in
//! force *at* that instant.
//!
//! Which is normally a zone database's job, and this crate ships none. It does
//! not have to: RFC 5545 §3.6.5 says a `TZID` is defined by the `VTIMEZONE` in
//! the same object, so a document that names a zone carries the rules for it —
//! Evolution writes one, and so does every invitation and every exported
//! `.ics`. This module reads the answer out of the document rather than out of
//! a database, and where the document does not answer it says so, which leaves
//! the caller exactly where it was before.
//!
//! Deliberately narrow. Only the shape a transition rule is actually written
//! in is counted — a yearly rule naming one day of one month — and anything
//! else, an unknown part included, is refused whole. A wrong offset moves the
//! end of a series by an hour or by twelve with nothing downstream able to
//! tell; refusing costs only the conversion.

use crate::event::{WEEKDAYS, days_from_civil, days_in_month, offset_seconds, to_local_date_time};
use crate::syntax::{Component, Property};

/// When the zone last changed its offset, and what that offset is — the
/// `TZOFFSETTO` of the observance the transition belongs to, or, for the
/// earliest transition of all, the `TZOFFSETFROM` it moved away from.
type Transition = (i64, i64);

/// The offset from UTC in force at `utc` in the zone `vtimezone` describes, as
/// a count of seconds east of UTC, or `None` for a definition whose
/// transitions this cannot work out.
///
/// `utc` is a UTC instant spelled as a LocalDateTime — the digits of a `Z`
/// value with the `Z` taken off, which is what [`to_local_date_time`] hands
/// back.
///
/// The offset is the one the *latest* transition at or before that instant
/// moved to. A transition happens on its own instant, so a value falling
/// exactly on one gets the new offset: that is what an observance's `DTSTART`
/// means, the first moment the new offset is in force.
///
/// An instant before every transition the definition states gets the
/// `TZOFFSETFROM` of the earliest of them, which is the only thing the
/// definition says about the zone before it started describing it.
pub(crate) fn offset_at(vtimezone: &Component, utc: &str) -> Option<i64> {
    let (year, ..) = parts(utc)?;
    let target = seconds_at(utc)?;
    let mut in_force: Option<Transition> = None;
    let mut first: Option<Transition> = None;
    for observance in &vtimezone.children {
        if !matches!(observance.name.as_str(), "STANDARD" | "DAYLIGHT") {
            continue;
        }
        let from = offset_seconds(&observance.text("TZOFFSETFROM")?)?;
        let to = offset_seconds(&observance.text("TZOFFSETTO")?)?;
        for onset in onsets(observance, from, year)? {
            if onset <= target && in_force.is_none_or(|(latest, _)| onset > latest) {
                in_force = Some((onset, to));
            }
            if first.is_none_or(|(earliest, _)| onset < earliest) {
                first = Some((onset, from));
            }
        }
    }
    in_force.or(first).map(|(_, offset)| offset)
}

/// When one observance starts being in force, as UTC instants, taking in only
/// the occurrences that could matter for a target in `year` — see
/// [`rule_onsets`].
///
/// RFC 5545 §3.6.5 dates an observance in the zone it is defining, against the
/// offset it is moving *from*, so that is what each local time here is resolved
/// with. The `DTSTART` is itself the first occurrence, as it is for any
/// recurrence.
fn onsets(observance: &Component, from: i64, year: i64) -> Option<Vec<i64>> {
    let start = to_local_date_time(&observance.text("DTSTART")?)?;
    let mut onsets = vec![seconds_at(&start)? - from];
    for date in observance
        .all("RDATE")
        .into_iter()
        .flat_map(Property::texts)
    {
        // A period says how long an occurrence lasts, which is not a thing a
        // transition has; §3.8.5.2 admits the spelling and no zone uses it, so
        // it is refused rather than read as its first half.
        if date.contains('/') {
            return None;
        }
        onsets.push(seconds_at(&to_local_date_time(&date)?)? - from);
    }
    for rule in observance.all("RRULE") {
        onsets.extend(rule_onsets(&rule.raw_value(), &start, from, year)?);
    }
    Some(onsets)
}

/// The occurrences of one transition rule that could be the latest at or before
/// an instant in `year`, as UTC instants.
///
/// Two of them: the rule's occurrence in the last year it can still have one at
/// or before `year`, and the one in the year before that — which is what the
/// caller needs, because a target in February is before that year's spring
/// transition and after the previous autumn's. Every further occurrence is
/// either later than the target or earlier than one of these two, and the
/// caller only ever takes a maximum.
///
/// The shape counted is the one a transition rule is written in: yearly, one
/// day of one month. `FREQ` is asked of every rule and only `YEARLY` answers,
/// an `INTERVAL` other than 1 is refused rather than aligned, and a part
/// outside the handful below is refused too — a rule that says more than was
/// understood is a rule that was not understood.
fn rule_onsets(rule: &str, start: &str, from: i64, year: i64) -> Option<Vec<i64>> {
    let (start_year, start_month, start_day, of_day) = parts(start)?;
    let mut frequency: Option<&str> = None;
    let mut interval: i64 = 1;
    let (mut count, mut until, mut until_year) = (None, None, None);
    let (mut by_month, mut by_month_day, mut by_day) = (None, None, None);
    for part in rule.split(';') {
        let (key, value) = part.split_once('=')?;
        match key.to_ascii_uppercase().as_str() {
            "FREQ" => frequency = Some(value),
            "INTERVAL" => interval = value.parse().ok()?,
            "COUNT" => count = Some(value.parse::<i64>().ok()?),
            "UNTIL" => {
                let local = to_local_date_time(value)?;
                until_year = Some(parts(&local)?.0);
                // §3.6.5's own examples state it as a UTC instant, which is
                // what every producer writes; a value without the `Z` is a
                // local time in the zone, dated like the `DTSTART` beside it.
                until = Some(match value.ends_with(['Z', 'z']) {
                    true => seconds_at(&local)?,
                    false => seconds_at(&local)? - from,
                });
            }
            "BYMONTH" => by_month = Some(value.parse::<i64>().ok()?),
            "BYMONTHDAY" => by_month_day = Some(value.parse::<i64>().ok()?),
            "BYDAY" => by_day = Some(value),
            _ => return None,
        }
    }
    if !frequency?.eq_ignore_ascii_case("YEARLY") || interval != 1 {
        return None;
    }

    // One occurrence a year, so a `COUNT` bounds the years directly, and an
    // `UNTIL` bounds them by its own — the instant itself still decides, below,
    // whether that year's occurrence is inside it.
    let last = [
        Some(year),
        count.map(|count| start_year + count - 1),
        until_year,
    ]
    .into_iter()
    .flatten()
    .min()?;
    let month = by_month.unwrap_or(start_month);

    let mut onsets = Vec::new();
    for year in [last, last - 1] {
        if year < start_year {
            continue;
        }
        let day = match (by_day, by_month_day) {
            // Two ways of naming the day, which is a rule stating a set rather
            // than the single transition this counts.
            (Some(_), Some(_)) => return None,
            (Some(token), None) => weekday_of_month(year, month, token)?,
            (None, Some(day)) => day_of_month(year, month, day)?,
            // No `BYxxx` for the day at all: §3.3.10 takes it from `DTSTART`.
            (None, None) => start_day,
        };
        let onset = seconds(year, month, day, of_day)? - from;
        if until.is_none_or(|until| onset <= until) {
            onsets.push(onset);
        }
    }
    Some(onsets)
}

/// The day of the month one `BYDAY` token names in a given month — `-1SU` is
/// its last Sunday, `2SU` its second.
///
/// A weekday with no ordinal is every one of them in the month, which is a set
/// of days and not the one a transition happens on, so it is refused with
/// everything else this cannot count: an ordinal of zero, a token that is no
/// weekday, and one whose occurrence that month does not exist (a fifth Sunday
/// of a month with four).
fn weekday_of_month(year: i64, month: i64, token: &str) -> Option<i64> {
    let token = token.strip_prefix('+').unwrap_or(token);
    let (ordinal, name) = token.split_at_checked(token.len().checked_sub(2)?)?;
    let nth: i64 = ordinal.parse().ok()?;
    let wanted = i64::try_from(
        WEEKDAYS
            .iter()
            .position(|weekday| weekday.eq_ignore_ascii_case(name))?,
    )
    .ok()?;
    let length = length_of(year, month)?;
    let day = match nth.signum() {
        1 => 1 + (wanted - weekday(year, month, 1)).rem_euclid(7) + 7 * (nth - 1),
        -1 => length - (weekday(year, month, length) - wanted).rem_euclid(7) + 7 * (nth + 1),
        _ => return None,
    };
    (1..=length).contains(&day).then_some(day)
}

/// The day of the month one `BYMONTHDAY` token names, RFC 5545 §3.3.10's
/// negative spelling — which counts back from the end of the month — included.
fn day_of_month(year: i64, month: i64, day: i64) -> Option<i64> {
    match day {
        0 => None,
        day if day > 0 => Some(day),
        day => Some(length_of(year, month)? + 1 + day),
    }
}

/// The day of the week a date falls on, numbered as [`WEEKDAYS`] is: 0 is
/// Monday. 1970-01-01, the epoch [`days_from_civil`] counts from, was a
/// Thursday.
fn weekday(year: i64, month: i64, day: i64) -> i64 {
    (days_from_civil(year, month, day) + 3).rem_euclid(7)
}

/// [`days_in_month`] on the signed fields this module counts in, refusing a
/// month no year has.
fn length_of(year: i64, month: i64) -> Option<i64> {
    let length = days_in_month(u32::try_from(year).ok()?, u32::try_from(month).ok()?);
    (length > 0).then_some(i64::from(length))
}

/// A LocalDateTime as its year, month, day and seconds since its own midnight.
fn parts(local: &str) -> Option<(i64, i64, i64, i64)> {
    let (date, time) = local.split_once('T')?;
    let field = |value: Option<&str>| value?.parse::<i64>().ok();
    Some((
        field(date.get(..4))?,
        field(date.get(5..7))?,
        field(date.get(8..10))?,
        field(time.get(..2))? * 3_600 + field(time.get(3..5))? * 60 + field(time.get(6..8))?,
    ))
}

/// A LocalDateTime as seconds from 1970-01-01T00:00:00 on its own wall clock —
/// a number to compare and to shift, not an instant on any timeline until an
/// offset has been taken off it.
fn seconds_at(local: &str) -> Option<i64> {
    let (year, month, day, of_day) = parts(local)?;
    seconds(year, month, day, of_day)
}

/// The same, from fields — refusing a date that does not exist, so that a rule
/// naming the thirtieth of February is refused rather than counted as March.
fn seconds(year: i64, month: i64, day: i64, of_day: i64) -> Option<i64> {
    (1..=length_of(year, month)?)
        .contains(&day)
        .then(|| days_from_civil(year, month, day) * 86_400 + of_day)
}
