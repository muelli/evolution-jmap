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
//! Also covered: `links` (`diff_links`). `links_round_trip` covers plain
//! `ATTACH`-shaped entries (an `https:` `href` and an optional `contentType`);
//! `links_image_shape_round_trip` covers the `rel: "icon"` `IMAGE` shape (an
//! optional `display`); both vary only `href` per key, holding
//! `contentType`/`display` constant, since `diff_links` never patches either.
//! `links_size_change_left_alone` varies an `ATTACH` entry's `size`
//! independently of `href` and checks the unpatched property directly, since
//! `size` is never patched either. `links_key_add_and_drop_left_alone` covers
//! a key one side adds or drops, which `diff_links` leaves alone by design,
//! checked the same way.
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
//!
//! Also covered: `locations` with a member besides `name`, held constant
//! (a `description`, chosen only because `maps_locations` never looks at it)
//! while `name` varies as before. The entry no longer holds nothing but the
//! name, so a clear takes `diff_locations`'s other branch: only
//! `locations/<key>/name` is removed, and `description` rides through the
//! patch untouched on both sides, exactly the branch the increment above
//! scoped out.
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
//! Also covered: a custom (solidus-prefixed) identifier moved into, in
//! `custom_time_zone_definition_is_added_when_moved_into` (`diff_time_zones`
//! adding a `timeZones` entry to make it sendable at all), and a `TZID` that
//! is neither a name nor sendable, in
//! `time_zone_left_alone_when_edited_holds_one_maps_time_zone_refuses`:
//! `maps_time_zone` refuses it and `diff` leaves `timeZone` alone regardless
//! of what `edited` states.
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
//!
//! Also covered: `recurrenceRule`'s `firstDayOfWeek`
//! ([`jmap_ical::first_day_of_week_part`]), which likewise has no frequency
//! gate and interacts with no other part. `Some("mo")` collapses to `None`
//! after `stable()`, the same shape `timeZone`'s named-zone increment above
//! already handles, since `WKST=MO` is RFC 5545's default and is never
//! written; the other six weekdays round-trip literally. Scoped out, left
//! for later: `byDay`, `byMonthDay`, `byYearDay`, `byWeekNo` and `bySetPos`
//! (each frequency-gated or interacting with another part) and `until` (a
//! zone-dependent conversion).
//!
//! Also covered: `byDay`, `byMonthDay`, `byYearDay`, `byWeekNo` and
//! `bySetPos`, the remaining `BYxxx` parts from the paragraph above.
//! `by_day_part`/`by_month_day_part`/`by_year_day_part`/`by_week_no_part`
//! each gate their part on the rule's own frequency (an ordinal in `byDay`
//! needs `MONTHLY`/`YEARLY`; `byMonthDay` is refused under `WEEKLY`;
//! `byYearDay` needs a frequency that holds a year; `byWeekNo` needs
//! `YEARLY` alone), so the generator draws each part's values only from the
//! frequencies its own gate accepts. `bySetPos` has no frequency gate of its
//! own, only `by_set_position_part`'s "beside another `BYxxx` part that was
//! actually written" rule, so the generator always carries a `byDay` beside
//! it: a weekday is valid at every frequency drawn here, so it is always
//! one of the parts `rule_to_rrule` writes, and `bySetPos` is therefore
//! always writable when present. `until` stays scoped out here, covered
//! below.
//!
//! Also covered: `recurrenceRule`'s `until`, a rule restricted to frequency
//! plus `until` alone, round tripped under both a UTC and a non-UTC named
//! zone (`zoned_base`). `rule_to_rrule` writes a `Z` suffix only for `Ends::
//! In` a zone `is_utc` names, and `read_until` only reverses that suffix for
//! the same zone identity check (no `VTIMEZONE` observance is involved for a
//! plain named zone, matching the `timeZone` increment above), so the two
//! zones exercise both of `until`'s write/read branches. `until` is drawn
//! from `arb_start`'s format, which `writable` always accepts.
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
//! `title`/`description`'s plain-text round trip above is restricted to
//! alphanumeric text with no punctuation at all. `title_round_trip_with_
//! escapes_and_folding`/`description_round_trip_with_escapes_and_folding`
//! cover the rest: text mixing a comma, a semicolon, a backslash and an
//! embedded newline (the four characters RFC 5545 §3.3.11 TEXT escaping
//! must round trip) at a length that forces
//! [`jmap_ical::event::fold_overlong_lines`] to fold the `SUMMARY`/
//! `DESCRIPTION` line at least once.
//!
//! Also covered: `links`' key add/drop. `diff_links` only ever patches an
//! existing key's `href`, by design (a resource the server does not hold is
//! not created, and one the component stopped naming is left where it is), so
//! a key `edited` adds that `current` lacks must never appear in the patch,
//! and a key `edited` drops must survive applying the patch unchanged. This
//! needs a check against the unpatched property rather than a round trip to
//! `edited`, unlike `links_round_trip` above.
//!
//! Also covered: `virtualLocations`' key add/drop, the same shape as
//! `links`' above and for the same reason: `diff_virtual_locations` only
//! ever patches an existing key's members, never creating one `edited` adds
//! or removing one it drops. `name`/`features` are held absent throughout,
//! since this increment is about the key set, not the member-level patch
//! `virtual_locations_round_trip` already covers.
//!
//! Also covered: a `virtualLocations` entry whose `uri` stops being one
//! `names_a_uri` accepts. `drawn_conference` refuses to write a `CONFERENCE`
//! line for such a value, so the entry never comes back from the
//! `event_to_ical`/`ical_to_event` round trip at all — a different code path
//! from the key-drop case above, which never puts the entry in the input
//! event to begin with. The end state `diff_virtual_locations` reaches is the
//! same (the key is left entirely alone), but this increment exercises the
//! write-side filter that gets it there.
//!
//! Also covered: the `keywords` and `alerts` refusal cases the two paragraphs
//! above scope out. A `keywords` tag `current` holds with a non-bool value
//! (`maps_keyword` refuses it regardless of the tag name) is carried forward
//! by `diff_keywords` beside whatever `edited` states for the tags that do
//! map, so this needs the three-event "expected" shape the book-sync
//! anniversaries/media/online_services increments used. An `alerts` entry
//! `current` holds with an `AbsoluteTrigger` (`drawn_trigger` maps neither
//! direction) makes `maps_alerts(current)` false, so `diff_alerts` drops the
//! whole property instead of carrying the one entry forward: there is no key
//! to leave unnamed in a property replaced whole, so the check is that the
//! patch states no `alerts` member at all and the applied event's `alerts`
//! is `current`'s own, unchanged.
//!
//! Also covered: the restated-field override's `start` and `timeZone`
//! members, the two `OVERRIDE_PROPERTIES` the increment above scoped out
//! because neither compares against a `None` baseline the way the other
//! seven do. `start` restates only when it differs from the override's own
//! id (RFC 8984 §4.3.4: the key *is* the instance's start unless the patch
//! says otherwise), so the generator draws it away from the id rather than
//! from `None`. `timeZone` restates against the *series'* own zone rather
//! than absence, since `base()` always carries one, so the generator draws
//! it away from `base()`'s own `"Europe/Berlin"`, the same move `duration`
//! needed above. Each is independently present or absent, at least one
//! present.
//!
//! Also covered: a `recurrenceOverrides` id no `EXDATE` can spell (no `T`
//! separator at all, so `to_ical_date_time` refuses it before any digit
//! check). `override_maps_by` checks the id before the patch, so
//! `sends_recurrence_override` refuses such an entry regardless of what it
//! says, and `diff_overrides` leaves the whole property alone rather than
//! carrying the one entry forward: the same "no key to leave unnamed in a
//! property replaced whole" shape the `alerts` refusal case above needed, so
//! this checks the patch states no `recurrenceOverrides` member at all and
//! the applied event's overrides equal `current`'s own, unchanged.

