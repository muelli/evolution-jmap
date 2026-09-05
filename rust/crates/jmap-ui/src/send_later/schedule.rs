// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! When a held message should go out: the moments the menu offers, worked
//! out against the local calendar every time they are asked for.
//!
//! The menu is deliberately *not* a fixed list of relative offsets. What a
//! person wants is "this evening" or "first thing tomorrow", and those are
//! calendar facts — 18:00 is a different number of seconds away at every
//! moment of the day, and "tomorrow 08:00" is not `now + 24h` across a DST
//! boundary. So each entry names a time of day (or the next working day) and
//! is resolved to seconds-from-now at the instant it is clicked, which also
//! means a composer left open past one of its own suggestions still does the
//! right thing: the target rolls to the next occurrence rather than landing
//! in the past.
//!
//! The set of times follows Signal's, which offers the next 08:00, 12:00,
//! 18:00 and 21:00, plus the one email wants that a messenger does not: the
//! next *working* day at 09:00, so a Friday evening's "next workday" is
//! Monday and not Saturday.
//!
//! GLib's `GDateTime` does all the arithmetic; nothing here reads the system
//! clock except through it.

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
    /// A flat hour from now — the one genuinely relative choice, and the one
    /// people reach for to mean "not right this second".
    InOneHour,
    /// The next time the local clock reads this hour, today or tomorrow.
    TimeOfDay(i32),
    /// [`WORKDAY_HOUR`] on the next day that is not a Saturday or Sunday,
    /// today included if that hour is still ahead.
    NextWorkday,
}

/// The times of day the menu suggests, in the order it lists them.
pub const TIMES_OF_DAY: &[i32] = &[8, 12, 18, 21];

/// When a working day is taken to start.
pub const WORKDAY_HOUR: i32 = 9;

/// Every preset the menu offers, in order.
pub fn offered() -> Vec<Preset> {
    let mut presets = vec![Preset::InOneHour];
    presets.extend(TIMES_OF_DAY.iter().map(|hour| Preset::TimeOfDay(*hour)));
    presets.push(Preset::NextWorkday);
    presets
}

/// The stable action-name suffix for `preset` — what the menu item is known
/// by, so one `activate` handler can recover which moment was clicked
/// instead of needing a trampoline each.
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

/// A resolved preset: how long to hold, and how to name that moment now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    /// Seconds from now, which is what RFC 4865's `HOLDFOR` takes.
    pub hold: u64,
    /// The menu label, e.g. `Today, 18:00`, `Tomorrow, 08:00`, `Monday,
    /// 09:00` — or `In One _Hour` for the relative one.
    pub label: String,
}

