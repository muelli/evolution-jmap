// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Calendars types (draft-ietf-jmap-calendars-28). A `CalendarEvent` is a
//! jscalendarbis (`draft-ietf-calext-jscalendarbis`, the JSCalendar 2.0 base
//! that obsoletes RFC 8984) Event carrying the JMAP-side `id` and
//! `calendarIds` properties.
//!
//! The draft is in final approval as of mid-2026; property names follow
//! draft-ietf-jmap-calendars-28. Unmodeled properties ride in `extra`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::SetError;
use crate::id::Id;
use crate::methods::SetRequest;
#[cfg(feature = "principals")]
use crate::principals::Principal;
use crate::state::UtcDate;

#[cfg(not(feature = "principals"))]
type Principal = serde_json::Value;

/// A calendar (draft §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Calendar {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_subscribed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_visible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_in_availability: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    /// Server-computed permissions on this calendar (calendars draft §4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_rights: Option<CalendarRights>,
    /// Rights granted to other principals (calendars draft §4), keyed by
    /// principal id. `jmap-mock::calendars::calendar_get` enforces it: a
    /// foreign caller sees only calendars shared with them, computed
    /// `myRights` and all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_with: Option<BTreeMap<Id, CalendarRights>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Calendar {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    pub fn with_sort_order(mut self, sort_order: u32) -> Self {
        self.sort_order = Some(sort_order);
        self
    }

    pub fn is_default(mut self, is_default: bool) -> Self {
        self.is_default = Some(is_default);
        self
    }

    pub fn is_subscribed(mut self, is_subscribed: bool) -> Self {
        self.is_subscribed = Some(is_subscribed);
        self
    }

    pub fn is_visible(mut self, is_visible: bool) -> Self {
        self.is_visible = Some(is_visible);
        self
    }

    pub fn with_time_zone(mut self, time_zone: impl Into<String>) -> Self {
        self.time_zone = Some(time_zone.into());
        self
    }

    pub fn with_my_rights(mut self, my_rights: CalendarRights) -> Self {
        self.my_rights = Some(my_rights);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `Calendar.myRights` or `shareWith` entry (JMAP Calendars §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarRights {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_read_free_busy: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_read_items: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_write_all: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_write_own: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_update_private: Option<bool>,
    #[serde(rename = "mayRSVP", default, skip_serializing_if = "Option::is_none")]
    pub may_rsvp: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_share: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarRights {
    pub fn all() -> Self {
        Self {
            may_read_free_busy: Some(true),
            may_read_items: Some(true),
            may_write_all: Some(true),
            may_write_own: Some(true),
            may_update_private: Some(true),
            may_rsvp: Some(true),
            may_share: Some(true),
            may_delete: Some(true),
            extra: BTreeMap::new(),
        }
    }

    pub fn read_only() -> Self {
        Self {
            may_read_items: Some(true),
            may_read_free_busy: Some(true),
            ..Self::default()
        }
    }

    /// Whether these rights let the holder write to the calendar.
    /// Either `may_write_all` or `may_write_own` counts; absent fields default to `false`.
    pub fn is_writable(&self) -> bool {
        self.may_write_all.unwrap_or(false) || self.may_write_own.unwrap_or(false)
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }

    pub fn with_may_read_free_busy(mut self, value: bool) -> Self {
        self.may_read_free_busy = Some(value);
        self
    }

    pub fn with_may_read_items(mut self, value: bool) -> Self {
        self.may_read_items = Some(value);
        self
    }

    pub fn with_may_write_all(mut self, value: bool) -> Self {
        self.may_write_all = Some(value);
        self
    }

    pub fn with_may_write_own(mut self, value: bool) -> Self {
        self.may_write_own = Some(value);
        self
    }

    pub fn with_may_update_private(mut self, value: bool) -> Self {
        self.may_update_private = Some(value);
        self
    }

    pub fn with_may_rsvp(mut self, value: bool) -> Self {
        self.may_rsvp = Some(value);
        self
    }

    pub fn with_may_share(mut self, value: bool) -> Self {
        self.may_share = Some(value);
        self
    }

    pub fn with_may_delete(mut self, value: bool) -> Self {
        self.may_delete = Some(value);
        self
    }
}

/// `Calendar/set` arguments (draft-ietf-jmap-calendars-28 §4.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarSetRequest {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_in_state: Option<crate::state::State>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create: Option<BTreeMap<String, Calendar>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<BTreeMap<Id, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destroy: Option<Vec<Id>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_destroy_remove_contents: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_set_is_default: Option<Id>,
}

impl CalendarSetRequest {
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            if_in_state: None,
            create: None,
            update: None,
            destroy: None,
            on_destroy_remove_contents: None,
            on_success_set_is_default: None,
        }
    }

    pub fn with_on_success_set_is_default(mut self, id: impl Into<Id>) -> Self {
        self.on_success_set_is_default = Some(id.into());
        self
    }
}

