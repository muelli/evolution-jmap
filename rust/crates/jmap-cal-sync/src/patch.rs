// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning an edited component back into a `CalendarEvent/set` PatchObject.
//!
//! The whole point of patching rather than replacing is that a `VEVENT` is a
//! lossy view of a JSCalendar event. The mapping keeps seventeen properties and
//! drops everything else, so a save that sent the parsed event back whole
//! would silently delete what it could not represent — the guest list, the
//! sequence number — none of which the user ever saw, let alone asked to
//! remove.
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
//! Eight properties need more than the baseline, because for them "no
//! difference" is not the whole question:
//!
//! - **`locations` is a map of places and the component has one line.** So the
//!   name is patched *into* the server's own entry rather than the property
//!   being replaced, which is one of the two places this module reaches below
//!   the top level — see `diff_locations`.
//! - **`virtualLocations` is a map of places and the component has a line
//!   each.** Which line stands for which entry is therefore a real question,
//!   and the answer rides on the line as an `X-JMAP-KEY`; the members the line
//!   shows are patched into the entry it names, and a line naming an entry the
//!   server does not hold is neither created nor read as a deletion — see
//!   `diff_virtual_locations`.
//! - **`links` is a map of resources, and only one member of an entry is the
//!   user's.** A line shows the address, the media type and the size; the type
//!   and the size are the server's own description of what it holds, and the
//!   `cid` and `title` beside them have no room on the line at all. So the save
//!   patches `links/<key>/href` and nothing else — see `diff_links`.
//! - **`keywords` is a set, and a set has no keys to leave unnamed.** The
//!   property goes back replaced whole, so a tag that never reached the
//!   `CATEGORIES` line is one the user never saw and a plain rewrite would
//!   delete. It is carried onto the set the save writes instead, which is how
//!   the "an unshown entry is not the user's to delete" rule is kept where
//!   leaving an entry unnamed is not available — see `diff_keywords`.
//! - **`alerts` is a map replaced whole, and a second property decides
//!   whether anything reads it.** A `VALARM` cannot say that the user already
//!   dismissed the reminder, or that it fires at an absolute instant, or that
//!   it sends mail; and RFC 8984 §4.5.1's `useDefaultAlerts` says the property
//!   is ignored altogether. Either way the reminders were not shown, so they
//!   are not written — see `diff_alerts`.
//! - **`recurrenceRule` is one property, not a merge point** (jscalendarbis
//!   §3.3.3: singular, not RFC 8984's plural `recurrenceRules` array). A rule
//!   with `rscale` cannot be spelled as an `RRULE` this crate emits, so
//!   patching it at all would narrow the user's recurrence behind their back.
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
//!   [`sends_recurrence_override`], the property is left alone entirely — as it
//!   is when an override the *save* brings holds a value that cannot be sent,
//!   which is the same check the series' `timeZone` gets below.
//! - **`timeZones` is not diffed at all — it is added to.** The map says what
//!   the identifiers the event names mean, and a server's entry may hold an
//!   `url`, a `validUntil` or a set of `aliases` no `VTIMEZONE` has room for. So
//!   it is never compared and never replaced: where the save sends an identifier
//!   the server has no definition for, that one entry is written and no other —
//!   see `diff_time_zones`.
//! - **`start` is required by RFC 8984.** A component whose `DTSTART` the
//!   mapping cannot read yields no start, and `"start": null` is not a legal
//!   way to say so, so the server's start stands.
//!
//! And one property is checked on its way *out* rather than against the
//! baseline: a `TZID` is an iCalendar identifier, which RFC 8984 §1.4.9 admits
//! as a time zone under its own name or beside the definition that says what it
//! is, and otherwise not at all. See [`jmap_ical::maps_time_zone`] and [`diff`].

use std::collections::{BTreeMap, BTreeSet};

