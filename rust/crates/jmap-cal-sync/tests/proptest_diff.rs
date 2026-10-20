// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Property tests for the plain scalar properties of `patch::diff` (RFC 8620
//! §5.3): `title`, `description`, `duration`, `status`, `freeBusyStatus`,
//! `privacy` and `priority`. For these properties, `diff` uses no keyed-map,
//! merge or rekey logic. It compares the *baseline* to `edited` and writes the
//! property when they differ. The baseline is the server's event after
//! `ical_to_event(event_to_ical(current))`.
//!
//! The invariant: when you apply `diff(current, edited)` to `current` with
//! plain PatchObject semantics, the result is `edited`. Also, `diff(current,
//! current)` is empty. The invariant applies when the fields of `current` and
//! `edited` are already a fixed point of that round trip. This is realistic: a
//! real `current` always passed through the server's own representation. A real
//! `edited` always passed through the EDS parse of a saved component. `stable()`
//! makes such a fixed point: it sends a new event through the same
//! `event_to_ical`/`ical_to_event` round trip. Thus `current` and `edited`
//! never hold a value that the round trip changes. `diff` handles such values
//! by design through the baseline, not through a direct check of `current`, and
//! this file does not test them.
//!
//! `title` and `description`: `title_round_trip` and `description_round_trip`
//! use only alphanumeric text. `title_round_trip_with_escapes_and_folding` and
//! `description_round_trip_with_escapes_and_folding` add a comma, a semicolon,
//! a backslash and a newline. These are the four characters that RFC 5545
//! §3.3.11 TEXT escaping must round-trip. The text length makes
//! [`jmap_ical::event::fold_overlong_lines`] fold the `SUMMARY`/`DESCRIPTION`
//! line at least once.
//!
//! `keywords` (`diff_keywords`): in `keywords_round_trip`, every tag on both
//! sides is one that `maps_keyword` draws. These tags are plain alphanumeric,
//! thus non-empty, trimmed and `\r`-free, and their value is
//! `Value::Bool(true)`. Thus the `diff_keywords` carry-forward of a `current`
//! tag that the `CATEGORIES` line cannot draw never occurs. The patch is a
//! literal round trip with `edited.keywords`, and no third "expected" event is
//! necessary. `keywords_carry_forward_tag_maps_keyword_refuses` gives `current`
//! a tag with a non-bool value, which `maps_keyword` refuses for all tag names.
//! `diff_keywords` carries that tag forward beside the `edited` tags that map.
//! This test uses the three-event "expected" shape of the book-sync tests for
//! anniversaries, media and online services. Not covered: a tag with an empty
//! or whitespace-only name, or with a `\r`.
//!
//! `links` (`diff_links`): `links_round_trip` uses plain `ATTACH`-shaped
//! entries (an `https:` `href` and an optional `contentType`).
//! `links_image_shape_round_trip` uses the `rel: "icon"` `IMAGE` shape (an
//! optional `display`). Both tests change only the `href` of each key and keep
//! `contentType`/`display` constant, because `diff_links` never patches them.
//! `links_size_change_left_alone` changes the `size` of an `ATTACH` entry
//! independently of `href`. `diff_links` never patches `size`, so the test
//! checks the unpatched property directly. `links_key_add_and_drop_left_alone`
//! uses a key that one side adds or drops. By design, `diff_links` only patches
//! the `href` of an existing key. It does not create a resource that the server
//! does not hold, or remove one that the component stopped naming. Thus an
//! added key never appears in the patch, and a dropped key stays unchanged
//! after the patch. The test checks the unpatched property, not a round trip to
//! `edited`.
//!
//! `virtualLocations` (`diff_virtual_locations`):
//! `virtual_locations_round_trip` uses entries with a plain `https:` `uri` on
//! both sides. Thus every entry passes [`maps_virtual_locations`] and continues
//! to name a place, and the diff never removes it. `name` and `features` (a
//! subset of `CONFERENCE_FEATURES`) change independently.
//! `diff_virtual_locations` patches `uri`, `name` and `features` into an
//! existing key, one member at a time. It does not add a key that only `edited`
//! holds, or remove a key that `edited` drops. Thus the generator keeps the
//! same key set on both sides, as in `links_round_trip`.
//! `virtual_locations_key_add_and_drop_left_alone` uses the same shape as the
//! `links` key test, for the same reason. `name` and `features` stay absent,
//! because this test is about the key set.
//! `virtual_locations_uri_stops_naming_a_place_left_alone` uses an entry whose
//! `uri` `names_a_uri` refuses. `drawn_conference` writes no `CONFERENCE` line
//! for it, so the entry does not come back from the round trip. This is a
//! different code path from the key-drop case, where the input event does not
//! hold the entry. The end state is the same: `diff_virtual_locations` leaves
//! the key alone.
//!
//! `locations` (`diff_locations`): `locations_round_trip` uses only the bare
//! `{"@type": "Location", "name": ...}` shape that `read_locations` makes, with
//! no other member. It uses zero or one entry, the only cases that
//! `maps_locations` accepts. With that shape, every clear uses the
//! whole-property branch, because the entry holds only the name. The test
//! changes an `Option<String>` name independently on each side and keeps the
//! key constant. This covers rename, clear, create and no-op (absent on both
//! sides, or present with the same name).
//! `locations_with_extra_member_round_trip` adds a constant `description`,
//! which `maps_locations` does not examine. A clear then uses the other branch
//! of `diff_locations`: it removes only `locations/<key>/name`, and
//! `description` stays unchanged on both sides.
//!
//! `alerts` (`diff_alerts`): `alerts_round_trip` uses only the shapes that
//! `read_alert` makes for an offset trigger. Relative to the start, the shape
//! is `{"@type": "Alert", "trigger": {"@type": "OffsetTrigger", "offset": ...},
//! "action": "display"}`. Relative to the end, the shape also has
//! `"relativeTo": "end"`. Unlike the keyed maps, `diff_alerts` replaces the
//! whole property (like `keywords`), and its only gate is `maps_alerts` on
//! `current`. The edited side needs no gate, because `read_alert` only makes
//! shapes that `drawn_alert` accepts. With these shapes, every alert maps, so
//! the patch is a literal round trip with `edited.alerts`.
//! `alerts_left_alone_when_current_holds_one_maps_alerts_refuses` adds an
//! `AbsoluteTrigger` alert to `current`. `drawn_trigger` maps it in neither
//! direction, so `maps_alerts(current)` is false. Then `diff_alerts` leaves the
//! whole property alone and does not carry the one entry forward. A property
//! that is replaced whole has no key to leave unnamed. The test checks that the
//! patch has no `alerts` member, and that the applied `alerts` is equal to the
//! `alerts` of `current`.
//!
//! `start` (plain compare against the baseline): a calendar-valid date, a time
//! of day with seconds from 0 to 59, and a constant `timeZone`. A leap second
//! is legal RFC 5545 and RFC 8984 input, but it is a separate question, so this
//! file does not test it. `start` is never cleared, because its absence has no
//! PatchObject shape. Thus the round trip only tests a change and a no-op, like
//! `duration_round_trip`.
//!
//! `timeZone` (compare against the baseline, with `maps_time_zone` as gate):
//! `time_zone_round_trip` uses only plain IANA zone names, for which
//! `names_time_zone` is true. Thus `maps_time_zone` accepts both sides, and
//! `diff_time_zones` adds no `timeZones` entry, because a named zone needs no
//! definition. `zone_of` accepts a `names_time_zone` value with no embedded
//! `VTIMEZONE`, so the round trip does not depend on tzdata resolution. Like
//! `start`, `timeZone` is never cleared.
//! `custom_time_zone_definition_is_added_when_moved_into` moves the event into
//! a custom (solidus-prefixed) identifier. `diff_time_zones` then adds a
//! `timeZones` entry, so that the identifier can be sent.
//! `time_zone_left_alone_when_edited_holds_one_maps_time_zone_refuses` uses a
//! `TZID` that is not a name and cannot be sent. `maps_time_zone` refuses it,
//! and `diff` leaves `timeZone` alone for all values of `edited`.
//!
//! `showWithoutTime` (plain compare, and a clear writes `null`, not `false`)
//! uses its own base event. This base has a midnight start, a whole-day (`P1D`)
//! duration and no time zone. The shared timed `base()` can never show as
//! all-day, for all values of the flag. `shows_without_time` needs `Some(true)`
//! and those three conditions to write a `VALUE=DATE` `DTSTART`. A `false` or
//! absent flag round-trips through the same component as `None`, because the
//! writer cannot spell "explicitly timed". Thus this base has only two stable
//! states, `Some(true)` and `None`, and the generator uses only those.
//!
//! `recurrenceRule` (`diff_recurrence`): `diff_recurrence` replaces the whole
//! property, like `alerts`. [`jmap_ical::maps_recurrence_rule`] gates both
//! `current` and `edited`, so no side can hold a rule that the `RRULE` would
//! narrow or refuse. `recurrence_rule_round_trip` uses a frequency, an optional
//! interval and count, and no `until`. It also uses `byMonth`, `byHour`,
//! `byMinute`, `bySecond` and `firstDayOfWeek`. [`jmap_ical::by_month_part`],
//! [`jmap_ical::by_hour_part`], [`jmap_ical::by_minute_part`],
//! [`jmap_ical::by_second_part`] and [`jmap_ical::first_day_of_week_part`]
//! write these parts. They write them at every frequency, with no interaction
//! between parts. This is unlike the ordinal of `byDay` or the "beside another
//! part" rule of `bySetPos`. All values are in range: an hour, minute or second
//! in the range of RFC 5545 §3.3.10, and a month `"1"`..`"12"` with no leading
//! zero. That shape passes every `maps_recurrence_rule` check, so the patch is
//! a literal round trip with `edited.recurrence_rule`. `Some("mo")` becomes
//! `None` after `stable()`, because `WKST=MO` is the RFC 5545 default and is
//! never written. The other six weekdays round-trip literally. `rrule_to_rule`
//! does not restore `RecurrenceRule::rule_type`, so `stable()` processes both
//! sides first, as for all other properties.
//!
//! `recurrence_rule_by_parts_round_trip` covers `byDay`, `byMonthDay`,
//! `byYearDay`, `byWeekNo` and `bySetPos`. `by_day_part`, `by_month_day_part`,
//! `by_year_day_part` and `by_week_no_part` gate their part on the frequency of
//! the rule. An ordinal in `byDay` needs `MONTHLY` or `YEARLY`, and
//! `byMonthDay` is refused under `WEEKLY`. `byYearDay` needs a frequency that
//! holds a year, and `byWeekNo` needs `YEARLY`. The generator draws the values
//! of each part only at frequencies that its gate accepts. `bySetPos` has no
//! frequency gate. `by_set_position_part` only needs another `BYxxx` part that
//! is written, so the generator always adds a `byDay`. A weekday is valid at
//! every frequency here, so `rule_to_rrule` always writes it, and thus
//! `bySetPos` can always be written.
//!
//! `recurrence_rule_until_round_trip` uses a frequency and `until` only, with a
//! UTC zone and a non-UTC named zone (`zoned_base`). `rule_to_rrule` writes a
//! `Z` suffix only for `Ends::In` a zone that `is_utc` names. `read_until`
//! removes the suffix only after the same zone identity check. No `VTIMEZONE`
//! observance applies to a plain named zone (as in the `timeZone` test). Thus
//! the two zones test both write/read branches of `until`. `until` has the
//! `arb_start` format, which `writable` always accepts.
//!
//! `recurrenceOverrides` (`diff_overrides`):
//! `recurrence_overrides_excluded_round_trip` uses only the bare `{"excluded":
//! true}` shape (an `EXDATE`). `override_maps_by` accepts that shape for all
//! values of `recurrence_rule`, because an `EXDATE` or `RDATE` line is written
//! with or without an `RRULE`. Thus the generator needs no recurrence rule,
//! only a set of `LocalDateTime` ids in the `arb_start` format.
//! `diff_overrides` replaces the whole property, like `keywords` and `alerts`.
//! Thus the patch is a literal round trip with `edited.recurrence_overrides`,
//! and no third "expected" event is necessary. `diff` does not compare
//! `timeZones` (the `patch` module doc says that it is "added to", never
//! compared or replaced). Thus `timeZones` has no round-trip property, other
//! than the empty `timeZones` for a named zone that the `timeZone` test
//! confirms.
//!
//! `recurrence_overrides_restated_round_trip` covers the restated-field shape
//! (`OVERRIDE_PROPERTIES` on a `RECURRENCE-ID` instance). It uses the plain
//! scalar members (`title`, `description`, `duration`, `status`,
//! `freeBusyStatus`, `privacy`, `priority`) and `keywords` and `alerts`. Each
//! field is independently present or absent on each instance. `instance_patch`
//! restates a field only when it differs from the series. `base()` keeps all
//! these fields at the default, except `duration`, so the `duration` generator
//! does not use the `PT1H` of `base()`. Thus a present field always differs and
//! is always restated. `modified_instance` and `vevent_of`/`read_vevent` write
//! and read the restated fields with the same code as the scalar properties of
//! the series. Thus the values of each field use the same domains as the series
//! tests. `maps_override_field` gates `keywords` and `alerts` as at the series
//! level. Each tag must be one that `maps_keyword` draws, and each entry one
//! that `drawn_alert` draws. The field is never empty, because an empty object
//! is written the same as a removal. `base()` holds neither field, so a present
//! value always differs and is restated, with no "expected" event. Not covered:
//! an override that holds a tag or alert that the gate refuses. That case needs
//! the three-event "expected" shape.
//!
//! `recurrence_overrides_restated_start_and_time_zone_round_trip` covers
//! `start` and `timeZone`. Unlike the other seven fields, these do not compare
//! against a `None` baseline. `start` is restated only when it differs from the
//! id of the override. RFC 8984 §4.3.4: the key *is* the start of the instance,
//! unless the patch states otherwise. Thus the generator does not use the id
//! for `start`. `timeZone` is restated against the zone of the series, because
//! `base()` always has one. Thus the generator does not use the
//! `"Europe/Berlin"` of `base()`, as for `duration`. Each field is
//! independently present or absent, and at least one is present.
//!
//! `recurrence_overrides_left_alone_when_id_unspellable` uses an id that no
//! `EXDATE` can spell. The id has no `T` separator, so `to_ical_date_time`
//! refuses it before any digit check. `override_maps_by` checks the id before
//! the patch, so `sends_recurrence_override` refuses the entry for all
//! contents. Then `diff_overrides` leaves the whole property alone and does not
//! carry the one entry forward, as in the `alerts` refusal case. The test
//! checks that the patch has no `recurrenceOverrides` member, and that the
//! applied overrides are equal to the overrides of `current`.

