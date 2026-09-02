// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A preset as seconds of `HOLDFOR`, on the local clock.
//!
//! Presets rather than a date-time picker, deliberately: RFC 4865's
//! `HOLDFOR` is relative, three fixed moments cover the everyday cases the
//! way other mail clients' schedulers do, and a picker would drag a calendar
//! widget into the binding surface for its first version. GLib's `GDateTime`
//! does the local-calendar arithmetic — "tomorrow 08:00" is a calendar fact
//! (DST included), not `now + 24h`.

use glib_sys::{
    GDateTime, g_date_time_add_days, g_date_time_get_day_of_month, g_date_time_get_day_of_week,
    g_date_time_get_month, g_date_time_get_year, g_date_time_new_local, g_date_time_new_now_local,
    g_date_time_to_unix, g_date_time_unref,
};

/// When a held message should go out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    InOneHour,
    TomorrowMorning,
    MondayMorning,
}

/// Mornings start at eight, the convention every scheduler shares.
const MORNING: i32 = 8;

/// The seconds the server should hold a message for `preset`, from now.
/// `None` when the local calendar refuses (a clock so broken that
/// `GDateTime` cannot represent it) — the caller reports rather than sends.
pub fn hold_seconds(preset: Preset) -> Option<u64> {
    // SAFETY: no arguments; the result is checked and released below.
    let now = unsafe { g_date_time_new_now_local() };
    if now.is_null() {
        return None;
    }
    let held = match preset {
        Preset::InOneHour => Some(3600),
        // SAFETY: `now` is the live GDateTime just built.
        Preset::TomorrowMorning => unsafe { until_morning(now, 1) },
        Preset::MondayMorning => {
            // SAFETY: as above; ISO weekday, 1 = Monday … 7 = Sunday, so a
            // Monday schedules for the *next* one.
            let weekday = unsafe { g_date_time_get_day_of_week(now) };
            // SAFETY: as above.
            unsafe { until_morning(now, 8 - weekday) }
        }
    };
    // SAFETY: releasing the reference this function took.
    unsafe { g_date_time_unref(now) };
    held
}

/// Seconds from `now` until `days` ahead at [`MORNING`] local time.
///
/// # Safety
///
/// `now` must be a live `GDateTime`.
unsafe fn until_morning(now: *mut GDateTime, days: i32) -> Option<u64> {
    // SAFETY: `now` per this function's contract; every derived GDateTime is
    // checked and released on each path.
    unsafe {
        let day = g_date_time_add_days(now, days);
        if day.is_null() {
            return None;
        }
        let target = g_date_time_new_local(
            g_date_time_get_year(day),
            g_date_time_get_month(day),
            g_date_time_get_day_of_month(day),
            MORNING,
            0,
            0.0,
        );
        g_date_time_unref(day);
        if target.is_null() {
            return None;
        }
        let hold = g_date_time_to_unix(target) - g_date_time_to_unix(now);
        g_date_time_unref(target);
        u64::try_from(hold).ok().filter(|&seconds| seconds > 0)
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
            glib_sys::g_date_time_to_unix(now) + i64::try_from(seconds).ok()?,
        );
        g_date_time_unref(now);
        if target.is_null() {
            return None;
        }
        // The explicit pattern rather than format_iso8601 (which glib-sys
        // gates behind a newer-GLib feature): `target` is UTC by
        // construction, so the literal Z is the truth.
        let raw = glib_sys::g_date_time_format(target, c"%Y-%m-%dT%H:%M:%SZ".as_ptr());
        g_date_time_unref(target);
        let formatted = jmap_backend_core::marshal::read_string(raw);
        glib_sys::g_free(raw.cast());
        formatted
    }
}

#[cfg(test)]
mod tests {
    use glib_sys::{
        g_date_time_get_day_of_week, g_date_time_get_hour, g_date_time_get_minute,
        g_date_time_new_from_unix_local, g_date_time_new_now_local, g_date_time_to_unix,
        g_date_time_unref,
    };

    use super::*;

    /// The moment `hold` seconds from now lands at, as (weekday, hour, minute).
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
        assert_eq!(hold_seconds(Preset::InOneHour), Some(3600));
    }

    #[test]
    fn tomorrow_morning_lands_at_eight() {
        let hold = hold_seconds(Preset::TomorrowMorning).unwrap();
        assert!(
            hold > 0 && hold <= 48 * 3600,
            "not within a day-and-DST: {hold}"
        );
        let (_, hour, minute) = lands_at(hold);
        assert_eq!((hour, minute), (8, 0));
    }

    #[test]
    fn monday_morning_lands_on_a_monday_at_eight() {
        let hold = hold_seconds(Preset::MondayMorning).unwrap();
        assert!(hold > 0 && hold <= 8 * 24 * 3600);
        let (weekday, hour, minute) = lands_at(hold);
        assert_eq!((weekday, hour, minute), (1, 8, 0));
    }

    /// The string `snoozed.until` gets: RFC 8620's `UTCDate` ends in `Z`,
    /// which is the part `g_date_time_format_iso8601` only does for a UTC
    /// GDateTime.
    #[test]
    fn utc_in_writes_a_z_terminated_utc_date() {
        let stamp = utc_in(600).unwrap();
        assert!(stamp.ends_with('Z'), "not UTC-suffixed: {stamp}");
        assert!(stamp.len() >= 20, "not a full date-time: {stamp}");
    }
}