use std::collections::BTreeMap;

use jmap_cal_sync::patch::diff;
use jmap_ical::{event_to_ical, ical_to_event};
use jmap_proto::calendars::{CalendarEvent, NDay, RecurrenceRule};
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

/// `base()` with its own zone replaced: the zone `until` states against
/// (RFC 5545 §3.3.10, [`jmap_ical::event::Ends::In`]).
fn zoned_base(zone: &str) -> CalendarEvent {
    base().with_time_zone(zone)
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

/// Text that exercises RFC 5545 §3.3.11 TEXT escaping (a comma, a semicolon,
/// a backslash and an embedded newline, any of which the writer must escape
/// and the reader must reverse) at a length that forces
/// [`jmap_ical::event::fold_overlong_lines`] to fold at least one physical
/// line once the `SUMMARY:`/`DESCRIPTION:` name is prepended.
fn arb_text_with_escapes_and_folding() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            "[a-zA-Z0-9 ]",
            Just(",".to_owned()),
            Just(";".to_owned()),
            Just("\\".to_owned()),
            Just("\n".to_owned()),
        ],
        100..200,
    )
    .prop_map(|chars| chars.concat())
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
/// `contentType`.
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

/// A `display` `spelled` always draws back unchanged, or none at all: one of
/// the four names `LINK_DISPLAYS` shares between RFC 8984 §1.4.11 and RFC
/// 7986 §6.1, or absent (the `DISPLAY` parameter is never required).
fn arb_display() -> impl Strategy<Value = Option<String>> {
    prop::option::of(prop::sample::select(
        &["badge", "graphic", "fullsize", "thumbnail"][..],
    ))
    .prop_map(|display| display.map(str::to_owned))
}