use std::collections::BTreeMap;

use jmap_cal_sync::patch::diff;
use jmap_ical::{event_to_ical, ical_to_event};
use jmap_proto::calendars::{CalendarEvent, NDay, RecurrenceRule};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

/// RFC 8620 §5.3: a `null` value at the end of a path removes that member, and
/// all other values set it. All path segments before the last already exist.
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
/// baseline. The round trip that `diff` compares against cannot change the
/// scalar properties that this file changes.
fn base() -> CalendarEvent {
    CalendarEvent::new()
        .with_uid("seed-uid")
        .with_start("2026-01-15T13:00:00")
        .with_time_zone("Europe/Berlin")
        .with_duration("PT1H")
}

/// `base()` with a different zone: the zone that `until` refers to (RFC 5545
/// §3.3.10, [`jmap_ical::event::Ends::In`]).
fn zoned_base(zone: &str) -> CalendarEvent {
    base().with_time_zone(zone)
}

/// A midnight start, a whole-day duration and no time zone. With the flag,
/// `shows_without_time` needs these three conditions, so this base can show as
/// all-day, but `base()` cannot.
fn all_day_base() -> CalendarEvent {
    CalendarEvent::new()
        .with_uid("seed-uid")
        .with_start("2026-01-15T00:00:00")
        .with_duration("P1D")
}

