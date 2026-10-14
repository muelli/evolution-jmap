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
//! Also covered: `locations` (`diff_locations`), restricted to the bare
//! `{"@type": "Location", "name": ...}` shape `read_locations` itself ever
//! produces — no other member — and to zero or one entry, the only cases
//! `maps_locations` allows through at all. That shape means every clear goes
//! through the whole-property branch (the entry holds nothing but the name,
//! so there is nothing left to keep), so the generator exercises rename
//! (entry present on both sides, name changes), clear (present then absent),
//! create (absent then present) and no-op (absent on both, or present with
//! the same name) by varying an `Option<String>` name independently on each
//! side, with the key held constant where an entry exists on either side.
//! Scoped out, left for later: an entry with more than a name (so a clear
//! would remove only `locations/<key>/name` and keep the rest, the branch
//! this increment's shape never reaches).
//!
//! Also covered: `alerts` (`diff_alerts`), restricted to the exact shapes
//! `read_alert` ever produces for an offset trigger — relative to the start,
//! `{"@type": "Alert", "trigger": {"@type": "OffsetTrigger", "offset": ...},
//! "action": "display"}`, and relative to the end, the same shape plus
//! `"relativeTo": "end"`. Unlike the keyed maps above, `diff_alerts` replaces
//! the whole property (like `keywords`), gated only by `maps_alerts` on
//! `current`; the edited side needs no such check, since `read_alert` only
//! ever produces a shape `drawn_alert` accepts. Restricted to those shapes,
//! every alert on both sides maps, so the patch is a literal round trip with
//! `edited.alerts`, no third "expected" event needed. Scoped out, left for
//! later: an alert `current` holds that `maps_alerts` refuses (which drops
//! the whole property rather than carrying the one entry forward, since
//! there is no key to leave unnamed in a property replaced whole by this
//! path).
//!
//! Also covered: `start` (`diff`'s plain compare against the baseline),
//! restricted to a calendar-valid date and a time of day with seconds in
//! `0..59` (a leap second is legal RFC 5545 and RFC 8984 input, but its own
//! question, so scoped out here) and `timeZone` held constant. `start` is
//! never cleared, since its absence has no PatchObject shape, so the round
//! trip only exercises a change and a no-op, the same shape
//! `duration_round_trip` uses.
//!
//! Also covered: `timeZone` (`diff`'s check against the baseline, gated by
//! `maps_time_zone`), restricted to plain IANA zone names (`names_time_zone`
//! true), so `maps_time_zone` always accepts both sides and `diff_time_zones`
//! never adds a `timeZones` entry for this increment's patch at all — a named
//! zone needs no definition beside it. `zone_of` takes a `names_time_zone`
//! value at its word with no embedded `VTIMEZONE` needed, so the round trip
//! holds independently of tzdata resolution. `timeZone` is never cleared, the
//! same as `start`, so the round trip only exercises a change and a no-op.
//! Scoped out, left for later: a custom (solidus-prefixed) identifier, which
//! needs `diff_time_zones`' own `timeZones` entry to be sendable at all, and a
//! `TZID` that is neither a name nor sendable, which `maps_time_zone` refuses
//! and `diff` leaves alone regardless of what `edited` states.
//!
//! Also covered: `showWithoutTime` (`diff`'s plain compare, writing `null`
//! rather than `false` to clear), using its own base event — a midnight
//! start, a whole-day (`P1D`) duration and no time zone — since the shared
//! timed `base()` this file otherwise uses can never render as all-day
//! regardless of the flag. `shows_without_time` needs `Some(true)` plus
//! those three conditions to write a `VALUE=DATE` `DTSTART`; a `false` or
//! absent flag round-trips through the same component as `None` (the writer
//! has no distinct way to spell "explicitly timed"), so the only two stable
//! states this base can reach are `Some(true)` and `None`, and the generator
//! varies between exactly those.
//!
//! Also covered: `recurrenceRule` (`diff_recurrence`), replaced whole like
//! `alerts`, gated by [`jmap_ical::maps_recurrence_rule`] on both `current`
//! and `edited` (neither side may hold a rule the `RRULE` could only narrow
//! or reject). Restricted to frequency plus an optional interval and count,
//! no `until` and no `BYxxx` part: that shape clears every
//! `maps_recurrence_rule` check trivially, so the patch is a literal round
//! trip with `edited.recurrence_rule`, no third "expected" event needed.
//! `rrule_to_rule` never restores `RecurrenceRule::rule_type`, so both sides
//! are run through `stable()` first, the same as every other property here.
//!
//! Also covered: `recurrenceRule`'s `byMonth`, `byHour`, `byMinute` and
//! `bySecond`, the four `BYxxx` parts [`jmap_ical::by_month_part`],
//! [`jmap_ical::by_hour_part`], [`jmap_ical::by_minute_part`] and
//! [`jmap_ical::by_second_part`] write at every frequency with no
//! interaction between parts, unlike `byDay`'s ordinal or `bySetPos`'s
//! "beside another part" rule. Restricted to values each part always
//! accepts (an hour, minute or second within RFC 5545 §3.3.10's range, a
//! month `"1"`..`"12"` with no leading zero), so the round trip needs no
//! "expected" event, the same as the frequency/interval/count shape above.
//! Scoped out, left for later: `byDay`, `byMonthDay`, `byYearDay`,
//! `byWeekNo` and `bySetPos` (each frequency-gated or interacting with
//! another part), `until` (a zone-dependent conversion) and
//! `firstDayOfWeek` (Monday is omitted rather than written).
//!
//! Also covered: `recurrenceOverrides` (`diff_overrides`), restricted to the
//! bare `{"excluded": true}` shape (an `EXDATE`). `override_maps_by` accepts
//! that shape unconditionally regardless of `recurrence_rule` — an `EXDATE`
//! or `RDATE` line is written whether or not the event carries an `RRULE` —
//! so the generator needs no recurrence rule at all, only a set of
//! `LocalDateTime` ids (`arb_start`'s format). The whole property replaces
//! like `keywords`/`alerts`, so the patch is a literal round trip with
//! `edited.recurrence_overrides`, no third "expected" event needed. Scoped
//! out, left for later: the restated-field shape (`OVERRIDE_PROPERTIES`:
//! `title`, `start`, `timeZone`, `duration`, `status`, `freeBusyStatus`,
//! `priority`, `privacy`, `keywords`, `alerts` on a `RECURRENCE-ID`
//! instance), which needs its own per-field generator, and an id a bare
//! `EXDATE` cannot spell. `timeZones` itself is not diffed at all (the
//! module doc: "added to", never compared or replaced), so it has no
//! round-trip property to state beyond what the `timeZone` increment above
//! already confirms stays empty for a named zone.
//!
//! Also covered: the restated-field override shape, scoped to the plain
//! scalar members of `OVERRIDE_PROPERTIES` (`title`, `description`,
//! `duration`, `status`, `freeBusyStatus`, `privacy`, `priority`), each
//! independently present or absent on each instance. `instance_patch`
//! restates a field exactly when it differs from the series, and `base()`
//! leaves all seven at the default (`duration` is the one exception, so its
//! generator is drawn away from `base()`'s own `PT1H`), so a present field
//! always differs and is always restated back. `modified_instance` and
//! `vevent_of`/`read_vevent` draw and read an instance's restated fields with
//! the same code the series' own scalar properties use, already proven
//! round-trip-safe by the increments above, so the per-field values reuse
//! those same domains. Scoped out, left for later: `start`/`timeZone` (the
//! id's own meaning).
//!
//! Also covered: `keywords` and `alerts` as restated override fields, the two
//! whole-set members of `OVERRIDE_PROPERTIES`. `maps_override_field` gates
//! each the same way its series-level counterpart does (one `maps_keyword`
//! draws per tag, one `drawn_alert` draws per entry, and never empty: an
//! empty object is written the same way as a removal), and `base()` carries
//! neither, so a present value always differs from the series and
//! `instance_patch` always restates it back, needing no "expected" event.
//! Scoped out, left for later: an override holding a tag or alert the gate
//! refuses, which the series-level increments already found needs the
//! three-event "expected" shape.
//!
//! `title`/`description` are restricted to plain alphanumeric text with no
//! leading, trailing or interior punctuation: `SUMMARY`/`SSCRIPTION` escaping
//! and line folding are a separate question from the baseline-compare shape
//! this increment covers.

