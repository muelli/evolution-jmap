// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Converts an edited component back into a `CalendarEvent/set` PatchObject.
//!
//! A `VEVENT` is a lossy view of a JSCalendar event. The mapping reads a fixed
//! set of properties and drops all others. If a save
//! sent the parsed event back whole, it would delete data that the user did not
//! see and did not remove (for example, a guest's `sendTo` or the sequence number).
//!
//! Data also gets lost *inside* the mapped properties. Thus this module does not
//! compare the edited event with the server's event. It compares the edited event
//! with **the server's event after the same round trip**. This baseline is exactly
//! what Evolution showed, so each difference from it is an edit, and nothing else is.
//! As a result:
//!
//! - a `timeZone` of `UTC` does not change to `Etc/UTC` because the `Z` suffix on
//!   the component reads back as `Etc/UTC`,
//! - an `RRULE` that dropped `INTERVAL=1` does not send a rule without `interval`,
//! - a `status` outside the closed vocabulary stays when a save does not change it.
//!
//! Ten properties need more than the baseline, because "no difference" does not
//! answer the full question for them:
//!
//! - **`locations` is a map of places, and the component has one line.** The save
//!   patches the name *into* the server's entry and does not replace the property.
//!   See `diff_locations`.
//! - **`virtualLocations` is a map of places, and the component has one line for
//!   each.** Each line carries the key of its entry in an `X-JMAP-KEY`. The save
//!   patches the members that the line shows into that entry. A line with a key
//!   that the server does not hold creates nothing and deletes nothing. See
//!   `diff_virtual_locations`.
//! - **`links` is a map of resources, and only one member of an entry is the
//!   user's.** A line shows the address, the media type and the size. The type and
//!   the size are the server's description, and the `cid` and the `title` have no
//!   space on the line. Thus the save patches only `links/<key>/href`. See
//!   `diff_links`.
//! - **`keywords` is a set, and a set has no keys.** The save replaces the property
//!   whole. A tag that the `CATEGORIES` line cannot carry was not shown, so the save
//!   copies it into the new set. This keeps the rule "the user does not delete an
//!   entry that was not shown". See `diff_keywords`.
//! - **`alerts` is a map that the save replaces whole, and a second property
//!   controls if clients read it.** A `VALARM` here cannot show a reminder at an
//!   absolute time or a reminder that sends mail. When RFC 8984 §4.5.1's
//!   `useDefaultAlerts` is true, clients ignore the property. In both cases the
//!   reminders were not shown, so the save does not write them. See `diff_alerts`.
//! - **`recurrenceRule` is one property, not a merge point** (jscalendarbis
//!   §3.3.3: singular, not RFC 8984's plural `recurrenceRules` array). This crate
//!   cannot write a rule with `rscale` as an `RRULE`, so a patch would narrow the
//!   recurrence without the user's knowledge. If a server rule fails
//!   [`maps_recurrence_rule`], the save does not touch the property. The same
//!   applies to a rule from the *save* that fails it, as for the series'
//!   `timeZone`: the save replaces the property whole, so one part that the server
//!   can refuse costs all other edits in the save.
//! - **`recurrenceOverrides` has the same problem one level down.** An `EXDATE`,
//!   an `RDATE` and a `RECURRENCE-ID` component can say that an instance is off,
//!   that it occurs, or that it occurs with a different title, start, zone,
//!   duration, description, status, transparency, priority, privacy, set of tags or
//!   set of reminders. They cannot say that it occurs at a different place or with
//!   different guests. An override that only a bare `RDATE` can show comes back as
//!   the empty patch and deletes what was not shown. Thus, if a server override
//!   fails [`sends_recurrence_override`], the save does not touch the property. The
//!   same applies when an override from the *save* holds a value that the save
//!   cannot send, as for the series' `timeZone`.
//! - **`timeZones` is not compared: the save only adds to it.** The map defines the
//!   identifiers that the event uses. A server entry can hold a `url`, a
//!   `validUntil` or `aliases`, and a `VTIMEZONE` has no space for them. Thus the
//!   save never replaces the map. It writes one entry for each identifier that the
//!   save sends and the server does not define. See `diff_time_zones`.
//! - **RFC 8984 requires `start`.** If the mapping cannot read the `DTSTART`, the
//!   component has no start. `"start": null` is not legal, so the server's start
//!   stays.
//!
//! - **`participants` is a map of guests: the server sets the keys, and the
//!   component identifies guests by address.** The save patches the changes of a
//!   guest into the server entry for that address. It compares roles in the way
//!   that servers normalise them, or each save would patch them back. See
//!   `diff_participants`.
//!
//! One property gets a check on output, not against the baseline. A `TZID` is an
//! iCalendar identifier. RFC 8984 §1.4.8 accepts it as a time zone only under its
//! own name or next to its definition. See [`jmap_ical::maps_time_zone`] and [`diff`].

use std::collections::{BTreeMap, BTreeSet};

