// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![forbid(unsafe_code)]

//! Translates between JSCalendar and iCalendar for the Evolution calendar backend.
//!
//! Translates between JSCalendar ([RFC 8984]) and iCalendar ([RFC 5545]).
//! `ECalMetaBackend` uses iCalendar text, while JMAP uses JSCalendar `CalendarEvent` objects.
//! The crate does not depend on GLib or Evolution headers.
//!
//! [`event`] maps between JSCalendar and iCalendar events.
//! [`freebusy`] converts busy periods to `VFREEBUSY` components.
//!
//! [RFC 5545]: https://www.rfc-editor.org/rfc/rfc5545
//! [RFC 8984]: https://www.rfc-editor.org/rfc/rfc8984

pub mod error;
pub mod event;
pub mod freebusy;
mod zone;

pub use error::ICalError;
pub use event::{
    MAPPED_PROPERTIES, MAX_DEPTH, OVERRIDE_PROPERTIES, WINDOWS_TIME_ZONES, defines_time_zone,
    event_to_ical, ical_to_event, maps_alerts, maps_keyword, maps_locations,
    maps_recurrence_override, maps_recurrence_rule, maps_time_zone, maps_virtual_locations,
    names_time_zone, prune_time_zones, resolve_canonical_time_zone, scheduling_ical,
    sends_recurrence_override, time_zone_definition, unique_tzid_to_iana, unstateable_until,
    windows_time_zone_to_iana,
};
pub use freebusy::{busy_periods_to_vfreebusy, free_busy_type};