use std::collections::BTreeMap;

use jmap_cal_sync::patch::diff;
use jmap_ical::{event_to_ical, ical_to_event};
use jmap_proto::calendars::{CalendarEvent, RecurrenceRule};
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

/// A midnight start, a whole-day duration and no time zone: the three
/// conditions `shows_without_time` needs besides the flag itself, so this
/// base can actually render all-day, unlike the shared timed `base()` above.
fn all_day_base() -> CalendarEvent {
    CalendarEvent::new()
        .with_uid("seed-uid")
        .with_start("2026-01-15T00:00:00")
        .with_duration("P1D")
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

/// A LocalDateTime `to_local_date_time`/`to_ical_date_time` always draws back
/// byte-identical: a calendar-valid date and a time of day with seconds kept
/// to `0..59` (a leap second is legal input but its own question).
fn arb_start() -> impl Strategy<Value = String> {
    (
        2000u32..2100,
        1u32..=12,
        1u32..=28,
        0u32..24,
        0u32..60,
        0u32..60,
    )
        .prop_map(|(year, month, day, hour, minute, second)| {
            format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}")
        })
}

/// A plain IANA zone name `names_time_zone` always accepts and that
/// `event_to_ical`/`ical_to_event` round-trips byte-identical, with no
/// embedded `VTIMEZONE` needed (`zone_of` takes such a value at its word).
fn arb_time_zone() -> impl Strategy<Value = &'static str> {
    prop::sample::select(
        &[
            "Europe/Berlin",
            "America/New_York",
            "Asia/Tokyo",
            "Etc/UTC",
            "Pacific/Auckland",
            "America/Los_Angeles",
        ][..],
    )
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

