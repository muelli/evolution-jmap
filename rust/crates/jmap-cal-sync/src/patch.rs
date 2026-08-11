// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning an edited component back into a `CalendarEvent/set` PatchObject.
//!
//! The whole point of patching rather than replacing is that a `VEVENT` is a
//! lossy view of a JSCalendar event. The mapping keeps sixteen properties and
//! drops everything else, so a save that sent the parsed event back whole
//! would silently delete what it could not represent — participants, links —
//! neither of which the user ever saw, let alone asked to remove.
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
//! Seven properties need more than the baseline, because for them "no
//! difference" is not the whole question:
//!
//! - **`locations` is a map of places and the component has one line.** So the
//!   name is patched *into* the server's own entry rather than the property
//!   being replaced, which is one of the two places this module reaches below
//!   the top level — see [`diff_locations`].
//! - **`virtualLocations` is a map of places and the component has a line
//!   each.** Which line stands for which entry is therefore a real question,
//!   and the answer rides on the line as an `X-JMAP-KEY`; the members the line
//!   shows are patched into the entry it names, and a line naming an entry the
//!   server does not hold is neither created nor read as a deletion — see
//!   [`diff_virtual_locations`].
//! - **`keywords` is a set, and a set shown in part is not editable.** The
//!   property goes back replaced whole, which is only safe if every tag the
//!   server holds reached the `CATEGORIES` line — see [`diff_keywords`].
//! - **`alerts` is a map replaced whole, and a second property decides
//!   whether anything reads it.** A `VALARM` cannot say that the user already
//!   dismissed the reminder, or that it fires at an absolute instant, or that
//!   it sends mail; and RFC 8984 §4.5.1's `useDefaultAlerts` says the property
//!   is ignored altogether. Either way the reminders were not shown, so they
//!   are not written — see [`diff_alerts`].
//! - **`recurrenceRules` is one property, not a merge point.** A rule with
//!   `rscale` cannot be spelled as an `RRULE` this crate emits, so patching
//!   the array at all would narrow the user's recurrence behind their back.
//!   If any rule the server holds fails [`maps_recurrence_rule`], the
//!   property is left alone entirely — as does one the *save* brings that
//!   cannot be sent, which is the same check the series' `timeZone` gets
//!   below: the property goes out replaced whole, so a part the server is
//!   entitled to reject would cost every other edit in the save.
//! - **`recurrenceOverrides` is the same story one level down.** An
//!   `EXDATE`, an `RDATE` and a `RECURRENCE-ID` component between them say that
//!   an instance is off, that it happens, and that it happens with another
//!   title, start, zone, length, description, status, transparency,
//!   importance, classification, set of tags or set of reminders — but not that
//!   it happens in another place or with another guest list. An
//!   override the component could only place with a bare
//!   `RDATE` would come back as the empty patch, deleting what it could not
//!   draw, so if any override the server holds fails
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

use std::collections::BTreeMap;

use jmap_ical::{
    event_to_ical, ical_to_event, maps_alerts, maps_keywords, maps_locations,
    maps_recurrence_override, maps_recurrence_rule, maps_virtual_locations, names_time_zone,
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
        // Whether the event blocks time. Like `status` it needs nothing beyond
        // the baseline: a value outside RFC 8984 §4.4.2's two states is dropped
        // on both sides of the comparison, so a save that never touched it sends
        // nothing, and clearing it is the `null` that asks for the default — the
        // same state a component with no `TRANSP` on it is in.
        (
            "freeBusyStatus",
            &baseline.free_busy_status,
            &edited.free_busy_status,
        ),
        // How much of the event may be shared. The same shape again, with one
        // thing worth naming: Evolution's appointment editor writes `CLASS` on
        // every save from its Options ▸ Classification menu, so an event the
        // server gave no `privacy` comes back from the editor stating the default
        // explicitly, and the first save of it patches `privacy` to `public`. That
        // is a redundant write, not a wrong one — RFC 8984 §4.4.3 makes `public`
        // and no value the same state — and it happens once: the baseline then
        // renders the line too, so every later save diffs clean.
        ("privacy", &baseline.privacy, &edited.privacy),
    ] {
        if was != now {
            set(&mut patch, property, now.as_deref());
        }
    }

    // How important the event is — an integer rather than a string, and otherwise
    // the `status` shape exactly: a value outside the range RFC 8984 §4.4.1 and
    // RFC 5545 §3.8.1.9 share is dropped on both sides of the comparison, so a
    // save that never touched it sends nothing, and clearing it is the `null` that
    // asks for the default of 0 — the same state a component with no `PRIORITY` on
    // it is in.
    if baseline.priority != edited.priority {
        patch.insert(
            "priority".to_owned(),
            edited.priority.map_or(Value::Null, Value::from),
        );
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

    diff_locations(&mut patch, current, &baseline, edited);
    diff_virtual_locations(&mut patch, current, &baseline, edited);
    diff_keywords(&mut patch, current, &baseline, edited);
    diff_alerts(&mut patch, current, &baseline, edited);
    diff_recurrence(&mut patch, current, &baseline, edited);
    diff_overrides(&mut patch, current, &baseline, edited);
    patch
}

