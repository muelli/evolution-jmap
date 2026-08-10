// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Calendars types (draft-ietf-jmap-calendars). A `CalendarEvent` is a
//! JSCalendar Event (RFC 8984) carrying the JMAP-side `id` and
//! `calendarIds` properties.
//!
//! The draft is in final approval as of mid-2026; property names follow
//! draft-ietf-jmap-calendars-27. Unmodeled properties ride in `extra`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::Id;

/// A calendar (draft §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
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
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSCalendar LocalDateTime, e.g. `2026-01-15T13:00:00` — interpreted in
    /// `timeZone`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    /// ISO 8601 duration, e.g. `PT1H`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    /// Whether the event is shown without a time — an all-day event (RFC 8984
    /// §4.1.5). `None` and `Some(false)` mean the same thing to a server, which
    /// defaults the property to false; the mapping uses `None` for "nothing was
    /// said", so that a save can tell an event that was never all-day from one
    /// the user just made timed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_without_time: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_rules: Option<Vec<RecurrenceRule>>,
    /// The instances named one at a time rather than by a rule (RFC 8984
    /// §4.3.4): a map from an instance's start, as a LocalDateTime, to a
    /// PatchObject describing how that instance differs.
    ///
    /// The patch is left as JSON because it is genuinely open — `excluded: true`
    /// for an instance that does not happen, `{}` for one that happens as the
    /// rules would have it, and any set of event properties for one that was
    /// edited on its own. Modeling only the shapes the iCalendar mapping can
    /// spell would quietly discard the third, which the mapping instead has to
    /// see in order to refuse to overwrite it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_overrides: Option<BTreeMap<String, Value>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarEvent {
    /// A minimal one-off event, ready for `CalendarEvent/set` create.
    pub fn simple(calendar_id: impl Into<Id>, title: &str, start: &str, duration: &str) -> Self {
        Self {
            calendar_ids: Some([(calendar_id.into(), true)].into()),
            event_type: Some("Event".to_owned()),
            title: Some(title.to_owned()),
            start: Some(start.to_owned()),
            time_zone: Some("Etc/UTC".to_owned()),
            duration: Some(duration.to_owned()),
            status: Some("confirmed".to_owned()),
            ..Self::default()
        }
    }
}

/// JSCalendar RecurrenceRule (RFC 8984 §4.3.3), modeled shallowly —
/// `byDay` & friends ride in `extra`.
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
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl RecurrenceRule {
    pub fn new(frequency: &str) -> Self {
        Self {
            rule_type: Some("RecurrenceRule".to_owned()),
            frequency: frequency.to_owned(),
            ..Self::default()
        }
    }
}

/// `CalendarEvent/query` filter conditions (draft §5.5). Flat conditions
/// only. `after`/`before` are UTC instants compared against the event start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
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
}

impl CalendarEventQueryFilter {
    pub fn in_calendar(calendar_id: impl Into<Id>) -> Self {
        Self {
            in_calendar: Some(calendar_id.into()),
            ..Self::default()
        }
    }

    pub fn time_range(after: &str, before: &str) -> Self {
        Self {
            after: Some(after.to_owned()),
            before: Some(before.to_owned()),
            ..Self::default()
        }
    }
}
