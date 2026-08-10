// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A JMAP date as the number Camel stores.
//!
//! Camel keeps both of a message's dates as a `gint64` count of seconds since
//! the epoch, and JMAP sends both as text: `UTCDate` for `receivedAt`, which
//! RFC 8620 §1.4 fixes to `Z`, and `Date` for `sentAt`, which carries whatever
//! offset the sender's clock was at. Something has to do the arithmetic, and
//! doing it here rather than in the Camel layer is what makes it testable
//! without the Evolution headers — the same reason the rest of this crate
//! exists.
//!
//! Hand-rolled rather than taken from a crate, unlike the iCalendar and vCard
//! parsing this project deliberately outsources: the whole of it is the two
//! grammars above and the proleptic Gregorian calendar, both of which are
//! fixed, small, and fully covered by the tests below. `jmap-proto` keeps
//! `UtcDate` a string for its own good reason — it is a wire type, and a
//! protocol crate that reinterpreted values would be able to lose them — so
//! this is the layer that gets to interpret one.

/// The years a JMAP date can name, in either direction.
///
/// RFC 8620 §1.4 makes a `UTCDate` an RFC 3339 `date-time`, whose `date-fullyear`
/// is four digits, and the `Date` a `sentAt` carries is the same production at an
/// offset. So this is the grammar's own range — and, because it bounds the year
/// before any arithmetic is done with it, it is also what keeps
/// [`days_from_civil`] inside `i64`. There is no year zero: `0000` is four digits
/// and not a year of the proleptic Gregorian calendar as RFC 3339 counts them.
const YEARS: std::ops::RangeInclusive<i64> = 1..=9_999;

/// Seconds since 1970-01-01T00:00:00Z, or `None` if `text` is not a date.
///
/// `None` rather than an error on purpose. A message with an unreadable date is
/// still a message, and the alternative — failing the folder listing it arrived
/// in — would let one malformed `Date` header hide a mailbox. Camel's own
/// summary does the same thing by storing 0 for a date it could not parse; the
/// difference here is that a caller can tell "the server said nothing" from
/// "the server said midnight on the epoch".
pub(crate) fn epoch_seconds(text: &str) -> Option<i64> {
    let (date, rest) = text.split_once('T')?;
    let (year, month, day) = split3(date, '-')?;
    let (year, month, day): (i64, u32, u32) =
        (year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
    // The year is checked before it is used, not only for being a year: the
    // server picks this text, `i64` parses nineteen digits of it, and
    // `days_from_civil` multiplies the era by 146 097 and the result by 86 400 —
    // so a year the grammar does not allow is also one the arithmetic below
    // overflows on. [`YEARS`] is exactly the range [`civil_from_days`] will
    // write back, which is RFC 3339 §5.6's four-digit year.
    if !YEARS.contains(&year)
        || !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
    {
        return None;
    }

    // The offset is what ends the time, so the time is what is left of it. A
    // `sentAt` without one is not a date this can read: RFC 3339 requires the
    // zone, and guessing UTC would silently move the message by hours.
    let (time, offset) = split_offset(rest)?;
    let (hour, minute, second) = split3(time, ':')?;
    // Fractional seconds carry no information Camel has room for.
    let second = second.split_once('.').map_or(second, |(whole, _)| whole);
    // `u32` rather than `i64` throughout: a field that parses as negative is a
    // sign this text is not a time, and reading `-9` as an hour would move the
    // message rather than reject it.
    let (hour, minute, second): (u32, u32, u32) = (
        hour.parse().ok()?,
        minute.parse().ok()?,
        second.parse().ok()?,
    );
    // A leap second is 60, and clamping it to 59 is a second's worth of lie in
    // exchange for not dropping the date.
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let seconds_of_day =
        i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second.min(59));
    Some(days_from_civil(year, month, day) * 86_400 + seconds_of_day - offset)
}

/// The `UTCDate` naming the same instant as `seconds`, or `None` if no
/// `UTCDate` names it.
///
/// [`epoch_seconds`] run backwards, and it exists for the one thing this crate
/// sends a date *to* the server in: an import carries the moment Camel says the
/// message was received, and Camel says it as a count of seconds.
///
/// `None` is the answer for an instant outside the grammar rather than a
/// clamped date at the end of it. RFC 8620 §1.4 makes a `UTCDate` an RFC 3339
/// `date-time` at `Z`, whose year is four digits — so a `gint64` of seconds can
/// hold instants that simply cannot be written, and writing the nearest one that
/// can would be this layer inventing a date the caller never gave it. What the
/// caller does with `None` is its own decision; for an import it is to send no
/// date and let the server choose, which is what RFC 8621 §4.8 has it do.
///
/// Whole seconds, no fraction: Camel has nowhere to keep one, so there is never
/// one to write.
pub(crate) fn utc_date(seconds: i64) -> Option<String> {
    // Euclidean, not truncating: an instant before the epoch is a negative
    // count, and `-1 / 86_400` is the day *after* the one -1 seconds falls in.
    let (days, seconds_of_day) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days)?;

    let (hour, minute, second) = (
        seconds_of_day / 3_600,
        (seconds_of_day / 60) % 60,
        seconds_of_day % 60,
    );
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