/// `event` after the `event_to_ical`/`ical_to_event` round trip that `diff`
/// uses for the baseline of a server's event. The result is a fixed point of
/// that round trip.
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

/// Text with RFC 5545 §3.3.11 TEXT escapes: a comma, a semicolon, a backslash
/// and a newline. The writer must escape them and the reader must reverse them.
/// The length makes [`jmap_ical::event::fold_overlong_lines`] fold at least one
/// line after the `SUMMARY:`/`DESCRIPTION:` name.
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

/// A LocalDateTime that `to_local_date_time`/`to_ical_date_time` round-trip
/// byte-identical. The date is calendar-valid, and the seconds are from 0 to 59
/// (a leap second is legal input, but a separate question).
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

/// A plain IANA zone name that `names_time_zone` always accepts.
/// `event_to_ical`/`ical_to_event` round-trip it byte-identical with no
/// embedded `VTIMEZONE` (`zone_of` accepts such a value as it is).
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

/// A tag that `maps_keyword` always draws: plain alphanumeric, thus trimmed,
/// non-empty and `\r`-free.
fn arb_tag() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9]{1,10}"
}

/// A `keywords` set of `arb_tag` members.
fn arb_keywords() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_set(arb_tag(), 0..5).prop_map(|tags| {
        tags.into_iter()
            .map(|tag| (tag, Value::Bool(true)))
            .collect()
    })
}

