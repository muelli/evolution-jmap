// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `BusyPeriod` → `VFREEBUSY`: what a meeting scheduler is told about an
//! attendee's time.
//!
//! JMAP answers "when is this person busy?" with `Principal/getAvailability`
//! (draft-ietf-jmap-calendars §2.2) and a list of [`BusyPeriod`]s. EDS asks
//! the question through `ECalBackendSync::get_free_busy_sync` and wants the
//! answer as iCalendar text. This module is the join.
//!
//! ## The shape is the incumbents', not ours
//!
//! Three implementations in the EDS/evolution-ews trees answer that vfunc, and
//! they agree on the envelope, so this one copies it rather than inventing a
//! fourth: **one bare `VFREEBUSY` component per person**, rendered to a string,
//! carrying an `ATTENDEE` that names whose time it is and one `FREEBUSY`
//! property per busy interval. `ECalMetaBackend`'s own
//! `ecmb_get_free_busy_sync` also states the requested window as `DTSTART`/
//! `DTEND`, which the Microsoft 365 backend omits; it is stated here, because
//! without it a component with no `FREEBUSY` line cannot be told apart from a
//! component about some other window.
//!
//! Deliberately absent, following `ecmb_get_free_busy_sync`: `UID` and
//! `DTSTAMP`. Both would need a clock or a random source, and this crate has
//! neither — it is a pure mapping, which is what makes it testable everywhere
//! the workspace builds.
//!
//! ## Why the answer is an `Option`
//!
//! Everywhere else in this crate an unreadable value is dropped and the rest
//! of the object survives, because a lost property is better than a lost
//! event. Free/busy inverts that. A busy period this code cannot read, if
//! dropped, renders the attendee **free** for a time they are not — and a
//! scheduler does not merely display that, it books it. So a period that
//! cannot be read refuses the whole component, and the attendee's row stays
//! blank: "we do not know" instead of a confident wrong answer.

use jmap_proto::principals::BusyPeriod;
use jmap_proto::state::UtcDate;

use crate::event::{Component, EntryExt, make_entry, to_utc_date_time};

/// The RFC 5545 §3.2.9 `FBTYPE` a draft `BusyPeriod.busyStatus` means.
///
/// `getAvailability` returns busy periods only — "free" is not one of its
/// values — so every status it can carry maps onto a *busy* type, and so does
/// every status it cannot. An unknown value is the one this has to get right:
/// a later draft revision adding a fourth status must not read as free time
/// that a meeting can be dropped into.
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
/// `attendee` is the address EDS asked about — bare, as the vfunc's `users`
/// list holds it; a `mailto:` already on the front is tolerated rather than
/// doubled. `utc_start`/`utc_end` are the window that was asked about, not the
/// extent of the periods: they are restated so the component says which
/// question it answers.
///
/// `None` if the window or any period cannot be read as a UTC date-time — see
/// the module docs for why that is a refusal and not a filter.
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

/// A JMAP `UTCDate` as RFC 5545's UTC `DATE-TIME`.
///
/// [`to_utc_date_time`] is the whole of it apart from one tolerance: RFC 3339
/// §5.6 admits a fractional second that neither RFC 8984's `UTCDateTime` nor
/// RFC 5545's `DATE-TIME` carries, and a server that sends one anyway should
/// cost the digits, not the attendee's whole answer — which, under this
/// module's refusal rule, is what dropping the period would cost.
fn instant(value: &UtcDate) -> Option<String> {
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

/// The address as a `mailto:` URI, which is how all three EDS backends name
/// the subject of a `VFREEBUSY` and therefore what reads it back.
fn mailto(attendee: &str) -> String {
    let address = attendee
        .split_at_checked("mailto:".len())
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("mailto:"))
        .map_or(attendee, |(_, rest)| rest);
    format!("mailto:{address}")
}
