// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A JMAP date as the number that Camel stores.
//!
//! Camel stores the two dates of a message as a `gint64` count of seconds
//! since the epoch. JMAP sends them as text: `receivedAt` is a `UTCDate`
//! (always `Z`, RFC 8620 §1.4), and `sentAt` is a `Date` with the offset of
//! the sender. This crate does the conversion, so the tests do not need the
//! Evolution headers.
//!
//! The code does not use an external crate, because it is small: two fixed
//! grammars and the proleptic Gregorian calendar, all covered by the tests.
//! `jmap-proto` keeps `UtcDate` as a string because it is a wire type, so
//! this layer interprets it. The calendar arithmetic itself is
//! [`jmap_proto::date`]; this module adds the RFC 8620 four-digit-year range
//! and the `UTCDate`/`Date` text grammars around it.

/// The years that this module accepts and writes.
///
/// RFC 8620 §1.4 makes a `UTCDate` an RFC 3339 `date-time`, with a four-digit
/// `date-fullyear`. A `Date` has the same grammar with an offset. The range
/// limits the year before [`jmap_proto::date::days_from_civil`] uses it, so
/// the arithmetic stays inside `i64`. The range excludes `0000`, although the
/// grammar allows it.
const YEARS: std::ops::RangeInclusive<i64> = 1..=9_999;

/// Seconds since 1970-01-01T00:00:00Z, or `None` if `text` is not a date.
///
/// `None`, not an error: a message with a bad date is still a message. An
/// error would fail the folder listing, so one bad `Date` header would hide a
/// mailbox. The Camel summary stores 0 for a bad date, but `None` lets a
/// caller see the difference between no date and the epoch.
pub(crate) fn epoch_seconds(text: &str) -> Option<i64> {
    let (date, rest) = text.split_once('T')?;
    let (year, month, day) = split3(date, '-')?;
    let (year, month, day): (i64, u32, u32) =
        (year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
    // The server supplies this text, and `i64` parses 19 digits. Check the
    // year before the arithmetic: `days_from_civil` multiplies the era by
    // 146 097, and the caller multiplies the days by 86 400, so a large year
    // overflows. [`YEARS`] is also the range that [`civil_from_days`] accepts.
    if !YEARS.contains(&year)
        || !(1..=12).contains(&month)
        || day < 1
        || day > jmap_proto::date::days_in_month(year, month)
    {
        return None;
    }

    // The offset ends the time. A `sentAt` without an offset is not valid:
    // RFC 3339 requires the zone, and UTC as a guess can move the message by
    // hours.
    let (time, offset) = split_offset(rest)?;
    let (hour, minute, second) = split3(time, ':')?;
    // Camel has no space for fractional seconds.
    let second = second.split_once('.').map_or(second, |(whole, _)| whole);
    // `u32`, not `i64`: a negative field such as `-9` is not a time, and must
    // not move the message.
    let (hour, minute, second): (u32, u32, u32) = (
        hour.parse().ok()?,
        minute.parse().ok()?,
        second.parse().ok()?,
    );
    // A leap second is 60. The calculation uses 59, so the date stays.
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let seconds_of_day =
        i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second.min(59));
    Some(jmap_proto::date::days_from_civil(year, month, day) * 86_400 + seconds_of_day - offset)
}