/// A key that `names_map_entry` always accepts (plain alphanumeric), the only
/// type of key that `X-JMAP-KEY` carries here.
fn arb_key() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9]{1,10}"
}

/// An `href` that `names_a_uri` always accepts and `fetched_locally` never
/// drops: an `https:` URI with a plain alphanumeric path.
fn arb_href() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9]{1,10}".prop_map(|path| format!("https://example.com/{path}"))
}

/// A `contentType` that `media_type` always round-trips unchanged, or no
/// `contentType`.
fn arb_content_type() -> impl Strategy<Value = Option<String>> {
    prop::option::of(prop::sample::select(
        &["text/plain", "image/png", "application/pdf"][..],
    ))
    .prop_map(|ct| ct.map(str::to_owned))
}

/// A plain `ATTACH`-shaped Link entry: an `href` and an optional `contentType`.
fn link_value(href: &str, content_type: &Option<String>) -> Value {
    let mut link = json!({"@type": "Link", "href": href});
    if let Some(content_type) = content_type {
        link["contentType"] = Value::String(content_type.clone());
    }
    link
}

/// 1 to 3 keys, each with an old `href` (for `current`), a new `href` (for
/// `edited`) and a constant `contentType`. `diff_links` only patches `href`, so
/// a change to a different member would fail the round trip.
fn arb_links() -> impl Strategy<Value = BTreeMap<String, (String, String, Option<String>)>> {
    prop::collection::btree_map(
        arb_key(),
        (arb_href(), arb_href(), arb_content_type()),
        1..4,
    )
}

