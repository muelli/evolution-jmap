// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSCalendar [`CalendarEvent`] ↔ iCalendar `VEVENT`.
//!
//! The mapped set is the one the calendar backend needs to be useful — UID,
//! SUMMARY, DESCRIPTION, DTSTART (with its time zone, or as a `VALUE=DATE` when
//! the event is shown without a time), DURATION, STATUS, RRULE, and the
//! instances an EXDATE, an RDATE or a `RECURRENCE-ID` component names one at a
//! time — and no more. Everything else on an event (participants, alarms,
//! locations, links, …) is *dropped*, which is only safe because saving goes
//! back to the server as a PatchObject naming the mapped properties: a property
//! we never mapped is a property we never overwrite. See [`MAPPED_PROPERTIES`],
//! [`maps_recurrence_rule`] and [`maps_recurrence_override`], which are that
//! knowledge in machine-readable form.
//!
//! A `VCALENDAR` here therefore holds more than one `VEVENT` whenever an
//! instance of a recurring event was edited on its own: the series first, then
//! one component per edited instance, each carrying the series' `UID` and the
//! `RECURRENCE-ID` of the occurrence it stands in for. That is also the shape
//! `ECalMetaBackend` stores and hands back to a save.
//!
//! The one property read but never written is `DTEND`: it is how Evolution
//! states an event's length, so [`read_duration`] measures it, while the length
//! goes back out as the `DURATION` the two formats share. Both directions pass
//! through [`stated_duration`], because the two formats spell an ISO 8601
//! duration identically but do not admit the same set of them.
//!
//! A time zone crosses under two different kinds of name: iCalendar refers to
//! one by a `TZID`, which is an identifier the document itself may define, and
//! JSCalendar wants the zone's IANA name. [`names_time_zone`] says which is
//! which and [`zone_names`] does the translating, off the `VTIMEZONE` the
//! document is required to carry.
//!
//! An all-day event has no property of its own in iCalendar; it is a `DTSTART`
//! written as a date rather than a date-time, which puts JSCalendar's
//! `showWithoutTime` in the value type of three properties at once. See
//! [`shows_without_time`] for the conditions that has to meet and what happens
//! when it cannot.
//!
//! Nothing here fails on unrecognised input. A property whose value the
//! mapping cannot read is treated as absent, because an event that loses a
//! field is better than a calendar that refuses to open; only a document
//! without any `VEVENT` in it is an error.

use std::collections::BTreeMap;

use jmap_proto::calendars::{CalendarEvent, NDay, RecurrenceRule};
use serde_json::{Map, Value, json};

use crate::error::ICalError;
use crate::syntax::{self, Component, Property};

/// Carries the JSCalendar `uid` when the iCalendar `UID` is taken by the JMAP
/// id, mirroring `X-JMAP-UID` on the address book side.
const X_JMAP_UID: &str = "X-JMAP-UID";

/// The `PRODID` of every calendar this crate emits.
const PRODID: &str = "-//evolution-jmap//JMAP calendar backend//EN";

/// The JSCalendar spelling of `Etc/UTC`, the one the client and the mock use.
const UTC: &str = "Etc/UTC";

/// libical's record, on the `VTIMEZONE` it builds, of which IANA zone that
/// component describes. It is not standard, but it is universal in this
/// neighbourhood: libical writes it for every builtin zone, and libical is
/// what puts the components Evolution saves together in the first place.
const X_LIC_LOCATION: &str = "X-LIC-LOCATION";

/// The property that ties an edited instance to the series it belongs to: the
/// start of the occurrence it stands in for (RFC 5545 §3.8.4.4), which is
/// exactly the key of a JSCalendar `recurrenceOverrides` entry.
const RECURRENCE_ID: &str = "RECURRENCE-ID";

/// The weekdays a `BYDAY` names, in their iCalendar spelling (RFC 5545
/// §3.3.10). JSCalendar's `day` (RFC 8984 §4.3.3) is the same closed set in
/// lowercase, so the two differ only in case — and a value outside it is
/// dropped rather than passed through in the other format's clothes.
const WEEKDAYS: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

/// JSCalendar `status` values and their iCalendar `STATUS` spelling. Both sets
/// are closed, so a value outside this table is dropped rather than passed
/// through in the other format's clothes.
const STATUSES: [(&str, &str); 3] = [
    ("confirmed", "CONFIRMED"),
    ("cancelled", "CANCELLED"),
    ("tentative", "TENTATIVE"),
];

/// The JSCalendar properties this mapping covers, and therefore the only ones
/// a save may name in a `CalendarEvent/set` update patch.
///
/// The last two are covered *conditionally* — see [`maps_recurrence_rule`] and
/// [`maps_recurrence_override`], which say when a save may name them.
pub const MAPPED_PROPERTIES: [&str; 9] = [
    "title",
    "description",
    "start",
    "timeZone",
    "duration",
    "showWithoutTime",
    "status",
    "recurrenceRules",
    "recurrenceOverrides",
];

/// The event properties an edited instance may restate, and therefore the only
/// keys — besides `excluded` — a `recurrenceOverrides` PatchObject may hold for
/// [`maps_recurrence_override`] to call it covered.
///
/// `timeZone` is here because iCalendar states a zone per property rather than
/// per document: the instance's own `DTSTART` carries its own `TZID`, so an
/// occurrence the user moved into another zone has a spelling of its own, and
/// one with no `TZID` at all is the floating instance a null asks for.
///
/// `showWithoutTime` is absent, one step further out — see
/// [`shows_without_time`], which is decided once for the whole document.
pub const OVERRIDE_PROPERTIES: [&str; 6] = [
    "title",
    "description",
    "start",
    "timeZone",
    "duration",
    "status",
];

/// Whether a recurrence rule survives the trip through iCalendar.
///
/// Only `frequency`, `interval`, `count`, `until`, `byDay`, `byMonthDay` and
/// `byMonth` are modeled; `bySetPosition` and the rest of RFC 8984 §4.3.3 ride
/// in [`RecurrenceRule::extra`] and would be lost. A caller that patches
/// `recurrenceRules` for a rule this returns `false` for narrows the user's
/// recurrence behind their back.
///
/// A rule [`rule_to_rrule`] refuses outright fails this too, so the save path
/// never patches over a recurrence the user was not shown — as does one whose
/// days of the week, days of the month or months of the year the `RRULE` cannot
/// carry, which [`by_day_part`], [`by_month_day_part`] and [`by_month_part`]
/// decide and [`rule_to_rrule`] then leaves off.
pub fn maps_recurrence_rule(rule: &RecurrenceRule) -> bool {
    rule.extra.is_empty()
        && writable(rule)
        && rule
            .by_day
            .as_ref()
            .is_none_or(|_| by_day_part(rule).is_some())
        && rule
            .by_month_day
            .as_ref()
            .is_none_or(|_| by_month_day_part(rule).is_some())
        && rule
            .by_month
            .as_ref()
            .is_none_or(|_| by_month_part(rule).is_some())
}

/// Whether a recurrence override survives the trip through iCalendar.
///
/// iCalendar names a single instance of a recurring event three ways, and
/// JSCalendar says all three with one `recurrenceOverrides` entry (RFC 8984
/// §4.3.4). `EXDATE` says the instance does not happen and `RDATE` says it does
/// (RFC 5545 §3.8.5.1, §3.8.5.2) — those are the empty patch and `excluded`.
/// The third, an instance that happens *differently*, is a `VEVENT` of its own
/// carrying a `RECURRENCE-ID`, and it can restate only the properties
/// [`OVERRIDE_PROPERTIES`] lists; a patch touching anything else — a
/// participant, a location, an alert — is a patch this mapping shows in part
/// and must not write back.
///
/// The `id` is checked as well as the patch. It is the instance's own start as a
/// LocalDateTime, and one no `EXDATE` can spell is an override that would vanish
/// from a property replaced whole.
///
/// An instance that is `excluded` may say nothing else: there is no occurrence
/// left to show an edited title on, so the `EXDATE` carries the exclusion and
/// drops the rest.
///
/// As with [`maps_recurrence_rule`], an override this returns `false` for is
/// still *drawn* as far as it can be — see [`event_to_ical`], which places an
/// override it cannot describe with a bare `RDATE` rather than hiding the
/// occurrence.
pub fn maps_recurrence_override(id: &str, patch: &Value) -> bool {
    let Some(fields) = patch.as_object() else {
        return false;
    };
    if to_ical_date_time(id).is_none() {
        return false;
    }
    if excluded(patch) {
        return fields.len() == 1;
    }
    fields
        .iter()
        .all(|(name, value)| maps_override_field(name, value))
}

