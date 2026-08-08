// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning an edited component back into a `CalendarEvent/set` PatchObject.
//!
//! The whole point of patching rather than replacing is that a `VEVENT` is a
//! lossy view of a JSCalendar event. The mapping keeps seven properties and
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
//! Two properties need more than the baseline, because for them "no
//! difference" is not the whole question:
//!
//! - **`recurrenceRules` is one property, not a merge point.** A rule with
//!   `byDay` cannot be spelled as an `RRULE` this crate emits, so patching
//!   the array at all would narrow the user's recurrence behind their back.
//!   If any rule the server holds fails [`maps_recurrence_rule`], the
//!   property is left alone entirely.
//! - **`start` is required by RFC 8984.** A component whose `DTSTART` the
//!   mapping cannot read yields no start, and `"start": null` is not a legal
//!   way to say so, so the server's start stands.

use jmap_ical::{event_to_ical, ical_to_event, maps_recurrence_rule};
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
    for (property, was, now) in [
        ("title", &baseline.title, &edited.title),
        ("description", &baseline.description, &edited.description),
        ("timeZone", &baseline.time_zone, &edited.time_zone),
        ("duration", &baseline.duration, &edited.duration),
        ("status", &baseline.status, &edited.status),
    ] {
        if was != now {
            set(&mut patch, property, now.as_deref());
        }
    }

    diff_recurrence(&mut patch, current, &baseline, edited);
    patch
}

/// The recurrence, replaced whole or not at all — and not at all whenever the
/// server holds a rule the `RRULE` could not carry.
fn diff_recurrence(
    patch: &mut Map<String, Value>,
    current: &CalendarEvent,
    baseline: &CalendarEvent,
    edited: &CalendarEvent,
) {
    if current
        .recurrence_rules
        .iter()
        .flatten()
        .any(|rule| !maps_recurrence_rule(rule))
    {
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

fn set(patch: &mut Map<String, Value>, property: &str, value: Option<&str>) {
    patch.insert(
        property.to_owned(),
        value.map_or(Value::Null, |value| Value::String(value.to_owned())),
    );
}