/// A bare Location entry: just the name `read_locations` itself ever
/// produces, nothing a clear would need to reach past to delete.
fn location_value(name: &str) -> Value {
    json!({"@type": "Location", "name": name})
}

/// An offset `drawn_trigger` always draws back unchanged, relative to the
/// start (no `relativeTo` member).
fn arb_offset() -> impl Strategy<Value = String> {
    prop::sample::select(&["-PT15M", "-PT1H", "-P1D", "PT0S", "PT15M"][..]).prop_map(str::to_owned)
}

/// The exact Alert shape `read_alert` produces for a start-relative offset
/// trigger: an explicit `@type`, `action` and trigger `@type`, nothing a
/// `relativeTo: "end"` or any other member would add.
fn alert_value(offset: &str) -> Value {
    alert_value_related(offset, None)
}

/// Like [`alert_value`], with an explicit `relativeTo` on the trigger: `None`
/// for the start (the default, so no member at all), `Some("end")` for the
/// one other value `drawn_trigger`/`read_alert` agree on.
fn alert_value_related(offset: &str, related: Option<&str>) -> Value {
    let mut value = json!({
        "@type": "Alert",
        "trigger": {"@type": "OffsetTrigger", "offset": offset},
        "action": "display",
    });
    if let Some(related) = related {
        value["trigger"]["relativeTo"] = json!(related);
    }
    value
}

/// `None` (relative to the start) or `Some("end")`, the only two values
/// `drawn_trigger` reads a `relativeTo` member back as.
fn arb_related() -> impl Strategy<Value = Option<&'static str>> {
    prop::option::of(Just("end"))
}

/// An `alerts` map of 0 to 4 keyed reminders, every one an `arb_offset` and
/// `arb_related` pair wrapped in `alert_value_related`.
fn arb_alerts() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_map(arb_key(), (arb_offset(), arb_related()), 0..4).prop_map(
        |entries| {
            entries
                .into_iter()
                .map(|(k, (o, r))| (k, alert_value_related(&o, r)))
                .collect()
        },
    )
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