/// Whether one field of an override's PatchObject reaches the component and
/// comes back meaning the same thing.
///
/// A PatchObject sets a property with a value and removes it with a null, and
/// the component says the removal by not carrying the line at all — so a null
/// round-trips wherever an absent property does. An *empty* string is neither:
/// the writer drops it like an absent value, so it would come back as a
/// removal, which is a different patch.
fn maps_override_field(name: &str, value: &Value) -> bool {
    match name {
        "excluded" => value.is_boolean(),
        _ if !OVERRIDE_PROPERTIES.contains(&name) => false,
        // Outside the closed vocabulary there is no STATUS to write, so the
        // instance would come back at the series' status.
        "status" => value.is_null() || value.as_str().is_some_and(known_status),
        // A start is required by RFC 8984, so a null says nothing, and the
        // value has to be one a DTSTART can carry.
        "start" => value.as_str().and_then(to_ical_date_time).is_some(),
        // A null is the floating instance, which a `DTSTART` with no `TZID`
        // says. A value has to be a name JSCalendar admits: the `TZID` this
        // reads back from is an iCalendar identifier (see [`names_time_zone`]),
        // and `recurrenceOverrides` goes back to the server replaced whole, so
        // one entry the server rejects costs every edit in the save.
        "timeZone" => value.is_null() || value.as_str().is_some_and(names_time_zone),
        // The only place an instance's own length is stated is its component's
        // DURATION, and one this mapping will not write there (see
        // [`vevent_of`]) leaves the instance at the series' length — which is
        // what the override said it was *not*.
        "duration" => value.is_null() || value.as_str().and_then(stated_duration).is_some(),
        // title, description.
        _ => value.is_null() || value.as_str().is_some_and(|text| !text.is_empty()),
    }
}

/// Whether an override says its instance does not happen — the `EXDATE` half of
/// [`recurrence_dates`].
///
/// Anything that is not literally `"excluded": true` counts as an instance that
/// happens, including the `false` RFC 8984 §4.3.4 defaults to and a value of the
/// wrong type. That is the reading that cannot make an appointment disappear:
/// [`maps_recurrence_override`] refuses the malformed shape separately, so the
/// mistake cannot be written back to the server either way.
fn excluded(patch: &Value) -> bool {
    patch.get("excluded") == Some(&Value::Bool(true))
}

/// Whether an `RRULE` can carry this rule's frequency and its end — the two
/// parts that cannot be narrowed, only lost.
///
/// A rule that names no frequency has no `RRULE` spelling at all: an empty one
/// is rejected by libical and means nothing to a reader. A rule whose `until`
/// is not a date-time this mapping can write must be refused rather than
/// written without it, because a recurrence that ends and one that never does
/// are different events, and the unbounded one repeats into every week of the
/// user's calendar for ever.
fn writable(rule: &RecurrenceRule) -> bool {
    !rule.frequency.is_empty()
        && rule
            .until
            .as_deref()
            .is_none_or(|until| to_ical_date_time(until).is_some())
}

/// Render an event as an iCalendar object, ready for
/// `i_cal_component_new_from_string()`.
///
/// The series comes first, then one component per instance edited on its own —
/// see [`modified_instances`].
pub fn event_to_ical(event: &CalendarEvent) -> String {
    let start = event.start.as_deref().and_then(to_ical_date_time);
    // Whether this event goes out as a date rather than a date-time, which is
    // the whole of `showWithoutTime` on this side. Decided once for the whole
    // document: `DTSTART`, `DURATION`, an `RRULE`'s `UNTIL`, the named instances
    // and every edited instance's own `RECURRENCE-ID` have to agree about it.
    let as_a_date = start
        .as_deref()
        .is_some_and(|start| shows_without_time(event, start));
    let zone = event.time_zone.as_deref();

    let mut vevent = vevent_of(event, as_a_date, zone, None);

    for rule in event.recurrence_rules.iter().flatten() {
        if let Some(value) = rule_to_rrule(rule, zone, as_a_date) {
            vevent = vevent.with(Property::raw("RRULE", &value));
        }
    }

    // The instances named one at a time. Both properties take a list, so each
    // kind is one content line however many instances it holds.
    for (name, is_excluded) in [("EXDATE", true), ("RDATE", false)] {
        let dates = recurrence_dates(event, is_excluded);
        if !dates.is_empty() {
            vevent = vevent.with(dated(name, &dates, as_a_date, zone));
        }
    }

    let mut calendar = Component::new("VCALENDAR")
        .with(Property::raw("VERSION", "2.0"))
        .with(Property::raw("PRODID", PRODID))
        .with_child(vevent);
    for (id, instance) in modified_instances(event) {
        calendar = calendar.with_child(vevent_of(&instance, as_a_date, zone, Some(&id)));
    }
    calendar.to_ics()
}

/// One `VEVENT`: the properties an event and an edited instance of it spell the
/// same way, plus the `RECURRENCE-ID` that makes it the latter.
///
/// The recurrence itself is *not* here. A series states its rules and its named
/// instances; an instance edited on its own states neither, because it is one
/// occurrence and RFC 5545 §3.8.4.4 already says which.
///
/// The two date-times this writes are in **two different zones** whenever an
/// instance moved into one of its own. `DTSTART` is in the event's own zone,
/// which for an instance is what its override said; `RECURRENCE-ID` is in
/// `series_zone`, because it names the occurrence the recurrence rules generated
/// and those run on the series' clock (RFC 5545 §3.8.4.4 — the value has to
/// match the series' `DTSTART`, or it points at an instant the series never
/// generated and the edit attaches to nothing). For the series itself the two
/// are the same zone.
fn vevent_of(
    event: &CalendarEvent,
    as_a_date: bool,
    series_zone: Option<&str>,
    recurrence_id: Option<&str>,
) -> Component {
    let mut vevent = Component::new("VEVENT");

    // EDS keys its cache on the iCalendar UID and passes it back to
    // load_component_sync()/remove_component_sync(), so it has to be the
    // identifier the JMAP methods take — the server-assigned id. The
    // JSCalendar uid, which is a different namespace, rides alongside; before
    // the first CalendarEvent/set there is no id and it stands in. An edited
    // instance carries the series' own, which is what ties the two together.
    if let Some(uid) = event
        .id
        .as_ref()
        .map(|id| id.as_str())
        .or(event.uid.as_deref())
    {
        vevent = vevent.with(Property::new("UID", uid));
    }
    if let Some(uid) = &event.uid {
        vevent = vevent.with(Property::new(X_JMAP_UID, uid));
    }
    if let Some(recurrence_id) = recurrence_id {
        vevent = vevent.with(dated(
            RECURRENCE_ID,
            std::slice::from_ref(&recurrence_id.to_owned()),
            as_a_date,
            series_zone,
        ));
    }

    for (name, value) in [
        ("SUMMARY", &event.title),
        ("DESCRIPTION", &event.description),
    ] {
        if let Some(value) = value.as_deref().filter(|value| !value.is_empty()) {
            vevent = vevent.with(Property::new(name, value));
        }
    }

    if let Some(start) = event.start.as_deref().and_then(to_ical_date_time) {
        vevent = vevent.with(dated(
            "DTSTART",
            std::slice::from_ref(&start),
            as_a_date,
            event.time_zone.as_deref(),
        ));
    }

    // Only a value that really is a length: the two formats spell an ISO 8601
    // duration identically, but not the same set of them, and a `DURATION` line
    // libical refuses costs the whole component — every field of the event, not
    // just its length. See [`stated_duration`], which is also what reads the
    // property back.
    if let Some(duration) = event.duration.as_deref().and_then(stated_duration) {
        vevent = vevent.with(Property::raw("DURATION", &duration));
    }

    if let Some(status) = event.status.as_deref().and_then(ical_status) {
        vevent = vevent.with(Property::raw("STATUS", status));
    }

    vevent
}

