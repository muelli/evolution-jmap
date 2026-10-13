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
//! Also covered: `keywords` (`diff_keywords`), scoped to the case where every
//! tag on both sides is one `maps_keyword` draws (plain alphanumeric, so
//! always a non-empty, `\r`-free trimmed string paired with `Value::Bool(true)`).
//! Restricted to that case, `diff_keywords`'s carry-forward of a tag `current`
//! holds that the `CATEGORIES` line could not draw never fires, so the patch
//! is a literal round trip with `edited.keywords` and needs no third
//! "expected" event. Scoped out, left for later: a tag the server holds that
//! `maps_keyword` refuses (an empty or whitespace-only name, a non-bool
//! value, a `\r`), which needs the three-event "expected" shape the book-sync
//! anniversaries/media/online_services increments used.
//!
//! Also covered: `links` (`diff_links`), restricted to plain `ATTACH`-shaped
//! entries (an `https:` `href` and an optional `contentType`, never the
//! `rel: "display"` `IMAGE` shape). `diff_links` only ever patches an
//! existing key's `href` — it neither adds a key `edited` states that
//! `current` does not, nor removes one `edited` drops — so the generator
//! keeps the same key set on both sides and only varies `href`, with
//! `contentType` held constant per key (never patched, so it would fail the
//! round trip if it varied). Scoped out, left for later: `size` (`ATTACH`
//! only), the `IMAGE`/`rel`/`display` shape, and a key `edited` adds or
//! drops, which `diff_links` leaves alone by design and so needs a check
//! against the unpatched property rather than a round trip to `edited`.
//!
//! Also covered: `virtualLocations` (`diff_virtual_locations`), restricted to
//! entries with a plain `https:` `uri` on both sides (so every entry survives
//! [`maps_virtual_locations`] and keeps naming a place, never removed by the
//! diff) and independently varied `name` and `features` (a subset of
//! `CONFERENCE_FEATURES`). `diff_virtual_locations` patches `uri`/`name`/
//! `features` into an existing key member by member and neither adds a key
//! `edited` states that `current` does not nor removes one `edited` drops, so
//! the generator keeps the same key set on both sides, like `links_round_trip`
//! above. Scoped out, left for later: a key one side adds or drops (needs a
//! check against the unpatched property, not a round trip to `edited`), and a
//! `uri` that stops naming a place at all (removes the entry from being read
//! back as a virtual location in the first place, not a patched member).
//!
//! Scoped out, left for later: `start` (its absence cannot be expressed, so
//! it needs its own generator) and `timeZone` (checked twice over: via
//! `maps_time_zone` as it goes out, then again by `diff_time_zones`
//! afterwards); `showWithoutTime` (its own base event, with no time zone, a
//! midnight start and a whole-day duration, since the shared timed base this
//! file uses can never render as all-day regardless of the flag); and the
//! remaining five properties the module doc already calls out as needing
//! more than the baseline (`locations`, `alerts`, `recurrenceRule`,
//! `recurrenceOverrides`, `timeZones`) — each needs its own generator and is
//! left for its own increment.
//!
//! `title`/`description` are restricted to plain alphanumeric text with no
//! leading, trailing or interior punctuation: `SUMMARY`/`SSCRIPTION` escaping
//! and line folding are a separate question from the baseline-compare shape
//! this increment covers.

use std::collections::BTreeMap;

use jmap_cal_sync::patch::diff;
use jmap_ical::{event_to_ical, ical_to_event};
use jmap_proto::calendars::CalendarEvent;
use proptest::prelude::*;
use serde_json::{Map, Value, json};

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

/// A tag `maps_keyword` always draws: plain alphanumeric, so trimmed,
/// non-empty and `\r`-free already.
fn arb_tag() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9]{1,10}"
}

/// A `keywords` set, every member one `arb_tag` builds.
fn arb_keywords() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_set(arb_tag(), 0..5).prop_map(|tags| {
        tags.into_iter()
            .map(|tag| (tag, Value::Bool(true)))
            .collect()
    })
}

/// A key `names_map_entry` always accepts (plain alphanumeric), the only
/// kind `X-JMAP-KEY` carries here.
fn arb_key() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9]{1,10}"
}

/// An `href` `names_a_uri` always accepts and `fetched_locally` never drops:
/// an `https:` URI with a plain alphanumeric path.
fn arb_href() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9]{1,10}".prop_map(|path| format!("https://example.com/{path}"))
}

/// A `contentType` `media_type` always draws back unchanged, or none at all.
fn arb_content_type() -> impl Strategy<Value = Option<String>> {
    prop::option::of(prop::sample::select(
        &["text/plain", "image/png", "application/pdf"][..],
    ))
    .prop_map(|ct| ct.map(str::to_owned))
}

