// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![forbid(unsafe_code)]

//! JSCalendar ↔ iCalendar for the Evolution calendar backend.
//!
//! `ECalMetaBackend` speaks `ICalComponent`, which is built from and rendered
//! to iCalendar text ([RFC 5545]); JMAP speaks JSCalendar Events
//! ([RFC 8984]) wrapped in `CalendarEvent`. This crate is the translation
//! between the two, and nothing else — it has no dependency on GLib or the
//! Evolution headers, so the mapping stays testable everywhere the workspace
//! builds. It is the calendar-side counterpart of `jmap-vcard`.
//!
//! [`event`] is the semantic mapping between JSCalendar and iCalendar, and
//! [`freebusy`] renders a principal's busy periods as the `VFREEBUSY` a
//! meeting scheduler reads.
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