use jmap_ical::event::{OWNER_ROLE, PARTICIPANT_ROLES, calendar_address};
use jmap_ical::{
    event_to_ical, ical_to_event, maps_alerts, maps_keyword, maps_locations, maps_recurrence_rule,
    maps_time_zone, maps_virtual_locations, sends_recurrence_override, time_zone_definition,
};
use jmap_proto::calendars::CalendarEvent;
use serde_json::{Map, Value};

/// Returns the patch that changes the server's event into the event that
/// Evolution saved. The patch is empty when the edit changes nothing this mapping
/// can see.
pub fn diff(current: &CalendarEvent, edited: &CalendarEvent) -> Map<String, Value> {
    let mut patch = diff_mapped(current, edited);
    diff_time_zones(&mut patch, current, edited);
    patch
}

/// Returns [`diff`] from `current` to `edited`, limited to the properties that
/// differ between `base` and `edited`.
///
/// `base` is the event that the user edited. A property that only another client
/// changed since then keeps the server's value.
pub fn diff_since(
    base: &CalendarEvent,
    current: &CalendarEvent,
    edited: &CalendarEvent,
) -> Map<String, Value> {
    diff_since_except(base, current, edited, &BTreeSet::new())
}

/// Returns [`diff_since`] without the properties in `kept`, which keep the server's value.
pub fn diff_since_except(
    base: &CalendarEvent,
    current: &CalendarEvent,
    edited: &CalendarEvent,
    kept: &BTreeSet<String>,
) -> Map<String, Value> {
    let touched = diff_mapped(base, edited);
    let mut patch = diff_mapped(current, edited);
    patch.retain(|path, _| {
        let name = property(path);
        !kept.contains(name) && touched.keys().any(|edit| property(edit) == name)
    });
    // The definitions follow the zones that stay in the patch.
    diff_time_zones(&mut patch, current, edited);
    patch
}

/// Returns the properties that the user and a different client both changed
/// after `base`, to different values.
pub fn conflicts(
    base: &CalendarEvent,
    current: &CalendarEvent,
    edited: &CalendarEvent,
) -> BTreeSet<String> {
    let remote = diff_mapped(current, base);
    diff_since(base, current, edited)
        .keys()
        .map(|path| property(path))
        .filter(|name| remote.keys().any(|theirs| property(theirs) == *name))
        .map(str::to_owned)
        .collect()
}

/// The top-level property that a patch path names.
fn property(path: &str) -> &str {
    path.split('/').next().unwrap_or(path)
}

/// [`diff`] without the `timeZones` definitions.
fn diff_mapped(current: &CalendarEvent, edited: &CalendarEvent) -> Map<String, Value> {
    let mut patch = Map::new();

    // The baseline is what Evolution showed. If the server's event does not parse
    // back, no baseline is reliable, so send nothing.
    let Ok(baseline) = ical_to_event(&event_to_ical(current)) else {
        return patch;
    };

    // `start` is the only property that has no legal way to be absent.
    let baseline_start = baseline.start.as_deref().filter(|s| !s.is_empty());
    let edited_start = edited.start.as_deref().filter(|s| !s.is_empty());
    if edited_start.is_some() && edited_start != baseline_start {
        set(&mut patch, "start", edited_start);
    }
    // A new `timeZone` value can be unsendable. A component states its zone with a
    // `TZID`, not with an RFC 8984 §1.4.8 name. If the document gives no
    // translation, keep the server's zone: do not send it or clear it.
    //
    // A custom identifier (§1.4.8's second form) is sendable next to a `timeZones`
    // entry that defines it. `jmap_ical::maps_time_zone` checks this, as on the
    // create path, and [`diff_time_zones`] adds the entry. An identifier that the
    // document neither names nor defines stays out (for example, a Windows zone
    // from an Exchange invitation, or a solidus-prefixed one with no `VTIMEZONE`).
    let baseline_tz = baseline.time_zone.as_deref().filter(|s| !s.is_empty());
    let edited_tz = edited.time_zone.as_deref().filter(|s| !s.is_empty());
    if maps_time_zone(edited) && baseline_tz != edited_tz {
        set(&mut patch, "timeZone", edited_tz);
    }
    for (property, was, now) in [
        ("title", &baseline.title, &edited.title),
        ("description", &baseline.description, &edited.description),
        ("duration", &baseline.duration, &edited.duration),
        ("status", &baseline.status, &edited.status),
        // Like `status`, this needs only the baseline. Both sides drop a value
        // outside RFC 8984 §4.4.2's two states, so a save that does not change it
        // sends nothing. Clearing it sends `null`, which is the default, the same
        // state as a component with no `TRANSP`.
        (
            "freeBusyStatus",
            &baseline.free_busy_status,
            &edited.free_busy_status,
        ),
        // Evolution's editor writes `CLASS` on each save (Options ▸ Classification).
        // Thus the first save of an event without `privacy` patches it to `public`.
        // This write is redundant but correct (RFC 8984 §4.4.3: `public` and no
        // value are the same state). It occurs one time, because after it the
        // baseline also shows `CLASS`.
        ("privacy", &baseline.privacy, &edited.privacy),
    ] {
        let was_norm = was.as_deref().filter(|s| !s.is_empty());
        let now_norm = now.as_deref().filter(|s| !s.is_empty());
        if was_norm != now_norm {
            set(&mut patch, property, now_norm);
        }
    }

    // `priority` is an integer, but otherwise like `status`. Both sides drop a
    // value outside the range of RFC 8984 §4.4.1 and RFC 5545 §3.8.1.9, so a save
    // that does not change it sends nothing. Clearing it sends `null`, which is the
    // default of 0, the same state as a component with no `PRIORITY`.
    if baseline.priority != edited.priority {
        patch.insert(
            "priority".to_owned(),
            edited.priority.map_or(Value::Null, Value::from),
        );
    }

    // The component shows "all day" only as a DATE-valued `DTSTART`. The mapping
    // renders some all-day events as timed (for example, one that starts at 09:00
    // or has a zone). The baseline loses the flag in the same way, so only a real
    // change gets patched.
    if baseline.show_without_time != edited.show_without_time {
        patch.insert(
            "showWithoutTime".to_owned(),
            // Send `null`, not `false`: the RFC 8984 default is false, and
            // removing a property restores its default.
            edited.show_without_time.map_or(Value::Null, Value::Bool),
        );
    }

    diff_locations(&mut patch, current, &baseline, edited);
    diff_virtual_locations(&mut patch, current, &baseline, edited);
    diff_links(&mut patch, current, &baseline, edited);
    diff_keywords(&mut patch, current, &baseline, edited);
    diff_alerts(&mut patch, current, &baseline, edited);
    diff_recurrence(&mut patch, current, &baseline, edited);
    diff_overrides(&mut patch, current, &baseline, edited);
    diff_participants(&mut patch, current, &baseline, edited);
    patch
}