/// An `IMAGE`-shaped Link entry: `rel: "icon"` is what sends `drawn_link` to
/// RFC 7986 §5.10's `IMAGE` rather than `ATTACH`, with an optional `display`
/// and `contentType`, never the `size` member `read_links` only ever puts on
/// an `ATTACH`.
fn image_link_value(href: &str, display: &Option<String>, content_type: &Option<String>) -> Value {
    let mut link = json!({"@type": "Link", "href": href, "rel": "icon"});
    if let Some(display) = display {
        link["display"] = Value::String(display.clone());
    }
    if let Some(content_type) = content_type {
        link["contentType"] = Value::String(content_type.clone());
    }
    link
}

/// A set of 1 to 4 keys, each carrying an old `href`, a new one, and a
/// `display`/`contentType` pair held constant across both, for the same
/// reason `arb_links` holds `contentType` constant: `diff_links` patches
/// `href` alone.
fn arb_image_links()
-> impl Strategy<Value = BTreeMap<String, (String, String, Option<String>, Option<String>)>> {
    prop::collection::btree_map(
        arb_key(),
        (arb_href(), arb_href(), arb_display(), arb_content_type()),
        1..4,
    )
}

/// A `size` `stated_size` always draws back unchanged: an `UnsignedInt` RFC
/// 8607 §4.1's `1*DIGIT` can state.
fn arb_size() -> impl Strategy<Value = Option<u64>> {
    prop::option::of(0u64..1_000_000)
}

