// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pure proleptic Gregorian calendar arithmetic.
//!
//! Howard Hinnant's `days_from_civil`/`civil_from_days` algorithm, shared by
//! any crate that converts between a day count and a year-month-day date. A
//! caller that needs a narrower year range (RFC 8620's four-digit year, for
//! example) checks the year that [`civil_from_days`] returns itself; this
//! module has no notion of such a range on its own.

/// Whether a year has a 29th of February, by the proleptic Gregorian rule.
pub fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// The number of days in `year-month`, or 0 for a month outside 1 to 12.
pub fn days_in_month(year: i64, month: u32) -> u32 {
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
/// The `days_from_civil` algorithm of Howard Hinnant. It starts the year in
/// March, so the leap day is the last day of the year and the month lengths
/// repeat: `(153 * m + 2) / 5` gives that pattern. The result is exact for
/// every year an `i64` can hold without overflowing the era multiplication.
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
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

/// The civil date `days` after 1970-01-01, or `None` if the shifted day count
/// overflows `i64`.
///
/// The `civil_from_days` algorithm of Howard Hinnant, the inverse of
/// [`days_from_civil`]. It starts the year in March, so the leap day is the
/// last day of the year. The result is exact whenever the arithmetic does not
/// overflow; a caller that needs a narrower year range checks the returned
/// year itself.
pub fn civil_from_days(days: i64) -> Option<(i64, u32, u32)> {
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
    Some((year, u32::try_from(month).ok()?, u32::try_from(day).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_is_day_zero() {
        assert_eq!(civil_from_days(0), Some((1970, 1, 1)));
        assert_eq!(days_from_civil(1970, 1, 1), 0);
    }

    #[test]
    fn every_day_of_a_whole_era_reads_back_as_the_day_it_was_written_for() {
        // 400 years is one full cycle of the calendar. This range covers each
        // century year (leap or not) and each 1 March after it, where the
        // year of `civil_from_days` starts.
        let first = days_from_civil(1600, 1, 1);
        let last = days_from_civil(2400, 12, 31);
        for day in first..=last {
            let (year, month, date) = civil_from_days(day).expect("inside i64 range");
            assert_eq!(days_from_civil(year, month, date), day, "day {day}");
        }
    }

    #[test]
    fn a_leap_year_has_a_29th_of_february_and_a_century_usually_does_not() {
        assert!(is_leap_year(2024));
        assert!(is_leap_year(2000));
        assert!(!is_leap_year(1900));
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
    }

    #[test]
    fn an_overflowing_day_count_gives_none_not_a_panic() {
        assert_eq!(civil_from_days(i64::MAX), None);
    }
}