use jmap_ical::{
    event_to_ical, ical_to_event, maps_alerts, maps_keyword, maps_locations, maps_recurrence_rule,
    maps_time_zone, maps_virtual_locations, sends_recurrence_override, time_zone_definition,
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
    let baseline_start = baseline.start.as_deref().filter(|s| !s.is_empty());
    let edited_start = edited.start.as_deref().filter(|s| !s.is_empty());
    if edited_start.is_some() && edited_start != baseline_start {
        set(&mut patch, "start", edited_start);
    }
    // `timeZone` is the one property whose *new* value may be unsendable. A
    // component states its zone with a `TZID`, which is an iCalendar identifier
    // and not the RFC 8984 §1.4.9 name JSCalendar wants; where the document
    // gave no way to translate it, the zone is left alone rather than sent as
    // it came or cleared. It is the same "seen in part, so not written back"
    // rule the recurrence properties follow, applied to one value.
    //
    // §1.4.9's second form — the custom identifier — is sendable too, and by the
    // same `jmap_ical::maps_time_zone` the create path uses: legal only beside
    // the `timeZones` entry defining it, which [`diff_time_zones`] adds below,
    // one entry at a time so that nothing the server already holds is written
    // over. What is left out is the identifier the document neither names nor
    // defines — a Windows zone off an Exchange invitation, or a solidus-prefixed
    // one with no `VTIMEZONE` beside it — which is left alone rather than sent as
    // it came or cleared.
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
        let was_norm = was.as_deref().filter(|s| !s.is_empty());
        let now_norm = now.as_deref().filter(|s| !s.is_empty());
        if was_norm != now_norm {
            set(&mut patch, property, now_norm);
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
    diff_links(&mut patch, current, &baseline, edited);
    diff_keywords(&mut patch, current, &baseline, edited);
    diff_alerts(&mut patch, current, &baseline, edited);
    diff_recurrence(&mut patch, current, &baseline, edited);
    diff_overrides(&mut patch, current, &baseline, edited);
    // Last, because it reads the patch the others wrote.
    diff_time_zones(&mut patch, current, edited);
    patch
}

/// The definitions the identifiers already in `patch` need, added beside them.
///
/// RFC 8984 §1.4.9's second form of a `TimeZoneId` — the solidus-prefixed one a
/// server or an Exchange organiser invents for a zone no database names — means
/// nothing on its own: it is legal only where the event's §4.7.2 `timeZones` map
/// says what it is. So a save that sends one has to send the other, or the
/// identifier reaches the server dangling and the whole `CalendarEvent/set` may
/// be refused over it. Everything above may therefore write such an identifier
/// on the understanding that this runs afterwards; the patch it wrote is what
/// says which zones are wanted, which is why it is read from there rather than
/// off the event — the same value in `edited` that never made it into the patch
/// is a zone this save is not sending.
///
/// **One entry at a time**, `timeZones/<pointer>`, not the property replaced
/// whole: a `VTIMEZONE` has no room for the `aliases`, `url` or `validUntil` a
/// server's own definition may carry, so a rewrite would delete what the user
/// was never shown — the rule this whole module exists to keep. For the same
/// reason an identifier the server *already* defines is left completely alone:
/// the entry the document brought is a drawing of a zone the server has already
/// described, and describing it again can only lose. Only where `timeZones` is
/// absent from the server's event altogether is the property written whole, and
/// then only because RFC 8620 §5.3 requires every path segment before the last
/// to exist already — there is nothing there to overwrite.
///
/// What is *not* done is pruning: a definition the edit stopped referring to
/// stays where it is. The create path drops those ([`jmap_ical::prune_time_zones`]),
/// having built the whole map itself; here the map is the server's, an
/// unreferenced entry is legal, and removing one is deleting something on a
/// guess.
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
        // The zone the series moved into, or the one each override moved its
        // instance into. Told apart by shape rather than by the name above:
        // `timeZone` is a string and `recurrenceOverrides` a map of patches, so
        // the two readings cannot be applied to the wrong property. Either may
        // be the `null` that removes the property, which names nothing and falls
        // out of both.
        let named: BTreeSet<String> = match value.as_object() {
            Some(overrides) => overrides
                .values()
                .filter_map(|instance| custom_zone(instance.get("timeZone")))
                .collect(),
            None => custom_zone(Some(value)).into_iter().collect(),
        };
        // The document defined every identifier it let into the patch — that is
        // what admitted it — so these lookups answer, and this is the guard for
        // the day one of them does not. The property naming a zone the save
        // cannot define goes back to being the server's, which is the same
        // answer the checks above give a zone the document could not name at
        // all; leaving it in the patch is the one outcome that would be worse
        // than not sending the edit, since a reference to nothing is grounds to
        // refuse the whole `CalendarEvent/set`.
        if named
            .iter()
            .any(|tzid| time_zone_definition(edited, tzid).is_none())
        {
            patch.remove(property);
            continue;
        }
        for tzid in named {
            // Already described where the patch is going: leave it exactly as
            // it stands.
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

/// The custom `TimeZoneId` a `timeZone` states, where it states one.
///
/// Only RFC 8984 §1.4.9's solidus-prefixed form, because that is the only one
/// that needs saying: an IANA name resolves against a database every reader has,
/// and a `null` is the zone cleared, which names nothing.
fn custom_zone(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|tzid| tzid.starts_with('/'))
        .map(str::to_owned)
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
        .find_map(|(_, place)| place.get("name")?.as_str().filter(|name| !name.is_empty()))
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

/// What the event points at — the third property patched *into* rather than
/// replaced, and the narrowest of the three: only the address goes back.
///
/// A Link (RFC 8984 §1.4.11) holds a `cid` and a `title` that neither an `ATTACH`
/// nor an `IMAGE` line has room for, so naming `links` in the patch would delete
/// half of every resource the user was never shown. The save names
/// `links/<key>/href` under the key the line was drawn with, and everything
/// beside it stays as the server had it.
///
/// `contentType` and `size` are shown on the line — an `FMTTYPE` and a `SIZE` —
/// and are still not written back, which is the one place this property differs
/// from [`diff_virtual_locations`]. They are the *server's* description of the
/// resource rather than a field the user was offered (§1.4.11 calls the size an
/// estimate), and an editor that rewrites a line without the parameters it has no
/// UI for is the ordinary case: reading that as "the media type was cleared"
/// would delete what the server knows on the first save of an unrelated edit. So
/// they are drawn to be read and left alone on the way back, like `created` and
/// `updated`. Neither is `rel`, which the property name states rather than the
/// line: what a resource *is* is not something Evolution offers to change, and a
/// picture rewritten as a plain attachment by an editor with no notion of `IMAGE`
/// must not turn into a save that says so.
///
/// The two cautions [`diff_virtual_locations`] takes apply unchanged, for the same
/// reasons — a resource the component stopped naming is left where it is, and one
/// the server does not hold is not created — and the second binds harder here: a
/// file the user attached in Evolution is a `file:` URI into a local store, which
/// is nobody else's to fetch and which `jmap_ical` therefore never reads. Filing
/// it as a Link would put a path from the user's home directory in a record every
/// other client of the account can read. Sending the file itself means uploading
/// it as a blob, which this crate does not do.
///
/// The condition is asked per entry rather than per property, which is what the
/// narrowness buys: a patch of one entry's `href` cannot touch another, so a
/// resource the drawing left out costs the sight of itself and nothing else. What
/// it must be is the entry the drawing really came from, and the address the
/// server stated is what says so — see [`the_servers_own_entry`].
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
        // `href` is the one member RFC 8984 §1.4.11 requires and the whole of the
        // line, so a component that named none is one `jmap_ical` would not have
        // read a resource off at all — there is no removal to express here.
        if let Some(href) = after.get("href") {
            patch.insert(format!("links/{}/href", escaped(key)), href.clone());
        }
    }
}