/// A plain `ATTACH`-shaped Link entry: an `href` and an optional
/// `contentType`, never the `rel`/`display`/`size` members this increment
/// scopes out.
fn link_value(href: &str, content_type: &Option<String>) -> Value {
    let mut link = json!({"@type": "Link", "href": href});
    if let Some(content_type) = content_type {
        link["contentType"] = Value::String(content_type.clone());
    }
    link
}

/// A set of 1 to 4 keys, each carrying an old `href` (for `current`), a new
/// one (for `edited`), and a `contentType` held constant across both: the one
/// member `diff_links` ever patches is `href`, so varying anything else would
/// fail the round trip against a diff that never writes it.
fn arb_links() -> impl Strategy<Value = BTreeMap<String, (String, String, Option<String>)>> {
    prop::collection::btree_map(
        arb_key(),
        (arb_href(), arb_href(), arb_content_type()),
        1..4,
    )
}

/// A `name` `stated_name` draws back unchanged, or none at all: plain
/// alphanumeric, so never the empty string `stated_name` treats as absent.
fn arb_place_name() -> impl Strategy<Value = Option<String>> {
    prop::option::of("[a-zA-Z0-9]{1,10}")
}

/// A `features` set drawn from `CONFERENCE_FEATURES`, as the `Value` a save
/// would see: `None` for an empty subset, since `read_virtual_locations` only
/// ever writes the member when at least one `FEATURE` param was read.
fn arb_features() -> impl Strategy<Value = Option<Value>> {
    prop::collection::btree_set(
        prop::sample::select(
            &[
                "audio",
                "chat",
                "feed",
                "moderator",
                "phone",
                "screen",
                "video",
            ][..],
        ),
        0..3,
    )
    .prop_map(|features| {
        (!features.is_empty()).then(|| {
            Value::Object(
                features
                    .into_iter()
                    .map(|feature| (feature.to_owned(), Value::Bool(true)))
                    .collect(),
            )
        })
    })
}

/// A VirtualLocation entry: a `uri` (required, held across old/new so the
/// entry always keeps naming a place) and independently varied `name` and
/// `features`.
fn virtual_location_value(uri: &str, name: &Option<String>, features: &Option<Value>) -> Value {
    let mut place = json!({"@type": "VirtualLocation", "uri": uri});
    if let Some(name) = name {
        place["name"] = Value::String(name.clone());
    }
    if let Some(features) = features {
        place["features"] = features.clone();
    }
    place
}

/// A set of 1 to 4 keys, each carrying a `uri` (held constant: the entry must
/// keep naming a place on both sides, since a `uri` that stops naming one is
/// a removal from the read, not a patched member) and old/new `name` and
/// `features`.
#[allow(clippy::type_complexity)]
fn arb_virtual_locations() -> impl Strategy<
    Value = BTreeMap<
        String,
        (
            String,
            Option<String>,
            Option<String>,
            Option<Value>,
            Option<Value>,
        ),
    >,
> {
    prop::collection::btree_map(
        arb_key(),
        (
            arb_href(),
            arb_place_name(),
            arb_place_name(),
            arb_features(),
            arb_features(),
        ),
        1..4,
    )
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

    #[test]
    fn keywords_round_trip(old in arb_keywords(), new in arb_keywords()) {
        let mut old_event = base();
        old_event.keywords = Some(old);
        let mut new_event = base();
        new_event.keywords = Some(new);
        let current = stable(old_event);
        let edited = stable(new_event);
        round_trips(&current, &edited)?;
    }

    #[test]
    fn links_round_trip(entries in arb_links()) {
        let mut old_event = base();
        old_event.links = Some(
            entries
                .iter()
                .map(|(key, (old, _, content_type))| (key.clone(), link_value(old, content_type)))
                .collect(),
        );
        let mut new_event = base();
        new_event.links = Some(
            entries
                .iter()
                .map(|(key, (_, new, content_type))| (key.clone(), link_value(new, content_type)))
                .collect(),
        );
        let current = stable(old_event);
        let edited = stable(new_event);
        round_trips(&current, &edited)?;
    }

    #[test]
    fn virtual_locations_round_trip(entries in arb_virtual_locations()) {
        let mut old_event = base();
        old_event.virtual_locations = Some(
            entries
                .iter()
                .map(|(key, (uri, old_name, _, old_features, _))| {
                    (key.clone(), virtual_location_value(uri, old_name, old_features))
                })
                .collect(),
        );
        let mut new_event = base();
        new_event.virtual_locations = Some(
            entries
                .iter()
                .map(|(key, (uri, _, new_name, _, new_features))| {
                    (key.clone(), virtual_location_value(uri, new_name, new_features))
                })
                .collect(),
        );
        let current = stable(old_event);
        let edited = stable(new_event);
        round_trips(&current, &edited)?;
    }
}
