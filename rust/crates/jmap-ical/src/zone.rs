// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What offset from UTC a `VTIMEZONE` puts in force at a given instant.
//!
//! Resolves recurrence `UNTIL` conversions between UTC instants (RFC 5545 §3.3.10)
//! and zoned local times (RFC 8984 §4.3.3) by reading the document's own `VTIMEZONE`
//! definitions (RFC 5545 §3.6.5) rather than relying on an external zone database.
//! Refuses unmappable or ambiguous transition rules cleanly.

use calcard::icalendar::ICalendarComponent;

use crate::event::{
    WEEKDAYS, component_entries, component_text, days_from_civil, days_in_month, entry_raw_value,
    entry_texts, offset_seconds, to_local_date_time,
};

/// When the zone last changed its offset, and what that offset is: the
/// `TZOFFSETTO` of the observance, or `TZOFFSETFROM` for the earliest transition.
type Transition = (i64, i64);

/// The offset from UTC in force at `utc` in the zone `observances` describe, as
/// seconds east of UTC, or `None` if transitions cannot be resolved.
///
/// `utc` is a UTC instant spelled as a LocalDateTime (without trailing `Z`).
pub(crate) fn offset_at(observances: &[&ICalendarComponent], utc: &str) -> Option<i64> {
    let (year, ..) = parts(utc)?;
    let target = seconds_at(utc)?;
    let mut in_force: Option<Transition> = None;
    let mut first: Option<Transition> = None;
    for observance in observances {
        if !matches!(observance.component_type.as_str(), "STANDARD" | "DAYLIGHT") {
            continue;
        }
        let from = offset_seconds(&component_text(observance, "TZOFFSETFROM")?)?;
        let to = offset_seconds(&component_text(observance, "TZOFFSETTO")?)?;
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

/// When one observance starts being in force, as UTC instants (see [`rule_onsets`]).
fn onsets(observance: &ICalendarComponent, from: i64, year: i64) -> Option<Vec<i64>> {
    let start = to_local_date_time(&component_text(observance, "DTSTART")?)?;
    let mut onsets = vec![seconds_at(&start)? - from];
    for date in component_entries(observance, "RDATE").flat_map(entry_texts) {
        // A period says how long an occurrence lasts, which is not a thing a
        // transition has; §3.8.5.2 admits the spelling and no zone uses it, so
        // it is refused rather than read as its first half.
        if date.contains('/') {
            return None;
        }
        onsets.push(seconds_at(&to_local_date_time(&date)?)? - from);
    }
    for rule in component_entries(observance, "RRULE") {
        onsets.extend(rule_onsets(&entry_raw_value(rule), &start, from, year)?);
    }
    Some(onsets)
}

/// The occurrences of one transition rule that could be the latest at or before
/// an instant in `year`, as UTC instants.
///
/// Searches [`SEARCH`] years back from `year`. Transitions outside this window
/// or in unsupported shapes are refused.
fn rule_onsets(rule: &str, start: &str, from: i64, year: i64) -> Option<Vec<i64>> {
    let (start_year, start_month, start_day, of_day) = parts(start)?;
    let mut frequency: Option<&str> = None;
    let mut interval: i64 = 1;
    let (mut count, mut until, mut until_year) = (None, None, None);
    let (mut by_month, mut by_month_day, mut by_day): (_, Option<&str>, _) = (None, None, None);
    let (mut by_hour, mut by_minute, mut by_second) = (None, None, None);
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
            "BYMONTHDAY" => by_month_day = Some(value),
            "BYDAY" => by_day = Some(value),
            "BYHOUR" => by_hour = Some(value),
            "BYMINUTE" => by_minute = Some(value),
            "BYSECOND" => by_second = Some(value),
            // §3.3.10 gives `WKST` meaning only with `WEEKLY` interval or `BYWEEKNO`.
            // Unsupported here, ignored for compatibility with Exchange and Zimbra.
            "WKST" => {}
            _ => return None,
        }
    }
    if !frequency?.eq_ignore_ascii_case("YEARLY") || interval != 1 {
        return None;
    }

    // One occurrence a year, so `COUNT` and `UNTIL` bound the years directly.
    let last = [
        Some(year),
        count.map(|count| start_year + count - 1),
        until_year,
    ]
    .into_iter()
    .flatten()
    .min()?;
    let month = by_month.unwrap_or(start_month);
    let names = Day::named(by_day, by_month_day, start_day)?;
    let of_day = restated(of_day, by_hour, by_minute, by_second)?;

    let earliest = start_year.max(last - SEARCH + 1);
    let mut onsets = Vec::new();
    for year in (earliest..=last).rev() {
        let day = match names.of(year, month) {
            Falls::On(day) => day,
            // A year the rule states no day in is a year it does not happen in,
            // not a rule that was misread: §3.3.10 says as much of a
            // `BYMONTHDAY` the month is too short for, and the same holds of a
            // fifth Sunday of a month with four.
            Falls::Never => continue,
            // More than one day is a rule stating a set, which this module
            // refuses whole rather than choosing from.
            Falls::Set => return None,
        };
        let onset = seconds(year, month, day, of_day)? - from;
        if until.is_none_or(|until| onset <= until) {
            onsets.push(onset);
        }
    }
    // Nothing found, and years the search did not reach: see the note above.
    (!onsets.is_empty() || earliest == start_year).then_some(onsets)
}