/// The place the event happens at — the one property patched *into* rather than
/// replaced.
///
/// A `LOCATION` is a line of text; a JSCalendar Location also holds coordinates,
/// links, types and a zone, and the event holds a whole map of them. So the name
/// is patched where it stands, `locations/<key>/name`, under the key the server
/// chose: everything the line could not show stays exactly where it was, which
/// is what replacing the property whole could not manage.
///
/// The key is taken from the event the **server** holds rather than from the
/// component. It does ride out and back in `X-JMAP-KEY`, but Evolution's
/// appointment editor writes the `LOCATION` afresh and need not keep a parameter
/// it knows nothing about; the name is what the diff compares, and there is only
/// ever one place on the component to compare.
///
/// [`maps_locations`] is asked of the server's own map first, for the reason the
/// recurrence properties are: a second place has no line to be shown on, and a
/// property shown in part is not the user's to have edited.
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
        // The server's own entry, renamed in place.
        (Some((key, _)), Some(name)) => {
            patch.insert(name_of(key), Value::String(name.to_owned()));
        }
        // The place was cleared. Where the entry said nothing but its name there
        // is nothing left to keep, and `maps_locations` has already ruled out a
        // second place that removing the property would strand; where it said
        // more, only the name the user cleared goes.
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
        // A place the event did not have. RFC 8620 §5.3 requires every path
        // segment before the last to exist on the object already, so the property
        // is written whole rather than reached into.
        (None, Some(_)) => {
            patch.insert(
                "locations".to_owned(),
                // Serialising a map this crate's own reader built cannot fail:
                // it holds one object of two strings.
                serde_json::to_value(&edited.locations).unwrap_or(Value::Null),
            );
        }
        // Both sides name no place, which the comparison above already returned
        // on.
        (None, None) => {}
    }
}

/// The name of the one place a component was shown with, or `None` for a
/// component that named none.
fn drawn_name(event: &CalendarEvent) -> Option<&str> {
    event
        .locations
        .iter()
        .flatten()
        .find_map(|(_, place)| place.get("name")?.as_str())
}

/// The patch path of one place's name.
fn name_of(key: &str) -> String {
    format!("locations/{}/name", escaped(key))
}

/// A map key as a patch path segment. RFC 8620 §5.3 spells one as a JSON
/// pointer segment, so a `~` or a `/` in a key the server chose is escaped
/// (RFC 6901 §3) rather than read as structure.
fn escaped(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// Where the event may be joined online — the second property patched *into*
/// rather than replaced, and for the same reason as [`diff_locations`]: RFC 8984
/// §4.2.6's VirtualLocation holds a `description` that a `CONFERENCE` line has no
/// room for, so naming `virtualLocations` in a patch would delete a note the user
/// was never shown. The save names `virtualLocations/<key>/uri`, `/name` and
/// `/features` — the three members the line does show — one member at a time, and
/// everything beside them stays as the server had it.
///
/// Unlike a `LOCATION` there is a line per entry (RFC 7986 §5.11), so which entry
/// a line stands for is a real question, and the answer rides on the line: the
/// key goes out in an `X-JMAP-KEY` and comes back in one. Position could not
/// answer it — an editor that drops a line it has no UI for would slide every
/// later conference onto the wrong entry.
///
/// Two things this deliberately does **not** do, both because a component that
/// names a conference the server does not is ambiguous in a way that would cost
/// data if read wrong:
///
/// - **A conference the component stopped naming is left where it is.** A missing
///   line does not say who removed it. Evolution 3.52 has no UI for a conference,
///   and whether its editor writes back a property it does not understand is not
///   something this repository can answer without a real Evolution; reading the
///   absence as a deletion would destroy a link the user never touched. The cost
///   of the other reading is only that a deletion made elsewhere comes back on
///   the next sync.
/// - **A conference the server does not hold is not created.** RFC 8620 §5.3
///   requires every path segment before the last to exist already, so an entry
///   the server never chose a key for cannot be patched into place — and a line
///   whose key is unknown is as likely to be a rewrite of one of the server's as
///   a new place. Only the create path, which writes the property whole, files a
///   conference the server has never seen.
///
/// [`maps_virtual_locations`] is asked of the server's own map first, for the
/// reason every conditionally-mapped property asks it: an entry the line could
/// not draw in full was not shown, so no difference from the drawing is the
/// user's to have made.
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
        // The key has to be one the *server* chose, not merely one the baseline
        // carries: a server key outside RFC 8984 §1.4.4's `Id` grammar does not
        // survive the round trip, so the baseline holds an invented key for it,
        // and an invented key names no entry to patch.
        let (Some(after), true) = (now.get(key), places.contains_key(key)) else {
            continue;
        };
        for member in ["uri", "name", "features"] {
            if before.get(member) == after.get(member) {
                continue;
            }
            match after.get(member) {
                Some(value) => {
                    patch.insert(member_of(key, member), value.clone());
                }
                // Removing the member is how a PatchObject asks for the RFC 8984
                // §4.2.6 default — the empty string for `name`, no ways of taking
                // part for `features`. Never for `uri`, which is the one member a
                // VirtualLocation is required to have and the whole of the line:
                // a component that names none is one this crate would not have
                // read a place off at all.
                None if member != "uri" => {
                    patch.insert(member_of(key, member), Value::Null);
                }
                None => {}
            }
        }
    }
}

