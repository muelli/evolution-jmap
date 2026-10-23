// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! When a held message must go out: the moments that the menu offers. The
//! code calculates each moment from the local calendar at each request.
//!
//! The menu is *not* a fixed list of relative offsets, on purpose. A person
//! wants "this evening" or "early tomorrow", and these are calendar facts.
//! The number of seconds to 18:00 changes at each moment of the day. Across a
//! DST change, "tomorrow 08:00" is not `now + 24h`. Thus each item names a
//! time of day (or the next working day). The code changes it into seconds
//! from now at the moment of the click. Thus a composer that stays open past
//! one of its suggestions still gives a correct result: the target moves to
//! the next occurrence and not into the past.
//!
//! The set of times follows Signal. Signal offers the next 08:00, 12:00,
//! 18:00 and 21:00. This module adds one time that email needs and a
//! messenger does not: the next *working* day at 09:00. Thus on a Friday
//! evening, "next workday" is Monday, not Saturday.
//!
//! GLib's `GDateTime` does all the arithmetic. This module reads the system
//! clock only through `GDateTime`.

use glib_sys::{
    GDateTime, g_date_time_add_days, g_date_time_format, g_date_time_get_day_of_month,
    g_date_time_get_day_of_week, g_date_time_get_month, g_date_time_get_year,
    g_date_time_new_local, g_date_time_new_now_local, g_date_time_to_unix, g_date_time_unref,
    g_free,
};
use jmap_backend_core::i18n::{N_, translate, translate_with};
use jmap_backend_core::marshal::read_string;

/// One offered moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// One hour from now: the only relative choice. People use it to mean
    /// "not immediately".
    InOneHour,
    /// The next time the local clock shows this hour, today or tomorrow.
    TimeOfDay(i32),
    /// [`WORKDAY_HOUR`] on the next day that is not a Saturday or Sunday.
    /// Today is included if that hour is still ahead.
    NextWorkday,
}

/// The times of day that the menu suggests, in menu order.
pub const TIMES_OF_DAY: &[i32] = &[8, 12, 18, 21];

/// The hour at which a working day starts.
pub const WORKDAY_HOUR: i32 = 9;

/// All presets that the menu offers, in order.
pub fn offered() -> Vec<Preset> {
    let mut presets = vec![Preset::InOneHour];
    presets.extend(TIMES_OF_DAY.iter().map(|hour| Preset::TimeOfDay(*hour)));
    presets.push(Preset::NextWorkday);
    presets
}

/// The stable action-name suffix for `preset`. With this name, one
/// `activate` handler finds the clicked moment, and each item does not need
/// its own trampoline.
pub fn slug(preset: Preset) -> String {
    match preset {
        Preset::InOneHour => "hour".to_owned(),
        Preset::TimeOfDay(hour) => format!("at-{hour}"),
        Preset::NextWorkday => "workday".to_owned(),
    }
}

/// The inverse of [`slug`].
pub fn from_slug(slug: &str) -> Option<Preset> {
    match slug {
        "hour" => Some(Preset::InOneHour),
        "workday" => Some(Preset::NextWorkday),
        other => other
            .strip_prefix("at-")?
            .parse()
            .ok()
            .map(Preset::TimeOfDay),
    }
}

/// A resolved preset: how long to hold, and the current name of that moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    /// Seconds from now: the unit of the RFC 4865 `HOLDFOR`.
    pub hold: u64,
    /// The menu label, for example `Today, 18:00`, `Tomorrow, 08:00`,
    /// `Monday, 09:00`, or `In One _Hour` for the relative preset.
    pub label: String,
}

/// Resolves `preset` against the local clock now. Returns `None` only if
/// `GDateTime` cannot calculate or format the result. This means a defective
/// clock, and the caller must report an error and not send.
pub fn resolve(preset: Preset) -> Option<Occurrence> {
    if preset == Preset::InOneHour {
        return Some(Occurrence {
            hold: 3600,
            label: translate(N_(c"In One _Hour")),
        });
    }

    // SAFETY: no arguments. The code checks each GDateTime that it makes and
    // releases it on each path.
    unsafe {
        let now = g_date_time_new_now_local();
        if now.is_null() {
            return None;
        }
        let target = match preset {
            Preset::InOneHour => unreachable!("handled above"),
            Preset::TimeOfDay(hour) => next_at_hour(now, hour),
            Preset::NextWorkday => next_workday(now),
        };
        let occurrence = target.and_then(|target| {
            let hold = g_date_time_to_unix(target) - g_date_time_to_unix(now);
            let label = day_label(now, target);
            g_date_time_unref(target);
            u64::try_from(hold)
                .ok()
                .filter(|seconds| *seconds > 0)
                .zip(label)
                .map(|(hold, label)| Occurrence { hold, label })
        });
        g_date_time_unref(now);
        occurrence
    }
}