/// The time of day a rule's occurrences happen at: `DTSTART`'s time of day,
/// with each field the rule restates replaced by the rule's value.
///
/// Returns `None` if a field states multiple values, an out-of-range value,
/// or a leap second (60).
fn restated(
    of_day: i64,
    hour: Option<&str>,
    minute: Option<&str>,
    second: Option<&str>,
) -> Option<i64> {
    let only = |value: Option<&str>, last| match value {
        None => Some(None),
        Some(value) => {
            let field: i64 = value.parse().ok()?;
            (0..=last).contains(&field).then_some(Some(field))
        }
    };
    Some(
        only(hour, 23)?.unwrap_or(of_day / 3_600) * 3_600
            + only(minute, 59)?.unwrap_or(of_day / 60 % 60) * 60
            + only(second, 59)?.unwrap_or(of_day % 60),
    )
}

/// How far back [`rule_onsets`] searches for a rule's occurrences (up to 40 years).
const SEARCH: i64 = 40;

/// How a transition rule names the day of the month it happens on.
enum Day {
    /// `BYDAY` with an ordinal: `-1SU` is the month's last Sunday, `2SU` its second.
    Nth(i64, i64),
    /// A `BYDAY` weekday limiting the dates a `BYMONTHDAY` lists (e.g. tzdata idioms).
    WeekdayAmong(i64, Vec<i64>),
    /// A single `BYMONTHDAY`, including negative offsets from month end.
    OfMonth(i64),
    /// No `BYxxx` for the day: takes it from `DTSTART`.
    OfStart(i64),
}

impl Day {
    /// What the rule's `BYDAY` and `BYMONTHDAY` state, or `None` for a pair
    /// this cannot count a single transition out of.
    fn named(by_day: Option<&str>, by_month_day: Option<&str>, start: i64) -> Option<Self> {
        Some(match (by_day, by_month_day) {
            (Some(token), Some(dates)) => Self::WeekdayAmong(
                weekday_named(token)?,
                dates
                    .split(',')
                    .map(|date| date.parse().ok().filter(|day| *day != 0))
                    .collect::<Option<Vec<_>>>()?,
            ),
            (Some(token), None) => {
                let token = token.strip_prefix('+').unwrap_or(token);
                let (ordinal, name) = token.split_at_checked(token.len().checked_sub(2)?)?;
                let nth: i64 = ordinal.parse().ok()?;
                Self::Nth((nth != 0).then_some(nth)?, weekday_named(name)?)
            }
            (None, Some(date)) => Self::OfMonth(date.parse().ok().filter(|day| *day != 0)?),
            (None, None) => Self::OfStart(start),
        })
    }

    /// Which day of `month` in `year` this names.
    fn of(&self, year: i64, month: i64) -> Falls {
        let Some(length) = length_of(year, month) else {
            return Falls::Never;
        };
        let of_month = |day: i64| match day > 0 {
            true => day,
            false => length + 1 + day,
        };
        let day = match self {
            Self::Nth(nth, wanted) => match nth.signum() {
                1 => 1 + (wanted - weekday(year, month, 1)).rem_euclid(7) + 7 * (nth - 1),
                _ => length - (weekday(year, month, length) - wanted).rem_euclid(7) + 7 * (nth + 1),
            },
            Self::WeekdayAmong(wanted, dates) => {
                let mut days = dates.iter().map(|date| of_month(*date)).filter(|day| {
                    (1..=length).contains(day) && weekday(year, month, *day) == *wanted
                });
                return match (days.next(), days.next()) {
                    (Some(day), None) => Falls::On(day),
                    (None, _) => Falls::Never,
                    (Some(_), Some(_)) => Falls::Set,
                };
            }
            Self::OfMonth(date) => of_month(*date),
            Self::OfStart(day) => *day,
        };
        match (1..=length).contains(&day) {
            true => Falls::On(day),
            false => Falls::Never,
        }
    }
}

/// What a transition rule states for one year.
enum Falls {
    /// This day of the month, the one transition the year holds.
    On(i64),
    /// No day: a year the rule does not happen in.
    Never,
    /// More than one day: a set of transitions rather than a single onset.
    Set,
}

/// The day of the week a two-letter `BYDAY` name states, numbered as
/// [`WEEKDAYS`] is: `None` for anything that is not one of the seven.
fn weekday_named(name: &str) -> Option<i64> {
    i64::try_from(
        WEEKDAYS
            .iter()
            .position(|weekday| weekday.eq_ignore_ascii_case(name))?,
    )
    .ok()
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

/// A LocalDateTime as seconds from 1970-01-01T00:00:00 on its own wall clock:
/// a number to compare and to shift, not an instant on any timeline until an
/// offset has been taken off it.
fn seconds_at(local: &str) -> Option<i64> {
    let (year, month, day, of_day) = parts(local)?;
    seconds(year, month, day, of_day)
}

/// The same, from fields, refusing a date that does not exist.
fn seconds(year: i64, month: i64, day: i64, of_day: i64) -> Option<i64> {
    (1..=length_of(year, month)?)
        .contains(&day)
        .then(|| days_from_civil(year, month, day) * 86_400 + of_day)
}