/// Adds the definitions that the custom identifiers in `patch` need.
///
/// RFC 8984 §1.4.8's second form of `TimeZoneId` (solidus-prefixed, made by a
/// server or an Exchange organiser for a zone that no database names) is legal
/// only when the event's §4.7.2 `timeZones` map defines it. Without the
/// definition, the server can refuse the full `CalendarEvent/set`. The other
/// functions write such identifiers and rely on this function. It reads them from
/// the patch, not from `edited`: a zone in `edited` that is not in the patch is
/// not sent.
///
/// The patch adds **one entry at a time** (`timeZones/<pointer>`) and does not
/// replace the property. A `VTIMEZONE` has no space for the `aliases`, `url` or
/// `validUntil` of a server definition, so a replacement deletes data that the
/// user did not see. For the same reason, an identifier that the server already
/// defines stays unchanged. The patch writes the full property only when the
/// server's event has no `timeZones`: RFC 8620 §5.3 requires each path segment
/// before the last to exist, and nothing is there to overwrite.
///
/// This function does not remove definitions that the edit no longer uses. The
/// create path removes them ([`jmap_ical::prune_time_zones`]) because it builds
/// the full map. Here the map is the server's, an unused entry is legal, and a
/// removal is a guess.
fn diff_time_zones(
    patch: &mut Map<String, Value>,
    current: &CalendarEvent,
    edited: &CalendarEvent,
) {
    let mut wanted: BTreeMap<String, Value> = BTreeMap::new();
    for property in ["timeZone", "recurrenceOverrides"] {
        let Some(value) = patch.get(property) else {
            continue;
        };
        // The zone of the series, or the zone of each override. `timeZone` is a
        // string and `recurrenceOverrides` is a map of patches, so the shape tells
        // them apart. A `null` removes the property and names no zone.
        let named: BTreeSet<String> = match value.as_object() {
            Some(overrides) => overrides
                .values()
                .filter_map(|instance| custom_zone(instance.get("timeZone")))
                .collect(),
            None => custom_zone(Some(value)).into_iter().collect(),
        };
        // The document defines each identifier that it lets into the patch, so
        // these lookups succeed. If one fails, remove the property from the
        // patch, as for a zone that the document cannot name. A reference to no
        // definition can cause the server to refuse the full `CalendarEvent/set`.
        if named
            .iter()
            .any(|tzid| time_zone_definition(edited, tzid).is_none())
        {
            patch.remove(property);
            continue;
        }
        for tzid in named {
            // The server already defines this zone: do not change it.
            if time_zone_definition(current, &tzid).is_some() {
                continue;
            }
            if let Some(definition) = time_zone_definition(edited, &tzid) {
                wanted.insert(tzid, definition.clone());
            }
        }
    }
    if wanted.is_empty() {
        return;
    }
    match current.time_zones.is_some() {
        true => {
            for (tzid, definition) in wanted {
                patch.insert(format!("timeZones/{}", escaped(&tzid)), definition);
            }
        }
        false => {
            patch.insert(
                "timeZones".to_owned(),
                Value::Object(wanted.into_iter().collect()),
            );
        }
    }
}

/// Returns the custom `TimeZoneId` that a `timeZone` value states, if any.
///
/// Only RFC 8984 §1.4.8's solidus-prefixed form needs a definition. All readers
/// have the IANA database, and a `null` clears the zone.
fn custom_zone(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|tzid| tzid.starts_with('/'))
        .map(str::to_owned)
}

