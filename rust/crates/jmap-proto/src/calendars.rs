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
    /// The calendar's id. `None` before the server assigns one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The calendar's display name.
    #[serde(default)]
    pub name: String,
    /// A long-form description of the calendar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A color to display the calendar with, as a CSS color value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Defines the sort order for lists of calendars in a UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<u32>,
    /// Whether this is the default calendar for new events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    /// Whether the user has chosen to see this calendar in a client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_subscribed: Option<bool>,
    /// Whether the calendar's events are currently shown in a client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_visible: Option<bool>,
    /// Whether this calendar's events count toward the owner's availability (calendars draft §4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_in_availability: Option<String>,
    /// The default time zone for this calendar's events.
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
    /// Whether the user may delete this calendar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Calendar {
    /// Create a new `Calendar` with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// Set the calendar's id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the calendar's description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the calendar's display color.
    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// Set the calendar's sort order.
    pub fn with_sort_order(mut self, sort_order: u32) -> Self {
        self.sort_order = Some(sort_order);
        self
    }

    /// Set whether this is the default calendar.
    pub fn is_default(mut self, is_default: bool) -> Self {
        self.is_default = Some(is_default);
        self
    }

    /// Set whether the user is subscribed to this calendar.
    pub fn is_subscribed(mut self, is_subscribed: bool) -> Self {
        self.is_subscribed = Some(is_subscribed);
        self
    }

    /// Set whether the calendar is visible.
    pub fn is_visible(mut self, is_visible: bool) -> Self {
        self.is_visible = Some(is_visible);
        self
    }

    /// Set the calendar's default time zone.
    pub fn with_time_zone(mut self, time_zone: impl Into<String>) -> Self {
        self.time_zone = Some(time_zone.into());
        self
    }

    /// Set the caller's rights on this calendar.
    pub fn with_my_rights(mut self, my_rights: CalendarRights) -> Self {
        self.my_rights = Some(my_rights);
        self
    }

    /// Set the calendar's unmodeled properties.
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
    /// Whether the holder may read free/busy information for this calendar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_read_free_busy: Option<bool>,
    /// Whether the holder may read full event details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_read_items: Option<bool>,
    /// Whether the holder may edit every event in this calendar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_write_all: Option<bool>,
    /// Whether the holder may edit only events they created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_write_own: Option<bool>,
    /// Whether the holder may edit their own per-user properties on events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_update_private: Option<bool>,
    /// Whether the holder may RSVP to events on this calendar.
    #[serde(rename = "mayRSVP", default, skip_serializing_if = "Option::is_none")]
    pub may_rsvp: Option<bool>,
    /// Whether the holder may share this calendar with others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_share: Option<bool>,
    /// Whether the holder may delete this calendar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarRights {
    /// Rights with every permission granted.
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

    /// Rights that only allow reading events and free/busy information.
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

    /// Set the unmodeled properties.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }

    /// Set whether the holder may read free/busy information.
    pub fn with_may_read_free_busy(mut self, value: bool) -> Self {
        self.may_read_free_busy = Some(value);
        self
    }

    /// Set whether the holder may read full event details.
    pub fn with_may_read_items(mut self, value: bool) -> Self {
        self.may_read_items = Some(value);
        self
    }

    /// Set whether the holder may edit every event.
    pub fn with_may_write_all(mut self, value: bool) -> Self {
        self.may_write_all = Some(value);
        self
    }

    /// Set whether the holder may edit only their own events.
    pub fn with_may_write_own(mut self, value: bool) -> Self {
        self.may_write_own = Some(value);
        self
    }

    /// Set whether the holder may edit their own per-user properties.
    pub fn with_may_update_private(mut self, value: bool) -> Self {
        self.may_update_private = Some(value);
        self
    }

    /// Set whether the holder may RSVP to events.
    pub fn with_may_rsvp(mut self, value: bool) -> Self {
        self.may_rsvp = Some(value);
        self
    }

    /// Set whether the holder may share the calendar.
    pub fn with_may_share(mut self, value: bool) -> Self {
        self.may_share = Some(value);
        self
    }

    /// Set whether the holder may delete the calendar.
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
    /// The account to act on.
    pub account_id: Id,
    /// Only apply the request if the account's calendar state matches this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_in_state: Option<crate::state::State>,
    /// Calendars to create, keyed by a client-chosen creation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create: Option<BTreeMap<String, Calendar>>,
    /// Patches to apply to existing calendars, keyed by calendar id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<BTreeMap<Id, Value>>,
    /// Ids of calendars to delete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destroy: Option<Vec<Id>>,
    /// Whether destroying a calendar also destroys its events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_destroy_remove_contents: Option<bool>,
    /// Id of the calendar to mark default once the other changes succeed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_set_is_default: Option<Id>,
}

impl CalendarSetRequest {
    /// Create a new `CalendarSetRequest` for the given account.
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

    /// Set the calendar to mark default once the request succeeds.
    pub fn with_on_success_set_is_default(mut self, id: impl Into<Id>) -> Self {
        self.on_success_set_is_default = Some(id.into());
        self
    }
}