/// A calendar event (draft §5): JSCalendar Event plus JMAP `id` and
/// `calendarIds`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_ids: Option<BTreeMap<Id, bool>>,
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    /// The JSCalendar version this object conforms to (jscalendarbis §3.1.2).
    /// In JMAP Calendars the value is `"2.0"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    /// When the event was created (RFC 8984 §4.1.5 `created`, UTCDateTime).
    /// Server-set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    /// When the event was last changed (RFC 8984 §4.1.6 `updated`, UTCDateTime).
    /// Server-set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSCalendar LocalDateTime, interpreted in `time_zone`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    /// ISO 8601 duration, e.g. `PT1H`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    /// Whether the event is shown without a time: an all-day event (RFC 8984 §4.1.5).
    /// Defaults to false; `None` preserves omission on save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_without_time: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Whether the event blocks time (RFC 8984 §4.4.2 `freeBusyStatus`: `free` or `busy`).
    /// `None` preserves omission on save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_busy_status: Option<String>,
    /// Event priority (RFC 8984 §4.4.1 / RFC 5545 §3.8.1.9): 0 undefined, 1 highest, 9 lowest.
    /// Signed `i64` allows lenient deserialization of out-of-range values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
    /// Sharing privacy (RFC 8984 §4.4.3: `public`, `private`, `secret`, or custom).
    /// Kept as String to accept open vocabulary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy: Option<String>,
    /// Locations (RFC 8984 §4.2.5), keyed by location id.
    /// Preserved as JSON map for lossless round-tripping and partial patching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locations: Option<BTreeMap<String, Value>>,
    /// Virtual locations / conference links (RFC 8984 §4.2.6), keyed by id.
    /// Preserved as JSON map for lossless round-tripping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_locations: Option<BTreeMap<String, Value>>,
    /// External resources / attachments (RFC 8984 §4.2.7), keyed by id.
    /// Preserved as JSON map for lossless round-tripping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<BTreeMap<String, Value>>,
    /// Event keywords / tags (RFC 8984 §4.2.9 Set: keys are tags, values `true`).
    /// Values kept as JSON to tolerate non-boolean server responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<BTreeMap<String, Value>>,
    /// Reminders / alerts (RFC 8984 §4.5.2), keyed by alert id.
    /// Preserved as JSON map for lossless round-tripping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alerts: Option<BTreeMap<String, Value>>,
    /// Participants and organizer (RFC 8984 §4.4.6), keyed by participant id.
    /// Preserved as JSON map for lossless round-tripping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participants: Option<BTreeMap<String, Value>>,
    /// Organizer `calendarAddress` for incoming iTIP scheduling messages
    /// (JMAP Calendars §5.9.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organizer_calendar_address: Option<String>,
    /// Whether the event is still a draft (JMAP Calendars §5.1).
    /// May only be set on create.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_draft: Option<bool>,
    /// Whether only owners see the full participant list (JMAP Calendars §5.1.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hide_attendees: Option<bool>,
    /// Single recurrence rule (jscalendarbis §3.3.3 replaces RFC 8984 array).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_rule: Option<RecurrenceRule>,
    /// Recurrence overrides (RFC 8984 §4.3.5): map from start `LocalDateTime`
    /// to patch object. Kept as JSON to preserve arbitrary patch properties.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_overrides: Option<BTreeMap<String, Value>>,
    /// Custom time zone definitions (RFC 8984 §4.7.2), keyed by time zone id.
    /// Kept as JSON for lossless preservation. Intentional interop deviation
    /// (ICAL-MAPPING §11, `timeZones`, and §4.2) for custom /... timezones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zones: Option<BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_default_alerts: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// Reserved in JSCalendar-bis §7.4.2.2; tolerated on deserialization, never emitted.
    #[serde(default, skip_serializing)]
    pub localizations: Option<BTreeMap<String, Value>>,
    /// Whether this account is the event's origin (JMAP Calendars §5.6, §10.9.5).
    /// Server-set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_origin: Option<bool>,
    /// End time zone if different from event start time zone (jscalendarbis §4.1.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_time_zone: Option<String>,
    /// Identifies the main physical location for iCalendar LOCATION interoperability
    /// (jscalendarbis §3.2.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_location_id: Option<Id>,
    /// Sequence number of the scheduling status (draft-ietf-jmap-calendars §5.2.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_sequence: Option<u32>,
    /// When the scheduling status was last updated (draft-ietf-jmap-calendars §5.2.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_updated: Option<UtcDate>,
    /// Whether the user may add themselves as a participant (draft §5.1.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_invite_self: Option<bool>,
    /// Whether the user may invite other participants (draft §5.1.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_invite_others: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarEvent {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_calendar_id(mut self, id: impl Into<Id>) -> Self {
        let mut map = self.calendar_ids.unwrap_or_default();
        map.insert(id.into(), true);
        self.calendar_ids = Some(map);
        self
    }

    pub fn with_uid(mut self, uid: impl Into<String>) -> Self {
        self.uid = Some(uid.into());
        self
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_start(mut self, start: impl Into<String>) -> Self {
        self.start = Some(start.into());
        self
    }

    pub fn with_time_zone(mut self, time_zone: impl Into<String>) -> Self {
        self.time_zone = Some(time_zone.into());
        self
    }

    pub fn with_duration(mut self, duration: impl Into<String>) -> Self {
        self.duration = Some(duration.into());
        self
    }

    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    pub fn with_free_busy_status(mut self, free_busy_status: impl Into<String>) -> Self {
        self.free_busy_status = Some(free_busy_status.into());
        self
    }

    pub fn with_priority(mut self, priority: i64) -> Self {
        self.priority = Some(priority);
        self
    }

    pub fn with_privacy(mut self, privacy: impl Into<String>) -> Self {
        self.privacy = Some(privacy.into());
        self
    }

    pub fn show_without_time(mut self, show_without_time: bool) -> Self {
        self.show_without_time = Some(show_without_time);
        self
    }

    pub fn use_default_alerts(mut self, use_default: bool) -> Self {
        self.use_default_alerts = Some(use_default);
        self
    }

    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    pub fn with_locale(mut self, locale: impl Into<String>) -> Self {
        self.locale = Some(locale.into());
        self
    }

    pub fn with_recurrence_rule(mut self, rrule: RecurrenceRule) -> Self {
        self.recurrence_rule = Some(rrule);
        self
    }

    /// A minimal one-off event, ready for `CalendarEvent/set` create.
    pub fn simple(calendar_id: impl Into<Id>, title: &str, start: &str, duration: &str) -> Self {
        Self {
            calendar_ids: Some([(calendar_id.into(), true)].into()),
            event_type: Some("Event".to_owned()),
            // A standalone Event MUST state its JSCalendar version
            // (jscalendarbis §3.1.2); in JMAP context the object is a
            // jscalendarbis Event (draft-ietf-jmap-calendars-28 §1.4) = "2.0".
            version: Some("2.0".to_owned()),
            title: Some(title.to_owned()),
            start: Some(start.to_owned()),
            time_zone: Some("Etc/UTC".to_owned()),
            duration: Some(duration.to_owned()),
            status: Some("confirmed".to_owned()),
            ..Self::default()
        }
    }

    pub fn with_end_time_zone(mut self, end_time_zone: impl Into<String>) -> Self {
        self.end_time_zone = Some(end_time_zone.into());
        self
    }

    pub fn with_main_location_id(mut self, main_location_id: impl Into<Id>) -> Self {
        self.main_location_id = Some(main_location_id.into());
        self
    }

    pub fn with_schedule_sequence(mut self, sequence: u32) -> Self {
        self.schedule_sequence = Some(sequence);
        self
    }

    pub fn with_schedule_updated(mut self, updated: UtcDate) -> Self {
        self.schedule_updated = Some(updated);
        self
    }

    pub fn may_invite_self(mut self, may_invite_self: bool) -> Self {
        self.may_invite_self = Some(may_invite_self);
        self
    }

    pub fn may_invite_others(mut self, may_invite_others: bool) -> Self {
        self.may_invite_others = Some(may_invite_others);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// JSCalendar RecurrenceRule (RFC 8984 §4.3.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RecurrenceRule {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub rule_type: Option<String>,
    #[serde(default)]
    pub frequency: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    /// Seconds of the minute (RFC 8984 §4.3.3 / RFC 5545 `BYSECOND`, 0..=60).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_second: Option<Vec<u32>>,
    /// Minutes of the hour (RFC 8984 §4.3.3 / RFC 5545 `BYMINUTE`, 0..=59).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_minute: Option<Vec<u32>>,
    /// Hours of the day (RFC 8984 §4.3.3 / RFC 5545 `BYHOUR`, 0..=23).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_hour: Option<Vec<u32>>,
    /// Days of the week (RFC 8984 §4.3.3 / RFC 5545 `BYDAY`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_day: Option<Vec<NDay>>,
    /// Days of the month (RFC 8984 §4.3.3 / RFC 5545 `BYMONTHDAY`, 1..=31 or -31..=-1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_month_day: Option<Vec<i32>>,
    /// Days of the year (RFC 8984 §4.3.3 / RFC 5545 `BYYEARDAY`, 1..=366 or -366..=-1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_year_day: Option<Vec<i32>>,
    /// Weeks of the year (RFC 8984 §4.3.3 / RFC 5545 `BYWEEKNO`, 1..=53 or -53..=-1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_week_no: Option<Vec<i32>>,
    /// Months of the year (RFC 8984 §4.3.3 / RFC 5545 `BYMONTH`, e.g. "1".."12" or "5L").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_month: Option<Vec<String>>,
    /// Occurrence indices to keep (RFC 8984 §4.3.3 / RFC 5545 `BYSETPOS`, 1..=366 or -366..=-1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_set_position: Option<Vec<i32>>,
    /// Day each week starts on (RFC 8984 §4.3.3 / RFC 5545 `WKST`, default "mo").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_day_of_week: Option<String>,
    /// The calendar scale the recurrence rule is defined in (RFC 8984 §4.3.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rscale: Option<String>,
    /// How to handle occurrences spanning leap months or invalid dates (RFC 8984 §4.3.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// JSCalendar NDay (RFC 8984 §4.3.3).
/// Identifies a weekday and an optional ordinal position within the recurrence period.
/// Positive values count from the start.
/// Negative values count from the end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct NDay {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub day_type: Option<String>,
    #[serde(default)]
    pub day: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nth_of_period: Option<i32>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl NDay {
    pub fn new(day: &str) -> Self {
        Self {
            day_type: Some("NDay".to_owned()),
            day: day.to_owned(),
            nth_of_period: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_nth_of_period(mut self, nth: i32) -> Self {
        self.nth_of_period = Some(nth);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

impl RecurrenceRule {
    pub fn new(frequency: impl Into<String>) -> Self {
        Self {
            rule_type: Some("RecurrenceRule".to_owned()),
            frequency: frequency.into(),
            ..Self::default()
        }
    }

    pub fn with_interval(mut self, interval: u32) -> Self {
        self.interval = Some(interval);
        self
    }

    pub fn with_count(mut self, count: u32) -> Self {
        self.count = Some(count);
        self
    }

    pub fn with_until(mut self, until: impl Into<String>) -> Self {
        self.until = Some(until.into());
        self
    }

    pub fn with_by_day(mut self, by_day: impl IntoIterator<Item = NDay>) -> Self {
        self.by_day = Some(by_day.into_iter().collect());
        self
    }

    pub fn with_by_month(mut self, by_month: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.by_month = Some(by_month.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_rscale(mut self, rscale: impl Into<String>) -> Self {
        self.rscale = Some(rscale.into());
        self
    }

    pub fn with_skip(mut self, skip: impl Into<String>) -> Self {
        self.skip = Some(skip.into());
        self
    }
}

/// `CalendarEvent/query` filter conditions (draft §5.5). Flat conditions
/// only. `after`/`before` are UTC instants compared against the event start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarEventQueryFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_calendar: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
}

impl CalendarEventQueryFilter {
    pub fn in_calendar(calendar_id: impl Into<Id>) -> Self {
        Self {
            in_calendar: Some(calendar_id.into()),
            ..Self::default()
        }
    }

    pub fn time_range(after: impl Into<String>, before: impl Into<String>) -> Self {
        Self {
            after: Some(after.into()),
            before: Some(before.into()),
            ..Self::default()
        }
    }

    pub fn with_time_range(mut self, after: impl Into<String>, before: impl Into<String>) -> Self {
        self.after = Some(after.into());
        self.before = Some(before.into());
        self
    }

    pub fn after(mut self, after: impl Into<String>) -> Self {
        self.after = Some(after.into());
        self
    }

    pub fn before(mut self, before: impl Into<String>) -> Self {
        self.before = Some(before.into());
        self
    }

    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    pub fn uid(mut self, uid: impl Into<String>) -> Self {
        self.uid = Some(uid.into());
        self
    }
}

/// Standard recurrence frequency values (RFC 8984 §4.3.3).
pub mod frequency {
    pub const SECONDLY: &str = "secondly";
    pub const MINUTELY: &str = "minutely";
    pub const HOURLY: &str = "hourly";
    pub const DAILY: &str = "daily";
    pub const WEEKLY: &str = "weekly";
    pub const MONTHLY: &str = "monthly";
    pub const YEARLY: &str = "yearly";
}

/// Standard recurrence skip values (RFC 8984 §4.3.3).
pub mod recurrence_skip {
    pub const OMIT: &str = "omit";
    pub const BACKWARD: &str = "backward";
    pub const FORWARD: &str = "forward";
}

/// Standard two-letter weekday codes (RFC 8984 §4.3.3).
pub mod weekday {
    pub const MO: &str = "mo";
    pub const TU: &str = "tu";
    pub const WE: &str = "we";
    pub const TH: &str = "th";
    pub const FR: &str = "fr";
    pub const SA: &str = "sa";
    pub const SU: &str = "su";
}

/// The `SetError` type draft-ietf-jmap-calendars §4.4 adds for `Calendar/set`.
pub mod calendar_set_error {
    pub const HAS_EVENT: &str = "calendarHasEvent";
}

/// Well-known values for `Calendar.includeInAvailability` (draft-ietf-jmap-calendars §4.1).
pub mod include_in_availability {
    pub const ALL: &str = "all";
    pub const ALL_EXCEPT_DECLINED: &str = "allExceptDeclined";
    pub const NONE: &str = "none";
}

/// Standard JSCalendar (RFC 8984 §5.1.3 / jscalendarbis §4.1.4) event status values.
pub mod event_status {
    pub const CONFIRMED: &str = "confirmed";
    pub const TENTATIVE: &str = "tentative";
    pub const CANCELLED: &str = "cancelled";
}

/// Standard JSCalendar (RFC 8984 §4.4.2) / jscalendarbis free/busy status values.
pub mod free_busy_status {
    pub const FREE: &str = "free";
    pub const BUSY: &str = "busy";
}

/// Standard JSCalendar (RFC 8984 §4.4.3) / jscalendarbis privacy classification values.
pub mod privacy {
    pub const PUBLIC: &str = "public";
    pub const PRIVATE: &str = "private";
    pub const SECRET: &str = "secret";
}

/// Standard JSCalendar (RFC 8984 §4.4.6 / jscalendarbis §3.4.6) participant roles.
pub mod participant_role {
    pub const OWNER: &str = "owner";
    pub const ADMIN: &str = "admin";
    pub const REQUIRED: &str = "required";
    pub const ATTENDEE: &str = "attendee";
    pub const OPTIONAL: &str = "optional";
    pub const INFORMATIONAL: &str = "informational";
}

/// Standard JSCalendar (RFC 8984 §4.4.6) participation status values.
pub mod participation_status {
    pub const NEEDS_ACTION: &str = "needs-action";
    pub const ACCEPTED: &str = "accepted";
    pub const DECLINED: &str = "declined";
    pub const TENTATIVE: &str = "tentative";
    pub const DELEGATED: &str = "delegated";
}

/// Standard JSCalendar (RFC 8984 §4.4.6) participant kinds.
pub mod participant_kind {
    pub const INDIVIDUAL: &str = "individual";
    pub const GROUP: &str = "group";
    pub const RESOURCE: &str = "resource";
    pub const LOCATION: &str = "location";
}

/// Standard JSCalendar (RFC 8984 §4.5.2) alert action types.
pub mod alert_action {
    pub const DISPLAY: &str = "display";
    pub const EMAIL: &str = "email";
}

/// Standard JSCalendar (RFC 8984 §4.5.2) offset trigger relation.
pub mod relative_to {
    pub const START: &str = "start";
    pub const END: &str = "end";
}

/// Standard JSCalendar (RFC 8984 §4.4.6) schedule agent values.
pub mod schedule_agent {
    pub const SERVER: &str = "server";
    pub const CLIENT: &str = "client";
    pub const NONE: &str = "none";
}

/// Standard JSCalendar (RFC 8984 §4.4.6) participant progress values.
pub mod participant_progress {
    pub const NEEDS_ACTION: &str = "needs-action";
    pub const IN_PROCESS: &str = "in-process";
    pub const COMPLETED: &str = "completed";
    pub const FAILED: &str = "failed";
}

/// Standard JSCalendar (RFC 8984 §4.4.6) participant attendance values.
pub mod participant_attendance {
    pub const REQUIRED: &str = "required";
    pub const OPTIONAL: &str = "optional";
    pub const INFORMATIONAL: &str = "informational";
}

/// JSCalendar Participant (jscalendarbis §3.4.6 / RFC 8984 §4.4.6): an attendee
/// or organizer of the event.
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct Participant {
    pub participant_type: Option<String>,
    pub name: Option<String>,
    pub email: Option<String>,
    pub description: Option<String>,
    pub description_content_type: Option<String>,
    /// The URI that represents this participant for calendar invitations and scheduling
    /// (jscalendarbis §3.4.6). Replaces RFC 8984's reserved `sendTo` property.
    pub calendar_address: Option<String>,
    /// Legacy RFC 8984 addressing methods. Tolerated on input; never emitted on output.
    pub send_to: Option<BTreeMap<String, String>>,
    pub kind: Option<String>,
    pub roles: Option<BTreeMap<String, bool>>,
    pub location_id: Option<String>,
    pub participation_status: Option<String>,
    pub participation_comment: Option<String>,
    pub attendance: Option<String>,
    pub expect_reply: Option<bool>,
    pub schedule_agent: Option<String>,
    pub schedule_sequence: Option<u32>,
    pub schedule_status: Option<Vec<String>>,
    pub schedule_updated: Option<UtcDate>,
    pub delegated_to: Option<Vec<String>>,
    pub delegated_from: Option<Vec<String>>,
    pub member_of: Option<BTreeMap<String, bool>>,
    pub progress: Option<String>,
    pub progress_updated: Option<UtcDate>,
    pub extra: BTreeMap<String, Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParticipantHelper {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    participant_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description_content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    calendar_address: Option<String>,
    #[serde(default, skip_serializing)]
    send_to: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    roles: Option<BTreeMap<String, bool>>,
    #[serde(default, skip_serializing)]
    location_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    participation_status: Option<String>,
    #[serde(default, skip_serializing)]
    participation_comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attendance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expect_reply: Option<bool>,
    #[serde(default, skip_serializing)]
    schedule_agent: Option<String>,
    #[serde(default, skip_serializing)]
    schedule_sequence: Option<u32>,
    #[serde(default, skip_serializing)]
    schedule_status: Option<Vec<String>>,
    #[serde(default, skip_serializing)]
    schedule_updated: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delegated_to: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delegated_from: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    member_of: Option<BTreeMap<String, bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    progress: Option<String>,
    #[serde(default, skip_serializing)]
    progress_updated: Option<UtcDate>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

impl<'de> serde::Deserialize<'de> for Participant {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let helper = ParticipantHelper::deserialize(deserializer)?;
        let calendar_address = helper.calendar_address.or_else(|| {
            helper.send_to.as_ref().and_then(|send_to| {
                send_to
                    .get("imip")
                    .or_else(|| send_to.values().next())
                    .cloned()
            })
        });
        let send_to = helper.send_to.or_else(|| {
            calendar_address.as_ref().map(|addr| {
                let mut map = BTreeMap::new();
                map.insert("imip".to_owned(), addr.clone());
                map
            })
        });
        Ok(Participant {
            participant_type: helper.participant_type,
            name: helper.name,
            email: helper.email,
            description: helper.description,
            description_content_type: helper.description_content_type,
            calendar_address,
            send_to,
            kind: helper.kind,
            roles: helper.roles,
            location_id: helper.location_id,
            participation_status: helper.participation_status,
            participation_comment: helper.participation_comment,
            attendance: helper.attendance,
            expect_reply: helper.expect_reply,
            schedule_agent: helper.schedule_agent,
            schedule_sequence: helper.schedule_sequence,
            schedule_status: helper.schedule_status,
            schedule_updated: helper.schedule_updated,
            delegated_to: helper.delegated_to,
            delegated_from: helper.delegated_from,
            member_of: helper.member_of,
            progress: helper.progress,
            progress_updated: helper.progress_updated,
            extra: helper.extra,
        })
    }
}

impl serde::Serialize for Participant {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let calendar_address = self.calendar_address.clone().or_else(|| {
            self.send_to.as_ref().and_then(|send_to| {
                send_to
                    .get("imip")
                    .or_else(|| send_to.values().next())
                    .cloned()
            })
        });
        let helper = ParticipantHelper {
            participant_type: self.participant_type.clone(),
            name: self.name.clone(),
            email: self.email.clone(),
            description: self.description.clone(),
            description_content_type: self.description_content_type.clone(),
            calendar_address,
            send_to: None,
            kind: self.kind.clone(),
            roles: self.roles.clone(),
            location_id: None,
            participation_status: self.participation_status.clone(),
            participation_comment: None,
            attendance: self.attendance.clone(),
            expect_reply: self.expect_reply,
            schedule_agent: None,
            schedule_sequence: None,
            schedule_status: None,
            schedule_updated: None,
            delegated_to: self.delegated_to.clone(),
            delegated_from: self.delegated_from.clone(),
            member_of: self.member_of.clone(),
            progress: self.progress.clone(),
            progress_updated: None,
            extra: self.extra.clone(),
        };
        helper.serialize(serializer)
    }
}

impl Participant {
    pub fn new(name: impl Into<String>, email: impl Into<String>) -> Self {
        Self {
            participant_type: Some("Participant".to_owned()),
            name: Some(name.into()),
            email: Some(email.into()),
            ..Self::default()
        }
    }

    pub fn with_roles(mut self, roles: BTreeMap<String, bool>) -> Self {
        self.roles = Some(roles);
        self
    }

    pub fn with_participation_status(mut self, status: impl Into<String>) -> Self {
        self.participation_status = Some(status.into());
        self
    }

    pub fn with_participation_comment(mut self, comment: impl Into<String>) -> Self {
        self.participation_comment = Some(comment.into());
        self
    }

    pub fn with_attendance(mut self, attendance: impl Into<String>) -> Self {
        self.attendance = Some(attendance.into());
        self
    }

    pub fn expect_reply(mut self, expect: bool) -> Self {
        self.expect_reply = Some(expect);
        self
    }

    pub fn with_schedule_agent(mut self, schedule_agent: impl Into<String>) -> Self {
        self.schedule_agent = Some(schedule_agent.into());
        self
    }

    pub fn with_calendar_address(mut self, address: impl Into<String>) -> Self {
        let addr = address.into();
        self.calendar_address = Some(addr.clone());
        let mut map = self.send_to.unwrap_or_default();
        map.insert("imip".to_owned(), addr);
        self.send_to = Some(map);
        self
    }

    pub fn with_description_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.description_content_type = Some(content_type.into());
        self
    }

    pub fn with_send_to_method(
        mut self,
        method: impl Into<String>,
        address: impl Into<String>,
    ) -> Self {
        let addr = address.into();
        if self.calendar_address.is_none() {
            self.calendar_address = Some(addr.clone());
        }
        let mut map = self.send_to.unwrap_or_default();
        map.insert(method.into(), addr);
        self.send_to = Some(map);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    pub fn with_location_id(mut self, location_id: impl Into<String>) -> Self {
        self.location_id = Some(location_id.into());
        self
    }

    pub fn with_schedule_sequence(mut self, schedule_sequence: u32) -> Self {
        self.schedule_sequence = Some(schedule_sequence);
        self
    }

    pub fn with_schedule_status(
        mut self,
        schedule_status: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.schedule_status = Some(schedule_status.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_schedule_updated(mut self, schedule_updated: impl Into<UtcDate>) -> Self {
        self.schedule_updated = Some(schedule_updated.into());
        self
    }

    pub fn with_delegated_to(
        mut self,
        delegated_to: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.delegated_to = Some(delegated_to.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_delegated_from(
        mut self,
        delegated_from: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.delegated_from = Some(delegated_from.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_member_of(mut self, member_of: BTreeMap<String, bool>) -> Self {
        self.member_of = Some(member_of);
        self
    }

    pub fn with_progress(mut self, progress: impl Into<String>) -> Self {
        self.progress = Some(progress.into());
        self
    }

    pub fn with_progress_updated(mut self, progress_updated: impl Into<UtcDate>) -> Self {
        self.progress_updated = Some(progress_updated.into());
        self
    }
}

/// JSCalendar Location (RFC 8984 §4.2.5): a physical location for the event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Location {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub location_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing)]
    pub description: Option<String>,
    #[serde(default, skip_serializing)]
    pub relative_to: Option<String>,
    #[serde(default, skip_serializing)]
    pub time_zone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinates: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_types: Option<BTreeMap<String, bool>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Location {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            location_type: Some("Location".to_owned()),
            name: Some(name.into()),
            description: None,
            relative_to: None,
            time_zone: None,
            coordinates: None,
            location_types: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_time_zone(mut self, time_zone: impl Into<String>) -> Self {
        self.time_zone = Some(time_zone.into());
        self
    }

    pub fn with_coordinates(mut self, coordinates: impl Into<String>) -> Self {
        self.coordinates = Some(coordinates.into());
        self
    }

    pub fn with_relative_to(mut self, relative_to: impl Into<String>) -> Self {
        self.relative_to = Some(relative_to.into());
        self
    }

    pub fn with_location_types(mut self, location_types: BTreeMap<String, bool>) -> Self {
        self.location_types = Some(location_types);
        self
    }
}

/// JSCalendar VirtualLocation (RFC 8984 §4.2.6): an online conference or meeting room.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct VirtualLocation {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub virtual_location_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing)]
    pub description: Option<String>,
    #[serde(default)]
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<BTreeMap<String, bool>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl VirtualLocation {
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            virtual_location_type: Some("VirtualLocation".to_owned()),
            name: None,
            description: None,
            uri: uri.into(),
            features: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_features(mut self, features: BTreeMap<String, bool>) -> Self {
        self.features = Some(features);
        self
    }
}

/// JSCalendar Link (draft-ietf-calext-jscalendarbis-22 §1.5.11): external resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Link {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub link_type: Option<String>,
    #[serde(default)]
    pub href: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<BTreeMap<String, bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Obsoleted RFC 8984 property; tolerated on deserialization, never emitted on serialization.
    #[serde(default, skip_serializing)]
    pub cid: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Link {
    pub fn new(href: impl Into<String>) -> Self {
        Self {
            link_type: Some("Link".to_owned()),
            href: href.into(),
            ..Default::default()
        }
    }

    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    pub fn with_rel(mut self, rel: impl Into<String>) -> Self {
        self.rel = Some(rel.into());
        self
    }

    pub fn with_display(mut self, display: BTreeMap<String, bool>) -> Self {
        self.display = Some(display);
        self
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

/// JSCalendar Alert (RFC 8984 §4.5.2): an alarm or reminder for the event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Alert {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub alert_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledged: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related_to: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Alert {
    pub fn new(action: impl Into<String>, trigger: Value) -> Self {
        Self {
            alert_type: Some("Alert".to_owned()),
            action: Some(action.into()),
            trigger: Some(trigger),
            acknowledged: None,
            related_to: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_acknowledged(mut self, acknowledged: impl Into<UtcDate>) -> Self {
        self.acknowledged = Some(acknowledged.into());
        self
    }

    pub fn with_related_to(mut self, related_to: impl Into<String>) -> Self {
        self.related_to = Some(related_to.into());
        self
    }
}

/// JSCalendar OffsetTrigger (RFC 8984 §4.5.2): an alert trigger specified as an offset duration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct OffsetTrigger {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub trigger_type: Option<String>,
    pub offset: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_to: Option<String>,
}

impl OffsetTrigger {
    pub fn new(offset: impl Into<String>) -> Self {
        Self {
            trigger_type: Some("OffsetTrigger".to_owned()),
            offset: offset.into(),
            relative_to: None,
        }
    }

    pub fn relative_to(mut self, relative_to: impl Into<String>) -> Self {
        self.relative_to = Some(relative_to.into());
        self
    }
}

/// Calendar user preferences (draft-ietf-jmap-calendars-28 §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarPreferences {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_day_of_week: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarPreferences {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_time_zone(mut self, time_zone: impl Into<String>) -> Self {
        self.time_zone = Some(time_zone.into());
        self
    }

    pub fn with_first_day_of_week(mut self, first_day: impl Into<String>) -> Self {
        self.first_day_of_week = Some(first_day.into());
        self
    }
}

/// Calendar preferences capability properties (draft-ietf-jmap-calendars-28 §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarPreferencesCapability {
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarPreferencesCapability {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_extra(mut self, extra: Value) -> Self {
        if let Value::Object(map) = extra {
            self.extra.extend(map);
        }
        self
    }
}

/// `CalendarEvent/set` arguments (draft-ietf-jmap-calendars-28 §5.9).
/// Extends [`SetRequest`] with `sendEventNotifications`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarEventSetRequest {
    #[serde(flatten)]
    pub set: SetRequest<CalendarEvent>,
    /// If true, the server sends iTIP scheduling messages after applying changes.
    /// Defaults to false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_scheduling_messages: Option<bool>,
}

impl CalendarEventSetRequest {
    pub fn new(set: SetRequest<CalendarEvent>) -> Self {
        Self {
            set,
            send_scheduling_messages: None,
        }
    }

    pub fn send_scheduling_messages(mut self, send: bool) -> Self {
        self.send_scheduling_messages = Some(send);
        self
    }
}

/// iTIP methods (RFC 5546 §3.2) a server sends for
/// draft-ietf-jmap-calendars-28 §5.9.2's scheduling messages.
pub mod scheduling_method {
    /// An invitation, from the event's origin to its participants.
    pub const REQUEST: &str = "REQUEST";
    /// A withdrawal, from the event's origin to a participant who was
    /// dropped, or to everyone when the event or one of its instances goes
    /// away.
    pub const CANCEL: &str = "CANCEL";
    /// An answer, from a participant back to the organizer.
    pub const REPLY: &str = "REPLY";
}

/// Properties stored per user (draft-ietf-jmap-calendars-28 §5.4).
/// Updates to these properties send no scheduling messages.
pub const PER_USER_PROPERTIES: &[&str] = &[
    "alerts",
    "color",
    "freeBusyStatus",
    "keywords",
    "updated",
    "useDefaultAlerts",
];

/// `CalendarEvent/parse` arguments (draft-ietf-jmap-calendars-28 §5.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarEventParseRequest {
    pub account_id: Id,
    pub blob_ids: Vec<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<String>>,
}

impl CalendarEventParseRequest {
    pub fn new(
        account_id: impl Into<Id>,
        blob_ids: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            blob_ids: blob_ids.into_iter().map(Into::into).collect(),
            properties: None,
        }
    }

    pub fn properties(mut self, properties: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.properties = Some(properties.into_iter().map(Into::into).collect());
        self
    }
}

/// `CalendarEvent/parse` response (draft-ietf-jmap-calendars-28 §5.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarEventParseResponse {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parsed: Option<BTreeMap<Id, CalendarEvent>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_parsable: Option<Vec<Id>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_found: Option<Vec<Id>>,
}

impl CalendarEventParseResponse {
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            parsed: None,
            not_parsable: None,
            not_found: None,
        }
    }

    pub fn with_parsed(mut self, parsed: BTreeMap<Id, CalendarEvent>) -> Self {
        self.parsed = Some(parsed);
        self
    }

    pub fn with_not_parsable(
        mut self,
        not_parsable: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        self.not_parsable = Some(not_parsable.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_not_found(mut self, not_found: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.not_found = Some(not_found.into_iter().map(Into::into).collect());
        self
    }
}

/// Calendars capability properties (draft-ietf-jmap-calendars-28 §1.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarsCapability {
    #[serde(default)]
    pub max_size_attachments_per_event: u64,
    #[serde(default)]
    pub max_concurrent_availabilities: u64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarsCapability {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_size_attachments_per_event(mut self, max: u64) -> Self {
        self.max_size_attachments_per_event = max;
        self
    }

    pub fn with_max_concurrent_availabilities(mut self, max: u64) -> Self {
        self.max_concurrent_availabilities = max;
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Standard RFC 8984 §1.4.10 event relation types.
pub mod event_relation_type {
    pub const FIRST: &str = "first";
    pub const NEXT: &str = "next";
    pub const PARENT: &str = "parent";
    pub const CHILD: &str = "child";
}

/// Standard RFC 8984 §4.4.1 / RFC 5545 §3.8.1.9 priority constants.
pub mod priority {
    pub const UNDEFINED: i64 = 0;
    pub const HIGH: i64 = 1;
    pub const MEDIUM: i64 = 5;
    pub const LOW: i64 = 9;
}

/// JSCalendar AbsoluteTrigger (RFC 8984 §4.5.2): an alert trigger specified as an absolute UTC timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AbsoluteTrigger {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub trigger_type: Option<String>,
    pub when: UtcDate,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl AbsoluteTrigger {
    pub fn new(when: impl Into<UtcDate>) -> Self {
        Self {
            trigger_type: Some("AbsoluteTrigger".to_owned()),
            when: when.into(),
            extra: BTreeMap::new(),
        }
    }
}

/// JSCalendar Relation (RFC 8984 §1.4.10): how this event relates to another.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EventRelation {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub relation_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<BTreeMap<String, bool>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl EventRelation {
    pub fn new(relation: BTreeMap<String, bool>) -> Self {
        Self {
            relation_type: Some("Relation".to_owned()),
            relation: Some(relation),
            extra: BTreeMap::new(),
        }
    }
}

/// `CalendarEvent/getFreeBusy` arguments (draft-ietf-jmap-calendars-28 §5.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct GetFreeBusyRequest {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_ids: Option<Vec<Id>>,
    pub utc_start: UtcDate,
    pub utc_end: UtcDate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
}

impl GetFreeBusyRequest {
    pub fn new(
        account_id: impl Into<Id>,
        utc_start: impl Into<UtcDate>,
        utc_end: impl Into<UtcDate>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            calendar_ids: None,
            utc_start: utc_start.into(),
            utc_end: utc_end.into(),
            time_zone: None,
        }
    }

    pub fn calendar_ids(mut self, ids: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.calendar_ids = Some(ids.into_iter().map(Into::into).collect());
        self
    }

    pub fn time_zone(mut self, time_zone: impl Into<String>) -> Self {
        self.time_zone = Some(time_zone.into());
        self
    }
}

/// One free/busy interval block within `CalendarEvent/getFreeBusy` (draft-ietf-jmap-calendars-28 §5.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct FreeBusyBlock {
    pub utc_start: UtcDate,
    pub utc_end: UtcDate,
    pub busy_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<Id>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl FreeBusyBlock {
    pub fn new(
        utc_start: impl Into<UtcDate>,
        utc_end: impl Into<UtcDate>,
        busy_status: impl Into<String>,
    ) -> Self {
        Self {
            utc_start: utc_start.into(),
            utc_end: utc_end.into(),
            busy_status: busy_status.into(),
            calendar_id: None,
            event_id: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_calendar_id(mut self, calendar_id: impl Into<Id>) -> Self {
        self.calendar_id = Some(calendar_id.into());
        self
    }

    pub fn with_event_id(mut self, event_id: impl Into<Id>) -> Self {
        self.event_id = Some(event_id.into());
        self
    }
}

/// `CalendarEvent/getFreeBusy` response (draft-ietf-jmap-calendars-28 §5.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct GetFreeBusyResponse {
    pub account_id: Id,
    #[serde(default)]
    pub list: Vec<FreeBusyBlock>,
}

impl GetFreeBusyResponse {
    pub fn new(account_id: impl Into<Id>, list: impl IntoIterator<Item = FreeBusyBlock>) -> Self {
        Self {
            account_id: account_id.into(),
            list: list.into_iter().collect(),
        }
    }
}

/// The `SetError` types draft-ietf-jmap-calendars-28 §5.9 adds for `CalendarEvent/set`.
pub mod calendar_event_set_error {
    pub const BLOB_NOT_FOUND: &str = "blobNotFound";
    pub const TOO_MANY_PARTICIPANTS: &str = "tooManyParticipants";
    pub const TOO_MANY_RECURRENCES: &str = "tooManyRecurrences";
    pub const CANNOT_CALCULATE_OCCURRENCES: &str = "cannotCalculateOccurrences";
    pub const NO_SUPPORTED_SCHEDULE_METHODS: &str = "noSupportedScheduleMethods";
}

/// Standard free/busy status values (draft-ietf-jmap-calendars-28 §5.7).
pub mod calendar_free_busy_status {
    pub const FREE: &str = "free";
    pub const BUSY: &str = "busy";
    pub const BUSY_TENTATIVE: &str = "busy-tentative";
    pub const BUSY_UNAVAILABLE: &str = "busy-unavailable";
}

/// Group entries representation (RFC 8984 §5.3.1), supporting both the standard
/// array of calendar objects and keyed maps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GroupEntries {
    Array(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

impl From<Vec<Value>> for GroupEntries {
    fn from(entries: Vec<Value>) -> Self {
        Self::Array(entries)
    }
}

impl From<BTreeMap<String, Value>> for GroupEntries {
    fn from(entries: BTreeMap<String, Value>) -> Self {
        Self::Map(entries)
    }
}

/// JSCalendar Group (RFC 8984 §2.3 / §5.3): a collection of calendar objects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub group_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entries: Option<GroupEntries>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<BTreeMap<String, bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<BTreeMap<String, Value>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarGroup {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            group_type: Some("Group".to_owned()),
            title: Some(title.into()),
            ..Self::default()
        }
    }

    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_uid(mut self, uid: impl Into<String>) -> Self {
        self.uid = Some(uid.into());
        self
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_time_zone(mut self, time_zone: impl Into<String>) -> Self {
        self.time_zone = Some(time_zone.into());
        self
    }

    pub fn with_updated(mut self, updated: impl Into<UtcDate>) -> Self {
        self.updated = Some(updated.into());
        self
    }

    pub fn with_entries(mut self, entries: impl Into<GroupEntries>) -> Self {
        self.entries = Some(entries.into());
        self
    }

    pub fn with_keywords(mut self, keywords: BTreeMap<String, Value>) -> Self {
        self.keywords = Some(keywords);
        self
    }

    pub fn with_categories(mut self, categories: BTreeMap<String, bool>) -> Self {
        self.categories = Some(categories);
        self
    }

    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn with_links(mut self, links: BTreeMap<String, Value>) -> Self {
        self.links = Some(links);
        self
    }
}

fn default_calendar_alert_type() -> String {
    CalendarAlert::TYPE.to_owned()
}

/// A push notification for a calendar alert (draft-ietf-jmap-calendars-28 §6.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarAlert {
    #[serde(rename = "@type", default = "default_calendar_alert_type")]
    pub kind: String,
    pub account_id: Id,
    pub calendar_event_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<String>,
    pub alert_id: Id,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarAlert {
    pub const TYPE: &'static str = "CalendarAlert";

    pub fn new(
        account_id: impl Into<Id>,
        calendar_event_id: impl Into<Id>,
        alert_id: impl Into<Id>,
    ) -> Self {
        Self {
            kind: Self::TYPE.to_owned(),
            account_id: account_id.into(),
            calendar_event_id: calendar_event_id.into(),
            uid: None,
            recurrence_id: None,
            alert_id: alert_id.into(),
            extra: BTreeMap::new(),
        }
    }

    pub fn with_uid(mut self, uid: impl Into<String>) -> Self {
        self.uid = Some(uid.into());
        self
    }

    pub fn with_recurrence_id(mut self, recurrence_id: impl Into<String>) -> Self {
        self.recurrence_id = Some(recurrence_id.into());
        self
    }
}

fn default_calendar_event_notification_type() -> String {
    CalendarEventNotification::TYPE.to_owned()
}

/// A notification of changes to calendar events (draft-ietf-jmap-calendars-28 §8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarEventNotification {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(rename = "@type", default = "default_calendar_event_notification_type")]
    pub kind: String,
    pub created: UtcDate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_by: Option<Principal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub notification_type: Option<String>,
    pub event_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<CalendarEvent>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarEventNotification {
    pub const TYPE: &'static str = "CalendarEventNotification";

    pub fn new(created: impl Into<UtcDate>, event_id: impl Into<Id>) -> Self {
        Self {
            id: None,
            kind: Self::TYPE.to_owned(),
            created: created.into(),
            changed_by: None,
            comment: None,
            notification_type: None,
            event_id: event_id.into(),
            recurrence_id: None,
            event: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = kind.into();
        self
    }

    pub fn with_changed_by(mut self, changed_by: Principal) -> Self {
        self.changed_by = Some(changed_by);
        self
    }

    pub fn with_comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = Some(comment.into());
        self
    }

    pub fn with_notification_type(mut self, notification_type: impl Into<String>) -> Self {
        self.notification_type = Some(notification_type.into());
        self
    }

    pub fn with_recurrence_id(mut self, recurrence_id: impl Into<String>) -> Self {
        self.recurrence_id = Some(recurrence_id.into());
        self
    }

    pub fn with_event(mut self, event: CalendarEvent) -> Self {
        self.event = Some(event);
        self
    }
}

/// Filter for `CalendarEventNotification/query` (draft-ietf-jmap-calendars-28 §8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarEventNotificationQueryFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarEventNotificationQueryFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_after(mut self, after: impl Into<UtcDate>) -> Self {
        self.after = Some(after.into());
        self
    }

    pub fn with_before(mut self, before: impl Into<UtcDate>) -> Self {
        self.before = Some(before.into());
        self
    }

    pub fn with_types(mut self, types: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.types = Some(types.into_iter().map(Into::into).collect());
        self
    }
}

/// Well-known notification types for calendar event notifications (draft-ietf-jmap-calendars-28 §8).
pub mod calendar_event_notification_type {
    pub const CREATED: &str = "created";
    pub const UPDATED: &str = "updated";
    pub const DESTROYED: &str = "destroyed";
    pub const REPLY: &str = "reply";
}

/// Arguments for `CalendarEvent/send` (draft-ietf-jmap-calendars-28 §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarEventSendRequest {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send: Option<BTreeMap<Id, SendCalendarEvent>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_update_calendar_event: Option<BTreeMap<Id, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_destroy_calendar_event_ids: Option<Vec<Id>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarEventSendRequest {
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            identity_id: None,
            send: None,
            on_success_update_calendar_event: None,
            on_success_destroy_calendar_event_ids: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_identity_id(mut self, identity_id: impl Into<Id>) -> Self {
        self.identity_id = Some(identity_id.into());
        self
    }

    pub fn with_send(mut self, send: BTreeMap<Id, SendCalendarEvent>) -> Self {
        self.send = Some(send);
        self
    }

    pub fn with_on_success_update_calendar_event(mut self, updates: BTreeMap<Id, Value>) -> Self {
        self.on_success_update_calendar_event = Some(updates);
        self
    }

    pub fn with_on_success_destroy_calendar_event_ids(
        mut self,
        destroy_ids: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        self.on_success_destroy_calendar_event_ids =
            Some(destroy_ids.into_iter().map(Into::into).collect());
        self
    }
}

/// A single calendar event to send via `CalendarEvent/send` (draft-ietf-jmap-calendars-28 §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SendCalendarEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_to: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_event: Option<CalendarEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_old_properties: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SendCalendarEvent {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_recipient(mut self, recipient: impl Into<String>) -> Self {
        self.recipient = Some(recipient.into());
        self
    }

    pub fn with_send_to(mut self, send_to: BTreeMap<String, String>) -> Self {
        self.send_to = Some(send_to);
        self
    }

    pub fn with_calendar_event(mut self, calendar_event: CalendarEvent) -> Self {
        self.calendar_event = Some(calendar_event);
        self
    }

    pub fn with_include_old_properties(mut self, include: bool) -> Self {
        self.include_old_properties = Some(include);
        self
    }
}

/// Response for `CalendarEvent/send` (draft-ietf-jmap-calendars-28 §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarEventSendResponse {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent: Option<BTreeMap<Id, SendCalendarEventResult>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_sent: Option<BTreeMap<Id, SetError>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarEventSendResponse {
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            sent: None,
            not_sent: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_sent(mut self, sent: BTreeMap<Id, SendCalendarEventResult>) -> Self {
        self.sent = Some(sent);
        self
    }

    pub fn with_not_sent(mut self, not_sent: BTreeMap<Id, SetError>) -> Self {
        self.not_sent = Some(not_sent);
        self
    }
}

/// Result of sending a single calendar event (draft-ietf-jmap-calendars-28 §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SendCalendarEventResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participant_problems: Option<BTreeMap<String, ParticipantProblem>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SendCalendarEventResult {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_send_status(mut self, status: impl Into<String>) -> Self {
        self.send_status = Some(status.into());
        self
    }

    pub fn with_participant_problems(
        mut self,
        problems: BTreeMap<String, ParticipantProblem>,
    ) -> Self {
        self.participant_problems = Some(problems);
        self
    }
}