/// The civil date `days` after 1970-01-01, or `None` for a year no four-digit
/// year can name.
///
/// Howard Hinnant's `civil_from_days`, the inverse of [`days_from_civil`] and
/// shifted the same way — the year starts in March so that the leap day is the
/// last day of it — and exact for every date it answers.
fn civil_from_days(days: i64) -> Option<(i64, u32, u32)> {
    let shifted = days.checked_add(719_468)?;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    // The grammar's whole range — [`YEARS`], which is also the range
    // [`epoch_seconds`] admits, so that what this writes is exactly what that
    // reads back.
    YEARS
        .contains(&year)
        .then_some((year, u32::try_from(month).ok()?, u32::try_from(day).ok()?))
}

/// Splits `text` into exactly three fields on `separator`.
fn split3(text: &str, separator: char) -> Option<(&str, &str, &str)> {
    let mut fields = text.split(separator);
    let three = (fields.next()?, fields.next()?, fields.next()?);
    fields.next().is_none().then_some(three)
}

/// The time and the offset it is at, in seconds east of UTC.
fn split_offset(text: &str) -> Option<(&str, i64)> {
    if let Some(time) = text.strip_suffix(['Z', 'z']) {
        return Some((time, 0));
    }
    // Not `rfind`: the sign is the only `+` or `-` in a time, and looking from
    // the right would find the same one anyway — but only after the fractional
    // seconds, which may be any length.
    let sign_at = text.find(['+', '-'])?;
    let (time, offset) = text.split_at(sign_at);
    let sign = if offset.starts_with('-') { -1 } else { 1 };
    let offset = &offset[1..];
    // `+HH:MM` and `+HHMM` are both current practice; RFC 3339 writes the
    // colon, RFC 5322's `Date` header does not, and JMAP servers relay both.
    let (hours, minutes) = match offset.split_once(':') {
        Some((hours, minutes)) => (hours, minutes),
        None if offset.len() == 4 => offset.split_at(2),
        None => return None,
    };
    let (hours, minutes): (u32, u32) = (hours.parse().ok()?, minutes.parse().ok()?);
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some((
        time,
        sign * (i64::from(hours) * 3_600 + i64::from(minutes) * 60),
    ))
}