/// A calendar event (draft §5): JSCalendar Event plus JMAP `id` and
/// `calendarIds`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarEvent {
    /// The event's id. `None` before the server assigns one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// Calendars the event belongs to, keyed by calendar id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_ids: Option<BTreeMap<Id, bool>>,
    /// The JSCalendar `@type` discriminator. Always `"Event"`.
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    /// The JSCalendar version this object conforms to (jscalendarbis §3.1.2).
    /// In JMAP Calendars the value is `"2.0"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// A globally unique identifier for the event, shared by its recurrence instances (RFC 8984 §4.1.2).
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
    /// The event's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// A longer description of the event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSCalendar LocalDateTime, interpreted in `time_zone`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    /// The time zone `start` is interpreted in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    /// ISO 8601 duration, e.g. `PT1H`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    /// Whether the event is shown without a time: an all-day event (RFC 8984 §4.1.5).
    /// Defaults to false; `None` preserves omission on save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_without_time: Option<bool>,
    /// The event's confirmation status (RFC 8984 §4.1.3).
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
    /// Whether to use the calendar owner's default alerts instead of `alerts`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_default_alerts: Option<bool>,
    /// A color to display the event with, as a CSS color value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// The language tag the event's text properties are written in.
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
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarEvent {
    /// Create a new, empty `CalendarEvent`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the event's id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Add a calendar the event belongs to.
    pub fn with_calendar_id(mut self, id: impl Into<Id>) -> Self {
        let mut map = self.calendar_ids.unwrap_or_default();
        map.insert(id.into(), true);
        self.calendar_ids = Some(map);
        self
    }

    /// Set the event's uid.
    pub fn with_uid(mut self, uid: impl Into<String>) -> Self {
        self.uid = Some(uid.into());
        self
    }

    /// Set the event's title.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the event's description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the event's start time.
    pub fn with_start(mut self, start: impl Into<String>) -> Self {
        self.start = Some(start.into());
        self
    }

    /// Set the time zone `start` is interpreted in.
    pub fn with_time_zone(mut self, time_zone: impl Into<String>) -> Self {
        self.time_zone = Some(time_zone.into());
        self
    }

    /// Set the event's duration.
    pub fn with_duration(mut self, duration: impl Into<String>) -> Self {
        self.duration = Some(duration.into());
        self
    }

    /// Set the event's confirmation status.
    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    /// Set the event's free/busy status.
    pub fn with_free_busy_status(mut self, free_busy_status: impl Into<String>) -> Self {
        self.free_busy_status = Some(free_busy_status.into());
        self
    }

    /// Set the event's priority.
    pub fn with_priority(mut self, priority: i64) -> Self {
        self.priority = Some(priority);
        self
    }

    /// Set the event's privacy classification.
    pub fn with_privacy(mut self, privacy: impl Into<String>) -> Self {
        self.privacy = Some(privacy.into());
        self
    }

    /// Set whether the event is shown without a time.
    pub fn show_without_time(mut self, show_without_time: bool) -> Self {
        self.show_without_time = Some(show_without_time);
        self
    }

    /// Set whether to use the calendar's default alerts.
    pub fn use_default_alerts(mut self, use_default: bool) -> Self {
        self.use_default_alerts = Some(use_default);
        self
    }

    /// Set the event's display color.
    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// Set the event's locale.
    pub fn with_locale(mut self, locale: impl Into<String>) -> Self {
        self.locale = Some(locale.into());
        self
    }

    /// Set the event's recurrence rule.
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

    /// Set the time zone the event ends in, if different from its start time zone.
    pub fn with_end_time_zone(mut self, end_time_zone: impl Into<String>) -> Self {
        self.end_time_zone = Some(end_time_zone.into());
        self
    }

    /// Set the id of the event's main physical location.
    pub fn with_main_location_id(mut self, main_location_id: impl Into<Id>) -> Self {
        self.main_location_id = Some(main_location_id.into());
        self
    }

    /// Set the sequence number of the event's scheduling status.
    pub fn with_schedule_sequence(mut self, sequence: u32) -> Self {
        self.schedule_sequence = Some(sequence);
        self
    }

    /// Set when the event's scheduling status was last updated.
    pub fn with_schedule_updated(mut self, updated: UtcDate) -> Self {
        self.schedule_updated = Some(updated);
        self
    }

    /// Set whether the user may add themselves as a participant.
    pub fn may_invite_self(mut self, may_invite_self: bool) -> Self {
        self.may_invite_self = Some(may_invite_self);
        self
    }

    /// Set whether the user may invite other participants.
    pub fn may_invite_others(mut self, may_invite_others: bool) -> Self {
        self.may_invite_others = Some(may_invite_others);
        self
    }

    /// Set the event's unmodeled properties.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// JSCalendar RecurrenceRule (RFC 8984 §4.3.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct RecurrenceRule {
    /// The JSCalendar `@type` discriminator. Always `"RecurrenceRule"`.
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub rule_type: Option<String>,
    /// How often the recurrence repeats.
    #[serde(default)]
    pub frequency: String,
    /// The interval between recurrences, in units of `frequency`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<u32>,
    /// The number of occurrences to generate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// The last possible occurrence date, as a `LocalDateTime`.
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
    /// Properties the server sent that this type does not model, round-tripped on save.
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
    /// The JSCalendar `@type` discriminator. Always `"NDay"`.
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub day_type: Option<String>,
    /// The two-letter weekday code (see the `weekday` module).
    #[serde(default)]
    pub day: String,
    /// Which occurrence of `day` within the recurrence period to use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nth_of_period: Option<i32>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl NDay {
    /// Create a new `NDay` for the given weekday.
    pub fn new(day: &str) -> Self {
        Self {
            day_type: Some("NDay".to_owned()),
            day: day.to_owned(),
            nth_of_period: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set which occurrence of the weekday to use.
    pub fn with_nth_of_period(mut self, nth: i32) -> Self {
        self.nth_of_period = Some(nth);
        self
    }

    /// Set the unmodeled properties.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

impl RecurrenceRule {
    /// Create a new `RecurrenceRule` with the given frequency.
    pub fn new(frequency: impl Into<String>) -> Self {
        Self {
            rule_type: Some("RecurrenceRule".to_owned()),
            frequency: frequency.into(),
            ..Self::default()
        }
    }

    /// Set the interval between recurrences.
    pub fn with_interval(mut self, interval: u32) -> Self {
        self.interval = Some(interval);
        self
    }

    /// Set the number of occurrences to generate.
    pub fn with_count(mut self, count: u32) -> Self {
        self.count = Some(count);
        self
    }

    /// Set the last possible occurrence date.
    pub fn with_until(mut self, until: impl Into<String>) -> Self {
        self.until = Some(until.into());
        self
    }

    /// Set the seconds of the minute to recur on.
    pub fn with_by_second(mut self, by_second: impl IntoIterator<Item = u32>) -> Self {
        self.by_second = Some(by_second.into_iter().collect());
        self
    }

    /// Set the minutes of the hour to recur on.
    pub fn with_by_minute(mut self, by_minute: impl IntoIterator<Item = u32>) -> Self {
        self.by_minute = Some(by_minute.into_iter().collect());
        self
    }

    /// Set the hours of the day to recur on.
    pub fn with_by_hour(mut self, by_hour: impl IntoIterator<Item = u32>) -> Self {
        self.by_hour = Some(by_hour.into_iter().collect());
        self
    }

    /// Set the days of the week to recur on.
    pub fn with_by_day(mut self, by_day: impl IntoIterator<Item = NDay>) -> Self {
        self.by_day = Some(by_day.into_iter().collect());
        self
    }

    /// Set the days of the month to recur on.
    pub fn with_by_month_day(mut self, by_month_day: impl IntoIterator<Item = i32>) -> Self {
        self.by_month_day = Some(by_month_day.into_iter().collect());
        self
    }

    /// Set the days of the year to recur on.
    pub fn with_by_year_day(mut self, by_year_day: impl IntoIterator<Item = i32>) -> Self {
        self.by_year_day = Some(by_year_day.into_iter().collect());
        self
    }

    /// Set the weeks of the year to recur on.
    pub fn with_by_week_no(mut self, by_week_no: impl IntoIterator<Item = i32>) -> Self {
        self.by_week_no = Some(by_week_no.into_iter().collect());
        self
    }

    /// Set the months of the year to recur on.
    pub fn with_by_month(mut self, by_month: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.by_month = Some(by_month.into_iter().map(Into::into).collect());
        self
    }

    /// Set which occurrences within the recurrence set to keep.
    pub fn with_by_set_position(mut self, by_set_position: impl IntoIterator<Item = i32>) -> Self {
        self.by_set_position = Some(by_set_position.into_iter().collect());
        self
    }

    /// Set the day each week starts on.
    pub fn with_first_day_of_week(mut self, first_day_of_week: impl Into<String>) -> Self {
        self.first_day_of_week = Some(first_day_of_week.into());
        self
    }

    /// Set the calendar scale the recurrence rule is defined in.
    pub fn with_rscale(mut self, rscale: impl Into<String>) -> Self {
        self.rscale = Some(rscale.into());
        self
    }

    /// Set how to handle occurrences that fall on an invalid date.
    pub fn with_skip(mut self, skip: impl Into<String>) -> Self {
        self.skip = Some(skip.into());
        self
    }

    /// Set the unmodeled properties.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `CalendarEvent/query` filter conditions (draft §5.5). Flat conditions
/// only. `after`/`before` are UTC instants compared against the event start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarEventQueryFilter {
    /// Match only events in this calendar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_calendar: Option<Id>,
    /// Match only events starting at or after this UTC instant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// Match only events starting before this UTC instant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// Match events whose text properties contain this substring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Match events whose title contains this substring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Match events whose description contains this substring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Match events whose location contains this substring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Match the event with this uid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
}

impl CalendarEventQueryFilter {
    /// Create a filter matching events in the given calendar.
    pub fn in_calendar(calendar_id: impl Into<Id>) -> Self {
        Self {
            in_calendar: Some(calendar_id.into()),
            ..Self::default()
        }
    }

    /// Create a filter matching events starting within the given range.
    pub fn time_range(after: impl Into<String>, before: impl Into<String>) -> Self {
        Self {
            after: Some(after.into()),
            before: Some(before.into()),
            ..Self::default()
        }
    }

    /// Set the time range to match.
    pub fn with_time_range(mut self, after: impl Into<String>, before: impl Into<String>) -> Self {
        self.after = Some(after.into());
        self.before = Some(before.into());
        self
    }

    /// Set the earliest start time to match.
    pub fn after(mut self, after: impl Into<String>) -> Self {
        self.after = Some(after.into());
        self
    }

    /// Set the latest start time to match.
    pub fn before(mut self, before: impl Into<String>) -> Self {
        self.before = Some(before.into());
        self
    }

    /// Set the substring to match against text properties.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Set the substring to match against the title.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the substring to match against the description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the substring to match against the location.
    pub fn location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    /// Set the uid to match.
    pub fn uid(mut self, uid: impl Into<String>) -> Self {
        self.uid = Some(uid.into());
        self
    }
}

/// Standard recurrence frequency values (RFC 8984 §4.3.3).
pub mod frequency {
    /// Recur every second.
    pub const SECONDLY: &str = "secondly";
    /// Recur every minute.
    pub const MINUTELY: &str = "minutely";
    /// Recur every hour.
    pub const HOURLY: &str = "hourly";
    /// Recur every day.
    pub const DAILY: &str = "daily";
    /// Recur every week.
    pub const WEEKLY: &str = "weekly";
    /// Recur every month.
    pub const MONTHLY: &str = "monthly";
    /// Recur every year.
    pub const YEARLY: &str = "yearly";
}

/// Standard recurrence skip values (RFC 8984 §4.3.3).
pub mod recurrence_skip {
    /// Drop occurrences that fall on an invalid date.
    pub const OMIT: &str = "omit";
    /// Move an occurrence on an invalid date back to the nearest valid date.
    pub const BACKWARD: &str = "backward";
    /// Move an occurrence on an invalid date forward to the nearest valid date.
    pub const FORWARD: &str = "forward";
}

/// Standard two-letter weekday codes (RFC 8984 §4.3.3).
pub mod weekday {
    /// Monday.
    pub const MO: &str = "mo";
    /// Tuesday.
    pub const TU: &str = "tu";
    /// Wednesday.
    pub const WE: &str = "we";
    /// Thursday.
    pub const TH: &str = "th";
    /// Friday.
    pub const FR: &str = "fr";
    /// Saturday.
    pub const SA: &str = "sa";
    /// Sunday.
    pub const SU: &str = "su";
}

/// The `SetError` type draft-ietf-jmap-calendars §4.4 adds for `Calendar/set`.
pub mod calendar_set_error {
    /// The calendar still has events and `onDestroyRemoveContents` was not set.
    pub const HAS_EVENT: &str = "calendarHasEvent";
}

/// Well-known values for `Calendar.includeInAvailability` (draft-ietf-jmap-calendars §4.1).
pub mod include_in_availability {
    /// Count every event when reporting availability.
    pub const ALL: &str = "all";
    /// Count every event except ones the user declined.
    pub const ALL_EXCEPT_DECLINED: &str = "allExceptDeclined";
    /// Never count this calendar's events when reporting availability.
    pub const NONE: &str = "none";
}

/// Standard JSCalendar (RFC 8984 §5.1.3 / jscalendarbis §4.1.4) event status values.
pub mod event_status {
    /// The event is confirmed.
    pub const CONFIRMED: &str = "confirmed";
    /// The event is tentative.
    pub const TENTATIVE: &str = "tentative";
    /// The event is cancelled.
    pub const CANCELLED: &str = "cancelled";
}

/// Standard JSCalendar (RFC 8984 §4.4.2) / jscalendarbis free/busy status values.
pub mod free_busy_status {
    /// The event does not block time.
    pub const FREE: &str = "free";
    /// The event blocks time.
    pub const BUSY: &str = "busy";
}

/// Standard JSCalendar (RFC 8984 §4.4.3) / jscalendarbis privacy classification values.
pub mod privacy {
    /// The event is visible to anyone who can see the calendar.
    pub const PUBLIC: &str = "public";
    /// The event's details are hidden from other users, but its existence is visible.
    pub const PRIVATE: &str = "private";
    /// The event is hidden entirely from other users.
    pub const SECRET: &str = "secret";
}

/// Standard JSCalendar (RFC 8984 §4.4.6 / jscalendarbis §3.4.6) participant roles.
pub mod participant_role {
    /// The participant owns the event.
    pub const OWNER: &str = "owner";
    /// The participant may administer the event on the owner's behalf.
    pub const ADMIN: &str = "admin";
    /// The participant's attendance is required.
    pub const REQUIRED: &str = "required";
    /// The participant is an attendee.
    pub const ATTENDEE: &str = "attendee";
    /// The participant's attendance is optional.
    pub const OPTIONAL: &str = "optional";
    /// The participant is informed but not expected to attend.
    pub const INFORMATIONAL: &str = "informational";
}

/// Standard JSCalendar (RFC 8984 §4.4.6) participation status values.
pub mod participation_status {
    /// The participant has not yet responded.
    pub const NEEDS_ACTION: &str = "needs-action";
    /// The participant accepted the invitation.
    pub const ACCEPTED: &str = "accepted";
    /// The participant declined the invitation.
    pub const DECLINED: &str = "declined";
    /// The participant tentatively accepted the invitation.
    pub const TENTATIVE: &str = "tentative";
    /// The participant delegated their attendance to someone else.
    pub const DELEGATED: &str = "delegated";
}

/// Standard JSCalendar (RFC 8984 §4.4.6) participant kinds.
pub mod participant_kind {
    /// The participant is a person.
    pub const INDIVIDUAL: &str = "individual";
    /// The participant is a group of people.
    pub const GROUP: &str = "group";
    /// The participant is a non-location resource, such as a projector.
    pub const RESOURCE: &str = "resource";
    /// The participant is a physical location.
    pub const LOCATION: &str = "location";
}

/// Standard JSCalendar (RFC 8984 §4.5.2) alert action types.
pub mod alert_action {
    /// Show the alert in the client's user interface.
    pub const DISPLAY: &str = "display";
    /// Send the alert by email.
    pub const EMAIL: &str = "email";
}

/// Standard JSCalendar (RFC 8984 §4.5.2) offset trigger relation.
pub mod relative_to {
    /// The offset is relative to the event's start.
    pub const START: &str = "start";
    /// The offset is relative to the event's end.
    pub const END: &str = "end";
}

/// Standard JSCalendar (RFC 8984 §4.4.6) schedule agent values.
pub mod schedule_agent {
    /// The server sends scheduling messages for this participant.
    pub const SERVER: &str = "server";
    /// The client sends scheduling messages for this participant.
    pub const CLIENT: &str = "client";
    /// No one sends scheduling messages for this participant.
    pub const NONE: &str = "none";
}

/// Standard JSCalendar (RFC 8984 §4.4.6) participant progress values.
pub mod participant_progress {
    /// The participant has not started the task.
    pub const NEEDS_ACTION: &str = "needs-action";
    /// The participant is working on the task.
    pub const IN_PROCESS: &str = "in-process";
    /// The participant completed the task.
    pub const COMPLETED: &str = "completed";
    /// The participant could not complete the task.
    pub const FAILED: &str = "failed";
}

/// Standard JSCalendar (RFC 8984 §4.4.6) participant attendance values.
pub mod participant_attendance {
    /// The participant's attendance is required.
    pub const REQUIRED: &str = "required";
    /// The participant's attendance is optional.
    pub const OPTIONAL: &str = "optional";
    /// The participant is informed but not expected to attend.
    pub const INFORMATIONAL: &str = "informational";
}

/// JSCalendar Participant (jscalendarbis §3.4.6 / RFC 8984 §4.4.6): an attendee
/// or organizer of the event.
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct Participant {
    /// The JSCalendar `@type` discriminator. Always `"Participant"`.
    pub participant_type: Option<String>,
    /// The participant's display name.
    pub name: Option<String>,
    /// The participant's email address.
    pub email: Option<String>,
    /// A description of the participant's role or relation to the event.
    pub description: Option<String>,
    /// The media type of `description`, if not plain text.
    pub description_content_type: Option<String>,
    /// The URI that represents this participant for calendar invitations and scheduling
    /// (jscalendarbis §3.4.6). Replaces RFC 8984's reserved `sendTo` property.
    pub calendar_address: Option<String>,
    /// Legacy RFC 8984 addressing methods. Tolerated on input; never emitted on output.
    pub send_to: Option<BTreeMap<String, String>>,
    /// What kind of entity the participant is (see the `participant_kind` module).
    pub kind: Option<String>,
    /// The participant's roles, keyed by role name with value `true` (see the `participant_role` module).
    pub roles: Option<BTreeMap<String, bool>>,
    /// Id of the location (in `CalendarEvent.locations`) this participant represents, for a `location`-kind participant.
    pub location_id: Option<String>,
    /// The participant's current RSVP status (see the `participation_status` module).
    pub participation_status: Option<String>,
    /// A comment the participant attached to their RSVP.
    pub participation_comment: Option<String>,
    /// Whether the participant's attendance is required (see the `participant_attendance` module).
    pub attendance: Option<String>,
    /// Whether the organizer expects an RSVP from this participant.
    pub expect_reply: Option<bool>,
    /// Who sends scheduling messages for this participant (see the `schedule_agent` module).
    pub schedule_agent: Option<String>,
    /// The last event sequence number the participant received.
    pub schedule_sequence: Option<u32>,
    /// iTIP delivery status codes for this participant's scheduling messages.
    pub schedule_status: Option<Vec<String>>,
    /// When the participant's scheduling status was last updated.
    pub schedule_updated: Option<UtcDate>,
    /// Calendar addresses this participant delegated their attendance to.
    pub delegated_to: Option<Vec<String>>,
    /// Calendar addresses that delegated their attendance to this participant.
    pub delegated_from: Option<Vec<String>>,
    /// Groups this participant is a member of, keyed by calendar address.
    pub member_of: Option<BTreeMap<String, bool>>,
    /// The participant's progress on a task-like event (see the `participant_progress` module).
    pub progress: Option<String>,
    /// When `progress` was last updated.
    pub progress_updated: Option<UtcDate>,
    /// Properties the server sent that this type does not model, round-tripped on save.
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
    /// Create a new `Participant` with the given name and email.
    pub fn new(name: impl Into<String>, email: impl Into<String>) -> Self {
        Self {
            participant_type: Some("Participant".to_owned()),
            name: Some(name.into()),
            email: Some(email.into()),
            ..Self::default()
        }
    }

    /// Set the participant's roles.
    pub fn with_roles(mut self, roles: BTreeMap<String, bool>) -> Self {
        self.roles = Some(roles);
        self
    }

    /// Set the participant's RSVP status.
    pub fn with_participation_status(mut self, status: impl Into<String>) -> Self {
        self.participation_status = Some(status.into());
        self
    }

    /// Set the participant's RSVP comment.
    pub fn with_participation_comment(mut self, comment: impl Into<String>) -> Self {
        self.participation_comment = Some(comment.into());
        self
    }

    /// Set the participant's attendance requirement.
    pub fn with_attendance(mut self, attendance: impl Into<String>) -> Self {
        self.attendance = Some(attendance.into());
        self
    }

    /// Set whether the organizer expects an RSVP from this participant.
    pub fn expect_reply(mut self, expect: bool) -> Self {
        self.expect_reply = Some(expect);
        self
    }

    /// Set who sends scheduling messages for this participant.
    pub fn with_schedule_agent(mut self, schedule_agent: impl Into<String>) -> Self {
        self.schedule_agent = Some(schedule_agent.into());
        self
    }

    /// Set the participant's calendar address, recorded for `imip` delivery too.
    pub fn with_calendar_address(mut self, address: impl Into<String>) -> Self {
        let addr = address.into();
        self.calendar_address = Some(addr.clone());
        let mut map = self.send_to.unwrap_or_default();
        map.insert("imip".to_owned(), addr);
        self.send_to = Some(map);
        self
    }

    /// Set the media type of the participant's description.
    pub fn with_description_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.description_content_type = Some(content_type.into());
        self
    }

    /// Add a scheduling delivery method and address for this participant.
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

    /// Set the participant's unmodeled properties.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }

    /// Set the participant's description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set what kind of entity the participant is.
    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// Set the id of the location this participant represents.
    pub fn with_location_id(mut self, location_id: impl Into<String>) -> Self {
        self.location_id = Some(location_id.into());
        self
    }

    /// Set the last event sequence number the participant received.
    pub fn with_schedule_sequence(mut self, schedule_sequence: u32) -> Self {
        self.schedule_sequence = Some(schedule_sequence);
        self
    }

    /// Set the iTIP delivery status codes for this participant.
    pub fn with_schedule_status(
        mut self,
        schedule_status: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.schedule_status = Some(schedule_status.into_iter().map(Into::into).collect());
        self
    }

    /// Set when the participant's scheduling status was last updated.
    pub fn with_schedule_updated(mut self, schedule_updated: impl Into<UtcDate>) -> Self {
        self.schedule_updated = Some(schedule_updated.into());
        self
    }

    /// Set the calendar addresses this participant delegated to.
    pub fn with_delegated_to(
        mut self,
        delegated_to: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.delegated_to = Some(delegated_to.into_iter().map(Into::into).collect());
        self
    }

    /// Set the calendar addresses that delegated to this participant.
    pub fn with_delegated_from(
        mut self,
        delegated_from: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.delegated_from = Some(delegated_from.into_iter().map(Into::into).collect());
        self
    }

    /// Set the groups this participant is a member of.
    pub fn with_member_of(mut self, member_of: BTreeMap<String, bool>) -> Self {
        self.member_of = Some(member_of);
        self
    }

    /// Set the participant's progress on a task-like event.
    pub fn with_progress(mut self, progress: impl Into<String>) -> Self {
        self.progress = Some(progress.into());
        self
    }

    /// Set when the participant's progress was last updated.
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
    /// The JSCalendar `@type` discriminator. Always `"Location"`.
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub location_type: Option<String>,
    /// The location's display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A longer description of the location.
    #[serde(default, skip_serializing)]
    pub description: Option<String>,
    /// What the location's time zone is relative to, for a multi-location event (RFC 8984 §4.2.5).
    #[serde(default, skip_serializing)]
    pub relative_to: Option<String>,
    /// The location's time zone, if different from the event's.
    #[serde(default, skip_serializing)]
    pub time_zone: Option<String>,
    /// A `geo:` URI giving the location's coordinates (RFC 5870).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinates: Option<String>,
    /// Well-known types describing this location, keyed by type name with value `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_types: Option<BTreeMap<String, bool>>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Location {
    /// Create a new `Location` with the given name.
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

    /// Set the location's description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the location's time zone.
    pub fn with_time_zone(mut self, time_zone: impl Into<String>) -> Self {
        self.time_zone = Some(time_zone.into());
        self
    }

    /// Set the location's coordinates.
    pub fn with_coordinates(mut self, coordinates: impl Into<String>) -> Self {
        self.coordinates = Some(coordinates.into());
        self
    }

    /// Set what the location's time zone is relative to.
    pub fn with_relative_to(mut self, relative_to: impl Into<String>) -> Self {
        self.relative_to = Some(relative_to.into());
        self
    }

    /// Set the location's well-known types.
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
    /// The JSCalendar `@type` discriminator. Always `"VirtualLocation"`.
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub virtual_location_type: Option<String>,
    /// The virtual location's display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A longer description of the virtual location.
    #[serde(default, skip_serializing)]
    pub description: Option<String>,
    /// The URI to join the virtual location, such as a conference link.
    #[serde(default)]
    pub uri: String,
    /// Features this virtual location supports, keyed by feature name with value `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<BTreeMap<String, bool>>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl VirtualLocation {
    /// Create a new `VirtualLocation` with the given URI.
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

    /// Set the virtual location's display name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the virtual location's description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the virtual location's features.
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
    /// The JSCalendar `@type` discriminator. Always `"Link"`.
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub link_type: Option<String>,
    /// The link's URI.
    #[serde(default)]
    pub href: String,
    /// The media type of the linked resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// The size in bytes of the linked resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The relation of the linked resource to the event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel: Option<String>,
    /// How to display the link, keyed by display hint with value `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<BTreeMap<String, bool>>,
    /// A human-readable title for the link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Obsoleted RFC 8984 property; tolerated on deserialization, never emitted on serialization.
    #[serde(default, skip_serializing)]
    pub cid: Option<String>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Link {
    /// Create a new `Link` with the given URI.
    pub fn new(href: impl Into<String>) -> Self {
        Self {
            link_type: Some("Link".to_owned()),
            href: href.into(),
            ..Default::default()
        }
    }

    /// Set the linked resource's media type.
    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Set the linked resource's size in bytes.
    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    /// Set the link's relation to the event.
    pub fn with_rel(mut self, rel: impl Into<String>) -> Self {
        self.rel = Some(rel.into());
        self
    }

    /// Set how to display the link.
    pub fn with_display(mut self, display: BTreeMap<String, bool>) -> Self {
        self.display = Some(display);
        self
    }

    /// Set the link's title.
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
    /// The JSCalendar `@type` discriminator. Always `"Alert"`.
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub alert_type: Option<String>,
    /// What the alert does when it triggers (see the `alert_action` module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// When the alert triggers: an `OffsetTrigger` or `AbsoluteTrigger`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Value>,
    /// When the user last acknowledged this alert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledged: Option<UtcDate>,
    /// Id of another alert this one is related to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related_to: Option<String>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Alert {
    /// Create a new `Alert` with the given action and trigger.
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

    /// Set when the alert was last acknowledged.
    pub fn with_acknowledged(mut self, acknowledged: impl Into<UtcDate>) -> Self {
        self.acknowledged = Some(acknowledged.into());
        self
    }

    /// Set the id of a related alert.
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
    /// The JSCalendar `@type` discriminator. Always `"OffsetTrigger"`.
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub trigger_type: Option<String>,
    /// The offset duration before or after the related time.
    pub offset: String,
    /// Whether the offset is relative to the event's start or end (see the `relative_to` module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_to: Option<String>,
}

impl OffsetTrigger {
    /// Create a new `OffsetTrigger` with the given offset.
    pub fn new(offset: impl Into<String>) -> Self {
        Self {
            trigger_type: Some("OffsetTrigger".to_owned()),
            offset: offset.into(),
            relative_to: None,
        }
    }

    /// Set whether the offset is relative to the event's start or end.
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
    /// The preferences object's id. Always `"singleton"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The user's preferred time zone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    /// The user's preferred first day of the week.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_day_of_week: Option<String>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarPreferences {
    /// Create a new, empty `CalendarPreferences`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the preferences object's id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the user's preferred time zone.
    pub fn with_time_zone(mut self, time_zone: impl Into<String>) -> Self {
        self.time_zone = Some(time_zone.into());
        self
    }

    /// Set the user's preferred first day of the week.
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
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarPreferencesCapability {
    /// Create a new, empty `CalendarPreferencesCapability`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Merge in unmodeled capability properties.
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
    /// The standard `/set` arguments.
    #[serde(flatten)]
    pub set: SetRequest<CalendarEvent>,
    /// If true, the server sends iTIP scheduling messages after applying changes.
    /// Defaults to false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_scheduling_messages: Option<bool>,
}

impl CalendarEventSetRequest {
    /// Create a new `CalendarEventSetRequest` wrapping the given `/set` arguments.
    pub fn new(set: SetRequest<CalendarEvent>) -> Self {
        Self {
            set,
            send_scheduling_messages: None,
        }
    }

    /// Set whether the server sends iTIP scheduling messages after this request.
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
    /// The account to act on.
    pub account_id: Id,
    /// Blob ids of the iCalendar data to parse.
    pub blob_ids: Vec<Id>,
    /// Which `CalendarEvent` properties to return. Defaults to all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<String>>,
}

impl CalendarEventParseRequest {
    /// Create a new `CalendarEventParseRequest` for the given account and blobs.
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

    /// Set which properties to return.
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
    /// The account acted on.
    pub account_id: Id,
    /// Parsed events, keyed by the blob id they came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parsed: Option<BTreeMap<Id, CalendarEvent>>,
    /// Blob ids that could not be parsed as a calendar event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_parsable: Option<Vec<Id>>,
    /// Blob ids that do not exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_found: Option<Vec<Id>>,
}

impl CalendarEventParseResponse {
    /// Create a new `CalendarEventParseResponse` for the given account.
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            parsed: None,
            not_parsable: None,
            not_found: None,
        }
    }

    /// Set the parsed events.
    pub fn with_parsed(mut self, parsed: BTreeMap<Id, CalendarEvent>) -> Self {
        self.parsed = Some(parsed);
        self
    }

    /// Set the blob ids that could not be parsed.
    pub fn with_not_parsable(
        mut self,
        not_parsable: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        self.not_parsable = Some(not_parsable.into_iter().map(Into::into).collect());
        self
    }

    /// Set the blob ids that do not exist.
    pub fn with_not_found(mut self, not_found: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.not_found = Some(not_found.into_iter().map(Into::into).collect());
        self
    }
}

/// Calendars capability properties (draft-ietf-jmap-calendars-33 §1.5.1).
///
/// Every field is optional on the wire, so a server that leaves one out still parses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarsCapability {
    /// The maximum number of calendars one event can be in. Absent or null means no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_calendars_per_event: Option<u64>,
    /// The earliest date-time (a LocalDateTime) the server accepts for an event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_date_time: Option<String>,
    /// The latest date-time (a LocalDateTime) the server accepts for an event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_date_time: Option<String>,
    /// The maximum duration (a Duration) of a `CalendarEvent/query` with `expandRecurrences`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_expanded_query_duration: Option<String>,
    /// The maximum number of participants one event can have. Absent or null means no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_participants_per_event: Option<u64>,
    /// True if the user can create a calendar in this account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_create_calendar: Option<bool>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarsCapability {
    /// Create a new `CalendarsCapability` with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set `max_calendars_per_event`.
    pub fn with_max_calendars_per_event(mut self, v: u64) -> Self {
        self.max_calendars_per_event = Some(v);
        self
    }

    /// Set `min_date_time`.
    pub fn with_min_date_time(mut self, v: impl Into<String>) -> Self {
        self.min_date_time = Some(v.into());
        self
    }

    /// Set `max_date_time`.
    pub fn with_max_date_time(mut self, v: impl Into<String>) -> Self {
        self.max_date_time = Some(v.into());
        self
    }

    /// Set `max_expanded_query_duration`.
    pub fn with_max_expanded_query_duration(mut self, v: impl Into<String>) -> Self {
        self.max_expanded_query_duration = Some(v.into());
        self
    }

    /// Set `max_participants_per_event`.
    pub fn with_max_participants_per_event(mut self, v: u64) -> Self {
        self.max_participants_per_event = Some(v);
        self
    }

    /// Set `may_create_calendar`.
    pub fn with_may_create_calendar(mut self, v: bool) -> Self {
        self.may_create_calendar = Some(v);
        self
    }

    /// Set the capability's unmodeled properties.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Standard RFC 8984 §1.4.10 event relation types.
pub mod event_relation_type {
    /// This event is the first in the series.
    pub const FIRST: &str = "first";
    /// This event follows the related event.
    pub const NEXT: &str = "next";
    /// This event is a parent of the related event.
    pub const PARENT: &str = "parent";
    /// This event is a child of the related event.
    pub const CHILD: &str = "child";
}

/// Standard RFC 8984 §4.4.1 / RFC 5545 §3.8.1.9 priority constants.
pub mod priority {
    /// No priority is set.
    pub const UNDEFINED: i64 = 0;
    /// The highest priority.
    pub const HIGH: i64 = 1;
    /// A medium priority.
    pub const MEDIUM: i64 = 5;
    /// The lowest priority.
    pub const LOW: i64 = 9;
}

/// JSCalendar AbsoluteTrigger (RFC 8984 §4.5.2): an alert trigger specified as an absolute UTC timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AbsoluteTrigger {
    /// The JSCalendar `@type` discriminator. Always `"AbsoluteTrigger"`.
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub trigger_type: Option<String>,
    /// The UTC instant the alert triggers at.
    pub when: UtcDate,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl AbsoluteTrigger {
    /// Create a new `AbsoluteTrigger` for the given instant.
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
    /// The JSCalendar `@type` discriminator. Always `"Relation"`.
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub relation_type: Option<String>,
    /// The relation types to the other event, keyed by type name with value `true` (see the `event_relation_type` module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<BTreeMap<String, bool>>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl EventRelation {
    /// Create a new `EventRelation` with the given relation types.
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
    /// The account to act on.
    pub account_id: Id,
    /// Calendars to include. Defaults to every calendar the account can read free/busy for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_ids: Option<Vec<Id>>,
    /// Start of the range to report free/busy information for.
    pub utc_start: UtcDate,
    /// End of the range to report free/busy information for.
    pub utc_end: UtcDate,
    /// Time zone to interpret the returned blocks in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
}

impl GetFreeBusyRequest {
    /// Create a new `GetFreeBusyRequest` for the given account and range.
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

    /// Set the calendars to include.
    pub fn calendar_ids(mut self, ids: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.calendar_ids = Some(ids.into_iter().map(Into::into).collect());
        self
    }

    /// Set the time zone to interpret the response in.
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
    /// Start of this free/busy block.
    pub utc_start: UtcDate,
    /// End of this free/busy block.
    pub utc_end: UtcDate,
    /// The busy status for this block (see the `calendar_free_busy_status` module).
    pub busy_status: String,
    /// Id of the calendar the block came from, if the caller may see it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_id: Option<Id>,
    /// Id of the event the block came from, if the caller may see it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<Id>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl FreeBusyBlock {
    /// Create a new `FreeBusyBlock` for the given range and busy status.
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

    /// Set the id of the calendar the block came from.
    pub fn with_calendar_id(mut self, calendar_id: impl Into<Id>) -> Self {
        self.calendar_id = Some(calendar_id.into());
        self
    }

    /// Set the id of the event the block came from.
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
    /// The account acted on.
    pub account_id: Id,
    /// The free/busy blocks in the requested range.
    #[serde(default)]
    pub list: Vec<FreeBusyBlock>,
}

impl GetFreeBusyResponse {
    /// Create a new `GetFreeBusyResponse` for the given account and blocks.
    pub fn new(account_id: impl Into<Id>, list: impl IntoIterator<Item = FreeBusyBlock>) -> Self {
        Self {
            account_id: account_id.into(),
            list: list.into_iter().collect(),
        }
    }
}

/// The `SetError` types draft-ietf-jmap-calendars-28 §5.9 adds for `CalendarEvent/set`.
pub mod calendar_event_set_error {
    /// A referenced blob id does not exist.
    pub const BLOB_NOT_FOUND: &str = "blobNotFound";
    /// The event has more participants than the server allows.
    pub const TOO_MANY_PARTICIPANTS: &str = "tooManyParticipants";
    /// The recurrence rule generates more instances than the server allows.
    pub const TOO_MANY_RECURRENCES: &str = "tooManyRecurrences";
    /// The server could not calculate the recurrence's occurrences.
    pub const CANNOT_CALCULATE_OCCURRENCES: &str = "cannotCalculateOccurrences";
    /// None of the participants support a scheduling method the server can use.
    pub const NO_SUPPORTED_SCHEDULE_METHODS: &str = "noSupportedScheduleMethods";
}

/// Standard free/busy status values (draft-ietf-jmap-calendars-28 §5.7).
pub mod calendar_free_busy_status {
    /// The block does not block time.
    pub const FREE: &str = "free";
    /// The block blocks time.
    pub const BUSY: &str = "busy";
    /// The block tentatively blocks time.
    pub const BUSY_TENTATIVE: &str = "busy-tentative";
    /// The block blocks time and the owner is unavailable.
    pub const BUSY_UNAVAILABLE: &str = "busy-unavailable";
}

/// Group entries representation (RFC 8984 §5.3.1), supporting both the standard
/// array of calendar objects and keyed maps.
/// Stays exhaustive: an untagged two-case type fixed by the RFC, not a type a server extends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GroupEntries {
    /// Entries as an ordered array of calendar objects (RFC 8984 §5.3.1).
    Array(Vec<Value>),
    /// Entries as a map of calendar objects keyed by id.
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
    /// The group's id. `None` before the server assigns one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The JSCalendar `@type` discriminator. Always `"Group"`.
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub group_type: Option<String>,
    /// A globally unique identifier for the group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    /// The group's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// A longer description of the group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// When the group was last changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<UtcDate>,
    /// The default time zone for the group's entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    /// The calendar objects in this group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entries: Option<GroupEntries>,
    /// Keywords or tags for the group, keyed by tag with value `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<BTreeMap<String, Value>>,
    /// Categories the group belongs to, keyed by category with value `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<BTreeMap<String, bool>>,
    /// A color to display the group with, as a CSS color value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// A URI the group was imported from or subscribes to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// External resources related to the group, keyed by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<BTreeMap<String, Value>>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarGroup {
    /// Create a new `CalendarGroup` with the given title.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            group_type: Some("Group".to_owned()),
            title: Some(title.into()),
            ..Self::default()
        }
    }

    /// Set the group's id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the group's uid.
    pub fn with_uid(mut self, uid: impl Into<String>) -> Self {
        self.uid = Some(uid.into());
        self
    }

    /// Set the group's title.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the group's description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the group's default time zone.
    pub fn with_time_zone(mut self, time_zone: impl Into<String>) -> Self {
        self.time_zone = Some(time_zone.into());
        self
    }

    /// Set when the group was last changed.
    pub fn with_updated(mut self, updated: impl Into<UtcDate>) -> Self {
        self.updated = Some(updated.into());
        self
    }

    /// Set the group's calendar objects.
    pub fn with_entries(mut self, entries: impl Into<GroupEntries>) -> Self {
        self.entries = Some(entries.into());
        self
    }

    /// Set the group's keywords.
    pub fn with_keywords(mut self, keywords: BTreeMap<String, Value>) -> Self {
        self.keywords = Some(keywords);
        self
    }

    /// Set the group's categories.
    pub fn with_categories(mut self, categories: BTreeMap<String, bool>) -> Self {
        self.categories = Some(categories);
        self
    }

    /// Set the group's display color.
    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// Set the URI the group was imported from.
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Set the group's external resources.
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
    /// The JSCalendar `@type` discriminator. Always `"CalendarAlert"`.
    #[serde(rename = "@type", default = "default_calendar_alert_type")]
    pub kind: String,
    /// The account the alert fired in.
    pub account_id: Id,
    /// Id of the event the alert is for.
    pub calendar_event_id: Id,
    /// The event's uid, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    /// Which recurrence instance the alert is for, if the event recurs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<String>,
    /// Id of the `Alert` within the event that fired.
    pub alert_id: Id,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarAlert {
    /// The `@type` discriminator value for this push notification.
    pub const TYPE: &'static str = "CalendarAlert";

    /// Create a new `CalendarAlert` for the given account, event, and alert id.
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

    /// Set the event's uid.
    pub fn with_uid(mut self, uid: impl Into<String>) -> Self {
        self.uid = Some(uid.into());
        self
    }

    /// Set which recurrence instance the alert is for.
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
    /// The notification's id. `None` before the server assigns one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The JSCalendar `@type` discriminator. Always `"CalendarEventNotification"`.
    #[serde(rename = "@type", default = "default_calendar_event_notification_type")]
    pub kind: String,
    /// When the notification was created.
    pub created: UtcDate,
    /// The principal who made the change, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_by: Option<Principal>,
    /// A comment the change included, such as a scheduling reply comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// What kind of change this notification reports (see the `calendar_event_notification_type` module).
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub notification_type: Option<String>,
    /// Id of the event that changed.
    pub event_id: Id,
    /// Which recurrence instance changed, if the event recurs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<String>,
    /// A snapshot of the event after the change, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<CalendarEvent>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarEventNotification {
    /// The `@type` discriminator value for this notification.
    pub const TYPE: &'static str = "CalendarEventNotification";

    /// Create a new `CalendarEventNotification` for the given creation time and event.
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

    /// Set the notification's id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the notification's `@type` discriminator.
    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = kind.into();
        self
    }

    /// Set the principal who made the change.
    pub fn with_changed_by(mut self, changed_by: Principal) -> Self {
        self.changed_by = Some(changed_by);
        self
    }

    /// Set the comment attached to the change.
    pub fn with_comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = Some(comment.into());
        self
    }

    /// Set what kind of change this notification reports.
    pub fn with_notification_type(mut self, notification_type: impl Into<String>) -> Self {
        self.notification_type = Some(notification_type.into());
        self
    }

    /// Set which recurrence instance changed.
    pub fn with_recurrence_id(mut self, recurrence_id: impl Into<String>) -> Self {
        self.recurrence_id = Some(recurrence_id.into());
        self
    }

    /// Set the snapshot of the changed event.
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
    /// Match only notifications created at or after this instant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<UtcDate>,
    /// Match only notifications created before this instant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<UtcDate>,
    /// Match only notifications of these types (see the `calendar_event_notification_type` module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<String>>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarEventNotificationQueryFilter {
    /// Create a new, empty `CalendarEventNotificationQueryFilter`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the earliest creation time to match.
    pub fn with_after(mut self, after: impl Into<UtcDate>) -> Self {
        self.after = Some(after.into());
        self
    }

    /// Set the latest creation time to match.
    pub fn with_before(mut self, before: impl Into<UtcDate>) -> Self {
        self.before = Some(before.into());
        self
    }

    /// Set the notification types to match.
    pub fn with_types(mut self, types: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.types = Some(types.into_iter().map(Into::into).collect());
        self
    }
}