/// The instances that get a `VEVENT` of their own, each with the rendered
/// `RECURRENCE-ID` naming the occurrence it replaces.
///
/// In the map's chronological order, so a document is stable across renderings
/// — the save path diffs against a re-rendering of what the server holds, and a
/// difference that is only an order is a difference it would have to explain.
fn modified_instances(event: &CalendarEvent) -> Vec<(String, CalendarEvent)> {
    event
        .recurrence_overrides
        .iter()
        .flatten()
        .filter_map(|(id, patch)| {
            Some((to_ical_date_time(id)?, modified_instance(event, id, patch)?))
        })
        .collect()
}

/// The event one override describes, or `None` when the override says nothing
/// a `VEVENT` of its own would show.
///
/// The instance starts as the series with its recurrence dropped and its start
/// moved to the occurrence's own — RFC 8984 §4.3.4's rule that an override's key
/// *is* the instance's start unless the patch says otherwise. Then the patch is
/// applied, one property at a time, and a key or a value outside what
/// [`maps_override_field`] accepts is skipped rather than fatal: the instance is
/// still worth drawing at the series' title, and [`maps_recurrence_override`]
/// separately tells the save path that it was not seen whole.
///
/// An `excluded` instance yields `None`: it does not happen, so there is nothing
/// to draw, and the `EXDATE` [`recurrence_dates`] emits is the whole of it.
fn modified_instance(event: &CalendarEvent, id: &str, patch: &Value) -> Option<CalendarEvent> {
    if excluded(patch) {
        return None;
    }
    let fields = patch.as_object()?;
    let mut instance = CalendarEvent {
        id: event.id.clone(),
        uid: event.uid.clone(),
        title: event.title.clone(),
        description: event.description.clone(),
        start: Some(id.to_owned()),
        time_zone: event.time_zone.clone(),
        duration: event.duration.clone(),
        show_without_time: event.show_without_time,
        status: event.status.clone(),
        ..CalendarEvent::default()
    };

    let mut modified = false;
    for (name, value) in fields {
        if !maps_override_field(name, value) {
            continue;
        }
        // Checked above: a null removes the property, and anything else here is
        // the non-empty string that sets it.
        let text = value.as_str().map(str::to_owned);
        match name.as_str() {
            "title" => instance.title = text,
            "description" => instance.description = text,
            "duration" => instance.duration = text,
            "status" => instance.status = text,
            "start" => instance.start = text,
            "timeZone" => instance.time_zone = text,
            // `excluded: false`, which says only that the instance happens —
            // and it happening is what an override with no other content
            // already means.
            _ => continue,
        }
        modified = true;
    }
    modified.then_some(instance)
}

/// A property carrying date-times in the one form this event spells them in.
///
/// `DTSTART` decides that form, and RFC 5545 obliges the properties that name
/// instances of it to agree: an `EXDATE` or `RDATE` in a different value type or
/// a different zone (§3.8.5.1, §3.8.5.2) resolves against another clock than the
/// occurrences it is meant to add to or remove from, so the exclusion misses and
/// the deleted appointment comes back. One function for all three is how they
/// are kept from drifting apart.
///
/// Each value is a rendered `YYYYMMDDTHHMMSS`; the caller has already decided
/// whether the event is written as a date, which is the only case where the time
/// may be dropped.
fn dated(name: &str, values: &[String], as_a_date: bool, zone: Option<&str>) -> Property {
    let join = |render: &dyn Fn(&String) -> String| -> String {
        values.iter().map(render).collect::<Vec<_>>().join(",")
    };
    match (as_a_date, zone) {
        // A DATE value, RFC 5545 §3.6.1's other form of an event. The parameter
        // is required: these properties are DATE-TIME by default, and libical
        // refuses the whole component over a value that is not one.
        (true, _) => {
            Property::raw(name, &join(&|value| value[..8].to_owned())).with_param("VALUE", "DATE")
        }
        // Form 2, a UTC instant. Form 3 with TZID=Etc/UTC would be legal but
        // obliges us to ship a VTIMEZONE for it.
        (false, Some(zone)) if is_utc(zone) => {
            Property::raw(name, &join(&|value| format!("{value}Z")))
        }
        // Form 3. libical resolves an IANA name from its built-in zone table, so
        // no VTIMEZONE is emitted; a zone it does not know falls back to
        // floating on its side, which is the same guess we would have to make.
        (false, Some(zone)) => Property::raw(name, &join(&String::clone)).with_param("TZID", zone),
        // Form 1, floating. Inventing UTC here would move the event.
        (false, None) => Property::raw(name, &join(&String::clone)),
    }
}

/// The instances of `event` to name in an `EXDATE` (`is_excluded`) or an
/// `RDATE`, rendered and in chronological order.
///
/// An instance drawn as a component of its own is left out of the `RDATE`: the
/// component already places it, and the date would only repeat that it happens.
/// What is left in is every override with a writable id that
/// [`modified_instance`] found nothing to draw — a patch naming a property
/// outside [`OVERRIDE_PROPERTIES`], say. Placing that one at the series' title
/// is a narrowing of the same kind as an `RRULE` that had to drop its `byDay`,
/// and better than an occurrence the user cannot see at all. Where the rules
/// already generate that instant the `RDATE` is a duplicate, and RFC 5545
/// §3.8.5.2 has the recurrence set absorb it.
fn recurrence_dates(event: &CalendarEvent, is_excluded: bool) -> Vec<String> {
    event
        .recurrence_overrides
        .iter()
        .flatten()
        .filter(|(_, patch)| excluded(patch) == is_excluded)
        .filter(|(id, patch)| modified_instance(event, id, patch).is_none())
        .filter_map(|(id, _)| to_ical_date_time(id))
        .collect()
}

/// The iCalendar `STATUS` for a JSCalendar status, or `None` for one outside the
/// closed vocabulary the two share.
fn ical_status(status: &str) -> Option<&'static str> {
    STATUSES
        .iter()
        .find(|(jscalendar, _)| jscalendar.eq_ignore_ascii_case(status))
        .map(|(_, ical)| *ical)
}

fn known_status(status: &str) -> bool {
    ical_status(status).is_some()
}

/// Read an iCalendar object into a calendar event.
///
/// The series is the `VEVENT` **without** a `RECURRENCE-ID`, found by that
/// rather than by position: EDS hands a save every instance of one uid it holds,
/// in no promised order, and taking the first component would read a single
/// edited day as if it were the whole series. The rest become
/// `recurrenceOverrides` entries — see [`read_overrides`].
///
/// A document holding *nothing but* detached instances has no series to attach
/// them to, so the first component is read as the event it describes and the
/// others are dropped. There is nothing better available: JSCalendar says
/// "this instance differs" only relative to a series.
///
/// The `id` is whatever the component's `UID` says, which for an event
/// Evolution has just created is a locally invented string rather than a JMAP
/// id — the caller knows which case it is in and must drop it before sending
/// a create.
pub fn ical_to_event(text: &str) -> Result<CalendarEvent, ICalError> {
    let calendar = syntax::parse(text)?;
    let vevents: Vec<&Component> = calendar
        .children
        .iter()
        .filter(|child| child.name == "VEVENT")
        .collect();
    let series = *vevents
        .iter()
        .find(|vevent| vevent.property(RECURRENCE_ID).is_none())
        .or_else(|| vevents.first())
        .ok_or(ICalError::NoEvent)?;

    let zones = zone_names(&calendar);
    let mut event = read_vevent(series, &zones);
    event.recurrence_overrides = read_overrides(series, &vevents, &event, &zones);
    Ok(event)
}