/// A `display` that `spelled` always round-trips unchanged, or no `display`
/// (the `DISPLAY` parameter is optional). The values are the four names that
/// `LINK_DISPLAYS` shares between RFC 8984 §1.4.11 and RFC 7986 §6.1.
fn arb_display() -> impl Strategy<Value = Option<String>> {
    prop::option::of(prop::sample::select(
        &["badge", "graphic", "fullsize", "thumbnail"][..],
    ))
    .prop_map(|display| display.map(str::to_owned))
}

/// An `IMAGE`-shaped Link entry with an optional `display` and `contentType`.
/// `rel: "icon"` makes `drawn_link` write the RFC 7986 §5.10 `IMAGE`, not
/// `ATTACH`. The entry has no `size`, because `read_links` puts `size` only on
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

/// 1 to 3 keys, each with an old `href`, a new `href` and a constant
/// `display`/`contentType` pair. The reason is the same as for `arb_links`:
/// `diff_links` only patches `href`.
fn arb_image_links()
-> impl Strategy<Value = BTreeMap<String, (String, String, Option<String>, Option<String>)>> {
    prop::collection::btree_map(
        arb_key(),
        (arb_href(), arb_href(), arb_display(), arb_content_type()),
        1..4,
    )
}

/// A `size` that `stated_size` always round-trips unchanged: an `UnsignedInt`
/// that the RFC 8607 §4.1 `1*DIGIT` can state.
fn arb_size() -> impl Strategy<Value = Option<u64>> {
    prop::option::of(0u64..1_000_000)
}