/// Well-known notification types for calendar event notifications (draft-ietf-jmap-calendars-28 §8).
pub mod calendar_event_notification_type {
    /// The event was created.
    pub const CREATED: &str = "created";
    /// The event was updated.
    pub const UPDATED: &str = "updated";
    /// The event was destroyed.
    pub const DESTROYED: &str = "destroyed";
    /// A participant replied to the event's invitation.
    pub const REPLY: &str = "reply";
}

/// Arguments for `CalendarEvent/send` (draft-ietf-jmap-calendars-28 §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CalendarEventSendRequest {
    /// The account to act on.
    pub account_id: Id,
    /// The identity to send the event as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_id: Option<Id>,
    /// Events to send, keyed by a client-chosen creation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send: Option<BTreeMap<Id, SendCalendarEvent>>,
    /// Patches to apply to calendar events after a successful send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_update_calendar_event: Option<BTreeMap<Id, Value>>,
    /// Ids of calendar events to destroy after a successful send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_destroy_calendar_event_ids: Option<Vec<Id>>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarEventSendRequest {
    /// Create a new `CalendarEventSendRequest` for the given account.
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

    /// Set the identity to send as.
    pub fn with_identity_id(mut self, identity_id: impl Into<Id>) -> Self {
        self.identity_id = Some(identity_id.into());
        self
    }

    /// Set the events to send.
    pub fn with_send(mut self, send: BTreeMap<Id, SendCalendarEvent>) -> Self {
        self.send = Some(send);
        self
    }

    /// Set the patches to apply after a successful send.
    pub fn with_on_success_update_calendar_event(mut self, updates: BTreeMap<Id, Value>) -> Self {
        self.on_success_update_calendar_event = Some(updates);
        self
    }

    /// Set the calendar event ids to destroy after a successful send.
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
    /// The calendar address to send the event to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient: Option<String>,
    /// Delivery addresses for the recipient, keyed by method.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_to: Option<BTreeMap<String, String>>,
    /// The event to send. Defaults to the referenced event unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_event: Option<CalendarEvent>,
    /// Whether to include the event's properties before this change, for a `CANCEL` or update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_old_properties: Option<bool>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SendCalendarEvent {
    /// Create a new, empty `SendCalendarEvent`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the calendar address to send to.
    pub fn with_recipient(mut self, recipient: impl Into<String>) -> Self {
        self.recipient = Some(recipient.into());
        self
    }

    /// Set the recipient's delivery addresses.
    pub fn with_send_to(mut self, send_to: BTreeMap<String, String>) -> Self {
        self.send_to = Some(send_to);
        self
    }

    /// Set the event to send.
    pub fn with_calendar_event(mut self, calendar_event: CalendarEvent) -> Self {
        self.calendar_event = Some(calendar_event);
        self
    }

    /// Set whether to include the event's prior properties.
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
    /// The account acted on.
    pub account_id: Id,
    /// Results for events that were sent, keyed by creation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent: Option<BTreeMap<Id, SendCalendarEventResult>>,
    /// Errors for events that were not sent, keyed by creation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_sent: Option<BTreeMap<Id, SetError>>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarEventSendResponse {
    /// Create a new `CalendarEventSendResponse` for the given account.
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            sent: None,
            not_sent: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set the results for sent events.
    pub fn with_sent(mut self, sent: BTreeMap<Id, SendCalendarEventResult>) -> Self {
        self.sent = Some(sent);
        self
    }

    /// Set the errors for events that were not sent.
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
    /// The outcome of sending the event to this recipient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_status: Option<String>,
    /// Problems delivering to individual participants, keyed by participant id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participant_problems: Option<BTreeMap<String, ParticipantProblem>>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SendCalendarEventResult {
    /// Create a new, empty `SendCalendarEventResult`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the outcome of sending the event.
    pub fn with_send_status(mut self, status: impl Into<String>) -> Self {
        self.send_status = Some(status.into());
        self
    }

    /// Set the delivery problems for individual participants.
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
    /// What kind of delivery problem occurred (see the `participant_problem_kind` module).
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// A human-readable description of the problem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParticipantProblem {
    /// Create a new `ParticipantProblem` of the given kind.
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: Some(kind.into()),
            description: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set the problem's description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// Standard participant problem error types (draft-ietf-jmap-calendars-28 §7.3).