/// Whether a value is a time zone JSCalendar can carry — an RFC 8984 §1.4.9
/// `TimeZoneId`.
///
/// That type is a name in the IANA Time Zone Database, or a custom identifier
/// beginning with a solidus **that the same object defines in its `timeZones`
/// property**. This mapping does not carry `timeZones`, so the second form has
/// nowhere to be defined and is refused: a dangling identifier is an event a
/// server may reject outright, and rejecting a `CalendarEvent/set` costs the
/// user every edit in it, not just the zone.
///
/// Which matters because a solidus-prefixed identifier is exactly what arrives.
/// libical names its builtin zones `/freeassociation.sourceforge.net/<zone>`
/// and Evolution's appointment editor sets the start with the zone object, so
/// every zoned component a save hands back carries one — even one this crate
/// wrote spelling the zone plainly. [`zone_names`] is how it gets translated.
///
/// What is checked is the *shape* of a name, not membership of the database:
/// non-empty segments of letters, digits, `_`, `-` and `+` separated by `/`.
/// That admits `Europe/Berlin`, `America/Argentina/Buenos_Aires`, `Etc/GMT+5`
/// and the bare `UTC`, and refuses the solidus form (an empty first segment),
/// a Windows zone name like `W. Europe Standard Time`, and anything carrying a
/// character a content line could be broken with. Checking membership would
/// mean shipping a zone database this crate has no other use for; refusing a
/// zone the database does not have is the server's job, and it can do it
/// without the whole save failing.
pub fn names_time_zone(value: &str) -> bool {
    value.split('/').all(|segment| {
        !segment.is_empty()
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+'))
    })
}

/// The zone each `VTIMEZONE` in the document says it describes, by the `TZID`
/// that refers to it.
///
/// Only components that answer the question are listed: a `TZID` that already
/// [`names_time_zone`] needs no translating, and an `X-LIC-LOCATION` that names
/// no zone either is no better than what it would replace.
fn zone_names(calendar: &Component) -> BTreeMap<String, String> {
    calendar
        .children
        .iter()
        .filter(|child| child.name == "VTIMEZONE")
        .filter_map(|vtimezone| {
            let tzid = vtimezone.text("TZID")?;
            let location = vtimezone.text(X_LIC_LOCATION)?;
            (!names_time_zone(&tzid) && names_time_zone(&location)).then_some((tzid, location))
        })
        .collect()
}

/// The zone a `TZID` refers to, as a JSCalendar `TimeZoneId` where the document
/// gives one.
///
/// A value that is already a name is taken at its word — the `VTIMEZONE` beside
/// it is then a description of a zone we already know the name of, and letting a
/// stray `X-LIC-LOCATION` overrule it could move the event to another continent.
/// Otherwise the document's own `VTIMEZONE` is asked, which is where RFC 5545
/// §3.6.5 says the zone a `TZID` refers to is defined.
///
/// A value neither route names is handed back **unchanged**, not dropped. The
/// save path has to be able to tell a zone this mapping cannot name from no
/// zone at all: the latter is a floating event, which is a real thing to save,
/// and reading the former as it would leave the server's zone cleared by an
/// edit that never touched it. Refusing to send it is that path's decision to
/// make, and it needs the value to make it — see `jmap_cal_sync::patch`.
fn zone_of(tzid: &str, zones: &BTreeMap<String, String>) -> String {
    match names_time_zone(tzid) {
        true => tzid.to_owned(),
        false => zones.get(tzid).cloned().unwrap_or_else(|| tzid.to_owned()),
    }
}

/// One `VEVENT` as an event, recurrence rules included and named instances not:
/// those are the document's, not the component's.
fn read_vevent(vevent: &Component, zones: &BTreeMap<String, String>) -> CalendarEvent {
    let text = |name: &str| vevent.text(name).filter(|value| !value.is_empty());
    let (start, time_zone, show_without_time) = read_start(vevent, zones);

    let rules: Vec<RecurrenceRule> = vevent
        .all("RRULE")
        .into_iter()
        .filter_map(|property| rrule_to_rule(&property.raw_value()))
        .collect();

    CalendarEvent {
        id: text("UID").map(Into::into),
        // Membership follows from which EDS source is being served, not from
        // the component, so the backend fills it in on create.
        calendar_ids: None,
        event_type: Some("Event".to_owned()),
        uid: text(X_JMAP_UID),
        title: text("SUMMARY"),
        description: text("DESCRIPTION"),
        start,
        time_zone,
        duration: read_duration(vevent),
        show_without_time,
        status: vevent.text("STATUS").and_then(|status| {
            STATUSES
                .iter()
                .find(|(_, ical)| ical.eq_ignore_ascii_case(&status))
                .map(|(jscalendar, _)| (*jscalendar).to_owned())
        }),
        recurrence_rules: (!rules.is_empty()).then_some(rules),
        recurrence_overrides: None,
        extra: Default::default(),
    }
}

/// The event's start as a JSCalendar LocalDateTime, its time zone, and whether
/// it is shown without a time — three answers because all three are read off
/// the one `DTSTART`, and a date-only one changes the other two.
///
/// A `VALUE=DATE` start is how iCalendar spells an all-day event, so it becomes
/// `showWithoutTime`; the start itself is still read as midnight, because that
/// is what the day begins at and dropping it would leave the event with no time
/// at all. Any `TZID` on such a property is ignored — RFC 5545 §3.2.19 says it
/// does not apply to a DATE value — which also keeps the pair symmetric with
/// [`shows_without_time`], the only shape the writer emits.
///
/// A timed start yields `None` rather than `Some(false)`: the RFC 8984 default
/// is false anyway, and the save path reads an edit off a difference from this,
/// so answering `false` where the server said nothing would invent one.
fn read_start(
    vevent: &Component,
    zones: &BTreeMap<String, String>,
) -> (Option<String>, Option<String>, Option<bool>) {
    let Some(property) = vevent.property("DTSTART") else {
        return (None, None, None);
    };
    let value = property.raw_value();
    let Some(start) = to_local_date_time(&value) else {
        return (None, None, None);
    };
    // calcard renders a DATE value with no `T` in it, whatever the parameters
    // said, so the value is what decides — a `VALUE=DATE` this mapping did not
    // write and a bare `20260115` mean the same thing to a reader.
    if !value.contains(['T', 't']) {
        return (Some(start), None, Some(true));
    }
    let zone = match value.ends_with('Z') {
        true => Some(UTC.to_owned()),
        false => property
            .param("TZID")
            .filter(|zone| !zone.is_empty())
            .map(|zone| zone_of(zone, zones)),
    };
    (Some(start), zone, None)
}

