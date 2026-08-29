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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_visible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_in_availability: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// The permissions the user has for a calendar (draft-ietf-jmap-calendars-28 §4.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CalendarRights {
    #[serde(default)]
    pub may_read_items: bool,
    #[serde(default)]
    pub may_add_items: bool,
    #[serde(default)]
    pub may_modify_items: bool,
    #[serde(default)]
    pub may_remove_items: bool,
    #[serde(default)]
    pub may_delete: bool,
    #[serde(default)]
    pub may_rename: bool,
    #[serde(default)]
    pub may_admin: bool,
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
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CalendarEvent {
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
            ..Self::default()
        }
    }
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

    pub fn time_range(after: &str, before: &str) -> Self {
        Self {
            after: Some(after.to_owned()),
            before: Some(before.to_owned()),
            ..Self::default()
        }
    }
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