/// Patches the place of the event. This is the one property that this module
/// patches *into* and does not replace.
///
/// A `LOCATION` is one line of text. A JSCalendar Location also holds
/// coordinates, links, types and a zone, and the event holds a map of them. Thus
/// the patch writes `locations/<key>/name` under the server's key, and all other
/// data stays.
///
/// The key comes from the server's event, not from the component. The key goes
/// out and comes back in `X-JMAP-KEY`, but Evolution's editor writes a new
/// `LOCATION` and can drop unknown parameters. The diff compares only the name,
/// and the component has only one place.
///
/// The function first asks [`maps_locations`] about the server's map, as for the
/// recurrence properties. A second place has no line, and the user cannot edit a
/// property that was shown only in part.
fn diff_locations(
    patch: &mut Map<String, Value>,
    current: &CalendarEvent,
    baseline: &CalendarEvent,
    edited: &CalendarEvent,
) {
    let empty = BTreeMap::new();
    let places = current.locations.as_ref().unwrap_or(&empty);
    if !maps_locations(places) {
        return;
    }
    if drawn_name(baseline) == drawn_name(edited) {
        return;
    }
    match (places.iter().next(), drawn_name(edited)) {
        // Rename the server's entry in place.
        (Some((key, _)), Some(name)) => {
            patch.insert(name_of(key), Value::String(name.to_owned()));
        }
        // The user cleared the place. If the entry holds only its name, remove the
        // property (`maps_locations` already makes sure that no second place gets
        // lost). Otherwise remove only the name.
        (Some((key, place)), None) => {
            let path = match place.as_object().is_some_and(|place| {
                place
                    .keys()
                    .all(|member| member == "name" || member == "@type")
            }) {
                true => "locations".to_owned(),
                false => name_of(key),
            };
            patch.insert(path, Value::Null);
        }
        // The event had no place. RFC 8620 §5.3 requires each path segment before
        // the last to exist, so write the full property.
        (None, Some(_)) => {
            patch.insert(
                "locations".to_owned(),
                // Serialising a map from this crate's reader cannot fail: it
                // holds one object with two strings.
                serde_json::to_value(&edited.locations).unwrap_or(Value::Null),
            );
        }
        // Neither side names a place. The comparison above already returned.
        (None, None) => {}
    }
}

/// Returns the name of the one place on a component, or `None` if it has none.
fn drawn_name(event: &CalendarEvent) -> Option<&str> {
    event
        .locations
        .iter()
        .flatten()
        .find_map(|(_, place)| place.get("name")?.as_str().filter(|name| !name.is_empty()))
}

/// Returns the patch path of the name of a place.
fn name_of(key: &str) -> String {
    format!("locations/{}/name", escaped(key))
}

/// Returns a map key as a patch path segment. RFC 8620 §5.3 uses JSON pointer
/// segments, so a `~` or a `/` in a server key gets escaped (RFC 6901 §3).
fn escaped(key: &str) -> String {
    jmap_proto::text::escape_pointer_segment(key)
}

/// Patches the online places of the event. This is the second property that this
/// module patches *into*, for the same reason as [`diff_locations`].
///
/// RFC 8984 §4.2.6's VirtualLocation holds a `description`, and a `CONFERENCE`
/// line has no space for it. If the patch named `virtualLocations`, it would
/// delete a note that the user did not see. The save writes
/// `virtualLocations/<key>/uri`, `/name` and `/features` (the three members that
/// the line shows), one member at a time. All other members stay.
///
/// A component has one line for each entry (RFC 7986 §5.11). Each line carries
/// its key out and back in an `X-JMAP-KEY`. Position cannot identify the entry:
/// if an editor drops a line that it cannot show, each later conference moves to
/// the wrong entry.
///
/// This function does **not** do two things. A component that names a conference
/// that the server does not name is ambiguous, and a wrong reading causes data
/// loss:
///
/// - **A conference that the component no longer names stays.** A missing line
///   does not show who removed it. Evolution 3.52 has no UI for a conference.
///   Without a real Evolution, this repository cannot find if its editor writes
///   back a property that it does not know. A deletion here can destroy a link
///   that the user did not touch. The cost of the other reading is only that a
///   deletion from a different client comes back on the next sync.
/// - **A conference that the server does not hold is not created.** RFC 8620 §5.3
///   requires each path segment before the last to exist, so the patch cannot
///   write an entry without a server key. Also, a line with an unknown key can be
///   a rewrite of a server entry or a new place. Only the create path, which
///   writes the full property, adds a new conference.
///
/// The function first asks [`maps_virtual_locations`] about the server's map, as
/// for each conditionally mapped property. If the line cannot show an entry in
/// full, the user did not see it, so no difference is an edit by the user.
fn diff_virtual_locations(
    patch: &mut Map<String, Value>,
    current: &CalendarEvent,
    baseline: &CalendarEvent,
    edited: &CalendarEvent,
) {
    let empty = BTreeMap::new();
    let places = current.virtual_locations.as_ref().unwrap_or(&empty);
    if !maps_virtual_locations(places) {
        return;
    }
    let was = baseline.virtual_locations.as_ref().unwrap_or(&empty);
    let now = edited.virtual_locations.as_ref().unwrap_or(&empty);

    for (key, before) in was {
        // The key must be a server key, not only a baseline key. A server key
        // outside RFC 8984 §1.4.4's `Id` grammar does not survive the round trip,
        // so the baseline holds an invented key, which names no entry to patch.
        let (Some(after), true) = (now.get(key), places.contains_key(key)) else {
            continue;
        };
        for member in ["uri", "name", "features"] {
            let before_val = before.get(member);
            let after_val = after.get(member);
            let before_norm = match before_val {
                Some(Value::String(s)) if s.is_empty() => None,
                Some(Value::Object(m)) if m.is_empty() => None,
                _ => before_val,
            };
            let after_norm = match after_val {
                Some(Value::String(s)) if s.is_empty() => None,
                Some(Value::Object(m)) if m.is_empty() => None,
                _ => after_val,
            };
            if before_norm == after_norm {
                continue;
            }
            match after_norm {
                Some(value) => {
                    patch.insert(member_of(key, member), value.clone());
                }
                // Removing the member restores the RFC 8984 §4.2.6 default: the
                // empty string for `name`, no features for `features`. Never
                // remove `uri`: it is the one required member and the full line,
                // and this crate reads no place from a line without one.
                None if member != "uri" => {
                    patch.insert(member_of(key, member), Value::Null);
                }
                None => {}
            }
        }
    }
}