/// Whether a year has a 29th of February, by the proleptic Gregorian rule.
fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days from 1970-01-01 to `year-month-day`, negative before it.
///
/// Howard Hinnant's `days_from_civil`, which shifts the year to start in March
/// so that the leap day is the last day of it and the month lengths become a
/// repeating pattern — that is what the `(153 * m + 2) / 5` is. Valid for every
/// year JMAP can express; the arithmetic is exact, not an approximation.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let month = i64::from(month);
    let day = i64::from(day);
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::{epoch_seconds, utc_date};

    #[test]
    fn the_epoch_itself_is_the_epoch_written_out() {
        assert_eq!(utc_date(0).as_deref(), Some("1970-01-01T00:00:00Z"));
    }

    #[test]
    fn an_instant_is_written_as_the_date_it_is() {
        assert_eq!(
            utc_date(1_768_469_400).as_deref(),
            Some("2026-01-15T09:30:00Z")
        );
        // The leap day, and a moment before the epoch: the two places the
        // arithmetic is easiest to get wrong.
        assert_eq!(
            utc_date(951_825_600).as_deref(),
            Some("2000-02-29T12:00:00Z")
        );
        assert_eq!(
            utc_date(-14_182_940).as_deref(),
            Some("1969-07-20T20:17:40Z")
        );
    }

    #[test]
    fn every_instant_this_writes_it_reads_back_as_the_same_one() {
        for seconds in [
            0,
            1,
            -1,
            86_399,
            86_400,
            -86_400,
            1_768_469_400,
            951_825_600,
            -14_182_940,
            // The ends of what a four-digit year can say.
            -62_135_596_800,
            253_402_300_799,
        ] {
            let written = utc_date(seconds).expect("a date inside the grammar");
            assert_eq!(epoch_seconds(&written), Some(seconds), "{written}");
        }
    }

    #[test]
    fn an_instant_no_utc_date_can_name_is_not_written_as_one() {
        // A `UTCDate` is `date-time` with a four-digit year (RFC 8620 §1.4,
        // RFC 3339 §5.6), so an instant outside the first of year 1 and the last
        // second of year 9999 has no spelling — including the two ends of the
        // range Camel keeps a date in.
        for seconds in [
            i64::MIN,
            i64::MAX,
            -62_135_596_801,
            253_402_300_800,
            // Year zero: four digits, and not a year.
            -62_167_219_200,
        ] {
            assert_eq!(utc_date(seconds), None, "{seconds}");
        }
    }

    #[test]
    fn the_epoch_itself_is_zero() {
        assert_eq!(epoch_seconds("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn a_date_is_read_as_the_instant_it_names() {
        assert_eq!(epoch_seconds("2026-01-15T09:30:00Z"), Some(1_768_469_400));
        assert_eq!(epoch_seconds("2000-02-29T12:00:00Z"), Some(951_825_600));
        assert_eq!(epoch_seconds("1969-07-20T20:17:40Z"), Some(-14_182_940));
    }

    #[test]
    fn an_offset_moves_the_instant_the_other_way() {
        // 10:30 an hour east of UTC is 09:30 UTC, not 11:30.
        assert_eq!(
            epoch_seconds("2026-01-15T10:30:00+01:00"),
            Some(1_768_469_400)
        );
        assert_eq!(
            epoch_seconds("2026-01-15T08:30:00-01:00"),
            Some(1_768_469_400)
        );
        // The colon is optional in practice, and the offset need not be whole
        // hours — India is at +05:30.
        assert_eq!(
            epoch_seconds("2026-01-15T10:30:00+0100"),
            Some(1_768_469_400)
        );
        assert_eq!(
            epoch_seconds("2026-01-15T15:00:00+05:30"),
            Some(1_768_469_400)
        );
        assert_eq!(
            epoch_seconds("2026-01-15T09:30:00-00:00"),
            Some(1_768_469_400)
        );
    }

    #[test]
    fn a_fraction_of_a_second_is_dropped() {
        assert_eq!(
            epoch_seconds("2026-01-15T09:30:00.512Z"),
            Some(1_768_469_400)
        );
        assert_eq!(
            epoch_seconds("2026-01-15T09:30:00.512+00:00"),
            Some(1_768_469_400)
        );
    }

    #[test]
    fn a_leap_second_is_the_second_before_it() {
        assert_eq!(
            epoch_seconds("2016-12-31T23:59:60Z"),
            epoch_seconds("2016-12-31T23:59:59Z")
        );
    }

    #[test]
    fn a_lower_case_zone_is_still_a_zone() {
        // RFC 3339 §5.6 allows it, and a server that sends it is not wrong.
        assert_eq!(epoch_seconds("1970-01-01T00:00:00z"), Some(0));
    }

    #[test]
    fn what_is_not_a_date_is_not_read_as_one() {
        for text in [
            "",
            "T",
            "yesterday",
            // No time at all, and no zone: both are required.
            "2026-01-15",
            "2026-01-15T09:30:00",
            // Fields that are not numbers, or not the right number of them.
            "2026-01T09:30:00Z",
            "2026-01-15T09:30Z",
            "2026-1x-15T09:30:00Z",
            "2026-01-15T09:3x:00Z",
            "2026-01-15T09:30:00+0X:00",
            "2026-01-15T09:30:00+1",
            // Fields that are numbers but not dates.
            "2026-00-15T09:30:00Z",
            "2026-13-15T09:30:00Z",
            "2026-01-00T09:30:00Z",
            "2026-01-32T09:30:00Z",
            "2026-02-29T09:30:00Z",
            "2100-02-29T09:30:00Z",
            "2026-01-15T24:00:00Z",
            "2026-01-15T09:60:00Z",
            "2026-01-15T09:30:61Z",
            "2026-01-15T09:30:00+24:00",
        ] {
            assert_eq!(epoch_seconds(text), None, "{text:?} is not a date");
        }
    }

    /// The year is a four-digit field, so the range this reads is the range
    /// [`utc_date`] writes — and nothing outside it reaches the arithmetic.
    ///
    /// Which is the point: the server picks the text, `i64::from_str` accepts
    /// nineteen digits of it, and `days_from_civil` multiplies what comes out by
    /// 146 097 and then by 86 400. Both overflow long before the parse does, so
    /// the bound is what keeps a date the grammar never allowed from being an
    /// arithmetic overflow instead of a refusal. See
    /// `docs/AUDIT-FFI-20260810.md`, F11.
    #[test]
    fn a_year_outside_the_four_digit_grammar_is_not_a_date() {
        for text in [
            "0000-01-01T00:00:00Z",
            "10000-01-01T00:00:00Z",
            "300000000000-01-01T00:00:00Z",
            "30000000000000000-01-01T00:00:00Z",
            "9223372036854775807-01-01T00:00:00Z",
        ] {
            assert_eq!(epoch_seconds(text), None, "{text:?} is not a date");
        }
        // And the two ends that are.
        assert!(epoch_seconds("0001-01-01T00:00:00Z").is_some());
        assert!(epoch_seconds("9999-12-31T23:59:59Z").is_some());
    }

    #[test]
    fn a_leap_year_has_a_29th_of_february_and_a_century_usually_does_not() {
        assert!(epoch_seconds("2024-02-29T00:00:00Z").is_some());
        assert!(epoch_seconds("2000-02-29T00:00:00Z").is_some());
        assert!(epoch_seconds("1900-02-29T00:00:00Z").is_none());
    }
}