pub mod participant_problem_kind {
    /// The recipient is the sender's own calendar address.
    pub const CANNOT_SEND_TO_SELF: &str = "cannotSendToSelf";
    /// The referenced calendar does not exist.
    pub const CALENDAR_NOT_FOUND: &str = "calendarNotFound";
    /// The referenced participant does not exist on the event.
    pub const PARTICIPANT_NOT_FOUND: &str = "participantNotFound";
    /// The recipient's email address is invalid.
    pub const INVALID_EMAIL: &str = "invalidEmail";
    /// The recipient is a resource, which cannot receive scheduling messages.
    pub const CANNOT_SEND_TO_RESOURCE: &str = "cannotSendToResource";
    /// The caller is not authorized to send to this recipient.
    pub const NOT_AUTHORIZED: &str = "notAuthorized";
}

/// SetError types added for `CalendarEvent/send` (draft-ietf-jmap-calendars-28 §7).
pub mod calendar_send_error {
    /// The caller may not send as the given identity.
    pub const FORBIDDEN_FROM: &str = "forbiddenFrom";
    /// The referenced participant does not exist on the event.
    pub const PARTICIPANT_NOT_FOUND: &str = "participantNotFound";
    /// The event's participants are not valid for sending.
    pub const INVALID_PARTICIPANTS: &str = "invalidParticipants";
    /// The caller may not send events for this calendar.
    pub const CANNOT_SEND_FOR_CALENDAR: &str = "cannotSendForCalendar";
    /// The referenced calendar event does not exist.
    pub const EVENT_NOT_FOUND: &str = "eventNotFound";
}

