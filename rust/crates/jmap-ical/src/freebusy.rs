// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `BusyPeriod` -> `VFREEBUSY`: translates JMAP availability to iCalendar.
//!
//! Translates `Principal/getAvailability` [`BusyPeriod`]s (draft-ietf-jmap-calendars §2.2)
//! to the bare `VFREEBUSY` component EDS's `ECalBackendSync::get_free_busy_sync` expects.
//!
//! Omits `UID` and `DTSTAMP` per `ecmb_get_free_busy_sync` to keep the mapping pure.
//! Unreadable busy periods refuse the entire component to prevent falsely booking busy time.

use jmap_proto::principals::BusyPeriod;
use jmap_proto::state::UtcDate;

use crate::event::{Component, EntryExt, make_entry, to_utc_date_time};

/// The RFC 5545 §3.2.9 `FBTYPE` for a `BusyPeriod.busyStatus`.
///
/// Maps known busy statuses to their `FBTYPE` equivalent; unknown values map to `BUSY`.
pub fn free_busy_type(busy_status: &str) -> &'static str {
    match busy_status {
        "tentative" => "BUSY-TENTATIVE",
        "unavailable" => "BUSY-UNAVAILABLE",
        // "confirmed", and anything a future draft adds.
        _ => "BUSY",
    }
}

/// One attendee's busy periods as a bare `VFREEBUSY` component.
///
/// `attendee` is the address EDS asked about (bare or with `mailto:`).
/// `utc_start` and `utc_end` define the requested window.
/// Returns `None` if the window or any period cannot be parsed as a UTC date-time.
pub fn busy_periods_to_vfreebusy(
    attendee: &str,
    utc_start: &UtcDate,
    utc_end: &UtcDate,
    periods: &[BusyPeriod],
) -> Option<String> {
    let mut vfreebusy = Component::new("VFREEBUSY")
        .with(make_entry("DTSTART", &instant(utc_start)?))
        .with(make_entry("DTEND", &instant(utc_end)?))
        .with(make_entry("ATTENDEE", &mailto(attendee)));

    for period in periods {
        let start = instant(&period.utc_start)?;
        let end = instant(&period.utc_end)?;
        vfreebusy = vfreebusy.with(
            make_entry("FREEBUSY", &format!("{start}/{end}"))
                .with_named_param("FBTYPE", free_busy_type(&period.busy_status)),
        );
    }

    Some(vfreebusy.to_ics())
}

/// A JMAP `UTCDate` as RFC 5545's UTC `DATE-TIME`, truncating fractional seconds.
pub fn instant(value: &UtcDate) -> Option<String> {
    let value = value.as_str();
    let Some((seconds, fraction)) = value.split_once('.') else {
        return to_utc_date_time(value);
    };
    // The fraction runs from the `.` to the zone designator, which for a
    // `UTCDate` is the single character `Z`.
    let (digits, zone) = fraction.split_at_checked(fraction.len().checked_sub(1)?)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    to_utc_date_time(&format!("{seconds}{zone}"))
}

/// Formats the address as a `mailto:` URI for EDS backends.
pub fn mailto(attendee: &str) -> String {
    let address = attendee
        .split_at_checked("mailto:".len())
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("mailto:"))
        .map_or(attendee, |(_, rest)| rest);
    format!("mailto:{address}")
}