/// The instances the document names one at a time, as `recurrenceOverrides`.
///
/// An `RDATE` becomes an override that patches nothing — the instance happens as
/// the rules would have it — and an `EXDATE` one that is `excluded`. A value
/// neither property can be read as a date-time is skipped, like any other
/// unreadable value, and `None` rather than an empty map is the answer for a
/// document that names none: the save path reads an edit off a difference from
/// what was shown, so an empty map would be a claim that there are no overrides
/// where the document made no claim at all.
///
/// An `RDATE` of `VALUE=PERIOD` (RFC 5545 §3.8.5.2) states the instance's own
/// length as well as its start, which is iCalendar's way of saying "this extra
/// occurrence runs longer than the rest"; it becomes a `duration` patch, or an
/// empty one where the period is as long as the series already is. See
/// [`period_length`]. An `EXDATE` gets no such treatment: RFC 5545 §3.8.5.1
/// admits no period there, and an instance that does not happen has no length.
///
/// `EXDATE` is read after `RDATE` deliberately. A component naming one instant
/// in both properties contradicts itself, and taking it as excluded is the
/// reading that cannot invent an appointment; RFC 5545 §3.8.5.1 also has an
/// `EXDATE` win over the rest of the recurrence set.
///
/// The detached instances are read last, so a `VEVENT` describing an instant an
/// `RDATE` also names wins: they agree that it happens, and only one of them
/// says how. An `EXDATE` for the same instant is a document contradicting
/// itself the other way round, and the edit is the more specific statement.
///
/// Two are skipped rather than read. One whose `RECURRENCE-ID` is not a
/// date-time names no instance to attach to. One carrying `RANGE=THISANDFUTURE`
/// (RFC 5545 §3.2.13) stands for *every* instance from that one on, which
/// `recurrenceOverrides` has no single entry for; reading it as one would move
/// one day and silently drop the change to all the others.
fn read_overrides(
    series: &Component,
    vevents: &[&Component],
    event: &CalendarEvent,
    zones: &BTreeMap<String, String>,
) -> Option<BTreeMap<String, Value>> {
    let mut overrides: BTreeMap<String, Value> = BTreeMap::new();
    let values = |name: &str| series.all(name).into_iter().flat_map(Property::texts);
    for value in values("RDATE") {
        // A period is the only RDATE value with a `/` in it, and calcard renders
        // both its spellings that way, so the separator is what decides.
        let (start, length) = match value.split_once('/') {
            Some((start, end)) => (start, Some(period_length(start, end))),
            None => (value.as_str(), None),
        };
        let Some(date) = to_local_date_time(start) else {
            continue;
        };
        // Only a length that *differs* from the series' is an override, the
        // same rule [`instance_patch`] applies to a detached component; a
        // period restating the series' length says nothing new. An unreadable
        // one patches a `null`, which removes the duration rather than letting
        // the instance inherit a length the document did not give it.
        let patch = match length.filter(|length| *length != event.duration) {
            Some(length) => json!({ "duration": length }),
            None => Value::Object(Default::default()),
        };
        overrides.insert(date, patch);
    }
    for value in values("EXDATE") {
        if let Some(date) = to_local_date_time(&value) {
            overrides.insert(date, json!({"excluded": true}));
        }
    }

    for vevent in vevents {
        if std::ptr::eq(*vevent, series) {
            continue;
        }
        let Some(property) = vevent.property(RECURRENCE_ID) else {
            continue;
        };
        if property.param("RANGE").is_some() {
            continue;
        }
        let Some(id) = to_local_date_time(&property.raw_value()) else {
            continue;
        };
        let patch = instance_patch(event, &read_vevent(vevent, zones), &id);
        overrides.insert(id, patch);
    }

    (!overrides.is_empty()).then_some(overrides)
}

/// How a detached instance differs from the series it belongs to, as an RFC 8984
/// §4.3.4 PatchObject.
///
/// Only [`OVERRIDE_PROPERTIES`] are compared, because only those are restated on
/// the component; a property the instance carries and the mapping does not read
/// is invisible here exactly as it is everywhere else. A property the series has
/// and the instance does not comes back as a `null`, which is how a PatchObject
/// removes one — and how the component said it, by not carrying the line.
///
/// `start` is compared against `id` rather than against the series: an override's
/// key *is* its instance's start, so a `DTSTART` equal to it says nothing, and
/// one that differs is an occurrence the user moved.
fn instance_patch(series: &CalendarEvent, instance: &CalendarEvent, id: &str) -> Value {
    let mut patch = Map::new();
    for (name, was, now) in [
        ("title", &series.title, &instance.title),
        ("description", &series.description, &instance.description),
        // A zone belongs to the property that carries it, so this is a real
        // difference and not an inheritance: a component whose `DTSTART` has no
        // `TZID` floats however the series is written, which is the `null` a
        // PatchObject removes a property with.
        ("timeZone", &series.time_zone, &instance.time_zone),
        ("duration", &series.duration, &instance.duration),
        ("status", &series.status, &instance.status),
    ] {
        if was != now {
            patch.insert(
                (*name).to_owned(),
                now.clone().map_or(Value::Null, Value::String),
            );
        }
    }
    if let Some(start) = instance.start.as_deref().filter(|start| *start != id) {
        patch.insert("start".to_owned(), Value::String(start.to_owned()));
    }
    Value::Object(patch)
}

/// How long the event lasts, as a JSCalendar Duration.
///
/// `DURATION` is what this crate writes and it is passed straight back — the
/// two formats spell an ISO 8601 duration identically. But it is *not* what
/// Evolution writes: the appointment editor calls `e_cal_component_set_dtend`,
/// and RFC 5545 §3.6.1 makes `DTEND` and `DURATION` mutually exclusive, so an
/// event a user just created says how long it is only through its end. Reading
/// `DURATION` alone left every such event with no duration at all, which is
/// `P0D` by RFC 8984 §4.2.2 — a zero-length blip in place of the meeting.
///
/// The difference is taken on the wall clock, which is also how JSCalendar
/// reads the answer: its `P1D` is a nominal day, the same time on the next
/// day, rather than 24 exact hours. The two agree for as long as both ends are
/// in one zone, which is the only shape Evolution writes. A `DTEND` in a
/// *different* zone than the `DTSTART` — legal, and not something this mapping
/// can resolve — comes out short or long by the offset between them; the
/// alternative is dropping the length of an event that plainly states it, and
/// a zone database is a dependency this crate does not carry (see
/// [`rule_to_rrule`] for the same trade made about `UNTIL`).
fn read_duration(vevent: &Component) -> Option<String> {
    if let Some(duration) = vevent
        .property("DURATION")
        .map(Property::raw_value)
        .and_then(|value| stated_duration(&value))
    {
        return Some(duration);
    }
    let start = instant(&vevent.property("DTSTART")?.raw_value())?;
    let end = instant(&vevent.property("DTEND")?.raw_value())?;
    to_duration(end - start)
}

/// A stated length as a JSCalendar Duration, or `None` when the value states no
/// length an event can have. The one check both directions use.
///
/// The two formats spell a length identically, so a value that *is* one is
/// handed over as written rather than re-rendered. But they do not spell the
/// same *set* of lengths: RFC 5545 §3.3.6 admits a sign, and a `DURATION:-PT1H`
/// — an event lasting minus an hour — has nothing to map onto in RFC 8984
/// §1.4.6, while RFC 5545 has no room for a value that is not a duration at all.
/// A value that does not pass is treated as absent, like every other unreadable
/// one, and what that costs differs by direction:
///
/// - **Reading**, [`read_duration`] falls through to `DTEND`, so an event ends
///   up without a length rather than with one nobody can use. Passing it on put
///   a value the server rejects into the save patch, which fails the whole
///   `CalendarEvent/set` and takes the user's real edits down with it.
/// - **Writing**, [`vevent_of`] leaves the property out. libical refusing a
///   content line costs the whole component, so an unwritable length would cost
///   the appointment — every field of it, not just how long it lasts.
/// - **Inside an override**, [`maps_override_field`] refuses it, which tells the
///   save path that this instance was seen in part and its
///   `recurrenceOverrides` must not be written back. The only place an
///   instance's own length is stated is its component's `DURATION`, so a length
///   that cannot go there comes back as the series', which is what the override
///   said it was *not*.
///
/// A leading `+` is the same length said RFC 5545's way, and is dropped rather
/// than handed to a format with nowhere to put it. That is the one value this
/// returns changed rather than verbatim, so an override stating it is still
/// covered: the length that comes back means the same thing, which is what
/// [`maps_override_field`] asks of a field.
///
/// The grammar accepted is deliberately a little looser than RFC 5545's, which
/// nests its units (an hour may be followed only by minutes, a week stands
/// alone): here any of `W D H M S` may be measured, each at most once and in
/// that order, at least one of them, with `T` before the first time unit. That
/// admits `PT1H15S` and `P1W2D`, which every reader adds up the same way and
/// some emitters write. The check exists to refuse values that are not lengths,
/// not to be a conformance test — refusing a length an event plainly states is
/// the failure it is here to avoid, not to cause.
///
/// What it sees is calcard's rendering of the value, not the octets the
/// component carried: a `P1DT` arrives already trimmed to `P1D`. Only what
/// survives *that* is judged here.
fn stated_duration(value: &str) -> Option<String> {
    let value = value.strip_prefix('+').unwrap_or(value);
    let mut rest = value.strip_prefix(['P', 'p'])?;
    let mut measured = false;
    for unit in ['W', 'D', 'T', 'H', 'M', 'S'] {
        if unit == 'T' {
            // Not a unit but the divider before the first of the time ones; it
            // stands only if something is measured after it.
            if let Some(after) = rest.strip_prefix(['T', 't']) {
                rest = after;
                measured = false;
            }
            continue;
        }
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            continue;
        }
        let Some(after) = rest[digits..].strip_prefix([unit, unit.to_ascii_lowercase()]) else {
            continue;
        };
        rest = after;
        measured = true;
    }
    (measured && rest.is_empty()).then(|| value.to_owned())
}