/// A frequency `writable` always accepts (a non-empty string), the plain
/// iCalendar `FREQ` tokens `rule_to_rrule`/`rrule_to_rule` draw back
/// unchanged (lower case, the form `rrule_to_rule` always produces).
fn arb_frequency() -> impl Strategy<Value = &'static str> {
    prop::sample::select(&["daily", "weekly", "monthly", "yearly"][..])
}

/// A `RecurrenceRule` restricted to frequency plus an optional interval,
/// count, `byMonth`, `byHour`, `byMinute` and `bySecond`: no `until` and none
/// of the frequency-gated or part-interacting `BYxxx` parts, a shape
/// [`maps_recurrence_rule`] always accepts, since every other gate is vacuous
/// when its part is absent, `writable` needs only the non-empty frequency
/// this always has, and each of the four `BYxxx` parts here is drawn only
/// from values its own `by_*_part` function always accepts (non-empty, every
/// value within range).
///
/// [`maps_recurrence_rule`]: jmap_ical::maps_recurrence_rule
fn arb_recurrence_rule() -> impl Strategy<Value = Option<RecurrenceRule>> {
    prop::option::of(
        (
            arb_frequency(),
            prop::option::of(1u32..5),
            prop::option::of(1u32..20),
            prop::option::of(prop::collection::vec(1i32..=12, 1..4)),
            prop::option::of(prop::collection::vec(0u32..=23, 1..4)),
            prop::option::of(prop::collection::vec(0u32..=59, 1..4)),
            prop::option::of(prop::collection::vec(0u32..=60, 1..4)),
        )
            .prop_map(
                |(frequency, interval, count, by_month, by_hour, by_minute, by_second)| {
                    let mut rule = RecurrenceRule::new(frequency);
                    if let Some(interval) = interval {
                        rule = rule.with_interval(interval);
                    }
                    if let Some(count) = count {
                        rule = rule.with_count(count);
                    }
                    rule.by_month =
                        by_month.map(|months| months.into_iter().map(|m| m.to_string()).collect());
                    rule.by_hour = by_hour;
                    rule.by_minute = by_minute;
                    rule.by_second = by_second;
                    rule
                },
            ),
    )
}

/// A `recurrenceOverrides` map restricted to the bare `{"excluded": true}`
/// shape: `override_maps_by` accepts an `EXDATE` id unconditionally, so the
/// only constraint on a key is that `arb_start`'s format is one
/// `to_ical_date_time` reads.
fn arb_exclusions() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_set(arb_start(), 0..5).prop_map(|ids| {
        ids.into_iter()
            .map(|id| (id, json!({"excluded": true})))
            .collect()
    })
}

/// A non-empty `keywords` restated field: one or more `arb_tag`s, the shape
/// `maps_override_field` accepts (every tag `maps_keyword` draws, never
/// empty: an empty object is written the same way as a removal).
fn arb_restated_keywords() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_set(arb_tag(), 1..4).prop_map(|tags| {
        tags.into_iter()
            .map(|tag| (tag, Value::Bool(true)))
            .collect()
    })
}

/// A non-empty `alerts` restated field: one or more keyed `arb_offset`
/// reminders, the shape `maps_override_field` accepts (every entry
/// `drawn_alert` draws, never empty, same reason as `arb_restated_keywords`).
fn arb_restated_alerts() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_map(arb_key(), arb_offset(), 1..4).prop_map(|entries| {
        entries
            .into_iter()
            .map(|(k, o)| (k, alert_value(&o)))
            .collect()
    })
}