/// The next moment the clock shows `hour`:00: today if that moment is still
/// ahead, else tomorrow.
///
/// # Safety
///
/// `now` must be a live `GDateTime`. The caller owns the result.
unsafe fn next_at_hour(now: *mut GDateTime, hour: i32) -> Option<*mut GDateTime> {
    // SAFETY: the contract makes `now` live; `at_hour` checks its own result.
    unsafe {
        let today = at_hour(now, 0, hour)?;
        if g_date_time_to_unix(today) > g_date_time_to_unix(now) {
            return Some(today);
        }
        g_date_time_unref(today);
        at_hour(now, 1, hour)
    }
}

/// [`WORKDAY_HOUR`] on the next day from Monday to Friday. Today is included
/// if that hour has not passed.
///
/// # Safety
///
/// As [`next_at_hour`].
unsafe fn next_workday(now: *mut GDateTime) -> Option<*mut GDateTime> {
    // SAFETY: the contract makes `now` live. Any seven days contain a weekday,
    // thus the loop always finds one.
    unsafe {
        for ahead in 0..7 {
            let candidate = at_hour(now, ahead, WORKDAY_HOUR)?;
            // ISO weekday: 1 = Monday … 7 = Sunday.
            let weekday = g_date_time_get_day_of_week(candidate);
            let is_workday = (1..=5).contains(&weekday);
            if is_workday && g_date_time_to_unix(candidate) > g_date_time_to_unix(now) {
                return Some(candidate);
            }
            g_date_time_unref(candidate);
        }
        None
    }
}

/// `now` moved `days` ahead, at `hour`:00:00 local time.
///
/// # Safety
///
/// As [`next_at_hour`].
unsafe fn at_hour(now: *mut GDateTime, days: i32, hour: i32) -> Option<*mut GDateTime> {
    // SAFETY: the contract makes `now` live; the code releases the
    // intermediate day.
    unsafe {
        let day = g_date_time_add_days(now, days);
        if day.is_null() {
            return None;
        }
        let built = g_date_time_new_local(
            g_date_time_get_year(day),
            g_date_time_get_month(day),
            g_date_time_get_day_of_month(day),
            hour,
            0,
            0.0,
        );
        g_date_time_unref(day);
        (!built.is_null()).then_some(built)
    }
}

/// The name of `target` relative to `now`: today, tomorrow, or the weekday.
///
/// # Safety
///
/// Both arguments must be live `GDateTime`s.
unsafe fn day_label(now: *mut GDateTime, target: *mut GDateTime) -> Option<String> {
    // SAFETY: the contract makes both live; the code frees each formatted
    // string.
    let time = unsafe {
        // `%R` is 24-hour HH:MM. The locale-aware formatter of Evolution
        // (`e-datetime-format.h`) supports 12-hour locales, but it needs a
        // large new binding for four labels. Thus this simplification is on purpose.
        let raw = g_date_time_format(target, c"%R".as_ptr());
        let formatted = read_string(raw);
        g_free(raw.cast());
        formatted?
    };

    // SAFETY: as above.
    let (today, tomorrow) = unsafe {
        let today = day_number(now);
        let next = g_date_time_add_days(now, 1);
        let tomorrow = (!next.is_null()).then(|| {
            let number = day_number(next);
            g_date_time_unref(next);
            number
        });
        (today, tomorrow)
    };
    // SAFETY: as above.
    let target_day = unsafe { day_number(target) };

    if target_day == today {
        // TRANSLATORS: %1$s is a time of day, e.g. "18:00".
        return Some(translate_with(N_(c"Today, %1$s"), &[&time]));
    }
    if Some(target_day) == tomorrow {
        // TRANSLATORS: %1$s is a time of day, e.g. "08:00".
        return Some(translate_with(N_(c"Tomorrow, %1$s"), &[&time]));
    }
    // SAFETY: as above; `%A` is the full weekday name of the locale.
    let weekday = unsafe {
        let raw = g_date_time_format(target, c"%A".as_ptr());
        let formatted = read_string(raw);
        g_free(raw.cast());
        formatted?
    };
    // TRANSLATORS: %1$s is a weekday name, %2$s a time of day.
    Some(translate_with(N_(c"%1$s, %2$s"), &[&weekday, &time]))
}

/// A date as one number, to find if two moments are on the same day.
///
/// # Safety
///
/// `moment` must be a live `GDateTime`.
unsafe fn day_number(moment: *mut GDateTime) -> i64 {
    // SAFETY: the contract makes `moment` live.
    unsafe {
        i64::from(g_date_time_get_year(moment)) * 10_000
            + i64::from(g_date_time_get_month(moment)) * 100
            + i64::from(g_date_time_get_day_of_month(moment))
    }
}

/// Now plus `seconds`, as the RFC 8620 `UTCDate` string that a JMAP property
/// takes. Snooze writes this value into `snoozed.until`.
pub fn utc_in(seconds: u64) -> Option<String> {
    // Convert before `now` exists, so that no early return leaks it.
    let seconds = i64::try_from(seconds).ok()?;
    // SAFETY: plain GLib calendar calls; the code releases each object and
    // string.
    unsafe {
        let now = glib_sys::g_date_time_new_now_utc();
        if now.is_null() {
            return None;
        }
        let target = glib_sys::g_date_time_new_from_unix_utc(
            g_date_time_to_unix(now).saturating_add(seconds),
        );
        g_date_time_unref(now);
        if target.is_null() {
            return None;
        }
        // An explicit pattern, because glib-sys gives format_iso8601 only
        // with a newer-GLib feature. `target` is UTC, thus the literal Z is
        // correct.
        let raw = g_date_time_format(target, c"%Y-%m-%dT%H:%M:%SZ".as_ptr());
        g_date_time_unref(target);
        let formatted = read_string(raw);
        g_free(raw.cast());
        formatted
    }
}