/// The patch path of one member of one virtual location.
fn member_of(key: &str, member: &str) -> String {
    format!("virtualLocations/{}/{member}", escaped(key))
}

/// The tags the event carries — replaced whole, which is what separates this
/// from [`diff_locations`].
///
/// A `CATEGORIES` line holds a list of TEXT values and a JSCalendar keyword is a
/// bare string, so the whole set fits on the component and there is nothing in an
/// entry to preserve: the baseline says what was shown, and the difference from
/// it is the set the user now wants. Clearing the field is
/// `"keywords": null`, which is how a PatchObject asks for RFC 8984 §4.2.9's
/// default of no tags — an empty map would be a different thing to store.
///
/// [`maps_keywords`] is asked of the server's own set, for the reason the other
/// properties ask it: a tag the `CATEGORIES` line could not carry was never shown,
/// so a set replaced whole would delete it. The *edited* side needs no such check
/// — every tag it holds was read off a content line, and any string is a keyword
/// RFC 8984 admits.
///
/// This is the series' set only. An instance edited on its own states a set of
/// its own on its own component, and that difference rides in the override
/// [`diff_overrides`] sends — under the same [`maps_keywords`] rule, asked by
/// [`maps_recurrence_override`].
fn diff_keywords(
    patch: &mut Map<String, Value>,
    current: &CalendarEvent,
    baseline: &CalendarEvent,
    edited: &CalendarEvent,
) {
    let empty = BTreeMap::new();
    if !maps_keywords(current.keywords.as_ref().unwrap_or(&empty)) {
        return;
    }
    if baseline.keywords == edited.keywords {
        return;
    }
    patch.insert(
        "keywords".to_owned(),
        match &edited.keywords {
            // Serialising a set this crate's own reader built cannot fail: it
            // holds strings and `true`.
            Some(tags) => serde_json::to_value(tags).unwrap_or(Value::Null),
            None => Value::Null,
        },
    );
}

/// The reminders the event carries — replaced whole, like [`diff_keywords`] and
/// unlike [`diff_locations`].
///
/// A `VALARM` states an RFC 8984 Alert's whole content that this mapping carries
/// (its action and when it fires), so there is nothing inside an entry to
/// preserve and no key to patch into: the baseline says which reminders were
/// shown, and the difference from it is the set the user now wants. Clearing them
/// is `"alerts": null`, which is how a PatchObject asks for RFC 8984 §4.5.2's
/// default of no reminders.
///
/// [`maps_alerts`] is asked of the event the **server** holds, and it is asked
/// about more than the map: an alert the `VALARM` could not show — one the user
/// has already dismissed, one that fires at an absolute instant, one that sends
/// mail — was never drawn, so replacing the property would delete it, and an
/// event whose `useDefaultAlerts` says the property is ignored has nothing worth
/// writing there at all. The *edited* side needs no such check: every alert on it
/// was read off a `VALARM` this crate would draw again, key included.
///
/// This is the series' reminders only. An instance edited on its own states its
/// own `VALARM`s on its own component, and that difference rides in the override
/// [`diff_overrides`] sends — under the same rule about what a `VALARM` can show,
/// asked by [`maps_recurrence_override`], which is handed the series so that
/// `useDefaultAlerts` reaches an occurrence's reminders too.
fn diff_alerts(
    patch: &mut Map<String, Value>,
    current: &CalendarEvent,
    baseline: &CalendarEvent,
    edited: &CalendarEvent,
) {
    if !maps_alerts(current) {
        return;
    }
    if baseline.alerts == edited.alerts {
        return;
    }
    patch.insert(
        "alerts".to_owned(),
        match &edited.alerts {
            // Serialising a map this crate's own reader built cannot fail: it
            // holds two objects of strings.
            Some(alerts) => serde_json::to_value(alerts).unwrap_or(Value::Null),
            None => Value::Null,
        },
    );
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
            .any(|(id, override_patch)| !maps_recurrence_override(event, id, override_patch))
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