/// A plain `ATTACH`-shaped Link entry with a `size`, never a `contentType`:
/// this is only ever used to isolate `size`'s own effect on the diff.
fn attach_link_value(href: &str, size: Option<u64>) -> Value {
    let mut link = json!({"@type": "Link", "href": href});
    if let Some(size) = size {
        link["size"] = Value::from(size);
    }
    link
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

/// A Location entry with a `description` beside the name, the one member
/// `maps_locations` never inspects: present, a clear has something left to
/// keep, so `diff_locations` removes only the name rather than the whole
/// property.
fn location_value_with_description(name: &Option<String>, description: &str) -> Value {
    let mut place = json!({"@type": "Location", "description": description});
    if let Some(name) = name {
        place["name"] = Value::String(name.clone());
    }
    place
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

/// An Alert shape `drawn_trigger` never maps either direction: an
/// `AbsoluteTrigger` (RFC 8984 §4.5.4), which only `OffsetTrigger` does.
fn unmappable_alert_value() -> Value {
    json!({
        "@type": "Alert",
        "trigger": {"@type": "AbsoluteTrigger", "when": "2026-01-15T12:00:00Z"},
        "action": "display",
    })
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

/// A weekday `weekday_token` always accepts (RFC 8984 §4.3.3's lowercase
/// spelling), the domain `first_day_of_week_part` reads a rule's
/// `firstDayOfWeek` from.
fn arb_weekday() -> impl Strategy<Value = &'static str> {
    prop::sample::select(&["mo", "tu", "we", "th", "fr", "sa", "su"][..])
}

/// A `RecurrenceRule` restricted to frequency plus an optional interval,
/// count, `byMonth`, `byHour`, `byMinute`, `bySecond` and `firstDayOfWeek`:
/// no `until` and none of the frequency-gated or part-interacting `BYxxx`
/// parts, a shape [`maps_recurrence_rule`] always accepts, since every other
/// gate is vacuous when its part is absent, `writable` needs only the
/// non-empty frequency this always has, and each of the five parts here is
/// drawn only from values its own `by_*_part`/`first_day_of_week_part`
/// function always accepts (non-empty, every value within range).
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
            prop::option::of(arb_weekday()),
        )
            .prop_map(
                |(
                    frequency,
                    interval,
                    count,
                    by_month,
                    by_hour,
                    by_minute,
                    by_second,
                    first_day_of_week,
                )| {
                    let mut rule = RecurrenceRule::new(frequency);
                    if let Some(interval) = interval {
                        rule = rule.with_interval(interval);
                    }
                    if let Some(count) = count {
                        rule = rule.with_count(count);
                    }
                    rule.first_day_of_week = first_day_of_week.map(str::to_owned);
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

/// A `RecurrenceRule` covering the five frequency-gated, interacting `BYxxx`
/// parts `arb_recurrence_rule` leaves out: `byDay`, `byMonthDay`,
/// `byYearDay`, `byWeekNo` and `bySetPos`. Each part is drawn only from the
/// frequency this rule actually carries, matching the gate
/// `by_day_part`/`by_month_day_part`/`by_year_day_part`/`by_week_no_part`
/// enforces, so every value here clears `maps_recurrence_rule`. `byDay` is
/// always present (a plain weekday, valid at every frequency drawn), so
/// `bySetPos`, whenever drawn, always has another written `BYxxx` part to
/// select from: see [`jmap_ical::event::by_set_position_part`]'s doc.
///
/// `nthOfPeriod` is restricted to a small range (`-5..=-1`, `1..=5`): a
/// realistic "nth weekday of the month/year", not every value
/// [`jmap_ical::event::by_day_token`] happens to accept unchecked.
fn arb_recurrence_rule_by_parts() -> impl Strategy<Value = RecurrenceRule> {
    arb_frequency().prop_flat_map(|frequency| {
        let counts_within_a_period = matches!(frequency, "monthly" | "yearly");
        let nth = if counts_within_a_period {
            prop::option::of(prop_oneof![-5i32..0, 1i32..6]).boxed()
        } else {
            Just(None).boxed()
        };
        let by_day = prop::collection::vec((arb_weekday(), nth), 1..4).prop_map(|days| {
            days.into_iter()
                .map(|(day, nth)| match nth {
                    Some(nth) => NDay::new(day).with_nth_of_period(nth),
                    None => NDay::new(day),
                })
                .collect::<Vec<_>>()
        });

        let by_month_day = if frequency == "weekly" {
            Just(None).boxed()
        } else {
            prop::option::of(prop::collection::vec(
                prop_oneof![-31i32..0, 1i32..32],
                1..4,
            ))
            .boxed()
        };

        let holds_a_year = !matches!(frequency, "daily" | "weekly" | "monthly");
        let by_year_day = if holds_a_year {
            prop::option::of(prop::collection::vec(
                prop_oneof![-366i32..0, 1i32..367],
                1..4,
            ))
            .boxed()
        } else {
            Just(None).boxed()
        };

        let by_week_no = if frequency == "yearly" {
            prop::option::of(prop::collection::vec(
                prop_oneof![-53i32..0, 1i32..54],
                1..4,
            ))
            .boxed()
        } else {
            Just(None).boxed()
        };

        let by_set_position = prop::option::of(prop::collection::vec(
            prop_oneof![-366i32..0, 1i32..367],
            1..4,
        ));

        (
            by_day,
            by_month_day,
            by_year_day,
            by_week_no,
            by_set_position,
        )
            .prop_map(
                move |(by_day, by_month_day, by_year_day, by_week_no, by_set_position)| {
                    let mut rule = RecurrenceRule::new(frequency);
                    rule.by_day = Some(by_day);
                    rule.by_month_day = by_month_day;
                    rule.by_year_day = by_year_day;
                    rule.by_week_no = by_week_no;
                    rule.by_set_position = by_set_position;
                    rule
                },
            )
    })
}

/// A `RecurrenceRule` restricted to frequency plus `until`, no other part: a
/// shape [`maps_recurrence_rule`] always accepts (`writable`, since `until`
/// is `arb_start`'s format and always has a `to_ical_date_time` reading; no
/// `BYxxx`, `extra`, `rscale` or `skip`). Isolates `until`'s own
/// zone-dependent conversion from the frequency/interval/count shape
/// `arb_recurrence_rule` already covers.
///
/// [`maps_recurrence_rule`]: jmap_ical::maps_recurrence_rule
fn arb_recurrence_rule_with_until() -> impl Strategy<Value = RecurrenceRule> {
    (arb_frequency(), arb_start())
        .prop_map(|(frequency, until)| RecurrenceRule::new(frequency).with_until(until))
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

/// One instance's `start` and `timeZone` restated fields, independently
/// present or absent. `start` is filtered away from the id, since
/// `instance_patch` only restates it when it differs from the key (the key
/// *is* the instance's start otherwise); `timeZone` is filtered away from
/// `base()`'s own zone, since unlike every other restated field its baseline
/// is not `None`, the same reason `arb_restated_override`'s `duration` is
/// drawn away from `base()`'s own `PT1H`. A rejected draw (both filtered away
/// and neither present) is resampled rather than kept, so every entry this
/// produces restates at least one of the two.
fn arb_restated_start_and_time_zone_entry() -> impl Strategy<Value = (String, Value)> {
    (
        arb_start(),
        prop::option::of(arb_start()),
        prop::option::of(arb_time_zone()),
    )
        .prop_filter_map(
            "start differs from id, timeZone differs from base(), at least one present",
            |(id, start, zone)| {
                let start = start.filter(|start| start != &id);
                let zone = zone.filter(|zone| *zone != "Europe/Berlin");
                if start.is_none() && zone.is_none() {
                    return None;
                }
                let mut patch = Map::new();
                if let Some(start) = start {
                    patch.insert("start".to_owned(), Value::String(start));
                }
                if let Some(zone) = zone {
                    patch.insert("timeZone".to_owned(), Value::String(zone.to_owned()));
                }
                Some((id, Value::Object(patch)))
            },
        )
}

/// A `recurrenceOverrides` map of 0 to 3 instances, each an
/// `arb_restated_start_and_time_zone_entry`.
fn arb_restated_start_and_time_zone_overrides() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::vec(arb_restated_start_and_time_zone_entry(), 0..3)
        .prop_map(|entries| entries.into_iter().collect())
}

/// A `recurrenceOverrides` id no `EXDATE` can spell: no `T` separator at all,
/// which `to_ical_date_time` refuses before any digit check even runs.
fn unspellable_override_id() -> String {
    "2026-01-15".to_owned()
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

/// A custom `TimeZoneId` (RFC 8984 §1.4.8's solidus-prefixed second form)
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

/// A `TZID` `maps_time_zone` refuses: it neither names a zone
/// (`names_time_zone`) nor begins with a solidus, so it describes nothing a
/// server could resolve (a Windows zone name arriving unconverted is the one
/// that shows up in practice).
fn unmappable_time_zone() -> &'static str {
    "W. Europe Standard Time"
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
    fn title_round_trip_with_escapes_and_folding(
        old in arb_text_with_escapes_and_folding(),
        new in arb_text_with_escapes_and_folding(),
    ) {
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
    fn time_zone_left_alone_when_edited_holds_one_maps_time_zone_refuses(old in arb_time_zone()) {
        let current = stable(base().with_time_zone(old));
        let edited = base().with_time_zone(unmappable_time_zone());

        let patch = diff(&current, &edited);
        prop_assert!(!patch.contains_key("timeZone"), "patch was: {:?}", patch);

        let mut value = serde_json::to_value(&current).expect("CalendarEvent always serializes");
        apply_patch(&mut value, &patch);
        let applied: CalendarEvent =
            serde_json::from_value(value).expect("a patched CalendarEvent always parses");
        prop_assert_eq!(applied.time_zone, current.time_zone);
    }

    #[test]
    fn description_round_trip(old in arb_text(), new in arb_text()) {
        let current = stable(base().with_description(old));
        let edited = stable(base().with_description(new));
        round_trips(&current, &edited)?;
    }

    #[test]
    fn description_round_trip_with_escapes_and_folding(
        old in arb_text_with_escapes_and_folding(),
        new in arb_text_with_escapes_and_folding(),
    ) {
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
    fn keywords_carry_forward_tag_maps_keyword_refuses(
        carried_tag in arb_tag(),
        tags in arb_keywords(),
    ) {
        prop_assume!(!tags.contains_key(&carried_tag));

        let mut current = base();
        current.keywords = Some([(carried_tag.clone(), Value::Bool(false))].into());
        let mut new_event = base();
        new_event.keywords = Some(tags.clone());
        let edited = stable(new_event);

        let patch = diff(&current, &edited);

        let mut wanted = tags;
        wanted.insert(carried_tag, Value::Bool(false));
        let mut value = serde_json::to_value(&current).expect("CalendarEvent always serializes");
        apply_patch(&mut value, &patch);
        let applied: CalendarEvent =
            serde_json::from_value(value).expect("a patched CalendarEvent always parses");
        prop_assert_eq!(applied.keywords, Some(wanted), "patch was: {:?}", patch);
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
    fn locations_with_extra_member_round_trip(
        key in arb_key(),
        description in arb_text(),
        old_name in arb_place_name(),
        new_name in arb_place_name(),
    ) {
        let mut old_event = stable(base());
        old_event.locations = Some(
            [(key.clone(), location_value_with_description(&old_name, &description))].into(),
        );
        let mut new_event = stable(base());
        new_event.locations = Some(
            [(key.clone(), location_value_with_description(&new_name, &description))].into(),
        );
        round_trips(&old_event, &new_event)?;
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
    fn alerts_left_alone_when_current_holds_one_maps_alerts_refuses(
        mut alerts in arb_alerts(),
        new in arb_alerts(),
    ) {
        alerts.insert("unmappable".to_owned(), unmappable_alert_value());
        let mut current = base();
        current.alerts = Some(alerts);
        let mut new_event = base();
        new_event.alerts = (!new.is_empty()).then_some(new);
        let edited = stable(new_event);

        let patch = diff(&current, &edited);
        prop_assert!(!patch.contains_key("alerts"), "patch was: {:?}", patch);

        let mut value = serde_json::to_value(&current).expect("CalendarEvent always serializes");
        apply_patch(&mut value, &patch);
        let applied: CalendarEvent =
            serde_json::from_value(value).expect("a patched CalendarEvent always parses");
        prop_assert_eq!(applied.alerts, current.alerts);
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
    fn links_key_add_and_drop_left_alone(
        key_kept in arb_key(),
        key_dropped in arb_key(),
        key_added in arb_key(),
        old_href in arb_href(),
        new_href in arb_href(),
        dropped_href in arb_href(),
        added_href in arb_href(),
    ) {
        prop_assume!(key_kept != key_dropped && key_kept != key_added && key_dropped != key_added);

        let mut old_event = base();
        old_event.links = Some(
            [
                (key_kept.clone(), link_value(&old_href, &None)),
                (key_dropped.clone(), link_value(&dropped_href, &None)),
            ]
            .into(),
        );
        let mut new_event = base();
        new_event.links = Some(
            [
                (key_kept.clone(), link_value(&new_href, &None)),
                (key_added.clone(), link_value(&added_href, &None)),
            ]
            .into(),
        );
        let current = stable(old_event);
        let edited = stable(new_event);

        prop_assert!(diff(&current, &current).is_empty());
        let patch = diff(&current, &edited);

        // Only the retained key's href may move; the dropped and added keys
        // are left entirely alone, per diff_links's no-create, no-remove design.
        let mut expected = Map::new();
        if old_href != new_href {
            expected.insert(
                format!("links/{key_kept}/href"),
                Value::String(new_href.clone()),
            );
        }
        prop_assert_eq!(&patch, &expected);

        let mut value = serde_json::to_value(&current).expect("CalendarEvent always serializes");
        apply_patch(&mut value, &patch);
        let applied: CalendarEvent =
            serde_json::from_value(value).expect("a patched CalendarEvent always parses");
        let applied_links = applied.links.expect("links still present");
        let current_links = current.links.expect("current carried links");

        prop_assert_eq!(applied_links.get(&key_dropped), current_links.get(&key_dropped));
        prop_assert!(!applied_links.contains_key(&key_added));
        prop_assert_eq!(
            applied_links.get(&key_kept).and_then(|entry| entry.get("href")),
            Some(&Value::String(new_href)),
        );
    }

    #[test]
    fn links_image_shape_round_trip(entries in arb_image_links()) {
        let mut old_event = base();
        old_event.links = Some(
            entries
                .iter()
                .map(|(key, (old, _, display, content_type))| {
                    (key.clone(), image_link_value(old, display, content_type))
                })
                .collect(),
        );
        let mut new_event = base();
        new_event.links = Some(
            entries
                .iter()
                .map(|(key, (_, new, display, content_type))| {
                    (key.clone(), image_link_value(new, display, content_type))
                })
                .collect(),
        );
        let current = stable(old_event);
        let edited = stable(new_event);
        round_trips(&current, &edited)?;
    }

    #[test]
    fn links_size_change_left_alone(
        key in arb_key(),
        old_href in arb_href(),
        new_href in arb_href(),
        old_size in arb_size(),
        new_size in arb_size(),
    ) {
        let mut old_event = base();
        old_event.links = Some([(key.clone(), attach_link_value(&old_href, old_size))].into());
        let mut new_event = base();
        new_event.links = Some([(key.clone(), attach_link_value(&new_href, new_size))].into());
        let current = stable(old_event);
        let edited = stable(new_event);

        prop_assert!(diff(&current, &current).is_empty());
        let patch = diff(&current, &edited);

        // `size` is never patched, whatever the generated change, so the
        // patch can only ever name `href`.
        let mut expected = Map::new();
        if old_href != new_href {
            expected.insert(format!("links/{key}/href"), Value::String(new_href.clone()));
        }
        prop_assert_eq!(&patch, &expected);

        let mut value = serde_json::to_value(&current).expect("CalendarEvent always serializes");
        apply_patch(&mut value, &patch);
        let applied: CalendarEvent =
            serde_json::from_value(value).expect("a patched CalendarEvent always parses");
        let applied_entry = applied.links.as_ref().and_then(|links| links.get(&key));
        let current_entry = current.links.as_ref().and_then(|links| links.get(&key));

        prop_assert_eq!(
            applied_entry.and_then(|entry| entry.get("href")),
            Some(&Value::String(new_href)),
        );
        // The size the save carries forward is current's own, not edited's:
        // diff_links never writes it, whether it changed or not.
        prop_assert_eq!(
            applied_entry.and_then(|entry| entry.get("size")),
            current_entry.and_then(|entry| entry.get("size")),
        );
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
    fn virtual_locations_key_add_and_drop_left_alone(
        key_kept in arb_key(),
        key_dropped in arb_key(),
        key_added in arb_key(),
        old_uri in arb_href(),
        new_uri in arb_href(),
        dropped_uri in arb_href(),
        added_uri in arb_href(),
    ) {
        prop_assume!(key_kept != key_dropped && key_kept != key_added && key_dropped != key_added);

        let mut old_event = base();
        old_event.virtual_locations = Some(
            [
                (key_kept.clone(), virtual_location_value(&old_uri, &None, &None)),
                (key_dropped.clone(), virtual_location_value(&dropped_uri, &None, &None)),
            ]
            .into(),
        );
        let mut new_event = base();
        new_event.virtual_locations = Some(
            [
                (key_kept.clone(), virtual_location_value(&new_uri, &None, &None)),
                (key_added.clone(), virtual_location_value(&added_uri, &None, &None)),
            ]
            .into(),
        );
        let current = stable(old_event);
        let edited = stable(new_event);

        prop_assert!(diff(&current, &current).is_empty());
        let patch = diff(&current, &edited);

        // Only the retained key's uri may move; the dropped and added keys
        // are left entirely alone, per diff_virtual_locations's no-create,
        // no-remove design.
        let mut expected = Map::new();
        if old_uri != new_uri {
            expected.insert(
                format!("virtualLocations/{key_kept}/uri"),
                Value::String(new_uri.clone()),
            );
        }
        prop_assert_eq!(&patch, &expected);

        let mut value = serde_json::to_value(&current).expect("CalendarEvent always serializes");
        apply_patch(&mut value, &patch);
        let applied: CalendarEvent =
            serde_json::from_value(value).expect("a patched CalendarEvent always parses");
        let applied_places = applied.virtual_locations.expect("virtualLocations still present");
        let current_places = current.virtual_locations.expect("current carried virtualLocations");

        prop_assert_eq!(applied_places.get(&key_dropped), current_places.get(&key_dropped));
        prop_assert!(!applied_places.contains_key(&key_added));
        prop_assert_eq!(
            applied_places.get(&key_kept).and_then(|entry| entry.get("uri")),
            Some(&Value::String(new_uri)),
        );
    }

    #[test]
    fn virtual_locations_uri_stops_naming_a_place_left_alone(
        key_kept in arb_key(),
        key_vanishing in arb_key(),
        old_uri in arb_href(),
        new_uri in arb_href(),
        kept_uri in arb_href(),
        not_a_uri in arb_text(),
    ) {
        prop_assume!(key_kept != key_vanishing);

        let mut old_event = base();
        old_event.virtual_locations = Some(
            [
                (key_kept.clone(), virtual_location_value(&old_uri, &None, &None)),
                (key_vanishing.clone(), virtual_location_value(&kept_uri, &None, &None)),
            ]
            .into(),
        );
        let mut new_event = base();
        new_event.virtual_locations = Some(
            [
                (key_kept.clone(), virtual_location_value(&new_uri, &None, &None)),
                (key_vanishing.clone(), virtual_location_value(&not_a_uri, &None, &None)),
            ]
            .into(),
        );
        let current = stable(old_event);
        let edited = stable(new_event);

        // drawn_conference refuses to write a CONFERENCE line for a value
        // names_a_uri rejects, so the vanishing key never comes back from the
        // round trip at all, unlike the retained one.
        let edited_places = edited.virtual_locations.clone().unwrap_or_default();
        prop_assert!(!edited_places.contains_key(&key_vanishing));
        prop_assert!(edited_places.contains_key(&key_kept));

        prop_assert!(diff(&current, &current).is_empty());
        let patch = diff(&current, &edited);

        // Same no-create, no-remove shape as the key-drop case: only the
        // retained key's uri may move, the vanished one is left entirely
        // alone.
        let mut expected = Map::new();
        if old_uri != new_uri {
            expected.insert(
                format!("virtualLocations/{key_kept}/uri"),
                Value::String(new_uri.clone()),
            );
        }
        prop_assert_eq!(&patch, &expected);

        let mut value = serde_json::to_value(&current).expect("CalendarEvent always serializes");
        apply_patch(&mut value, &patch);
        let applied: CalendarEvent =
            serde_json::from_value(value).expect("a patched CalendarEvent always parses");
        let applied_places = applied.virtual_locations.expect("virtualLocations still present");
        let current_places = current.virtual_locations.expect("current carried virtualLocations");

        prop_assert_eq!(applied_places.get(&key_vanishing), current_places.get(&key_vanishing));
        prop_assert_eq!(
            applied_places.get(&key_kept).and_then(|entry| entry.get("uri")),
            Some(&Value::String(new_uri)),
        );
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
    fn recurrence_rule_by_parts_round_trip(
        old in arb_recurrence_rule_by_parts(),
        new in arb_recurrence_rule_by_parts(),
    ) {
        let mut old_event = base();
        old_event.recurrence_rule = Some(old);
        let mut new_event = base();
        new_event.recurrence_rule = Some(new);
        let current = stable(old_event);
        let edited = stable(new_event);
        round_trips(&current, &edited)?;
    }

    #[test]
    fn recurrence_rule_until_round_trip(
        zone in prop_oneof![Just("Etc/UTC"), Just("America/New_York")],
        old in arb_recurrence_rule_with_until(),
        new in arb_recurrence_rule_with_until(),
    ) {
        let mut old_event = zoned_base(zone);
        old_event.recurrence_rule = Some(old);
        let mut new_event = zoned_base(zone);
        new_event.recurrence_rule = Some(new);
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
    fn recurrence_overrides_restated_start_and_time_zone_round_trip(
        old in arb_restated_start_and_time_zone_overrides(),
        new in arb_restated_start_and_time_zone_overrides(),
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
    fn recurrence_overrides_left_alone_when_id_unspellable(
        mut overrides in arb_exclusions(),
        new in arb_exclusions(),
    ) {
        overrides.insert(unspellable_override_id(), json!({"excluded": true}));
        let mut current = base();
        current.recurrence_overrides = Some(overrides);
        let mut new_event = base();
        new_event.recurrence_overrides = (!new.is_empty()).then_some(new);
        let edited = stable(new_event);

        let patch = diff(&current, &edited);
        prop_assert!(!patch.contains_key("recurrenceOverrides"), "patch was: {:?}", patch);

        let mut value = serde_json::to_value(&current).expect("CalendarEvent always serializes");
        apply_patch(&mut value, &patch);
        let applied: CalendarEvent =
            serde_json::from_value(value).expect("a patched CalendarEvent always parses");
        prop_assert_eq!(applied.recurrence_overrides, current.recurrence_overrides);
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
