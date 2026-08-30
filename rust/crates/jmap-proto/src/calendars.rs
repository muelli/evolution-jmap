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
#[cfg(feature = "principals")]
use crate::principals::Principal;
use crate::state::UtcDate;

#[cfg(not(feature = "principals"))]
type Principal = serde_json::Value;

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
    /// principal id. Modeled but unread today — writing shares is Phase C,
    /// deliberately separate from reading `myRights`.
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
}

/// `Calendar.myRights`/a `shareWith` entry (calendars draft §4, draft-27 field
/// names — no plain `mayWrite`, unlike [`crate::contacts::AddressBookRights`]:
/// the draft splits it into "all items" and "just my own").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
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

    /// Whether these rights let the holder write *something* to the
    /// calendar. EDS's read-only flag is a single bit per source, with no
    /// "read-only except your own items" shade to represent — marking the
    /// whole calendar read-only for a holder who may still create and edit
    /// their own events (`mayWriteOwn`) would block ordinary use of it, so
    /// either write right counts. Absent fields read as `false` (fail
    /// closed), same reasoning as
    /// [`crate::contacts::AddressBookRights::is_writable`].
    pub fn is_writable(&self) -> bool {
        self.may_write_all.unwrap_or(false) || self.may_write_own.unwrap_or(false)
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
    /// The JSCalendar version this object conforms to — jscalendarbis §3.1.2,
    /// which a standalone Event MUST set. In JMAP the value is `"2.0"`:
    /// draft-ietf-jmap-calendars-28 §1.4 defines CalendarEvent as a
    /// jscalendarbis Event, and Fastmail refuses both a version-less create
    /// and `"1.0"` with `invalidProperties: ["version"]` (observed
    /// 2026-08-24, wire-traced).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    /// When the event first arrived — RFC 8984 §4.1.7's `created`, a
    /// UTCDateTime such as `2026-01-02T09:30:00Z`.
    ///
    /// The server's to state: it stamps this when the event is created and
    /// nothing here ever proposes a value for it. The iCalendar mapping draws it
    /// as a `CREATED` for whoever reads the document and does not read it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    /// When the event was last changed — RFC 8984 §4.1.8's `updated`, a
    /// UTCDateTime. The server's to state, for the reason [`Self::created`]
    /// gives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
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
    /// Whether the event blocks the time it occupies — RFC 8984 §4.4.2's
    /// `freeBusyStatus`, one of `free` and `busy`, which is what Evolution's
    /// "Show Time as" states.
    ///
    /// `None` is "nothing was said" rather than the RFC's default of `busy`, for
    /// the reason [`Self::show_without_time`] gives: a save reads an edit off a
    /// difference from what the component showed, so answering with the default
    /// would have it state the value where the server never did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_busy_status: Option<String>,
    /// How important the event is — RFC 8984 §4.4.1's `priority`, the same
    /// integer iCalendar's `PRIORITY` (RFC 5545 §3.8.1.9) carries: 0 undefined,
    /// 1 highest, 9 lowest.
    ///
    /// `None` is "nothing was said" rather than the RFC's default of 0, for the
    /// reason [`Self::show_without_time`] gives.
    ///
    /// Signed and wider than the range, though the RFC admits only 0 to 9,
    /// because a whole `CalendarEvent/get` response is deserialized into this
    /// type at once: a server answering `-1` for one event must not fail the
    /// response and take every event in the calendar down with it. The mapping
    /// refuses to write a value outside the range instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
    /// How much of the event may be shared with other calendar users — RFC 8984
    /// §4.4.3's `privacy`, one of `public`, `private` and `secret`, which is what
    /// Evolution's Options ▸ Classification states.
    ///
    /// `None` is "nothing was said" rather than the RFC's default of `public`,
    /// for the reason [`Self::show_without_time`] gives.
    ///
    /// Held as a string rather than an enum because RFC 8984 §4.4.3 leaves the
    /// vocabulary open — a registered or a vendor-specific value is legal — and
    /// a whole `CalendarEvent/get` response is deserialized into this type at
    /// once, so one event's unusual value must not take the calendar down with
    /// it. The mapping refuses to write a value it cannot spell instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy: Option<String>,
    /// The places the event happens at (RFC 8984 §4.2.5), keyed by an id of
    /// whoever wrote them.
    ///
    /// Left as JSON, for the reason [`Self::recurrence_overrides`] is: a
    /// Location holds a `description`, `coordinates`, `links`, `locationTypes`
    /// and a `timeZone` besides its `name`, and iCalendar's `LOCATION` is one
    /// line of text. The mapping therefore patches the name *in place*, by the
    /// entry's key, rather than replacing the property — which it can only do
    /// if it can see what else is in there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locations: Option<BTreeMap<String, Value>>,
    /// The places the event may be joined online at (RFC 8984 §4.2.6), keyed by
    /// an id of whoever wrote them — the conference link and the number to dial.
    ///
    /// Left as JSON, for the reason [`Self::participants`] is: a VirtualLocation
    /// holds a `description` and a set of `features` besides the `uri` and the
    /// `name`, and iCalendar spells the part of that it shares on a `CONFERENCE`
    /// line (RFC 7986 §5.11) and its parameters. The mapping draws the places
    /// and never reads them back, so nothing here has to model the rest; holding
    /// them as values keeps one server's unusual entry from failing a
    /// `CalendarEvent/get` and taking every event in the calendar with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_locations: Option<BTreeMap<String, Value>>,
    /// The external resources the event points at (RFC 8984 §4.2.7), keyed by
    /// an id of whoever wrote them — the agenda document, the minutes, the
    /// picture shown beside the title.
    ///
    /// Left as JSON, for the reason [`Self::virtual_locations`] is: a Link (RFC
    /// 8984 §1.4.11) holds a `cid` and a `title` besides the `href`,
    /// `contentType`, `size`, `rel` and `display` that iCalendar's `ATTACH` (RFC
    /// 5545 §3.8.1.1) and `IMAGE` (RFC 7986 §5.10) have room for. The mapping
    /// reads back only what it drew and a save patches one member of an entry, so
    /// nothing here has to model the rest; holding them as values keeps one
    /// server's unusual link from failing a `CalendarEvent/get` and taking every
    /// event in the calendar with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<BTreeMap<String, Value>>,
    /// The tags the event carries (RFC 8984 §4.2.9), an RFC 8984 §1.4.3 Set:
    /// the keys are the keywords and every value is `true`.
    ///
    /// The values are left as JSON rather than as `bool`, which is how
    /// [`crate::mail::Email`] holds its keywords, because of what this type is:
    /// a whole `CalendarEvent/get` response is deserialized into it at once, so
    /// a server answering `{"offsite": 1}` for one event would fail the response
    /// and take every event in the calendar down with it. Held as values, the
    /// odd entry is visible as itself and the mapping refuses to write the
    /// property back — the calendar still opens, which is the trade the
    /// iCalendar mapping makes everywhere else too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<BTreeMap<String, Value>>,
    /// The reminders the event carries (RFC 8984 §4.5.2), keyed by an id of
    /// whoever wrote them.
    ///
    /// Left as JSON, for the reason [`Self::locations`] is: an Alert holds a
    /// `trigger` that is one of two object types — an offset from the event or an
    /// absolute instant — and an `acknowledged` timestamp saying the user has
    /// already dismissed it (RFC 9074 §6.1), none of which the `VALARM` this
    /// mapping writes carries. The save path has to see them in order to refuse
    /// to replace the property.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alerts: Option<BTreeMap<String, Value>>,
    /// Who is invited to the event (RFC 8984 §4.4.6), keyed by an id of whoever
    /// wrote them — the guest list, and the organizer with them: RFC 8984 gives
    /// the organizer no property of its own, it is the participant holding the
    /// `owner` role.
    ///
    /// Left as JSON, for the reason [`Self::locations`] is and then some: a
    /// Participant holds a `sendTo` map of addressing methods, a set of `roles`,
    /// a `kind`, a `participationStatus`, `delegatedTo`/`delegatedFrom`,
    /// `memberOf`, a scheduling agent and more — iCalendar spells the part of
    /// that it shares on the parameters of an `ATTENDEE` line. The mapping draws
    /// the guest list and never reads it back, so nothing here has to model the
    /// rest; holding it as values keeps one server's unusual participant from
    /// failing a `CalendarEvent/get` and taking every event in the calendar with
    /// it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participants: Option<BTreeMap<String, Value>>,
    /// jscalendarbis §3.3.3 (draft-ietf-calext-jscalendarbis): a single
    /// `RecurrenceRule`, not RFC 8984's plural `recurrenceRules` array — the
    /// property was renamed and restructured from array-valued to
    /// singular/object-valued in the base that obsoletes it. Confirmed
    /// against the draft's own text (three independent fetches, one a
    /// full-text search for every "recurrenceRule" occurrence): no plural
    /// form appears anywhere, including the IANA properties registry entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_rule: Option<RecurrenceRule>,
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
    /// The zones the event defines for itself (RFC 8984 §4.7.2), keyed by the
    /// identifier [`Self::time_zone`] and the overrides refer to them by.
    ///
    /// §1.4.9 lets a `TimeZoneId` be either an IANA name or a custom identifier
    /// beginning with a solidus **that this property defines** — the second is
    /// what a server has to invent for a zone no database names, such as the
    /// one an Exchange invitation carries its own `VTIMEZONE` for. A reader
    /// cannot look such an identifier up anywhere, so the definition is the only
    /// thing that says what the zone is, and the iCalendar mapping draws it as a
    /// `VTIMEZONE` beside the event.
    ///
    /// Left as JSON, for the reason [`Self::locations`] is: a TimeZone holds
    /// `aliases`, a `validUntil`, a `url` and two arrays of transition rules with
    /// names and comments on them, of which the mapping draws what a `VTIMEZONE`
    /// can hold. Nothing ever writes the property back — a zone definition is the
    /// server's, not something Evolution offers to edit — so the odd entry costs
    /// only the sight of itself rather than failing a whole
    /// `CalendarEvent/get`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zones: Option<BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_default_alerts: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub localizations: Option<BTreeMap<String, Value>>,
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
}