/// A plain `ATTACH`-shaped Link entry with a `size` and no `contentType`, to
/// isolate the effect of `size` on the diff.
fn attach_link_value(href: &str, size: Option<u64>) -> Value {
    let mut link = json!({"@type": "Link", "href": href});
    if let Some(size) = size {
        link["size"] = Value::from(size);
    }
    link
}

/// A `name` that `stated_name` round-trips unchanged, or no `name`. It is plain
/// alphanumeric, thus never the empty string that `stated_name` treats as
/// absent.
fn arb_place_name() -> impl Strategy<Value = Option<String>> {
    prop::option::of("[a-zA-Z0-9]{1,10}")
}

/// A `features` subset of `CONFERENCE_FEATURES`, as the `Value` that a save
/// sees. An empty subset is `None`, because `read_virtual_locations` writes the
/// member only when it reads a `FEATURE` param.
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

/// A bare Location entry: only the name, the shape that `read_locations` makes.
/// A clear has no other member to keep.
fn location_value(name: &str) -> Value {
    json!({"@type": "Location", "name": name})
}

/// A Location entry with a `description`, which `maps_locations` does not
/// examine, and an optional name. A clear then has a member to keep, so
/// `diff_locations` removes only the name, not the whole property.
fn location_value_with_description(name: &Option<String>, description: &str) -> Value {
    let mut place = json!({"@type": "Location", "description": description});
    if let Some(name) = name {
        place["name"] = Value::String(name.clone());
    }
    place
}

/// An offset that `drawn_trigger` always round-trips unchanged, relative to the
/// start (no `relativeTo` member).
fn arb_offset() -> impl Strategy<Value = String> {
    prop::sample::select(&["-PT15M", "-PT1H", "-P1D", "PT0S", "PT15M"][..]).prop_map(str::to_owned)
}

/// The Alert shape that `read_alert` makes for a start-relative offset trigger.
/// It has an explicit `@type`, `action` and trigger `@type`, and no
/// `relativeTo: "end"` or other member.
fn alert_value(offset: &str) -> Value {
    alert_value_related(offset, None)
}

/// Like [`alert_value`], with a `relativeTo` on the trigger. `None` is the
/// start (the default, so no member). `Some("end")` is the one other value that
/// `drawn_trigger`/`read_alert` agree on.
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

/// `None` (relative to the start) or `Some("end")`: the only two `relativeTo`
/// values that `drawn_trigger` reads back.
fn arb_related() -> impl Strategy<Value = Option<&'static str>> {
    prop::option::of(Just("end"))
}

/// An Alert shape that `drawn_trigger` maps in neither direction: an
/// `AbsoluteTrigger` (RFC 8984 §4.5.4). `drawn_trigger` maps only an
/// `OffsetTrigger`.
fn unmappable_alert_value() -> Value {
    json!({
        "@type": "Alert",
        "trigger": {"@type": "AbsoluteTrigger", "when": "2026-01-15T12:00:00Z"},
        "action": "display",
    })
}

/// An `alerts` map of 0 to 3 keyed reminders, each an `arb_offset` and
/// `arb_related` pair in `alert_value_related`.
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

/// A VirtualLocation entry: a required `uri` and an optional `name` and
/// `features`. The generators keep the `uri` the same on both sides, so the
/// entry continues to name a place.
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

/// 1 to 3 keys, each with a constant `uri` and an old and new `name` and
/// `features`. A `uri` that stops naming a place removes the entry from the
/// read, so it is not a patched member.
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

/// A frequency that `writable` always accepts (a non-empty string). These are
/// plain iCalendar `FREQ` tokens that `rule_to_rrule`/`rrule_to_rule`
/// round-trip unchanged, in the lower case that `rrule_to_rule` makes.
fn arb_frequency() -> impl Strategy<Value = &'static str> {
    prop::sample::select(&["daily", "weekly", "monthly", "yearly"][..])
}

