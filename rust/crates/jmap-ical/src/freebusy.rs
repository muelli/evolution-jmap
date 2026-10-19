// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Translates JMAP availability to iCalendar `VFREEBUSY` components.
//!
//! This module converts `Principal/getAvailability` [`BusyPeriod`] items
//! (draft-ietf-jmap-calendars Section 2.2) to `VFREEBUSY` components.
//! It omits `UID` and `DTSTAMP` per EDS `ecmb_get_free_busy_sync`.
//! Invalid periods reject the entire component to avoid false availability.

use jmap_proto::principals::BusyPeriod;
use jmap_proto::state::UtcDate;

use crate::event::{Component, EntryExt, make_entry, to_utc_date_time};

/// Returns the RFC 5545 Section 3.2.9 `FBTYPE` parameter for a busy status.
///
/// Maps known statuses to `FBTYPE` values; unknown values map to `BUSY`.
pub fn free_busy_type(busy_status: &str) -> &'static str {
    match busy_status {
        "tentative" => "BUSY-TENTATIVE",
        "unavailable" => "BUSY-UNAVAILABLE",
        // Confirmed and other draft statuses map to BUSY.
        _ => "BUSY",
    }
}

/// Formats attendee busy periods as a bare `VFREEBUSY` component.
///
/// `attendee` is the email address with or without a `mailto:` prefix.
/// `utc_start` and `utc_end` define the requested query window.
/// Returns `None` if any date-time value cannot be parsed.
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

/// Converts a JMAP `UtcDate` to an RFC 5545 UTC `DATE-TIME`, removing fractional seconds.
pub fn instant(value: &UtcDate) -> Option<String> {
    let value = value.as_str();
    let Some((seconds, fraction)) = value.split_once('.') else {
        return to_utc_date_time(value);
    };
    // The fraction runs from the period to the trailing zone designator Z.
    let (digits, zone) = fraction.split_at_checked(fraction.len().checked_sub(1)?)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    to_utc_date_time(&format!("{seconds}{zone}"))
}

/// Formats the address as a `mailto:` URI.
pub fn mailto(attendee: &str) -> String {
    let address = attendee
        .split_at_checked("mailto:".len())
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("mailto:"))
        .map_or(attendee, |(_, rest)| rest);
    format!("mailto:{address}")
}