/// Returns the patch path of one member of one virtual location.
fn member_of(key: &str, member: &str) -> String {
    format!("virtualLocations/{}/{member}", escaped(key))
}

/// Patches the links of the event. This is the third property that this module
/// patches *into*, and the narrowest: only the address goes back.
///
/// A Link (RFC 8984 §1.4.11) holds a `cid` and a `title`, and neither an `ATTACH`
/// nor an `IMAGE` line has space for them. If the patch named `links`, it would
/// delete half of each resource. The save writes `links/<key>/href` under the key
/// of the line, and all other members stay.
///
/// The line shows `contentType` and `size` (as `FMTTYPE` and `SIZE`), but the
/// save does not write them back. This is the one difference from
/// [`diff_virtual_locations`]. They are the *server's* description of the
/// resource, not a field for the user (§1.4.11 calls the size an estimate). An
/// editor often rewrites a line without the parameters that it has no UI for. If
/// the save read this as "the media type was cleared", it would delete server
/// data on an unrelated edit. Thus they are read-only, like `created` and
/// `updated`. The same applies to `rel`, which the property name states, not the
/// line. Evolution does not let the user change the type of a resource, and a
/// picture that an editor without `IMAGE` support rewrites as an attachment must
/// not cause a patch.
///
/// The two limits of [`diff_virtual_locations`] apply for the same reasons: a
/// resource that the component no longer names stays, and a resource that the
/// server does not hold is not created. The second limit is more important here.
/// A file that the user attaches in Evolution is a `file:` URI into a local store.
/// Other clients cannot get it, so `jmap_ical` never reads it. As a Link, it would
/// put a path from the user's home directory in a record that all other clients
/// of the account can read. To send the file, a client must upload it as a blob,
/// and this crate does not do that.
///
/// The check applies to each entry, not to the full property. A patch of the
/// `href` of one entry cannot change a different entry, so a resource that was
/// not shown costs only its own display. The entry must be the one that the line
/// came from, and the server's address identifies it. See
/// [`the_servers_own_entry`].
fn diff_links(
    patch: &mut Map<String, Value>,
    current: &CalendarEvent,
    baseline: &CalendarEvent,
    edited: &CalendarEvent,
) {
    let empty = BTreeMap::new();
    let held = current.links.as_ref().unwrap_or(&empty);
    let was = baseline.links.as_ref().unwrap_or(&empty);
    let now = edited.links.as_ref().unwrap_or(&empty);

    for (key, before) in was {
        let (Some(after), true) = (now.get(key), the_servers_own_entry(held, key, before)) else {
            continue;
        };
        if before.get("href") == after.get("href") {
            continue;
        }
        // RFC 8984 §1.4.11 requires `href`, and it is the full line. `jmap_ical`
        // reads no resource from a line without one, so no removal is possible.
        if let Some(href) = after.get("href") {
            patch.insert(format!("links/{}/href", escaped(key)), href.clone());
        }
    }
}

/// Returns true if the key of a shown resource is the key of the server entry
/// that it comes from. This check makes `links/<key>/href` safe to send.
///
/// The key alone is not sufficient. [`diff_virtual_locations`] only checks that
/// the server holds the key, because a key outside RFC 8984 §1.4.4's `Id` grammar
/// cannot go on the line and reads back as an invented key. Here an invented key
/// can *collide* with a server key of a different entry. (`jmap_ical` invents keys
/// that the document does not use, so the collision is with an entry that was not
/// shown: one without an address, or one with a `file:` URI that it does not
/// read.) A patch of the `href` of that entry would move a resource that the user
/// did not see and lose the edit of the user.
///
/// Thus the address decides. The baseline is the server's entry after the round
/// trip, and `href` does not change in either direction, so the correct entry is
/// the one with the same address. If the addresses are different, the edit is
/// dropped: as for a conference, this side cannot patch under a key that it
/// invented.
fn the_servers_own_entry(held: &BTreeMap<String, Value>, key: &str, before: &Value) -> bool {
    held.get(key).and_then(|resource| resource.get("href")) == before.get("href")
}