/// An RSVP reply to a calendar event invitation (draft-ietf-jmap-calendars-28 §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ParticipantReply {
    /// Id of the event being replied to.
    pub calendar_event_id: Id,
    /// Which recurrence instance the reply is for, if the event recurs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<String>,
    /// The reply's RSVP status (see the `participant_participation_status` module).
    pub participation_status: String,
    /// A comment to attach to the reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Delivery addresses for the reply, keyed by method.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_to: Option<BTreeMap<String, String>>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParticipantReply {
    /// Create a new `ParticipantReply` with the given event id and RSVP status.
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

    /// Set which recurrence instance the reply is for.
    pub fn with_recurrence_id(mut self, recurrence_id: impl Into<String>) -> Self {
        self.recurrence_id = Some(recurrence_id.into());
        self
    }

    /// Set the reply's comment.
    pub fn with_comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = Some(comment.into());
        self
    }

    /// Set the reply's delivery addresses.
    pub fn with_send_to(mut self, send_to: BTreeMap<String, String>) -> Self {
        self.send_to = Some(send_to);
        self
    }
}

/// Standard participation status values for participants and replies (draft-ietf-jmap-calendars-28 §10, RFC 8984 §4.4.6).
pub mod participant_participation_status {
    /// The participant has not yet responded.
    pub const NEEDS_ACTION: &str = "needs-action";
    /// The participant accepted the invitation.
    pub const ACCEPTED: &str = "accepted";
    /// The participant declined the invitation.
    pub const DECLINED: &str = "declined";
    /// The participant tentatively accepted the invitation.
    pub const TENTATIVE: &str = "tentative";
    /// The participant delegated their attendance to someone else.
    pub const DELEGATED: &str = "delegated";
}