/// JSCalendar RecurrenceRule (RFC 8984 §4.3.3), modeled shallowly — `rscale`
/// & friends ride in `extra`.
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
    /// The seconds of the minute it repeats at — iCalendar's `BYSECOND`. 0 to
    /// 60, the sixtieth being the leap second RFC 5545 §3.3.10's `seconds`
    /// admits and UTC occasionally inserts.
    ///
    /// The first of the three parts that name a *time of day* rather than a
    /// date, which RFC 5545 §3.3.10 says MUST NOT stand beside a `DTSTART` of
    /// value type DATE — so an all-day event whose rule carries any of them is
    /// drawn as a timed event instead.
    ///
    /// All three are unsigned, unlike the day and week parts below: RFC 8984
    /// §4.3.3 has them as `UnsignedInt[]`, and RFC 5545 §3.3.10 gives no way to
    /// count a time backwards from the end of the period holding it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_second: Option<Vec<u32>>,
    /// The minutes of the hour it repeats at — iCalendar's `BYMINUTE`. 0 to 59.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_minute: Option<Vec<u32>>,
    /// The hours of the day it repeats at — iCalendar's `BYHOUR`. 0 to 23.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_hour: Option<Vec<u32>>,
    /// The days of the week the rule repeats on — iCalendar's `BYDAY`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_day: Option<Vec<NDay>>,
    /// The days of the *month* it repeats on — iCalendar's `BYMONTHDAY`. 1 to 31
    /// counting from the start of the month, -1 to -31 from its end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_month_day: Option<Vec<i32>>,
    /// The days of the *year* it repeats on — iCalendar's `BYYEARDAY`. 1 to 366
    /// counting from 1 January, -1 to -366 from 31 December.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_year_day: Option<Vec<i32>>,
    /// The weeks of the year it repeats in — iCalendar's `BYWEEKNO`. 1 to 53
    /// counting from the first week of the year, -1 to -53 from its last.
    ///
    /// Which days a week holds depends on [`Self::first_day_of_week`]: RFC 5545
    /// §3.3.10 numbers the weeks by ISO 8601, counting from that day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_week_no: Option<Vec<i32>>,
    /// The months of the year it repeats in — iCalendar's `BYMONTH`.
    ///
    /// A string rather than a number, as RFC 8984 §4.3.3 has it: the month
    /// number, optionally followed by `L` for a leap month in a non-Gregorian
    /// calendar (`5L`). Kept verbatim so a value this mapping cannot spell as an
    /// `RRULE` is visible as itself rather than as a number it would have to
    /// invent a spelling for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_month: Option<Vec<String>>,
    /// Which occurrences of the ones the rest of the rule names it keeps —
    /// iCalendar's `BYSETPOS`. 1 to 366 counting from the first occurrence in
    /// the interval, -1 to -366 from the last.
    ///
    /// The only part here that does not *name* dates: the others say which dates
    /// an interval expands to, and this one selects out of that set afterwards,
    /// so it means nothing on its own. RFC 5545 §3.3.10 says as much — it MUST
    /// only be used together with another `BYxxx` part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_set_position: Option<Vec<i32>>,
    /// The day each week of the rule starts on — iCalendar's `WKST`. One of the
    /// two-letter lowercase weekdays [`NDay::day`] uses, and RFC 8984 §4.3.3's
    /// default is `mo`.
    ///
    /// It changes which dates a rule produces rather than merely describing them:
    /// RFC 5545 §3.3.10 counts a `FREQ=WEEKLY;INTERVAL=2` series' weeks from this
    /// day, so the same `byDay` counted from Sunday and from Monday name different
    /// Tuesdays.
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

