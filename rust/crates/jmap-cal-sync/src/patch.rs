// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning an edited component back into a `CalendarEvent/set` PatchObject.
//!
//! The whole point of patching rather than replacing is that a `VEVENT` is a
//! lossy view of a JSCalendar event. The mapping keeps nine properties and
//! drops everything else, so a save that sent the parsed event back whole
//! would silently delete what it could not represent — locations,
//! participants, alerts, links — none of which the user ever saw, let alone
//! asked to remove.
//!
//! The lossiness also recurs *inside* the properties that are mapped, and
//! this module answers that with one move: it does not compare the edited
//! event against the event the server holds, but against **the server's event
//! put through the same round trip**. That baseline is, by construction,
//! exactly what Evolution was shown, so a difference from it is an edit and
//! nothing else is. It falls out that
//!
//! - a `timeZone` of `UTC` is not rewritten to `Etc/UTC` merely because the
//!   `Z` suffix the component carries reads back as the latter,
//! - an `RRULE` that had to drop `INTERVAL=1` does not come back as a rule
//!   with `interval` removed,
//! - and a `status` outside the closed vocabulary is not cleared by a save
//!   that never touched it.
//!
//! Three properties need more than the baseline, because for them "no
//! difference" is not the whole question:
//!
//! - **`recurrenceRules` is one property, not a merge point.** A rule with
//!   `byHour` cannot be spelled as an `RRULE` this crate emits, so patching
//!   the array at all would narrow the user's recurrence behind their back.
//!   If any rule the server holds fails [`maps_recurrence_rule`], the
//!   property is left alone entirely — as does one the *save* brings that
//!   cannot be sent, which is the same check the series' `timeZone` gets
//!   below: the property goes out replaced whole, so a part the server is
//!   entitled to reject would cost every other edit in the save.
//! - **`recurrenceOverrides` is the same story one level down.** An
//!   `EXDATE`, an `RDATE` and a `RECURRENCE-ID` component between them say that
//!   an instance is off, that it happens, and that it happens with another
//!   title, start, zone, length, description or status — but not that it happens
//!   in another place or with another guest list. An override the component could
//!   only place with a bare `RDATE` would come back as the empty patch,
//!   deleting what it could not draw, so if any override the server holds fails
//!   [`maps_recurrence_override`], the property is left alone entirely — as it
//!   is when an override the *save* brings holds a value that cannot be sent,
//!   which is the same check the series' `timeZone` gets below.
//! - **`start` is required by RFC 8984.** A component whose `DTSTART` the
//!   mapping cannot read yields no start, and `"start": null` is not a legal
//!   way to say so, so the server's start stands.
//!
//! And one property is checked on its way *out* rather than against the
//! baseline: a `TZID` is an iCalendar identifier, which RFC 8984 §1.4.9 only
//! sometimes admits as a time zone. See [`names_time_zone`] and [`diff`].

use jmap_ical::{
    event_to_ical, ical_to_event, maps_recurrence_override, maps_recurrence_rule, names_time_zone,
};
use jmap_proto::calendars::CalendarEvent;
use serde_json::{Map, Value};