/// Whether the key of a drawn resource is the key of the entry the server drew it
/// from — the check that makes `links/<key>/href` safe to send.
///
/// A key alone does not settle it. [`diff_virtual_locations`] asks whether the
/// server holds the key at all, because a key outside RFC 8984 §1.4.4's `Id`
/// grammar cannot ride on the line and reads back as an invented one; here that
/// is not enough, since an invented key can *collide* with a key the server holds
/// for some other entry. (`jmap_ical` invents keys that avoid the ones the
/// document names, so the entry collided with is one the drawing left out — one
/// with no address to show, or a `file:` URI it refuses to read.) Patching that
/// entry's `href` would move a resource the user never saw and lose the edit they
/// made.
///
/// So the address decides: the baseline is the server's own entry drawn and read
/// back, and `href` crosses both ways unchanged, so the entry a drawing belongs to
/// is the one stating the address it was drawn with. Where they disagree the edit
/// is dropped — a key this side invented is not one it can patch under, exactly as
/// a conference's is not.
fn the_servers_own_entry(held: &BTreeMap<String, Value>, key: &str, before: &Value) -> bool {
    held.get(key).and_then(|resource| resource.get("href")) == before.get("href")
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
/// [`maps_keyword`] is asked of the server's own set, for the reason the other
/// properties ask their predicate: a tag the `CATEGORIES` line could not carry
/// was never shown, so its absence from the edited component is not the user
/// asking for it to go. Where a keyed map answers that by leaving the entry
/// unnamed in the patch, a set has no key to leave alone — so the tag is carried
/// onto the set the save writes instead. That is the same rule reached by the
/// only means a set allows, and it is why an unstatable tag costs the sight of it
/// and nothing more; the edit around it still lands.
///
/// The carried tags are read off the event the **server** holds rather than off
/// the baseline, which is the one place this property cannot use the baseline:
/// the baseline is what was *shown*, and these are precisely the tags it does not
/// hold. The baseline still answers the other question — whether the user changed
/// anything — because a difference from what they were shown is what an edit is.
///
/// A tag is carried back exactly as the server stated it, value included — even
/// the value RFC 8984 §1.4.3 does not admit, because the server is the one who
/// said it and rewriting it here would be this mapping inventing a change. The
/// user's own set wins where the two name the same tag: a tag they typed is a tag
/// they mean to be set, whatever the server had against that name.
///
/// The *edited* side needs no such check — every tag it holds was read off a
/// content line, and any string is a keyword RFC 8984 admits.
///
/// This is the series' set only. An instance edited on its own states a set of
/// its own on its own component, and that difference rides in the override
/// [`diff_overrides`] sends — where the same [`maps_keyword`] is asked by
/// [`maps_recurrence_override`] of every restated tag, and an override holding
/// one it refuses is left alone whole rather than carried back: an override is
/// itself an entry of a keyed map, so there the patch has a key to leave unnamed.
fn non_empty_map(map: Option<&BTreeMap<String, Value>>) -> Option<&BTreeMap<String, Value>> {
    map.filter(|m| !m.is_empty())
}

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
            // Serialising a set this crate's own reader built cannot fail: it
            // holds strings and values the server itself sent.
            serde_json::to_value(wanted).unwrap_or(Value::Null)
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
    if non_empty_map(baseline.alerts.as_ref()) == non_empty_map(edited.alerts.as_ref()) {
        return;
    }
    patch.insert(
        "alerts".to_owned(),
        match non_empty_map(edited.alerts.as_ref()) {
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
            // Serialising a rule built from an RRULE cannot fail: it holds
            // strings and numbers.
            Some(rule) => serde_json::to_value(rule).unwrap_or(Value::Null),
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
    //
    // [`sends_recurrence_override`] rather than `maps_recurrence_override`,
    // because this save *can* send a zone's definition: [`diff_time_zones`] adds
    // a `timeZones` entry for every custom identifier the patch names, so an
    // instance moved into a zone the document defines is one of the identifiers
    // §1.4.9 admits rather than a dangling reference. Each side is asked of its
    // own definitions — the server's for what it holds, the document's for what
    // the save brings — which is what makes both answers about the event they
    // came from.
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
