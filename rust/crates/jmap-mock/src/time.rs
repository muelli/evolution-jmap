// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Date arithmetic for the mock's fixed clock.
//!
//! The mock runs on constant timestamps (`MOCK_NOW`) so tests reproduce, which
//! rules out the system clock and with it any need for a time library: all it
//! ever does is add a hold time to a constant and compare two instants. The
//! two conversions below are the classic civil-from-days/days-from-civil pair
//! (Howard Hinnant's algorithms), enough to do both on RFC 3339 UTC strings.

/// Parse an RFC 3339 UTC date-time (`YYYY-MM-DDTHH:MM:SSZ`, fractional
/// seconds tolerated and dropped) into seconds since the Unix epoch. `None`
/// for anything else, including the offset forms RFC 8620's `UTCDate`
/// forbids.
pub fn parse_utc(date: &str) -> Option<i64> {
    let rest = date.strip_suffix('Z')?;
    let (date_part, time_part) = rest.split_once('T')?;

    let mut ymd = date_part.split('-');
    let year: i64 = ymd.next()?.parse().ok()?;
    let month: u32 = ymd.next()?.parse().ok()?;
    let day: u32 = ymd.next()?.parse().ok()?;
    if ymd.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let time_part = time_part.split('.').next()?;
    let mut hms = time_part.split(':');
    let hour: i64 = hms.next()?.parse().ok()?;
    let minute: i64 = hms.next()?.parse().ok()?;
    let second: i64 = hms.next()?.parse().ok()?;
    if hms.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    Some(days_from_civil(year, month, day) * 86400 + hour * 3600 + minute * 60 + second)
}

/// The inverse of [`parse_utc`]: seconds since the Unix epoch as
/// `YYYY-MM-DDTHH:MM:SSZ`.
pub fn format_utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86400);
    let in_day = seconds.rem_euclid(86400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        in_day / 3600,
        (in_day / 60) % 60,
        in_day % 60
    )
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146097 + day_of_era - 719468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719468;
    let era = days.div_euclid(146097);
    let day_of_era = days - era * 146097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_the_epoch_and_the_mock_clock() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        let now = parse_utc("2026-01-01T00:00:00Z").unwrap();
        assert_eq!(format_utc(now), "2026-01-01T00:00:00Z");
        assert_eq!(format_utc(now + 600), "2026-01-01T00:10:00Z");
    }

    #[test]
    fn crosses_a_leap_day() {
        let before = parse_utc("2024-02-28T23:59:59Z").unwrap();
        assert_eq!(format_utc(before + 1), "2024-02-29T00:00:00Z");
        assert_eq!(format_utc(before + 1 + 86400), "2024-03-01T00:00:00Z");
    }

    #[test]
    fn tolerates_fractional_seconds_and_refuses_offsets() {
        assert_eq!(
            parse_utc("2026-01-01T00:00:00.123Z"),
            parse_utc("2026-01-01T00:00:00Z")
        );
        assert_eq!(parse_utc("2026-01-01T00:00:00+02:00"), None);
        assert_eq!(parse_utc("2026-01-01 00:00:00Z"), None);
        assert_eq!(parse_utc("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_utc("2026-01-01T24:00:00Z"), None);
    }
}