/// Resolve `preset` against the local clock now. `None` only when
/// `GDateTime` cannot represent the result, which means a clock so broken
/// that the caller should report rather than send.
pub fn resolve(preset: Preset) -> Option<Occurrence> {
    if preset == Preset::InOneHour {
        return Some(Occurrence {
            hold: 3600,
            label: translate(N_(c"In One _Hour")),
        });
    }

    // SAFETY: no arguments; every GDateTime built here is checked and
    // released on each path.
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

/// The next moment the clock reads `hour`:00 — today if that is still
/// ahead, else tomorrow.
///
/// # Safety
///
/// `now` must be a live `GDateTime`. The result is owned by the caller.
unsafe fn next_at_hour(now: *mut GDateTime, hour: i32) -> Option<*mut GDateTime> {
    // SAFETY: `now` per the contract; `at_hour` checks its own result.
    unsafe {
        let today = at_hour(now, 0, hour)?;
        if g_date_time_to_unix(today) > g_date_time_to_unix(now) {
            return Some(today);
        }
        g_date_time_unref(today);
        at_hour(now, 1, hour)
    }
}

/// [`WORKDAY_HOUR`] on the next Monday-to-Friday, today included if that
/// hour has not passed.
///
/// # Safety
///
/// As [`next_at_hour`].
unsafe fn next_workday(now: *mut GDateTime) -> Option<*mut GDateTime> {
    // SAFETY: `now` per the contract. Seven days is enough to find a
    // weekday from any starting point, so the loop always terminates.
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

/// `now` shifted `days` ahead, at `hour`:00:00 local.
///
/// # Safety
///
/// As [`next_at_hour`].
unsafe fn at_hour(now: *mut GDateTime, days: i32, hour: i32) -> Option<*mut GDateTime> {
    // SAFETY: `now` per the contract; the intermediate day is released.
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

/// `target` named relative to `now`: today, tomorrow, or by weekday.
///
/// # Safety
///
/// Both must be live `GDateTime`s.
unsafe fn day_label(now: *mut GDateTime, target: *mut GDateTime) -> Option<String> {
    // SAFETY: both live per the contract; each formatted string is freed.
    let time = unsafe {
        // `%R` is 24-hour HH:MM. Evolution has a locale-aware formatter
        // (`e-datetime-format.h`) that would respect a 12-hour locale; it is
        // a whole further binding surface for four labels, so this is the
        // deliberate simplification.
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
    // SAFETY: as above; `%A` is the locale's full weekday name.
    let weekday = unsafe {
        let raw = g_date_time_format(target, c"%A".as_ptr());
        let formatted = read_string(raw);
        g_free(raw.cast());
        formatted?
    };
    // TRANSLATORS: %1$s is a weekday name, %2$s a time of day.
    Some(translate_with(N_(c"%1$s, %2$s"), &[&weekday, &time]))
}

/// A date as one comparable number, for "is this the same day".
///
/// # Safety
///
/// `moment` must be a live `GDateTime`.
unsafe fn day_number(moment: *mut GDateTime) -> i64 {
    // SAFETY: live per the contract.
    unsafe {
        i64::from(g_date_time_get_year(moment)) * 10_000
            + i64::from(g_date_time_get_month(moment)) * 100
            + i64::from(g_date_time_get_day_of_month(moment))
    }
}

/// Now plus `seconds`, as the RFC 8620 `UTCDate` string a JMAP property
/// takes — what snooze writes into `snoozed.until`.
pub fn utc_in(seconds: u64) -> Option<String> {
    // SAFETY: plain GLib calendar calls; every object and string released.
    unsafe {
        let now = glib_sys::g_date_time_new_now_utc();
        if now.is_null() {
            return None;
        }
        let target = glib_sys::g_date_time_new_from_unix_utc(
            g_date_time_to_unix(now) + i64::try_from(seconds).ok()?,
        );
        g_date_time_unref(now);
        if target.is_null() {
            return None;
        }
        // The explicit pattern rather than format_iso8601 (which glib-sys
        // gates behind a newer-GLib feature): `target` is UTC by
        // construction, so the literal Z is the truth.
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

    /// Where `hold` seconds from now lands, as (weekday, hour, minute).
    fn lands_at(hold: u64) -> (i32, i32, i32) {
        // SAFETY: plain GLib calendar calls; every object released.
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

    /// Every offered time of day resolves to that hour, on the hour, always
    /// in the future and never more than a day out.
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

    /// The next workday is a Monday-to-Friday at [`WORKDAY_HOUR`], never a
    /// weekend, and always ahead.
    #[test]
    fn the_next_workday_is_never_a_weekend() {
        let occurrence = resolve(Preset::NextWorkday).unwrap();
        assert!(occurrence.hold > 0 && occurrence.hold <= 4 * 24 * 3600);
        let (weekday, hour, minute) = lands_at(occurrence.hold);
        assert!((1..=5).contains(&weekday), "landed on weekday {weekday}");
        assert_eq!((hour, minute), (WORKDAY_HOUR, 0));
    }

    /// Labels say which day they mean, so a stale menu is still readable.
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

    /// Every preset survives the round trip through its action name, which is
    /// what lets one handler serve the whole submenu.
    #[test]
    fn slugs_round_trip() {
        for preset in offered() {
            assert_eq!(from_slug(&slug(preset)), Some(preset), "for {preset:?}");
        }
        assert_eq!(from_slug("nonsense"), None);
        assert_eq!(from_slug("at-notanumber"), None);
    }

    /// The string `snoozed.until` gets: RFC 8620's `UTCDate` ends in `Z`.
    #[test]
    fn utc_in_writes_a_z_terminated_utc_date() {
        let stamp = utc_in(600).unwrap();
        assert!(stamp.ends_with('Z'), "not UTC-suffixed: {stamp}");
        assert!(stamp.len() >= 20, "not a full date-time: {stamp}");
    }
}