/// One instance's restated fields: every plain scalar `OVERRIDE_PROPERTIES`
/// member plus `keywords` and `alerts`, independently present or absent.
/// `duration` is drawn from 2..12 hours rather than `duration_round_trip`'s
/// 1..12, so a present value always differs from `base()`'s own `PT1H` and
/// `instance_patch` always restates it; every other field differs from
/// `base()`'s `None` regardless of value. At least one field is present,
/// since an empty patch draws no instance at all (`modified_instance`
/// returns `None`).
#[allow(clippy::type_complexity)]
fn arb_restated_override() -> impl Strategy<Value = Map<String, Value>> {
    (
        prop::option::of(arb_text()),
        prop::option::of(arb_text()),
        prop::option::of(2u32..12),
        prop::option::of(prop::sample::select(
            &["confirmed", "cancelled", "tentative"][..],
        )),
        prop::option::of(prop::sample::select(&["free", "busy"][..])),
        prop::option::of(prop::sample::select(&["public", "private", "secret"][..])),
        prop::option::of(0i64..=9),
        prop::option::of(arb_restated_keywords()),
        prop::option::of(arb_restated_alerts()),
    )
        .prop_filter(
            "at least one field restated",
            |(
                title,
                description,
                hours,
                status,
                free_busy_status,
                privacy,
                priority,
                keywords,
                alerts,
            )| {
                title.is_some()
                    || description.is_some()
                    || hours.is_some()
                    || status.is_some()
                    || free_busy_status.is_some()
                    || privacy.is_some()
                    || priority.is_some()
                    || keywords.is_some()
                    || alerts.is_some()
            },
        )
        .prop_map(
            |(
                title,
                description,
                hours,
                status,
                free_busy_status,
                privacy,
                priority,
                keywords,
                alerts,
            )| {
                let mut patch = Map::new();
                if let Some(title) = title {
                    patch.insert("title".to_owned(), Value::String(title));
                }
                if let Some(description) = description {
                    patch.insert("description".to_owned(), Value::String(description));
                }
                if let Some(hours) = hours {
                    patch.insert("duration".to_owned(), Value::String(format!("PT{hours}H")));
                }
                if let Some(status) = status {
                    patch.insert("status".to_owned(), Value::String(status.to_owned()));
                }
                if let Some(free_busy_status) = free_busy_status {
                    patch.insert(
                        "freeBusyStatus".to_owned(),
                        Value::String(free_busy_status.to_owned()),
                    );
                }
                if let Some(privacy) = privacy {
                    patch.insert("privacy".to_owned(), Value::String(privacy.to_owned()));
                }
                if let Some(priority) = priority {
                    patch.insert("priority".to_owned(), Value::from(priority));
                }
                if let Some(keywords) = keywords {
                    // Serialising a set this generator built cannot fail: it
                    // holds strings and `true`.
                    patch.insert(
                        "keywords".to_owned(),
                        serde_json::to_value(keywords).unwrap_or(Value::Null),
                    );
                }
                if let Some(alerts) = alerts {
                    // Serialising a map this generator built cannot fail: it
                    // holds two objects of strings.
                    patch.insert(
                        "alerts".to_owned(),
                        serde_json::to_value(alerts).unwrap_or(Value::Null),
                    );
                }
                patch
            },
        )
}

/// A `recurrenceOverrides` map of 0 to 3 instances, each an
/// `arb_restated_override` keyed by an `arb_start` id.
fn arb_restated_overrides() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_map(arb_start(), arb_restated_override(), 0..3).prop_map(|entries| {
        entries
            .into_iter()
            .map(|(id, patch)| (id, Value::Object(patch)))
            .collect()
    })
}

/// An RFC 5545 §3.3.14 UTC-OFFSET `utc_offset` draws back byte-identical:
/// already its own short `±hhmm` form, and never the negative zero the
/// grammar forbids.
fn arb_utc_offset() -> impl Strategy<Value = String> {
    (proptest::bool::ANY, 0u32..24, 0u32..60).prop_filter_map(
        "not -0000",
        |(negative, hour, minute)| {
            if negative && hour == 0 && minute == 0 {
                return None;
            }
            Some(format!(
                "{}{hour:02}{minute:02}",
                if negative { "-" } else { "+" }
            ))
        },
    )
}