/// The patch that turns the event the server holds into the event Evolution
/// just saved. Empty when the edit changed nothing this mapping can see.
pub fn diff(current: &CalendarEvent, edited: &CalendarEvent) -> Map<String, Value> {
    let mut patch = Map::new();

    // What Evolution was actually shown. Rendering an event we just parsed
    // from the server cannot normally fail to parse back; if it somehow does,
    // there is no trustworthy baseline to diff against and sending nothing is
    // the only safe answer.
    let Ok(baseline) = ical_to_event(&event_to_ical(current)) else {
        return patch;
    };

    // `start` is the one property whose absence cannot be expressed.
    if edited.start.is_some() && edited.start != baseline.start {
        set(&mut patch, "start", edited.start.as_deref());
    }
    // `timeZone` is the one property whose *new* value may be unsendable. A
    // component states its zone with a `TZID`, which is an iCalendar identifier
    // and not the RFC 8984 §1.4.9 name JSCalendar wants; where the document
    // gave no way to translate it, the zone is left alone rather than sent as
    // it came or cleared. It is the same "seen in part, so not written back"
    // rule the recurrence properties follow, applied to one value.
    if edited.time_zone.as_deref().is_none_or(names_time_zone)
        && baseline.time_zone != edited.time_zone
    {
        set(&mut patch, "timeZone", edited.time_zone.as_deref());
    }
    for (property, was, now) in [
        ("title", &baseline.title, &edited.title),
        ("description", &baseline.description, &edited.description),
        ("duration", &baseline.duration, &edited.duration),
        ("status", &baseline.status, &edited.status),
    ] {
        if was != now {
            set(&mut patch, property, now.as_deref());
        }
    }

    // `showWithoutTime` is a flag rather than a string, and the baseline is
    // what makes it safe to diff at all: the component says "all day" only as a
    // DATE-valued DTSTART, and there are events — one starting at 09:00, one
    // carrying a zone — the mapping has to render as timed even though the
    // server called them all-day. Rendering the server's own event the same way
    // loses the flag on both sides, so the two agree and nothing is patched;
    // only a component that really did change its mind reaches the server.
    if baseline.show_without_time != edited.show_without_time {
        patch.insert(
            "showWithoutTime".to_owned(),
            // Null rather than `false`: the RFC 8984 default is false, and
            // removing the property is how a PatchObject says "back to the
            // default".
            edited.show_without_time.map_or(Value::Null, Value::Bool),
        );
    }

    diff_recurrence(&mut patch, current, &baseline, edited);
    diff_overrides(&mut patch, current, &baseline, edited);
    patch
}

/// The recurrence, replaced whole or not at all — and not at all whenever
/// either side holds a rule the `RRULE` could not carry: the server's, which
/// would be narrowed by the drawing, or the save's, which would be sent as a
/// rule the server may refuse.
fn diff_recurrence(
    patch: &mut Map<String, Value>,
    current: &CalendarEvent,
    baseline: &CalendarEvent,
    edited: &CalendarEvent,
) {
    if [current, edited].iter().any(|event| {
        event
            .recurrence_rules
            .iter()
            .flatten()
            .any(|rule| !maps_recurrence_rule(rule))
    }) {
        return;
    }
    if baseline.recurrence_rules == edited.recurrence_rules {
        return;
    }
    patch.insert(
        "recurrenceRules".to_owned(),
        match &edited.recurrence_rules {
            // Serialising rules built from an RRULE cannot fail: they hold
            // strings and numbers.
            Some(rules) => serde_json::to_value(rules).unwrap_or(Value::Null),
            None => Value::Null,
        },
    );
}

/// The instances named one at a time, replaced whole or not at all — and not at
/// all whenever the server holds an override the two iCalendar properties could
/// not carry.
///
/// Deleting one occurrence of a recurring event is the whole point of this: it
/// reaches EDS as an `EXDATE` on the component and has to reach the server as an
/// `excluded` override. Removing that line again is a restore, which is
/// `"recurrenceOverrides": null` — a PatchObject removes a property to mean "back
/// to the default", and the default is no named instances.
fn diff_overrides(
    patch: &mut Map<String, Value>,
    current: &CalendarEvent,
    baseline: &CalendarEvent,
    edited: &CalendarEvent,
) {
    // The overrides the *server* holds, checked as above: one it could only draw
    // in part must not be replaced by the drawing.
    //
    // And the overrides the *save* brings, checked the same way for the same
    // reason the series' own `timeZone` is checked on its way out: an instance
    // states its zone with a `TZID`, which RFC 8984 §1.4.9 only sometimes admits
    // as a name, and this property goes out replaced whole — so one entry the
    // server is entitled to reject would cost every edit in the save.
    if [current, edited].iter().any(|event| {
        event
            .recurrence_overrides
            .iter()
            .flatten()
            .any(|(id, override_patch)| !maps_recurrence_override(id, override_patch))
    }) {
        return;
    }
    if baseline.recurrence_overrides == edited.recurrence_overrides {
        return;
    }
    patch.insert(
        "recurrenceOverrides".to_owned(),
        match &edited.recurrence_overrides {
            // Serialising a map of the two patches this mapping reads cannot
            // fail: they hold one boolean between them.
            Some(overrides) => serde_json::to_value(overrides).unwrap_or(Value::Null),
            None => Value::Null,
        },
    );
}

fn set(patch: &mut Map<String, Value>, property: &str, value: Option<&str>) {
    patch.insert(
        property.to_owned(),
        value.map_or(Value::Null, |value| Value::String(value.to_owned())),
    );
}
