// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calculates the UTC offset for a `VTIMEZONE` at a given instant.
//!
//! Resolves recurrence `UNTIL` conversions between UTC instants (RFC 5545 Section 3.3.10)
//! and zoned local times (RFC 8984 Section 4.3.3).
//! It reads the document's `VTIMEZONE` definitions (RFC 5545 Section 3.6.5) without an
//! external timezone database.
//! Refuses ambiguous or unmappable transition rules.

use calcard::icalendar::ICalendarComponent;

use crate::event::{
    WEEKDAYS, component_entries, component_text, days_from_civil, days_in_month, entry_raw_value,
    entry_texts, offset_seconds, to_local_date_time,
};

/// Time and offset for a timezone transition: `(timestamp, offset)`.
///
/// Contains `TZOFFSETTO` of the observance, or `TZOFFSETFROM` for the earliest transition.
type Transition = (i64, i64);

/// Returns the UTC offset in seconds east of UTC at `utc` for the given observances.
///
/// Returns `None` if transitions cannot be resolved.
/// `utc` is a UTC instant formatted as a local date-time without a trailing `Z`.
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

/// Returns UTC instants when an observance takes effect (see [`rule_onsets`]).
fn onsets(observance: &ICalendarComponent, from: i64, year: i64) -> Option<Vec<i64>> {
    let start = to_local_date_time(&component_text(observance, "DTSTART")?)?;
    let mut onsets = vec![seconds_at(&start)? - from];
    for date in component_entries(observance, "RDATE").flat_map(entry_texts) {
        // RFC 5545 Section 3.8.5.2 permits period formats for RDATE.
        // Transitions have zero duration, so period formats are refused.
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

/// Returns transition occurrences for a rule near an instant in `year`.
///
/// Searches [`SEARCH`] years before `year`.
/// Refuses transitions outside this window or with unsupported formats.
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
                // RFC 5545 Section 3.6.5 states UNTIL as a UTC instant with Z.
                // Values without Z are local times with offset applied.
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
            // RFC 5545 Section 3.3.10 defines WKST for WEEKLY or BYWEEKNO.
            // Ignored here for compatibility with Exchange and Zimbra.
            "WKST" => {}
            _ => return None,
        }
    }
    if !frequency?.eq_ignore_ascii_case("YEARLY") || interval != 1 {
        return None;
    }

    // The frequency is yearly, so COUNT and UNTIL limit years directly.
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
            // Skip years where the day does not occur, such as day 31 in short months.
            Falls::Never => continue,
            // Rules specifying multiple days are unsupported and refused.
            Falls::Set => return None,
        };
        let onset = seconds(year, month, day, of_day)? - from;
        if until.is_none_or(|until| onset <= until) {
            onsets.push(onset);
        }
    }
    // Return onsets if non-empty or if the search started at the start year.
    (!onsets.is_empty() || earliest == start_year).then_some(onsets)
}

/// Calculates the time of day for occurrences of a rule in seconds.
///
/// Replaces fields from `DTSTART` with values from the rule.
/// Returns `None` for invalid or out-of-range values.
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

/// Maximum years [`rule_onsets`] searches backwards for rule occurrences.
const SEARCH: i64 = 40;

/// Specification of a transition day of the month.
enum Day {
    /// `BYDAY` with an ordinal number, such as `-1SU` or `2SU`.
    Nth(i64, i64),
    /// `BYDAY` weekday matching a list of `BYMONTHDAY` values.
    WeekdayAmong(i64, Vec<i64>),
    /// Single `BYMONTHDAY`, including negative offsets from month end.
    OfMonth(i64),
    /// Day taken from `DTSTART` when no `BYxxx` rule is present.
    OfStart(i64),
}

impl Day {
    /// Parses `BYDAY` and `BYMONTHDAY` into a [`Day`] variant.
    ///
    /// Returns `None` if the combination cannot be resolved to a single transition.
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

    /// Resolves the day of `month` in `year`.
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

/// Occurrence result for a transition rule in a specific year.
enum Falls {
    /// Transition occurs on this day of the month.
    On(i64),
    /// Transition does not occur in this year.
    Never,
    /// Multiple transition days specified.
    Set,
}

/// Converts a two-letter `BYDAY` name to a weekday index matching [`WEEKDAYS`].
///
/// Returns `None` if the name is not one of the seven weekdays.
fn weekday_named(name: &str) -> Option<i64> {
    i64::try_from(
        WEEKDAYS
            .iter()
            .position(|weekday| weekday.eq_ignore_ascii_case(name))?,
    )
    .ok()
}

/// Returns the weekday index matching [`WEEKDAYS`] where 0 is Monday.
///
/// The epoch 1970-01-01 was a Thursday (index 3).
fn weekday(year: i64, month: i64, day: i64) -> i64 {
    (days_from_civil(year, month, day) + 3).rem_euclid(7)
}

/// Calls [`days_in_month`] for signed integers. Returns `None` for invalid months.
fn length_of(year: i64, month: i64) -> Option<i64> {
    let length = days_in_month(u32::try_from(year).ok()?, u32::try_from(month).ok()?);
    (length > 0).then_some(i64::from(length))
}

/// Parses a local date-time into year, month, day, and seconds since midnight.
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

/// Converts a local date-time to wall-clock seconds from 1970-01-01T00:00:00.
fn seconds_at(local: &str) -> Option<i64> {
    let (year, month, day, of_day) = parts(local)?;
    seconds(year, month, day, of_day)
}

/// Calculates wall-clock seconds from civil date fields. Returns `None` for invalid dates.
fn seconds(year: i64, month: i64, day: i64, of_day: i64) -> Option<i64> {
    (1..=length_of(year, month)?)
        .contains(&day)
        .then(|| days_from_civil(year, month, day) * 86_400 + of_day)
}