/// Problem delivering an event to a participant (draft-ietf-jmap-calendars-28 §7.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ParticipantProblem {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParticipantProblem {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: Some(kind.into()),
            description: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// Standard participant problem error types (draft-ietf-jmap-calendars-28 §7.3).
pub mod participant_problem_kind {
    pub const CANNOT_SEND_TO_SELF: &str = "cannotSendToSelf";
    pub const CALENDAR_NOT_FOUND: &str = "calendarNotFound";
    pub const PARTICIPANT_NOT_FOUND: &str = "participantNotFound";
    pub const INVALID_EMAIL: &str = "invalidEmail";
    pub const CANNOT_SEND_TO_RESOURCE: &str = "cannotSendToResource";
    pub const NOT_AUTHORIZED: &str = "notAuthorized";
}

/// SetError types added for `CalendarEvent/send` (draft-ietf-jmap-calendars-28 §7).
pub mod calendar_send_error {
    pub const FORBIDDEN_FROM: &str = "forbiddenFrom";
    pub const PARTICIPANT_NOT_FOUND: &str = "participantNotFound";
    pub const INVALID_PARTICIPANTS: &str = "invalidParticipants";
    pub const CANNOT_SEND_FOR_CALENDAR: &str = "cannotSendForCalendar";
    pub const EVENT_NOT_FOUND: &str = "eventNotFound";
}