fn non_empty_map(map: Option<&BTreeMap<String, Value>>) -> Option<&BTreeMap<String, Value>> {
    map.filter(|m| !m.is_empty())
}

/// Replaces the tags of the event whole. This is the difference from
/// [`diff_locations`].
///
/// A `CATEGORIES` line holds a list of TEXT values, and a JSCalendar keyword is a
/// plain string. Thus the full set fits on the component, and no entry has data to
/// keep. The baseline is what was shown, and the difference from it is the set
/// that the user wants. To clear the field, the patch sends `"keywords": null`,
/// which is RFC 8984 §4.2.9's default of no tags. An empty map is a different
/// stored value.
///
/// The function asks [`maps_keyword`] about the server's set. A tag that the
/// `CATEGORIES` line cannot carry was not shown, so its absence from the edited
/// component is not a removal. A keyed map leaves such an entry out of the patch,
/// but a set has no key. Thus the save copies the tag into the new set. This is the
/// same rule, and the edit around the tag still applies.
///
/// The copied tags come from the server's event, not from the baseline: the
/// baseline is what was *shown*, and it does not hold these tags. The baseline
/// still tells if the user changed something.
///
/// A copied tag keeps the value that the server stated, also a value that RFC 8984
/// §1.4.3 does not accept. A rewrite here would be a change that the mapping
/// invents. The set of the user wins when both name the same tag: the user typed
/// the tag and wants it set.
///
/// The *edited* side needs no check. Each of its tags comes from a content line,
/// and RFC 8984 accepts any string as a keyword.
///
/// This applies to the series only. An instance that the user edits alone states
/// its own set on its own component, and the override from [`diff_overrides`]
/// carries it. There [`sends_recurrence_override`] asks [`maps_keyword`] about
/// each tag. If an override holds a refused tag, the save does not touch
/// `recurrenceOverrides` and does not copy the tag.
fn diff_keywords(
    patch: &mut Map<String, Value>,
    current: &CalendarEvent,
    baseline: &CalendarEvent,
    edited: &CalendarEvent,
) {
    if non_empty_map(baseline.keywords.as_ref()) == non_empty_map(edited.keywords.as_ref()) {
        return;
    }
    let empty = BTreeMap::new();
    let mut wanted: BTreeMap<String, Value> = current
        .keywords
        .as_ref()
        .unwrap_or(&empty)
        .iter()
        .filter(|(tag, set)| !maps_keyword(tag, set))
        .map(|(tag, set)| (tag.clone(), set.clone()))
        .collect();
    wanted.extend(
        edited
            .keywords
            .clone()
            .unwrap_or_default()
            .into_iter()
            .filter(|(tag, _)| !tag.is_empty()),
    );
    patch.insert(
        "keywords".to_owned(),
        if wanted.is_empty() {
            Value::Null
        } else {
            // Serialising a set from this crate's reader cannot fail: it holds
            // strings and values that the server sent.
            serde_json::to_value(wanted).unwrap_or(Value::Null)
        },
    );
}

/// Replaces the reminders of the event whole, like [`diff_keywords`] and unlike
/// [`diff_locations`].
///
/// A `VALARM` states all the content of an RFC 8984 Alert that this mapping
/// carries (its action and its trigger). Thus no entry has data to keep and no key
/// to patch into. To clear the reminders, the patch sends `"alerts": null`, which
/// is RFC 8984 §4.5.2's default of no reminders.
///
/// The function asks [`maps_alerts`] about the server's event, not only about its
/// map. Some alerts were not shown (one at an absolute time, one that sends mail,
/// one acknowledged at no UTC time), and a replacement would delete them. When
/// `useDefaultAlerts` is true, clients ignore the property, so nothing is worth
/// writing. The *edited* side needs no check: each of its alerts comes from a
/// `VALARM` that this crate writes again, key included.
///
/// This applies to the series only. An instance that the user edits alone states
/// its own `VALARM`s, and the override from [`diff_overrides`] carries them. The
/// same `VALARM` rule applies there through [`sends_recurrence_override`], which
/// gets the series so that `useDefaultAlerts` also applies to the reminders of an
/// occurrence.
fn diff_alerts(
    patch: &mut Map<String, Value>,
    current: &CalendarEvent,
    baseline: &CalendarEvent,
    edited: &CalendarEvent,
) {
    if !maps_alerts(current) {
        return;
    }
    if non_empty_map(baseline.alerts.as_ref()) == non_empty_map(edited.alerts.as_ref()) {
        return;
    }
    patch.insert(
        "alerts".to_owned(),
        match non_empty_map(edited.alerts.as_ref()) {
            // Serialising a map from this crate's reader cannot fail: each alert
            // holds two objects of strings.
            Some(alerts) => serde_json::to_value(alerts).unwrap_or(Value::Null),
            None => Value::Null,
        },
    );
}

