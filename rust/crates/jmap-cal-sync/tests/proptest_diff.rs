// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Property tests for `patch::diff`'s plain baseline-compared scalar
//! properties (RFC 8620 §5.3): `title`, `description`, `duration`, `status`,
//! `freeBusyStatus`, `privacy` and `priority`. These are the properties
//! `diff` handles with no keyed-map or merge/rekey logic at all: compare the
//! *baseline* (the server's event put through
//! `ical_to_event(event_to_ical(current))`) to `edited`, and write the
//! property when they differ.
//!
//! The invariant: for a pair of events (`current`, `edited`) whose fields are
//! already a fixed point of that same round trip — realistic, since a real
//! `current` is always something the server's own representation already
//! went through, and a real `edited` is always something EDS's parse of a
//! saved component went through — applying `diff(current, edited)` to
//! `current` under plain PatchObject semantics yields `edited`, and
//! `diff(current, current)` is empty. `stable()` below builds exactly such a
//! fixed point: it runs a freshly built event through the same
//! `event_to_ical`/`ical_to_event` round trip `diff` baselines against, so
//! `current`/`edited` never carry a value the round trip would itself have
//! rewritten — which is the situation `diff` was never asked to handle, and
//! handles by design via the baseline rather than by checking `current`
//! directly.
//!
//! Scoped out, left for later: `start` (its absence cannot be expressed, so
//! it needs its own generator) and `timeZone` (checked twice over: via
//! `maps_time_zone` as it goes out, then again by `diff_time_zones`
//! afterwards); `showWithoutTime` (its own base event, with no time zone, a
//! midnight start and a whole-day duration, since the shared timed base this
//! file uses can never render as all-day regardless of the flag); and the
//! eight properties the module doc already calls out as needing more than
//! the baseline (`locations`, `virtualLocations`, `links`, `keywords`,
//! `alerts`, `recurrenceRule`, `recurrenceOverrides`, `timeZones`) — each
//! needs its own generator and is left for its own increment.
//!
//! `title`/`description` are restricted to plain alphanumeric text with no
//! leading, trailing or interior punctuation: `SUMMARY`/`SSCRIPTION` escaping
//! and line folding are a separate question from the baseline-compare shape
//! this increment covers.

use jmap_cal_sync::patch::diff;
use jmap_ical::{event_to_ical, ical_to_event};
use jmap_proto::calendars::CalendarEvent;
use proptest::prelude::*;
use serde_json::{Map, Value};

/// RFC 8620 §5.3: a `null` value at the end of a path removes that member,
/// anything else sets it; every path segment before the last already exists.
fn apply_patch(target: &mut Value, patch: &Map<String, Value>) {
    for (path, value) in patch {
        let segments: Vec<String> = path.split('/').map(unescape_pointer).collect();
        apply_at(target, &segments, value.clone());
    }
}

fn apply_at(target: &mut Value, segments: &[String], value: Value) {
    let Value::Object(map) = target else {
        return;
    };
    if segments.len() == 1 {
        if value.is_null() {
            map.remove(&segments[0]);
        } else {
            map.insert(segments[0].clone(), value);
        }
        return;
    }
    let child = map
        .entry(segments[0].clone())
        .or_insert_with(|| Value::Object(Map::new()));
    apply_at(child, &segments[1..], value);
}

/// RFC 6901 §3, reversed.
fn unescape_pointer(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

/// A timed event (a time zone and a non-midnight start) that is its own
/// baseline: the round trip `diff` compares against cannot rewrite any of
/// the scalar properties this file varies.
fn base() -> CalendarEvent {
    CalendarEvent::new()
        .with_uid("seed-uid")
        .with_start("2026-01-15T13:00:00")
        .with_time_zone("Europe/Berlin")
        .with_duration("PT1H")
}

/// `event`, put through the same `event_to_ical`/`ical_to_event` round trip
/// `diff` baselines a server's event against, so the result is already a
/// fixed point of it.
fn stable(event: CalendarEvent) -> CalendarEvent {
    ical_to_event(&event_to_ical(&event)).expect("a rendered event always parses back")
}

fn round_trips(current: &CalendarEvent, edited: &CalendarEvent) -> Result<(), TestCaseError> {
    prop_assert!(diff(current, current).is_empty());
    let patch = diff(current, edited);
    let mut value = serde_json::to_value(current).expect("CalendarEvent always serializes");
    apply_patch(&mut value, &patch);
    let applied: CalendarEvent =
        serde_json::from_value(value).expect("a patched CalendarEvent always parses");
    prop_assert_eq!(&applied, edited, "patch was: {:?}", patch);
    Ok(())
}

fn arb_text() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9]{1,20}"
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn title_round_trip(old in arb_text(), new in arb_text()) {
        let current = stable(base().with_title(old));
        let edited = stable(base().with_title(new));
        round_trips(&current, &edited)?;
    }

    #[test]
    fn description_round_trip(old in arb_text(), new in arb_text()) {
        let current = stable(base().with_description(old));
        let edited = stable(base().with_description(new));
        round_trips(&current, &edited)?;
    }

    #[test]
    fn duration_round_trip(
        old_hours in 1u32..12,
        new_hours in 1u32..12,
    ) {
        let current = stable(base().with_duration(format!("PT{old_hours}H")));
        let edited = stable(base().with_duration(format!("PT{new_hours}H")));
        round_trips(&current, &edited)?;
    }

    #[test]
    fn status_round_trip(
        old in prop::sample::select(&["confirmed", "cancelled", "tentative"][..]),
        new in prop::sample::select(&["confirmed", "cancelled", "tentative"][..]),
    ) {
        let current = stable(base().with_status(old));
        let edited = stable(base().with_status(new));
        round_trips(&current, &edited)?;
    }

    #[test]
    fn free_busy_status_round_trip(
        old in prop::sample::select(&["free", "busy"][..]),
        new in prop::sample::select(&["free", "busy"][..]),
    ) {
        let current = stable(base().with_free_busy_status(old));
        let edited = stable(base().with_free_busy_status(new));
        round_trips(&current, &edited)?;
    }

    #[test]
    fn privacy_round_trip(
        old in prop::sample::select(&["public", "private", "secret"][..]),
        new in prop::sample::select(&["public", "private", "secret"][..]),
    ) {
        let current = stable(base().with_privacy(old));
        let edited = stable(base().with_privacy(new));
        round_trips(&current, &edited)?;
    }

    #[test]
    fn priority_round_trip(old in 0i64..=9, new in 0i64..=9) {
        let current = stable(base().with_priority(old));
        let edited = stable(base().with_priority(new));
        round_trips(&current, &edited)?;
    }
}