/// A participant identity (draft-ietf-jmap-calendars-28 §3): how a user is
/// identified when participating in events and scheduling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ParticipantIdentity {
    /// The identity's id. `None` before the server assigns one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The identity's display name.
    #[serde(default)]
    pub name: String,
    /// The identity's iTIP `SCHEDULE-ID`, if different from its calendar address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<String>,
    /// The URI that represents this participant for iTIP scheduling (draft-ietf-jmap-calendars-28 §3).
    /// Matches participant `calendarAddress` using RFC 3986 §6.2.2 normalization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_address: Option<String>,
    /// Delivery addresses for this identity, keyed by method.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_to: Option<BTreeMap<String, String>>,
    /// Whether this is the account's default identity for new events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    /// Properties the server sent that this type does not model, round-tripped on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParticipantIdentity {
    /// Create a new `ParticipantIdentity` with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// Set the identity's id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the identity's schedule id.
    pub fn with_schedule_id(mut self, schedule_id: impl Into<String>) -> Self {
        self.schedule_id = Some(schedule_id.into());
        self
    }

    /// Set the identity's calendar address.
    pub fn with_calendar_address(mut self, calendar_address: impl Into<String>) -> Self {
        self.calendar_address = Some(calendar_address.into());
        self
    }

    /// Set the identity's delivery addresses.
    pub fn with_send_to(mut self, send_to: BTreeMap<String, String>) -> Self {
        self.send_to = Some(send_to);
        self
    }

    /// Add a delivery method and address for this identity.
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

    /// Set whether this is the account's default identity.
    pub fn is_default(mut self, is_default: bool) -> Self {
        self.is_default = Some(is_default);
        self
    }

    /// Set the identity's unmodeled properties.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// The `SetError` types draft-ietf-jmap-calendars-28 §3.2 adds for `ParticipantIdentity/set`.
pub mod participant_identity_set_error {
    /// The identity is the account's default and cannot be destroyed.
    pub const CANNOT_DESTROY_DEFAULT: &str = "cannotDestroyDefault";
}

/// `ParticipantIdentity/set` arguments (draft-ietf-jmap-calendars-28 §3.2).
/// Adds the `onSuccessSetIsDefault` argument to the standard `/set` shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ParticipantIdentitySetRequest {
    /// The standard `/set` arguments.
    #[serde(flatten)]
    pub set: SetRequest<ParticipantIdentity>,
    /// Id of the identity to mark default once the other changes succeed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_set_is_default: Option<String>,
}

impl ParticipantIdentitySetRequest {
    /// Create a new `ParticipantIdentitySetRequest` wrapping the given `/set` arguments.
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