/// Replaces the recurrence whole or not at all. The function does not touch it
/// when either side holds a rule that the `RRULE` cannot carry: a server rule
/// would become narrower, and the server can refuse a rule from the save.
fn diff_recurrence(
    patch: &mut Map<String, Value>,
    current: &CalendarEvent,
    baseline: &CalendarEvent,
    edited: &CalendarEvent,
) {
    if [current, edited].iter().any(|event| {
        event
            .recurrence_rule
            .iter()
            .any(|rule| !maps_recurrence_rule(rule))
    }) {
        return;
    }
    if baseline.recurrence_rule == edited.recurrence_rule {
        return;
    }
    patch.insert(
        // jscalendarbis §3.3.3: singular `recurrenceRule`, not RFC 8984's
        // plural `recurrenceRules` array.
        "recurrenceRule".to_owned(),
        match &edited.recurrence_rule {
            // Serialising a rule from an `RRULE` cannot fail: it holds strings
            // and numbers.
            Some(rule) => serde_json::to_value(rule).unwrap_or(Value::Null),
            None => Value::Null,
        },
    );
}

/// Replaces the separately named instances whole or not at all. The function
/// does not touch them when either side holds an override that the component
/// cannot carry.
///
/// The main use is the deletion of one occurrence of a recurring event. It reaches
/// EDS as an `EXDATE` on the component and must reach the server as an `excluded`
/// override. The removal of that line is a restore: `"recurrenceOverrides": null`,
/// because the removal of a property restores its default (no named instances).
fn diff_overrides(
    patch: &mut Map<String, Value>,
    current: &CalendarEvent,
    baseline: &CalendarEvent,
    edited: &CalendarEvent,
) {
    // Check the server's overrides as above: the save must not replace an
    // override that the component shows only in part.
    //
    // Check the save's overrides too, for the same reason as the series'
    // `timeZone`. An instance states its zone with a `TZID`, which RFC 8984 §1.4.8
    // does not always accept as a name. The save replaces this property whole, so
    // one entry that the server can refuse costs all edits in the save.
    //
    // Use [`sends_recurrence_override`], not `maps_recurrence_override`, because
    // this save *can* send a zone definition: [`diff_time_zones`] adds a
    // `timeZones` entry for each custom identifier in the patch. Thus §1.4.8
    // accepts an instance in a zone that the document defines. Each side uses its
    // own definitions (the server's for its overrides, the document's for the
    // save's), so each answer is about the correct event.
    if [current, edited].iter().any(|event| {
        event
            .recurrence_overrides
            .iter()
            .flatten()
            .any(|(id, override_patch)| !sends_recurrence_override(event, id, override_patch))
    }) {
        return;
    }
    if non_empty_map(baseline.recurrence_overrides.as_ref())
        == non_empty_map(edited.recurrence_overrides.as_ref())
    {
        return;
    }
    patch.insert(
        "recurrenceOverrides".to_owned(),
        match non_empty_map(edited.recurrence_overrides.as_ref()) {
            // Serialising a map of JSON objects cannot fail.
            Some(overrides) => serde_json::to_value(overrides).unwrap_or(Value::Null),
            None => Value::Null,
        },
    );
}