/// An RSVP reply to a calendar event invitation (draft-ietf-jmap-calendars-28 §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ParticipantReply {
    pub calendar_event_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<String>,
    pub participation_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_to: Option<BTreeMap<String, String>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParticipantReply {
    pub fn new(calendar_event_id: impl Into<Id>, participation_status: impl Into<String>) -> Self {
        Self {
            calendar_event_id: calendar_event_id.into(),
            recurrence_id: None,
            participation_status: participation_status.into(),
            comment: None,
            send_to: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_recurrence_id(mut self, recurrence_id: impl Into<String>) -> Self {
        self.recurrence_id = Some(recurrence_id.into());
        self
    }

    pub fn with_comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = Some(comment.into());
        self
    }

    pub fn with_send_to(mut self, send_to: BTreeMap<String, String>) -> Self {
        self.send_to = Some(send_to);
        self
    }
}

/// Standard participation status values for participants and replies (draft-ietf-jmap-calendars-28 §10, RFC 8984 §4.4.6).
pub mod participant_participation_status {
    pub const NEEDS_ACTION: &str = "needs-action";
    pub const ACCEPTED: &str = "accepted";
    pub const DECLINED: &str = "declined";
    pub const TENTATIVE: &str = "tentative";
    pub const DELEGATED: &str = "delegated";
}

/// A participant identity (draft-ietf-jmap-calendars-28 §3): how a user is
/// identified when participating in events and scheduling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ParticipantIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<String>,
    /// The URI that represents this participant for iTIP scheduling (draft-ietf-jmap-calendars-28 §3).
    /// Matches participant `calendarAddress` using RFC 3986 §6.2.2 normalization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_to: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParticipantIdentity {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_schedule_id(mut self, schedule_id: impl Into<String>) -> Self {
        self.schedule_id = Some(schedule_id.into());
        self
    }

    pub fn with_calendar_address(mut self, calendar_address: impl Into<String>) -> Self {
        self.calendar_address = Some(calendar_address.into());
        self
    }

    pub fn with_send_to(mut self, send_to: BTreeMap<String, String>) -> Self {
        self.send_to = Some(send_to);
        self
    }

    pub fn with_send_to_method(
        mut self,
        method: impl Into<String>,
        uri: impl Into<String>,
    ) -> Self {
        self.send_to
            .get_or_insert_with(BTreeMap::new)
            .insert(method.into(), uri.into());
        self
    }

    pub fn is_default(mut self, is_default: bool) -> Self {
        self.is_default = Some(is_default);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// The `SetError` types draft-ietf-jmap-calendars-28 §3.2 adds for `ParticipantIdentity/set`.
pub mod participant_identity_set_error {
    pub const CANNOT_DESTROY_DEFAULT: &str = "cannotDestroyDefault";
}

/// `ParticipantIdentity/set` arguments (draft-ietf-jmap-calendars-28 §3.2).
/// Adds the `onSuccessSetIsDefault` argument to the standard `/set` shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ParticipantIdentitySetRequest {
    #[serde(flatten)]
    pub set: SetRequest<ParticipantIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_set_is_default: Option<String>,
}

impl ParticipantIdentitySetRequest {
    pub fn new(set: SetRequest<ParticipantIdentity>) -> Self {
        Self {
            set,
            on_success_set_is_default: None,
        }
    }

    /// `id` may be a plain id or a `#`-prefixed creation id from the same call.
    pub fn setting_default(mut self, id: impl Into<String>) -> Self {
        self.on_success_set_is_default = Some(id.into());
        self
    }
}