/// A weekday that `weekday_token` always accepts (the lowercase spelling of RFC
/// 8984 §4.3.3). `first_day_of_week_part` reads `firstDayOfWeek` from this
/// domain.
fn arb_weekday() -> impl Strategy<Value = &'static str> {
    prop::sample::select(&["mo", "tu", "we", "th", "fr", "sa", "su"][..])
}

/// A `RecurrenceRule` with a frequency and an optional interval, count,
/// `byMonth`, `byHour`, `byMinute`, `bySecond` and `firstDayOfWeek`. It has no
/// `until` and no frequency-gated or interacting `BYxxx` part, so
/// [`maps_recurrence_rule`] always accepts it. The other gates have no effect
/// when their part is absent, and `writable` needs only the non-empty
/// frequency. Each of the five parts uses only values that its
/// `by_*_part`/`first_day_of_week_part` function always accepts (non-empty and
/// in range).
///
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

/// A `RecurrenceRule` with the five frequency-gated, interacting `BYxxx` parts
/// that `arb_recurrence_rule` does not use: `byDay`, `byMonthDay`, `byYearDay`,
/// `byWeekNo` and `bySetPos`. Each part uses only the frequencies that its
/// `by_day_part`/`by_month_day_part`/`by_year_day_part`/`by_week_no_part` gate
/// accepts, so every value passes `maps_recurrence_rule`. `byDay` is always
/// present (a plain weekday, valid at every frequency here). Thus `bySetPos`
/// always has another written `BYxxx` part to select from: see
/// [`jmap_ical::event::by_set_position_part`].
///
/// `nthOfPeriod` is in a small range (`-5..=-1`, `1..=5`): a realistic "nth
/// weekday of the month or year", not every value that
/// [`jmap_ical::event::by_day_token`] accepts unchecked.
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

/// A `RecurrenceRule` with a frequency and `until` only, a shape that
/// [`maps_recurrence_rule`] always accepts. `writable` accepts it, because
/// `until` has the `arb_start` format, which `to_ical_date_time` always reads.
/// There is no `BYxxx`, `extra`, `rscale` or `skip`. This isolates the
/// zone-dependent conversion of `until` from the frequency/interval/count shape
/// of `arb_recurrence_rule`.
///
fn arb_recurrence_rule_with_until() -> impl Strategy<Value = RecurrenceRule> {
    (arb_frequency(), arb_start())
        .prop_map(|(frequency, until)| RecurrenceRule::new(frequency).with_until(until))
}

/// A `recurrenceOverrides` map with only the bare `{"excluded": true}` shape.
/// `override_maps_by` accepts an `EXDATE` id unconditionally, so a key only
/// needs the `arb_start` format, which `to_ical_date_time` reads.
fn arb_exclusions() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_set(arb_start(), 0..5).prop_map(|ids| {
        ids.into_iter()
            .map(|id| (id, json!({"excluded": true})))
            .collect()
    })
}

/// A non-empty `keywords` restated field of `arb_tag` members, a shape that
/// `maps_override_field` accepts. Every tag is one that `maps_keyword` draws.
/// The field is never empty, because an empty object is written the same as a
/// removal.
fn arb_restated_keywords() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_set(arb_tag(), 1..4).prop_map(|tags| {
        tags.into_iter()
            .map(|tag| (tag, Value::Bool(true)))
            .collect()
    })
}

/// A non-empty `alerts` restated field of keyed `arb_offset` reminders, a shape
/// that `maps_override_field` accepts. Every entry is one that `drawn_alert`
/// draws, and the field is never empty (for the same reason as
/// `arb_restated_keywords`).
fn arb_restated_alerts() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_map(arb_key(), arb_offset(), 1..4).prop_map(|entries| {
        entries
            .into_iter()
            .map(|(k, o)| (k, alert_value(&o)))
            .collect()
    })
}