/// The `UTCDate` for the instant `seconds`, or `None` if no `UTCDate` can
/// write it.
///
/// The inverse of [`epoch_seconds`]. An import is the only place where this
/// crate sends a date to the server, and Camel gives that date as seconds.
///
/// Out of range gives `None`, not the nearest valid date. A `UTCDate` has a
/// four-digit year (RFC 8620 §1.4), so a `gint64` can hold instants that no
/// `UTCDate` can write. The nearest valid date is a date that the caller did
/// not give. For an import, the caller then sends no date and the server
/// selects one (RFC 8621 §4.8).
///
/// Whole seconds only: Camel stores no fraction.
pub(crate) fn utc_date(seconds: i64) -> Option<String> {
    // Euclidean, not truncating: before the epoch the count is negative, and
    // `-1 / 86_400` gives the day after the day of second -1.
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

/// The civil date `days` after 1970-01-01, or `None` for a year outside
/// [`YEARS`].
///
/// The arithmetic is [`jmap_proto::date::civil_from_days`]; this wrapper adds
/// the four-digit-year range that `epoch_seconds` reads back.
fn civil_from_days(days: i64) -> Option<(i64, u32, u32)> {
    let (year, month, day) = jmap_proto::date::civil_from_days(days)?;
    YEARS.contains(&year).then_some((year, month, day))
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
    // The sign is the only `+` or `-` in the time, so `find` and `rfind`
    // give the same index.
    let sign_at = text.find(['+', '-'])?;
    let (time, offset) = text.split_at(sign_at);
    let sign = if offset.starts_with('-') { -1 } else { 1 };
    let offset = &offset[1..];
    // Accept `+HH:MM` and `+HHMM`. RFC 3339 has the colon and the RFC 5322
    // `Date` header does not. JMAP servers send both.
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
        // The leap day and an instant before the epoch: the arithmetic fails
        // most easily at these two points.
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
            // The first and last instants with a four-digit year.
            -62_135_596_800,
            253_402_300_799,
        ] {
            let written = utc_date(seconds).expect("a date inside the grammar");
            assert_eq!(epoch_seconds(&written), Some(seconds), "{written}");
        }
    }

    #[test]
    fn an_instant_no_utc_date_can_name_is_not_written_as_one() {
        // A `UTCDate` has a four-digit year (RFC 8620 §1.4, RFC 3339 §5.6), so
        // no `UTCDate` can write an instant before year 1 or after year 9999.
        // This includes the two ends of the `gint64` range of Camel.
        for seconds in [
            i64::MIN,
            i64::MAX,
            -62_135_596_801,
            253_402_300_800,
            // Year zero, which [`super::YEARS`] excludes.
            -62_167_219_200,
        ] {
            assert_eq!(utc_date(seconds), None, "{seconds}");
        }
    }

    #[test]
    fn every_day_of_a_whole_era_reads_back_as_the_day_it_was_written_for() {
        // 400 years is one full cycle of the calendar. This range covers each
        // century year (leap or not) and each 1 March after it, where the
        // year of `civil_from_days` starts. Some sample instants cannot find
        // an error that occurs only on a few days late in the cycle.
        let first = epoch_seconds("1600-01-01T00:00:00Z").unwrap() / 86_400;
        let last = epoch_seconds("2400-12-31T00:00:00Z").unwrap() / 86_400;
        for day in first..=last {
            let written = utc_date(day * 86_400).expect("a date inside the grammar");
            assert_eq!(epoch_seconds(&written), Some(day * 86_400), "{written}");
        }
    }

    #[test]
    fn no_day_of_year_zero_is_written_as_a_date() {
        // Test each day of year zero, not only the first. The calculation uses
        // March-to-February years, so a wrong era can move the January and
        // February of year zero into year one.
        let year_one = epoch_seconds("0001-01-01T00:00:00Z").unwrap() / 86_400;
        for day in year_one - 366..year_one {
            assert_eq!(utc_date(day * 86_400), None, "day {day}");
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
        // 10:30 at one hour east of UTC is 09:30 UTC, not 11:30.
        assert_eq!(
            epoch_seconds("2026-01-15T10:30:00+01:00"),
            Some(1_768_469_400)
        );
        assert_eq!(
            epoch_seconds("2026-01-15T08:30:00-01:00"),
            Some(1_768_469_400)
        );
        // The colon is optional in practice, and an offset can include
        // minutes: India is at +05:30.
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
    fn the_largest_offset_is_still_an_offset() {
        // RFC 3339 §5.6 `time-numoffset` goes to 23:59 in each direction. The
        // limit refuses larger values and accepts 23:59.
        assert_eq!(
            epoch_seconds("2026-01-16T09:29:00+23:59"),
            Some(1_768_469_400)
        );
        assert_eq!(
            epoch_seconds("2026-01-14T09:31:00-23:59"),
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
        // RFC 3339 §5.6 allows a lower-case `z`.
        assert_eq!(epoch_seconds("1970-01-01T00:00:00z"), Some(0));
    }

    #[test]
    fn what_is_not_a_date_is_not_read_as_one() {
        for text in [
            "",
            "T",
            "yesterday",
            // No time, and no zone: both are necessary.
            "2026-01-15",
            "2026-01-15T09:30:00",
            // Fields that are not numbers, or the wrong number of fields.
            "2026-01T09:30:00Z",
            "2026-01-15T09:30Z",
            "2026-1x-15T09:30:00Z",
            "2026-01-15T09:3x:00Z",
            "2026-01-15T09:30:00+0X:00",
            "2026-01-15T09:30:00+1",
            // Fields that are numbers, but not valid dates or times.
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
            "2026-01-15T09:30:00+00:60",
        ] {
            assert_eq!(epoch_seconds(text), None, "{text:?} is not a date");
        }
    }

    /// The year has four digits, so this reads the range that [`utc_date`]
    /// writes, and no other year gets to the arithmetic.
    ///
    /// The server supplies the text, and `i64::from_str` accepts 19 digits.
    /// `days_from_civil` multiplies the era by 146 097, and `epoch_seconds`
    /// multiplies the days by 86 400. Both overflow before the parse fails,
    /// so the limit makes a bad year a refusal, not an overflow.
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
        // The two limits are dates.
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