/// How long a period lasts, as a JSCalendar Duration, given its two halves.
///
/// RFC 5545 §3.3.9 spells a period either way — `19960403T020000Z/PT3H` or
/// `19960403T020000Z/19960403T050000Z` — and the two halves of this mapping
/// answer them exactly as [`read_duration`] answers `DURATION` and `DTEND`: a
/// stated duration goes through [`stated_duration`], because both formats spell
/// an ISO 8601 duration identically but not the same set of them, and a stated
/// end is measured on the wall clock.
///
/// `None` is "this period states no length an occurrence could have": a period
/// that ends at or before it starts, and a half that is not a length — RFC 5545
/// §3.3.9 requires the end to be after the start, and RFC 8984 §1.4.6 has no
/// negative duration to map one onto. The negative case falls out of the second
/// branch for free: `-PT1H` is not a `P`, and it is not a date-time either.
///
/// A duration stated as zero is *not* caught, because catching it would mean
/// parsing the value rather than passing it through — `PT0S`, `P0D` and
/// `PT0H0M0S` all spell it. It comes back as written, which RFC 8984 §4.2.2
/// reads as the same zero length the `None` answer leaves behind; the two
/// spellings differ on paper and not in the calendar.
fn period_length(start: &str, end: &str) -> Option<String> {
    if end.starts_with(['P', 'p']) {
        return stated_duration(end);
    }
    to_duration(instant(end)? - instant(start)?)
}