/// A custom `TimeZoneId` (RFC 8984 §1.4.9's solidus-prefixed second form)
/// paired with the one-observance `TimeZone` definition that names it.
///
/// `vtimezone_of` draws the single `standard` rule whole (RFC 5545 §3.6.5's
/// minimum, one subcomponent), and with no `recurrenceRules` or `names`
/// member, `read_time_zones` reads it back as exactly this shape, `tzId`
/// included: the round trip the test below depends on.
fn arb_custom_zone() -> impl Strategy<Value = (String, Value)> {
    (
        "[a-zA-Z0-9]{1,10}",
        arb_start(),
        arb_utc_offset(),
        arb_utc_offset(),
    )
        .prop_map(|(suffix, start, offset_from, offset_to)| {
            let tzid = format!("/example.com/{suffix}");
            let definition = json!({
                "@type": "TimeZone",
                "tzId": tzid,
                "standard": [{
                    "@type": "TimeZoneRule",
                    "start": start,
                    "offsetFrom": offset_from,
                    "offsetTo": offset_to,
                }],
            });
            (tzid, definition)
        })
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
    fn start_round_trip(old in arb_start(), new in arb_start()) {
        let current = stable(base().with_start(old));
        let edited = stable(base().with_start(new));
        round_trips(&current, &edited)?;
    }

    #[test]
    fn time_zone_round_trip(old in arb_time_zone(), new in arb_time_zone()) {
        let current = stable(base().with_time_zone(old));
        let edited = stable(base().with_time_zone(new));
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
    fn locations_round_trip(
        key in arb_key(),
        old_name in arb_place_name(),
        new_name in arb_place_name(),
    ) {
        let mut old_event = base();
        old_event.locations = old_name
            .as_deref()
            .map(|name| [(key.clone(), location_value(name))].into());
        let mut new_event = base();
        new_event.locations = new_name
            .as_deref()
            .map(|name| [(key.clone(), location_value(name))].into());
        let current = stable(old_event);
        let edited = stable(new_event);
        round_trips(&current, &edited)?;
    }

    #[test]
    fn alerts_round_trip(old in arb_alerts(), new in arb_alerts()) {
        let mut old_event = base();
        old_event.alerts = (!old.is_empty()).then_some(old);
        let mut new_event = base();
        new_event.alerts = (!new.is_empty()).then_some(new);
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
    fn show_without_time_round_trip(old in proptest::bool::ANY, new in proptest::bool::ANY) {
        let mut old_event = all_day_base();
        old_event.show_without_time = old.then_some(true);
        let mut new_event = all_day_base();
        new_event.show_without_time = new.then_some(true);
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

    #[test]
    fn recurrence_rule_round_trip(old in arb_recurrence_rule(), new in arb_recurrence_rule()) {
        let mut old_event = base();
        old_event.recurrence_rule = old;
        let mut new_event = base();
        new_event.recurrence_rule = new;
        let current = stable(old_event);
        let edited = stable(new_event);
        round_trips(&current, &edited)?;
    }

    #[test]
    fn recurrence_overrides_excluded_round_trip(old in arb_exclusions(), new in arb_exclusions()) {
        let mut old_event = base();
        old_event.recurrence_overrides = (!old.is_empty()).then_some(old);
        let mut new_event = base();
        new_event.recurrence_overrides = (!new.is_empty()).then_some(new);
        let current = stable(old_event);
        let edited = stable(new_event);
        round_trips(&current, &edited)?;
    }

    #[test]
    fn recurrence_overrides_restated_round_trip(
        old in arb_restated_overrides(),
        new in arb_restated_overrides(),
    ) {
        let mut old_event = base();
        old_event.recurrence_overrides = (!old.is_empty()).then_some(old);
        let mut new_event = base();
        new_event.recurrence_overrides = (!new.is_empty()).then_some(new);
        let current = stable(old_event);
        let edited = stable(new_event);
        round_trips(&current, &edited)?;
    }

    #[test]
    fn custom_time_zone_definition_is_added_when_moved_into(
        (tzid, definition) in arb_custom_zone(),
    ) {
        let current = stable(base());
        let mut new_event = base();
        new_event.time_zone = Some(tzid.clone());
        new_event.time_zones = Some([(tzid, definition)].into());
        let edited = stable(new_event);
        round_trips(&current, &edited)?;
    }
}