/// JSCalendar NDay (RFC 8984 §4.3.3): one weekday a `byDay` names, and
/// optionally which occurrence of it within the recurrence period.
///
/// `day` is the two-letter lowercase weekday (`mo` … `su`) both formats spell
/// the same way but for case; `nthOfPeriod` is the ordinal iCalendar writes in
/// front of it, negative for the nth-last (`-1FR`, the last Friday of the
/// month).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
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

/// Standard JSCalendar (RFC 8984 §4.4.4) / jscalendarbis event status values.
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

/// Standard JSCalendar (RFC 8984 §4.4.6) participant roles.
pub mod participant_role {
    pub const OWNER: &str = "owner";
    pub const ADMIN: &str = "admin";
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

/// JSCalendar Participant (RFC 8984 §4.4.6): an attendee or organizer of the event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Participant {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub participant_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_to: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<BTreeMap<String, bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participation_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participation_comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attendance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect_reply: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_sequence: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_status: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_updated: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_to: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_from: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_of: Option<BTreeMap<String, bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_updated: Option<UtcDate>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
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
}

/// JSCalendar Location (RFC 8984 §4.2.5): a physical location for the event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub location_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
}

/// JSCalendar VirtualLocation (RFC 8984 §4.2.6): an online conference or meeting room.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VirtualLocation {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub virtual_location_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

/// JSCalendar Alert (RFC 8984 §4.5.2): an alarm or reminder for the event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
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

/// `CalendarEvent/parse` arguments (draft-ietf-jmap-calendars-28 §5.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
}

/// Standard RFC 8984 §4.4.5 event relation types.
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

/// JSCalendar Relation (RFC 8984 §4.4.5): how this event relates to another.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
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

/// The `SetError` types draft-ietf-jmap-calendars-28 §5.4 adds for `CalendarEvent/set`.
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

/// JSCalendar Group (RFC 8984 §6): a collection of calendar objects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
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
    pub entries: Option<BTreeMap<String, Value>>,
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

    pub fn with_entries(mut self, entries: BTreeMap<String, Value>) -> Self {
        self.entries = Some(entries);
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

fn default_calendar_event_notification_type() -> String {
    CalendarEventNotification::TYPE.to_owned()
}

/// A notification of changes to calendar events (draft-ietf-jmap-calendars-28 §8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

/// Standard participation status values for participants and replies (draft-ietf-jmap-calendars-28 §10, RFC 8984 §4.4.5).
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
pub struct ParticipantIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<String>,
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