/// A `DTSTART`/`DTEND` value as seconds from 1970-01-01T00:00:00 on its own
/// wall clock — a number to subtract, not an instant on any timeline.
fn instant(value: &str) -> Option<i64> {
    let local = to_local_date_time(value)?;
    let fields: Vec<i64> = local
        .split(['-', 'T', ':'])
        .filter_map(|field| field.parse().ok())
        .collect();
    // Six fields, and each parses, because `to_local_date_time` wrote them.
    let [year, month, day, hour, minute, second] = fields[..] else {
        return None;
    };
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Days from 1970-01-01 to a proleptic Gregorian date, by Howard Hinnant's
/// `days_from_civil`. Exact for every year either format can spell.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    // Count the year as starting in March, which puts a leap day at the end of
    // it and so needs no special case.
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// `5400` → `PT1H30M`, and `86400` → `P1D`.
///
/// Whole days are named as days rather than as 24 hours each, because that is
/// what was measured: the difference came off a wall clock, and JSCalendar's
/// day is the nominal one that survives a daylight saving change.
///
/// A length that is zero or negative yields nothing. Zero is the RFC 8984
/// default anyway, and a negative duration — an event ending before it begins
/// — is a component saying nothing usable about its length, which is better
/// answered with silence than with a value the server would reject or, worse,
/// accept.
fn to_duration(seconds: i64) -> Option<String> {
    if seconds <= 0 {
        return None;
    }
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let mut duration = String::from("P");
    if days > 0 {
        duration.push_str(&format!("{days}D"));
    }
    if rest > 0 {
        duration.push('T');
        for (amount, unit) in [(rest / 3_600, 'H'), (rest / 60 % 60, 'M'), (rest % 60, 'S')] {
            if amount > 0 {
                duration.push_str(&format!("{amount}{unit}"));
            }
        }
    }
    Some(duration)
}

/// Whether this event can be written the way iCalendar writes an all-day one:
/// a `DTSTART` of `VALUE=DATE`, with no time anywhere on the component.
///
/// `showWithoutTime` asks for it, but cannot on its own get it. RFC 8984 §4.1.5
/// says an event shown without a time starts at midnight and lasts whole days;
/// a server is free to send otherwise, and RFC 5545 has no way to write the
/// result — a DATE value has no time to hold 09:00, takes no `TZID`
/// (§3.2.19), and stands only beside a duration of whole days (§3.6.1) and an
/// `UNTIL` that is itself a DATE (§3.3.10). So each of those is a condition
/// here, and an event failing any of them is written as the timed event it
/// half is: wrong about its day-ness, right about when it happens, and — since
/// the save path diffs against this same rendering — not read back as the user
/// having cleared the flag.
///
/// `start` is the already-rendered `DTSTART` value, `YYYYMMDDTHHMMSS`.
fn shows_without_time(event: &CalendarEvent, start: &str) -> bool {
    event.show_without_time == Some(true)
        && event.time_zone.is_none()
        && at_midnight(start)
        && event
            .duration
            .as_deref()
            .filter(|duration| !duration.is_empty())
            .is_none_or(whole_days)
        && event
            .recurrence_rules
            .iter()
            .flatten()
            .filter(|rule| writable(rule))
            .all(|rule| {
                rule.until
                    .as_deref()
                    .and_then(to_ical_date_time)
                    .is_none_or(|until| at_midnight(&until))
            })
        // An instance named at 09:00 cannot be truncated to its date without
        // excluding — or adding — a different occurrence than the server named.
        && event
            .recurrence_overrides
            .iter()
            .flatten()
            .all(|(id, patch)| instance_shows_without_time(event, id, patch))
}

/// Whether one override can be named — and, where it is drawn as a component of
/// its own, written whole — without a time.
///
/// An id no property could carry is dropped rather than written, so it puts no
/// condition on the form. An id that *is* written has to land on a day, and an
/// instance with a component of its own has to meet the same conditions the
/// series does: a start at midnight, a length in whole days and no zone of its
/// own, since RFC 5545 §3.6.1 lets nothing else stand beside a DATE-valued
/// `DTSTART` and §3.2.19 gives such a value no `TZID` to carry a zone on.
fn instance_shows_without_time(event: &CalendarEvent, id: &str, patch: &Value) -> bool {
    let Some(rendered) = to_ical_date_time(id) else {
        return true;
    };
    if !at_midnight(&rendered) {
        return false;
    }
    let Some(instance) = modified_instance(event, id, patch) else {
        return true;
    };
    instance.time_zone.is_none()
        && instance
            .start
            .as_deref()
            .and_then(to_ical_date_time)
            .is_none_or(|start| at_midnight(&start))
        && instance
            .duration
            .as_deref()
            .filter(|duration| !duration.is_empty())
            .is_none_or(whole_days)
}

/// Whether a rendered `YYYYMMDDTHHMMSS` names the top of its day.
fn at_midnight(value: &str) -> bool {
    value.ends_with("T000000")
}

/// Whether an ISO 8601 duration is a whole number of days — RFC 5545's
/// `dur-day`/`dur-week`, the only lengths that may stand beside a DATE start.
///
/// Anything after the designator's `T` is a time component, so its absence is
/// the whole test; a negative duration (a leading `-`) is not a length an event
/// can have and fails here with the rest.
fn whole_days(duration: &str) -> bool {
    let Some(parts) = duration.strip_prefix(['P', 'p']) else {
        return false;
    };
    !parts.is_empty() && !parts.contains(['T', 't'])
}

fn is_utc(zone: &str) -> bool {
    zone.eq_ignore_ascii_case(UTC) || zone.eq_ignore_ascii_case("UTC")
}

/// `2026-01-15T13:00:00` → `20260115T130000`.
fn to_ical_date_time(local: &str) -> Option<String> {
    let (date, time) = local.split_once('T')?;
    let date: String = strip(date, '-', 8)?;
    let time: String = strip(time, ':', 6)?;
    exists(&date, &time).then(|| format!("{date}T{time}"))
}

/// `20260115T130000`, `20260115T130000Z` or `20260115` (`VALUE=DATE`) →
/// `2026-01-15T13:00:00`. A date without a time is read as midnight:
/// `showWithoutTime` is not modeled yet, and an all-day event that lost its
/// start entirely would be worse than one pinned to the top of the day.
fn to_local_date_time(value: &str) -> Option<String> {
    let value = value.strip_suffix(['Z', 'z']).unwrap_or(value);
    let (date, time) = match value.split_once('T') {
        Some((date, time)) => (date, time),
        None => (value, "000000"),
    };
    if date.len() != 8 || time.len() < 6 {
        return None;
    }
    // Sub-second precision is legal in neither format's DATE-TIME, but a
    // trailing fraction is easy to ignore and hard to guess at.
    let time = &time[..6];
    if !date.bytes().chain(time.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !exists(date, time) {
        return None;
    }
    Some(format!(
        "{}-{}-{}T{}:{}:{}",
        &date[..4],
        &date[4..6],
        &date[6..],
        &time[..2],
        &time[2..4],
        &time[4..],
    ))
}

/// Remove `separator` and check that exactly `digits` digits are left.
fn strip(value: &str, separator: char, digits: usize) -> Option<String> {
    let stripped: String = value.chars().filter(|c| *c != separator).collect();
    (stripped.len() == digits && stripped.bytes().all(|b| b.is_ascii_digit())).then_some(stripped)
}

/// Whether `YYYYMMDD` and `HHMMSS` digits name an instant that exists.
///
/// Digits in the right places are not a date, and neither format's reader
/// checks which: calcard reads `20261315T250000` into a date-time whose month
/// is 13, and libical is asked for the value only after this mapping has
/// written it. So the check is here, and a month of 13 is treated the same way
/// as a value that cannot be read at all — as absent — because both directions
/// are worse than losing the property. Handed to libical, an impossible
/// `DTSTART` costs the whole component and with it every field of the event;
/// sent to the server, `"start": "2026-13-15T25:00:00"` is not a JSCalendar
/// LocalDateTime and fails the entire `CalendarEvent/set`, so the user's edit
/// to the title is lost along with it.
///
/// Both callers have already established that the arguments are 8 and 6 ASCII
/// digits.
fn exists(date: &str, time: &str) -> bool {
    let field = |value: &str| value.parse::<u32>().unwrap_or(u32::MAX);
    let (year, month, day) = (field(&date[..4]), field(&date[4..6]), field(&date[6..8]));
    let (hour, minute, second) = (field(&time[..2]), field(&time[2..4]), field(&time[4..6]));
    (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && hour <= 23
        && minute <= 59
        // RFC 5545 §3.3.12 and RFC 3339 §5.6 both allow the leap second, and a
        // server that stores one has to get it back unchanged.
        && second <= 60
}

/// The length of a month, in the proleptic Gregorian calendar both formats use.
fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}

/// An `RRULE` value, or `None` for a rule [`writable`] refuses.
///
/// A rule carrying unmodeled parts in `extra` *is* written, narrowed to what an
/// `RRULE` holds — showing a weekly event on the wrong days beats showing none
/// — and [`maps_recurrence_rule`] is how the save path knows not to write that
/// narrowing back.
fn rule_to_rrule(
    rule: &RecurrenceRule,
    time_zone: Option<&str>,
    as_a_date: bool,
) -> Option<String> {
    if !writable(rule) {
        return None;
    }
    let mut parts = vec![format!("FREQ={}", rule.frequency.to_ascii_uppercase())];
    // INTERVAL=1 is the RFC 5545 default and only makes the line longer.
    if let Some(interval) = rule.interval.filter(|interval| *interval != 1) {
        parts.push(format!("INTERVAL={interval}"));
    }
    if let Some(count) = rule.count {
        parts.push(format!("COUNT={count}"));
    }
    if let Some(until) = rule.until.as_deref().and_then(to_ical_date_time) {
        // JSCalendar's `until` is a local time in the event's own zone, so it
        // is spelled the way DTSTART is — which RFC 5545 §3.3.10 also requires
        // of its value *type*, hence the date-only form for an event written as
        // a date. The time dropped there is midnight, because
        // [`shows_without_time`] does not choose that form otherwise.
        //
        // §3.3.10 asks for a UTC instant when DTSTART carries a TZID;
        // converting one would need a zone database, which this crate
        // deliberately does not depend on, so a zoned event's UNTIL stays
        // local. It round-trips, and libical reads it in the event's zone.
        parts.push(match as_a_date {
            true => format!("UNTIL={}", &until[..8]),
            false => {
                let suffix = match time_zone {
                    Some(zone) if is_utc(zone) => "Z",
                    _ => "",
                };
                format!("UNTIL={until}{suffix}")
            }
        });
    }
    // Last, where RFC 5545's own examples put them, and in the order libical
    // writes them — so a rule that went out this way and came back through EDS's
    // own cache compares equal to itself.
    parts.extend(by_day_part(rule));
    parts.extend(by_month_day_part(rule));
    parts.extend(by_month_part(rule));
    Some(parts.join(";"))
}

/// The `BYDAY` part of a rule's `RRULE`, or `None` when the rule names no days
/// — and also when it names days this mapping will not write, which is what
/// [`maps_recurrence_rule`] reads the answer for.
///
/// It is all the days or none of them. A `BYDAY` holding a subset is a
/// *different* recurrence, not a narrower view of one: dropping `2MO` from
/// `BYDAY=2MO,TH` leaves an event that no longer happens on the Monday at all,
/// which is a worse lie than an event shown on every day of the week the series
/// starts on.
fn by_day_part(rule: &RecurrenceRule) -> Option<String> {
    let days = rule.by_day.as_ref()?;
    // `BYDAY=` names no day and is not a rule part any reader can use.
    if days.is_empty() {
        return None;
    }
    let tokens: Option<Vec<String>> = days
        .iter()
        .map(|day| by_day_token(day, &rule.frequency))
        .collect();
    Some(format!("BYDAY={}", tokens?.join(",")))
}

/// One weekday as an `RRULE` writes it — `TH`, `2WE`, `-1FR` — or `None` for
/// an NDay no `BYDAY` can carry.
///
/// The ordinal is refused outright unless the frequency gives it a period to
/// count within: RFC 5545 §3.3.10 says `BYDAY` MUST NOT carry a numeric value
/// when `FREQ` is not `MONTHLY` or `YEARLY`, and a content line libical refuses
/// costs the whole component — every field of the event, not just its
/// recurrence. Writing the weekday without its ordinal is not the fallback,
/// because "the second Monday" and "every Monday" are different events and the
/// second one fills the user's calendar.
fn by_day_token(day: &NDay, frequency: &str) -> Option<String> {
    if !day.extra.is_empty() {
        return None;
    }
    let weekday = WEEKDAYS
        .iter()
        .find(|weekday| weekday.eq_ignore_ascii_case(&day.day))?;
    match day.nth_of_period {
        None => Some((*weekday).to_owned()),
        // RFC 8984 §4.3.3 forbids zero, and RFC 5545's ordwk starts at 1.
        Some(0) => None,
        Some(nth) if counts_within_a_period(frequency) => Some(format!("{nth}{weekday}")),
        Some(_) => None,
    }
}

/// Whether a frequency gives an `nthOfPeriod` a period to count within — the
/// `MONTHLY`/`YEARLY` of RFC 5545 §3.3.10.
fn counts_within_a_period(frequency: &str) -> bool {
    ["monthly", "yearly"]
        .iter()
        .any(|period| period.eq_ignore_ascii_case(frequency))
}

/// The `BYMONTHDAY` part of a rule's `RRULE`, or `None` when the rule names no
/// days of the month — and, as with [`by_day_part`], when it names ones this
/// mapping will not write.
///
/// It is all the days or none of them, for the same reason: a `BYMONTHDAY`
/// holding a subset is a different recurrence, not a narrower view of one.
fn by_month_day_part(rule: &RecurrenceRule) -> Option<String> {
    let days = rule.by_month_day.as_ref()?;
    // `BYMONTHDAY=` names no day, and a week is not a period a day of the month
    // sits inside: RFC 5545 §3.3.10 says the part MUST NOT be specified when
    // `FREQ` is `WEEKLY`, and a content line libical refuses costs the whole
    // component.
    if days.is_empty() || "weekly".eq_ignore_ascii_case(&rule.frequency) {
        return None;
    }
    let tokens: Option<Vec<String>> = days.iter().copied().map(month_day_token).collect();
    Some(format!("BYMONTHDAY={}", tokens?.join(",")))
}

/// One day of the month as an `RRULE` writes it — `15`, `-1` — or `None` for a
/// value no `BYMONTHDAY` can carry.
fn month_day_token(day: i32) -> Option<String> {
    match day {
        // RFC 5545's `ordmoday` is 1 to 31, which RFC 8984 §4.3.3 counts
        // backwards from the end of the month as well. Zero is no day of any
        // month, and neither format admits it.
        -31..=-1 | 1..=31 => Some(day.to_string()),
        _ => None,
    }
}

/// The `BYMONTH` part of a rule's `RRULE`, or `None` when the rule names no
/// months of the year — and, as with [`by_day_part`], when it names ones this
/// mapping will not write.
///
/// It is all the months or none of them, for the same reason: a `BYMONTH`
/// holding a subset is a different recurrence, not a narrower view of one.
///
/// There is no frequency gate. RFC 5545 §3.3.10 defines `BYMONTH` at every
/// frequency — limiting the occurrences a shorter period expands to, rather than
/// expanding them — unlike `BYMONTHDAY`, which a week has no room for.
fn by_month_part(rule: &RecurrenceRule) -> Option<String> {
    let months = rule.by_month.as_ref()?;
    // `BYMONTH=` names no month and is not a rule part any reader can use.
    if months.is_empty() {
        return None;
    }
    let tokens: Option<Vec<&str>> = months.iter().map(|month| month_token(month)).collect();
    Some(format!("BYMONTH={}", tokens?.join(",")))
}

/// One month of the year as an `RRULE` writes it, or `None` for a value no
/// `BYMONTH` this mapping is willing to write can carry.
///
/// The value that comes back is the caller's own string, because the only
/// spelling accepted is the one iCalendar writes back unchanged: RFC 5545's
/// `monthnum` also admits a leading zero (`03`), which both libical and calcard
/// re-render as `3` — a rule written that way would return spelled differently
/// and read as an edit the user never made.
///
/// A leap month — RFC 8984 §4.3.3's `5L`, the reason `byMonth` holds strings at
/// all — is refused rather than written: iCalendar can only name one under
/// RFC 7529's `RSCALE`, and this mapping does not model an event's calendar
/// system, so `BYMONTH=5L` beside a Gregorian series would name a month that
/// series does not have.
///
/// Every other refusal is a month no year has, and libical answers those two
/// ways — dropping the whole `RRULE`, or keeping a rule that can never occur.
/// `jmap-backend-cal/tests/marshal.rs` records which is which.
fn month_token(month: &str) -> Option<&str> {
    match month.parse::<u32>() {
        Ok(number @ 1..=12) if month == number.to_string() => Some(month),
        _ => None,
    }
}

/// The reverse. Parts outside the modeled set are dropped rather than parked
/// in `extra`: a `BYSETPOS=-1` copied verbatim into JSCalendar would be
/// rejected by the server, whose `bySetPosition` is an array of numbers.
fn rrule_to_rule(value: &str) -> Option<RecurrenceRule> {
    let mut rule = RecurrenceRule::default();
    for part in value.split(';') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        match key.to_ascii_uppercase().as_str() {
            "FREQ" => rule.frequency = value.to_ascii_lowercase(),
            "INTERVAL" => rule.interval = value.parse().ok(),
            "COUNT" => rule.count = value.parse().ok(),
            "UNTIL" => rule.until = to_local_date_time(value),
            "BYDAY" => rule.by_day = Some(value.split(',').map(to_nday).collect()),
            "BYMONTHDAY" => {
                rule.by_month_day = Some(value.split(',').map(to_month_day).collect());
            }
            // Each token verbatim: JSCalendar holds a month as the string
            // iCalendar spells it with, so one this mapping will not write back —
            // a thirteenth month, a leap month — is carried as itself and refused
            // by [`month_token`] on the way out, which is what
            // [`maps_recurrence_rule`] then reads.
            "BYMONTH" => rule.by_month = Some(value.split(',').map(str::to_owned).collect()),
            _ => {}
        }
    }
    if rule.frequency.is_empty() {
        return None;
    }
    rule.rule_type = Some("RecurrenceRule".to_owned());
    Some(rule)
}

/// One `BYDAY` token as the NDay it names — `TH`, `2WE`, `-1FR`, and RFC 5545
/// §3.3.10's signed spelling `+3TU`, whose plus JSCalendar has no room for.
///
/// A token this cannot take apart keeps its whole self as the `day`, which is
/// outside the closed vocabulary and so is refused by [`by_day_token`] on the
/// way back out and flagged by [`maps_recurrence_rule`]. Reading it as the
/// weekday alone would drop an ordinal the rule was written with and repeat the
/// event on every one of that weekday instead.
fn to_nday(token: &str) -> NDay {
    let unsigned = token.strip_prefix(['+', '-']).unwrap_or(token);
    let digits = unsigned.len()
        - unsigned
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .len();
    if digits == 0 {
        return NDay::new(&token.to_ascii_lowercase());
    }
    let (ordinal, weekday) = token.split_at(token.len() - unsigned.len() + digits);
    match ordinal.parse::<i32>() {
        // Zero is no occurrence of a weekday, and a value too large for the
        // ordinal is one this cannot hand back unchanged; both keep the token.
        Ok(nth) if nth != 0 => NDay {
            nth_of_period: Some(nth),
            ..NDay::new(&weekday.to_ascii_lowercase())
        },
        _ => NDay::new(&token.to_ascii_lowercase()),
    }
}

/// One `BYMONTHDAY` token as the number JSCalendar holds — RFC 5545 §3.3.10's
/// signed `monthdaynum`, whose leading plus `str::parse` accepts and JSCalendar
/// has no room for.
///
/// A token this cannot read becomes zero, which is a day no month has: it is
/// refused by [`month_day_token`] on the way back out and so flagged by
/// [`maps_recurrence_rule`], exactly as a `BYDAY` token [`to_nday`] cannot take
/// apart is. Dropping it instead would leave a *smaller* set of days looking
/// like the whole rule, and a save would then delete whichever day the server
/// really held there.
fn to_month_day(token: &str) -> i32 {
    token.parse().unwrap_or(0)
}