/// Patches the guest list one guest and one member at a time, and also the
/// organizer.
///
/// The address identifies a guest. The reader gives invented keys (`p1`, `p2`,
/// ...) to the guests on a component, and the server uses its own keys. Thus the
/// function finds each shown address in the server's event and writes the change
/// into that entry: `participants/<key>/<member>` for the members that an
/// `ATTENDEE` line shows. A `sendTo`, an `invitedBy` or a scheduling sequence next
/// to them stays. An address that was not shown is a guest that the user added,
/// under a key that the server does not use. A shown address that the component no
/// longer names is a guest that the user removed. Unlike a conference (see
/// [`diff_virtual_locations`]), a missing line *is* a deletion here: the user edits
/// the guest list in Evolution's meeting editor.
///
/// The function compares roles in the way that Stalwart and Fastmail store them
/// (both tested live). No roles is RFC 8984 §4.4.6's default of `attendee`, and
/// `attendee` next to `owner` is dropped. Thus the organizer that Evolution also
/// lists as a guest matches the server's organizer with only `owner`. Without
/// this, each save would patch the roles back. For the same reason, the function
/// ignores the `ATTENDEE`-only members of a participant whose roles are only
/// `owner`. The component shows such an organizer only as an `ORGANIZER` line, so
/// the values that Evolution writes on the guest line that it adds do not come
/// from the server.
///
/// When the roles change, the patch writes them whole. It keeps each server role
/// that no `ROLE` parameter can state, as [`diff_keywords`] keeps a tag that the
/// `CATEGORIES` line cannot carry.
///
/// The function ignores an address that the server has under more than one key:
/// the component merged them, so no single entry is the one that the user edited.
fn diff_participants(
    patch: &mut Map<String, Value>,
    current: &CalendarEvent,
    baseline: &CalendarEvent,
    edited: &CalendarEvent,
) {
    let empty = BTreeMap::new();
    let held = current.participants.as_ref().unwrap_or(&empty);
    let was = by_address(baseline);
    let now = by_address(edited);

    let mut changes = Map::new();
    let mut removed = BTreeSet::new();
    for (address, before) in &was {
        let Some(key) = servers_key(held, address) else {
            continue;
        };
        match now
            .iter()
            .find(|(other, _)| other.eq_ignore_ascii_case(address))
        {
            Some((_, after)) => diff_participant(&mut changes, key, &held[key], before, after),
            None => {
                removed.insert(key);
                changes.insert(format!("participants/{}", escaped(key)), Value::Null);
            }
        }
    }
    let mut taken: BTreeSet<String> = held.keys().cloned().collect();
    let added: Vec<&Value> = now
        .iter()
        .filter(|(address, _)| {
            !was.iter()
                .any(|(other, _)| other.eq_ignore_ascii_case(address))
        })
        .map(|(_, participant)| *participant)
        .collect();

    if current.participants.is_none() {
        // RFC 8620 §5.3 requires each path segment before the last to exist, so
        // write the full property for the first guests.
        if !added.is_empty() {
            patch.insert(
                "participants".to_owned(),
                // Serialising a map from this crate's reader cannot fail.
                serde_json::to_value(&edited.participants).unwrap_or(Value::Null),
            );
        }
    } else if added.is_empty() && !removed.is_empty() && removed.len() == held.len() {
        // All guests are gone, and the server held no other participant.
        patch.insert("participants".to_owned(), Value::Null);
    } else {
        patch.extend(changes);
        for participant in added {
            let key = (1..)
                .map(|n| format!("p{n}"))
                .find(|key| !taken.contains(key))
                .expect("an unbounded range always has a free key");
            patch.insert(format!("participants/{key}"), participant.clone());
            taken.insert(key);
        }
    }

    let organizer = |event: &CalendarEvent| event.organizer_calendar_address.clone();
    if !organizer(baseline)
        .unwrap_or_default()
        .eq_ignore_ascii_case(&organizer(edited).unwrap_or_default())
    {
        patch.insert(
            "organizerCalendarAddress".to_owned(),
            organizer(edited).map_or(Value::Null, Value::String),
        );
    }
}

/// Writes the changed members of one guest as paths into the server entry `key`.
fn diff_participant(
    changes: &mut Map<String, Value>,
    key: &str,
    held: &Value,
    before: &Value,
    after: &Value,
) {
    let path = |member: &str| format!("participants/{}/{member}", escaped(key));
    let (roles_before, roles_after) = (roles_of(before), roles_of(after));
    if roles_before != roles_after {
        let mut roles: Map<String, Value> = held
            .get("roles")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter(|(role, _)| !spells(role))
            .map(|(role, set)| (role.clone(), set.clone()))
            .collect();
        roles.extend(
            after
                .get("roles")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .map(|(role, set)| (role.clone(), set.clone())),
        );
        changes.insert(path("roles"), Value::Object(roles));
    }
    let attends = roles_after.iter().any(|role| role != OWNER_ROLE);
    for member in ["name", "kind", "participationStatus", "expectReply"] {
        if member != "name" && !attends {
            continue;
        }
        let value = after.get(member);
        if before.get(member) != value {
            changes.insert(path(member), value.cloned().unwrap_or(Value::Null));
        }
    }
}

/// Returns the participants of an event by address, in the order of the event.
fn by_address(event: &CalendarEvent) -> Vec<(&str, &Value)> {
    event
        .participants
        .iter()
        .flatten()
        .filter_map(|(_, participant)| Some((calendar_address(participant)?, participant)))
        .collect()
}

/// Returns the server key for `address`, or `None` if the server has no key or
/// more than one key for it.
fn servers_key<'a>(held: &'a BTreeMap<String, Value>, address: &str) -> Option<&'a str> {
    let mut keys = held.iter().filter(|(_, participant)| {
        calendar_address(participant).is_some_and(|other| other.eq_ignore_ascii_case(address))
    });
    match (keys.next(), keys.next()) {
        (Some((key, _)), None) => Some(key),
        _ => None,
    }
}

/// Returns the roles of a participant in the form that both known servers store:
/// no roles is `attendee`, and `attendee` next to `owner` is removed.
fn roles_of(participant: &Value) -> BTreeSet<String> {
    let mut roles: BTreeSet<String> = participant
        .get("roles")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, set)| *set == &Value::Bool(true))
        .map(|(role, _)| role.clone())
        .collect();
    if roles.is_empty() {
        roles.insert("attendee".to_owned());
    }
    if roles.contains(OWNER_ROLE) {
        roles.remove("attendee");
    }
    roles
}

/// Returns true if the component can state the role: the role of the `ORGANIZER`
/// line, or a role that a `ROLE` parameter states.
fn spells(role: &str) -> bool {
    role == OWNER_ROLE
        || PARTICIPANT_ROLES
            .iter()
            .any(|(spelled, _)| *spelled == role)
}

fn set(patch: &mut Map<String, Value>, property: &str, value: Option<&str>) {
    patch.insert(
        property.to_owned(),
        value.map_or(Value::Null, |value| Value::String(value.to_owned())),
    );
}