#[cfg(test)]
mod tests {
    use glib_sys::{
        g_date_time_get_hour, g_date_time_get_minute, g_date_time_new_from_unix_local,
        g_date_time_new_now_local,
    };

    use super::*;

    /// The time `hold` seconds from now, as (weekday, hour, minute).
    fn lands_at(hold: u64) -> (i32, i32, i32) {
        // SAFETY: plain GLib calendar calls; the code releases each object.
        unsafe {
            let now = g_date_time_new_now_local();
            let target = g_date_time_new_from_unix_local(g_date_time_to_unix(now) + hold as i64);
            let landed = (
                g_date_time_get_day_of_week(target),
                g_date_time_get_hour(target),
                g_date_time_get_minute(target),
            );
            g_date_time_unref(target);
            g_date_time_unref(now);
            landed
        }
    }

    #[test]
    fn one_hour_is_exactly_that() {
        let occurrence = resolve(Preset::InOneHour).unwrap();
        assert_eq!(occurrence.hold, 3600);
    }

    /// Each offered time of day resolves to that hour, on the hour. The result
    /// is always in the future and at most 25 hours ahead (DST included).
    #[test]
    fn each_time_of_day_lands_on_its_own_hour() {
        for wanted in TIMES_OF_DAY {
            let occurrence = resolve(Preset::TimeOfDay(*wanted)).unwrap();
            assert!(
                occurrence.hold > 0 && occurrence.hold <= 25 * 3600,
                "{wanted}:00 resolved {} seconds out",
                occurrence.hold
            );
            let (_, hour, minute) = lands_at(occurrence.hold);
            assert_eq!((hour, minute), (*wanted, 0), "for {wanted}:00");
        }
    }

    /// The next workday is a day from Monday to Friday at [`WORKDAY_HOUR`],
    /// never a weekend day, and always ahead.
    #[test]
    fn the_next_workday_is_never_a_weekend() {
        let occurrence = resolve(Preset::NextWorkday).unwrap();
        assert!(occurrence.hold > 0 && occurrence.hold <= 4 * 24 * 3600);
        let (weekday, hour, minute) = lands_at(occurrence.hold);
        assert!((1..=5).contains(&weekday), "landed on weekday {weekday}");
        assert_eq!((hour, minute), (WORKDAY_HOUR, 0));
    }

    /// Each label names its day, thus a stale menu is still clear.
    #[test]
    fn labels_name_the_day() {
        for preset in offered() {
            let label = resolve(preset).unwrap().label;
            assert!(!label.is_empty(), "{preset:?} has no label");
            if preset != Preset::InOneHour {
                assert!(label.contains(','), "{preset:?} label lacks a day: {label}");
            }
        }
    }

    /// Each preset survives the round trip through its action name. This lets
    /// one handler serve the full submenu.
    #[test]
    fn slugs_round_trip() {
        for preset in offered() {
            assert_eq!(from_slug(&slug(preset)), Some(preset), "for {preset:?}");
        }
        assert_eq!(from_slug("nonsense"), None);
        assert_eq!(from_slug("at-notanumber"), None);
    }

    /// The menu offers the relative hour first, then the times of day in
    /// order, then the next workday.
    #[test]
    fn the_menu_offers_every_preset_in_order() {
        assert_eq!(
            offered(),
            vec![
                Preset::InOneHour,
                Preset::TimeOfDay(8),
                Preset::TimeOfDay(12),
                Preset::TimeOfDay(18),
                Preset::TimeOfDay(21),
                Preset::NextWorkday,
            ]
        );
    }

    /// `utc_in` names a moment ahead of now. The fixed-width UTC pattern
    /// sorts as the time does, thus a string comparison is sufficient.
    #[test]
    fn utc_in_names_a_moment_ahead_of_now() {
        let now = utc_in(0).unwrap();
        let later = utc_in(600).unwrap();
        assert!(later > now, "{later} is not after {now}");
    }

    /// A delay past any date that GLib can hold gives no date, and no panic.
    #[test]
    fn utc_in_gives_nothing_for_a_delay_past_every_date() {
        assert_eq!(utc_in(u64::MAX), None);
        assert_eq!(utc_in(i64::MAX as u64), None);
    }

    /// The string for `snoozed.until`: an RFC 8620 `UTCDate` ends in `Z`.
    #[test]
    fn utc_in_writes_a_z_terminated_utc_date() {
        let stamp = utc_in(600).unwrap();
        assert!(stamp.ends_with('Z'), "not UTC-suffixed: {stamp}");
        assert!(stamp.len() >= 20, "not a full date-time: {stamp}");
    }
}