/// The restated fields of one instance: every plain scalar
/// `OVERRIDE_PROPERTIES` member and `keywords` and `alerts`, each independently
/// present or absent. `duration` is `2..12` hours, not the `1..12` of
/// `duration_round_trip`. Thus a present value always differs from the `PT1H`
/// of `base()`, and `instance_patch` always restates it. All other fields
/// differ from the `None` of `base()` for all values. At least one field is
/// present, because an empty patch makes no instance (`modified_instance`
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
                    // Serialising this set cannot fail: it holds only strings
                    // and `true`.
                    patch.insert(
                        "keywords".to_owned(),
                        serde_json::to_value(keywords).unwrap_or(Value::Null),
                    );
                }
                if let Some(alerts) = alerts {
                    // Serialising this map cannot fail: it holds only nested
                    // objects of strings.
                    patch.insert(
                        "alerts".to_owned(),
                        serde_json::to_value(alerts).unwrap_or(Value::Null),
                    );
                }
                patch
            },
        )
}

/// A `recurrenceOverrides` map of 0 to 2 instances, each an
/// `arb_restated_override` with an `arb_start` id.
fn arb_restated_overrides() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_map(arb_start(), arb_restated_override(), 0..3).prop_map(|entries| {
        entries
            .into_iter()
            .map(|(id, patch)| (id, Value::Object(patch)))
            .collect()
    })
}

/// The `start` and `timeZone` restated fields of one instance, each
/// independently present or absent. The filter removes a `start` that is equal
/// to the id. `instance_patch` restates `start` only when it differs from the
/// key, because otherwise the key *is* the start of the instance. The filter
/// also removes a `timeZone` equal to the zone of `base()`. Unlike all other
/// restated fields, its baseline is not `None` (the same reason as for
/// `duration` in `arb_restated_override`). When neither field remains, the
/// strategy draws again, so every entry restates at least one of the two.
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

/// A `recurrenceOverrides` map of 0 to 2 instances, each an
/// `arb_restated_start_and_time_zone_entry`.
fn arb_restated_start_and_time_zone_overrides() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::vec(arb_restated_start_and_time_zone_entry(), 0..3)
        .prop_map(|entries| entries.into_iter().collect())
}

/// A `recurrenceOverrides` id that no `EXDATE` can spell: it has no `T`
/// separator, so `to_ical_date_time` refuses it before any digit check.
fn unspellable_override_id() -> String {
    "2026-01-15".to_owned()
}

/// An RFC 5545 §3.3.14 UTC-OFFSET that `utc_offset` round-trips byte-identical.
/// It is already in the short `±hhmm` form, and it is never the negative zero
/// that the grammar forbids.
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

/// A custom `TimeZoneId` (the solidus-prefixed second form of RFC 8984 §1.4.8)
/// and the one-observance `TimeZone` definition that names it.
///
/// `vtimezone_of` writes the single `standard` rule whole (the RFC 5545 §3.6.5
/// minimum, one subcomponent). With no `recurrenceRules` or `names` member,
/// `read_time_zones` reads back this shape, `tzId` included. The test below
/// needs this round trip.
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

/// A `TZID` that `maps_time_zone` refuses: it does not name a zone
/// (`names_time_zone`) and does not start with a solidus. Thus a server cannot
/// resolve it. In practice, this is a Windows zone name that arrives
/// unconverted.
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

        // Only the href of the retained key can change. By design, diff_links
        // does not create or remove a key, so the dropped and added keys stay
        // unchanged.
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

        // `size` is never patched, so the patch can only contain `href`.
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
        // The applied size is the size of current, not edited: diff_links never
        // writes `size`.
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

        // Only the uri of the retained key can change. By design,
        // diff_virtual_locations does not create or remove a key, so the
        // dropped and added keys stay unchanged.
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

        // drawn_conference writes no CONFERENCE line for a value that
        // names_a_uri refuses. Thus the round trip removes the vanishing key,
        // but not the retained key.
        let edited_places = edited.virtual_locations.clone().unwrap_or_default();
        prop_assert!(!edited_places.contains_key(&key_vanishing));
        prop_assert!(edited_places.contains_key(&key_kept));

        prop_assert!(diff(&current, &current).is_empty());
        let patch = diff(&current, &edited);

        // The same no-create, no-remove shape as the key-drop case: only the
        // uri of the retained key can change, and the vanished key stays
        // unchanged.
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
