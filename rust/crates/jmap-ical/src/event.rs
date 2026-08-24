// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSCalendar [`CalendarEvent`] ↔ iCalendar `VEVENT`.
//!
//! The mapped set is the one the calendar backend needs to be useful — UID,
//! SUMMARY, DESCRIPTION, DTSTART (with its time zone, or as a `VALUE=DATE` when
//! the event is shown without a time), DURATION, STATUS, TRANSP, PRIORITY,
//! CLASS, LOCATION, CATEGORIES, RRULE, the `VALARM`s that remind the user of the
//! event, the external resources it points at, and the instances an EXDATE, an
//! RDATE or a `RECURRENCE-ID` component names one at a time — and no more.
//! Everything else on an event is *dropped*,
//! which is only safe because saving goes back to the server as a PatchObject
//! naming the mapped properties: a property we never mapped is a property we
//! never overwrite. See [`MAPPED_PROPERTIES`], [`maps_locations`],
//! [`maps_keyword`], [`maps_alerts`], [`maps_recurrence_rule`] and
//! [`maps_recurrence_override`], which are that knowledge in machine-readable
//! form.
//!
//! A `VCALENDAR` here therefore holds more than one `VEVENT` whenever an
//! instance of a recurring event was edited on its own: the series first, then
//! one component per edited instance, each carrying the series' `UID` and the
//! `RECURRENCE-ID` of the occurrence it stands in for. That is also the shape
//! `ECalMetaBackend` stores and hands back to a save.
//!
//! The one property read but never written is `DTEND`: it is how Evolution
//! states an event's length, so `read_duration` measures it, while the length
//! goes back out as the `DURATION` the two formats share. Both directions pass
//! through `stated_duration`, because the two formats spell an ISO 8601
//! duration identically but do not admit the same set of them.
//!
//! Its mirror is `CREATED`, `DTSTAMP` and `LAST-MODIFIED`, which are written and
//! never read: RFC 8984 §4.1.7's `created` and §4.1.8's `updated` are the
//! *server's* record of the event, so they are drawn for whoever reads the
//! document and never appear in [`MAPPED_PROPERTIES`]. Reading them back would
//! be this side proposing a value — and, since libical stamps a `DTSTAMP` of its
//! own onto every component that arrives without one, the value proposed would
//! be the local clock.
//!
//! Where the event may be joined online — RFC 8984 §4.2.6's `virtualLocations`
//! — is a `CONFERENCE` line each (RFC 7986 §5.11), and the second property read
//! back *into* rather than as a whole. A VirtualLocation holds a `description`
//! the line has no room for, so a save that named the property would delete the
//! part of a place that was never shown; instead the key of the entry a line was
//! drawn from rides on it in an `X_JMAP_KEY`, and the save patches
//! `virtualLocations/<key>/uri`. See `drawn_conferences`,
//! `read_virtual_locations` and [`maps_virtual_locations`].
//!
//! What the event points at — RFC 8984 §4.2.7's `links` — is the third property
//! read back *into* rather than as a whole, and crosses as two properties rather
//! than one: a document attached to the event is an `ATTACH` (RFC 5545
//! §3.8.1.1), a picture *of* it is an `IMAGE` (RFC 7986 §5.10), and the
//! `ICON_REL` relation is what tells them apart. A Link holds a `cid` and a
//! `title` no line has room for, so a save naming the property would delete the
//! half of every resource the user was never shown; the key of the entry a line
//! was drawn from rides on it in an `X_JMAP_KEY` instead, and the save patches
//! `links/<key>/href`. See `drawn_links` and `read_links`.
//!
//! `ORGANIZER` and `ATTENDEE` are written and never read for a heavier reason:
//! who is invited, and what each of them replied, is *scheduling* state. Moving
//! it means an iTIP REQUEST or REPLY going out to those people (RFC 5546), which
//! this backend does not send — so the guest list RFC 8984 §4.4.6 states in
//! `participants` is drawn for the user to read, and a save can never name the
//! property. See `drawn_participants`.
//!
//! A time zone crosses under two different kinds of name: iCalendar refers to
//! one by a `TZID`, which is an identifier the document itself may define, and
//! JSCalendar wants the zone's IANA name. [`names_time_zone`] says which is
//! which and `zone_names` does the translating, off the `VTIMEZONE` the
//! document is required to carry. In the other direction the identifier a
//! zone *has no name* under is the one this crate has to define rather than
//! refer to, out of the event's own RFC 8984 §4.7.2 `timeZones` — see
//! `drawn_time_zone`. That definition crosses both ways: `read_time_zones`
//! reads it back off the `VTIMEZONE`, which is what lets a document whose zone
//! came from somewhere other than a database — an Exchange invitation, another
//! client's `.ics` — be saved as the event it is rather than as a floating one.
//! See [`maps_time_zone`], which is the question the save path asks.
//!
//! The one place a zone is needed and no database is at hand is a recurrence's
//! `UNTIL`: RFC 5545 §3.3.10 states it as a UTC instant beside a zoned
//! `DTSTART` where RFC 8984 §4.3.3 wants a local time in the event's own zone,
//! and the two differ by the offset in force at that instant. The document
//! answers that itself wherever it carries the `VTIMEZONE` §3.6.5 says defines
//! the zone — see the `zone` module — and a rule whose offset neither the
//! document nor this crate knows is read as unmappable rather than shifted, so
//! that the save path leaves the server's own rule alone. See `read_until`.
//! A `VTIMEZONE` observance's own `UNTIL` is the same shape one level down and
//! *is* converted, because an observance dates itself in the zone it defines:
//! the offset is the fixed one `TZOFFSETFROM` states in the same component,
//! not a zone whose rules have to be evaluated. See `Ends`.
//!
//! An all-day event has no property of its own in iCalendar; it is a `DTSTART`
//! written as a date rather than a date-time, which puts JSCalendar's
//! `showWithoutTime` in the value type of three properties at once. See
//! `shows_without_time` for the conditions that has to meet and what happens
//! when it cannot.
//!
//! Nothing here fails on unrecognised input. A property whose value the
//! mapping cannot read is treated as absent, because an event that loses a
//! field is better than a calendar that refuses to open; only a document
//! without any `VEVENT` in it is an error.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use calcard::common::{IanaParse, IanaString, PartialDateTime};
use calcard::icalendar::{
    ICalendar, ICalendarComponent, ICalendarComponentType, ICalendarEntry, ICalendarParameter,
    ICalendarParameterName, ICalendarParameterValue, ICalendarProperty, ICalendarValue,
    ICalendarValueType, Uri,
};
use calcard::{Entry, Parser};
use jmap_proto::calendars::{CalendarEvent, NDay, RecurrenceRule};
use serde_json::{Map, Value, json};

use crate::error::ICalError;

/// Carries the JSCalendar `uid` when the iCalendar `UID` is taken by the JMAP
/// id, mirroring `X-JMAP-UID` on the address book side.
const X_JMAP_UID: &str = "X-JMAP-UID";

/// Carries the key of the `locations` entry a `LOCATION` was drawn from, so
/// that a save patches that entry in place instead of replacing the property —
/// the same parameter, for the same reason, as the JSContact map keys on the
/// address book side.
const X_JMAP_KEY: &str = "X-JMAP-KEY";

/// The `locations` key for a `LOCATION` that carries none: the place is one
/// Evolution's appointment editor just typed, and no entry exists server-side
/// for it to name.
const INVENTED_KEY: &str = "l1";

/// The stem of the `virtualLocations` key invented for a `CONFERENCE` that
/// carries no [`X_JMAP_KEY`] — another client's line, or one an editor wrote
/// afresh. Numbered from 1, skipping the keys the document already named, so
/// that two conferences cannot collapse into one entry of a map.
const INVENTED_CONFERENCE_KEY: &str = "v";

/// The stem of the `links` key invented for an `ATTACH` or an `IMAGE` that
/// carries no [`X_JMAP_KEY`]. Numbered like [`INVENTED_CONFERENCE_KEY`] and
/// under a letter of its own, so that a key in a document says which map it was
/// invented for.
const INVENTED_LINK_KEY: &str = "k";

/// The stem of the `alerts` key invented for a `VALARM` that names no id of its
/// own — a reminder Evolution's editor has just added, which carries an
/// `X-EVOLUTION-ALARM-UID` and no RFC 9074 `UID`. Numbered from 1 in the order
/// the component holds them; see [`read_alerts`] for why positional keys are
/// what keeps a save from rewriting the property every time.
const INVENTED_ALERT_KEY: &str = "a";

/// The one `Alert.action` (RFC 8984 §4.5.2) this mapping carries, and its
/// iCalendar `ACTION` spelling (RFC 5545 §3.8.6.1).
///
/// RFC 8984 admits `email` beside it, and RFC 5545 admits `AUDIO` and the
/// deprecated `PROCEDURE`. Neither crosses: an `ACTION:EMAIL` alarm is required
/// to carry a `SUMMARY` and an `ATTENDEE` (§3.6.6) that a JSCalendar Alert has
/// nothing to fill in from, and a sound or a program is a reminder RFC 8984 has
/// no `action` for at all. So an alert this mapping cannot spell is left off the
/// document and [`maps_alerts`] refuses the property, and an alarm it cannot
/// read is dropped like every other unreadable value.
const DISPLAY_ALERT: (&str, &str) = ("display", "DISPLAY");

/// The `PRODID` of every calendar this crate emits.
const PRODID: &str = "-//evolution-jmap//JMAP calendar backend//EN";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Component {
    component_type: ICalendarComponentType,
    entries: Vec<ICalendarEntry>,
    children: Vec<Component>,
}

impl Component {
    pub(crate) fn new(name: &str) -> Self {
        let component_type = ICalendarComponentType::parse(name.as_bytes())
            .unwrap_or_else(|| ICalendarComponentType::Other(name.to_ascii_uppercase()));
        Self {
            component_type,
            entries: Vec::new(),
            children: Vec::new(),
        }
    }

    pub(crate) fn with(mut self, entry: ICalendarEntry) -> Self {
        self.entries.push(entry);
        self
    }

    fn with_child(mut self, child: Component) -> Self {
        self.children.push(child);
        self
    }

    fn write_into(&self, out: &mut String) {
        write!(out, "BEGIN:{}\r\n", self.component_type.as_str()).unwrap();
        for entry in &self.entries {
            entry.write_to(out).unwrap();
        }
        for child in &self.children {
            child.write_into(out);
        }
        write!(out, "END:{}\r\n", self.component_type.as_str()).unwrap();
    }

    pub(crate) fn to_ics(&self) -> String {
        let mut out = String::new();
        self.write_into(&mut out);
        out
    }
}

pub(crate) fn make_entry(name: &str, value: &str) -> ICalendarEntry {
    let prop = ICalendarProperty::parse(name.as_bytes())
        .unwrap_or_else(|| ICalendarProperty::Other(name.to_ascii_uppercase()));
    ICalendarEntry::new(prop).with_value(ICalendarValue::Text(value.to_owned()))
}

pub(crate) trait EntryExt {
    fn with_named_param(self, name: &str, value: &str) -> Self;
    fn with_named_params<I, S>(self, name: &str, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>;
}

impl EntryExt for ICalendarEntry {
    fn with_named_param(self, name: &str, value: &str) -> Self {
        self.with_named_params(name, [value])
    }

    fn with_named_params<I, S>(mut self, name: &str, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let param_name = ICalendarParameterName::try_parse(name.as_bytes())
            .unwrap_or_else(|| ICalendarParameterName::Other(name.to_ascii_uppercase()));
        for val in values {
            self.params.push(ICalendarParameter::new(
                param_name.clone(),
                ICalendarParameterValue::Text(val.as_ref().to_owned()),
            ));
        }
        self
    }
}

fn rrule_entry(rrule_str: &str) -> Option<ICalendarEntry> {
    let raw = format!("BEGIN:VEVENT\r\nRRULE:{rrule_str}\r\nEND:VEVENT\r\n");
    let mut parser = Parser::new(&raw);
    let Entry::ICalendar(mut calendar) = parser.entry() else {
        return None;
    };
    let mut component = calendar.components.pop()?;
    let idx = component
        .entries
        .iter()
        .position(|entry| entry.name == ICalendarProperty::Rrule)?;
    Some(component.entries.swap_remove(idx))
}

/// The JSCalendar spelling of `Etc/UTC`, the one the client and the mock use.
const UTC: &str = "Etc/UTC";

/// libical's record, on the `VTIMEZONE` it builds, of which IANA zone that
/// component describes. It is not standard, but it is universal in this
/// neighbourhood: libical writes it for every builtin zone, and libical is
/// what puts the components Evolution saves together in the first place.
const X_LIC_LOCATION: &str = "X-LIC-LOCATION";

/// The property that ties an edited instance to the series it belongs to: the
/// start of the occurrence it stands in for (RFC 5545 §3.8.4.4), which is
/// exactly the key of a JSCalendar `recurrenceOverrides` entry.
const RECURRENCE_ID: &str = "RECURRENCE-ID";

/// The weekdays a `BYDAY` names, in their iCalendar spelling (RFC 5545
/// §3.3.10). JSCalendar's `day` (RFC 8984 §4.3.3) is the same closed set in
/// lowercase, so the two differ only in case — and a value outside it is
/// dropped rather than passed through in the other format's clothes.
pub(crate) const WEEKDAYS: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

/// JSCalendar `status` values and their iCalendar `STATUS` spelling. Both sets
/// are closed, so a value outside this table is dropped rather than passed
/// through in the other format's clothes.
const STATUSES: [(&str, &str); 3] = [
    ("confirmed", "CONFIRMED"),
    ("cancelled", "CANCELLED"),
    ("tentative", "TENTATIVE"),
];

/// JSCalendar `freeBusyStatus` values (RFC 8984 §4.4.2) and their iCalendar
/// `TRANSP` spelling (RFC 5545 §3.8.2.7) — whether the event blocks the time it
/// occupies, which is Evolution's "Show Time as". Both sets are closed, so a
/// value outside this table is dropped rather than passed through in the other
/// format's clothes.
///
/// The two also agree about what a *missing* value means: RFC 8984 defaults the
/// property to `busy` and RFC 5545 defaults `TRANSP` to `OPAQUE`, which is the
/// same state. So a component with no line on it says exactly what an event with
/// no property does, and neither direction has to invent one.
const FREE_BUSY_STATUSES: [(&str, &str); 2] = [("free", "TRANSPARENT"), ("busy", "OPAQUE")];

/// The importances both formats admit: RFC 8984 §4.4.1's `priority` and RFC 5545
/// §3.8.1.9's `PRIORITY` are the same integer with the same meaning — 0
/// undefined, 1 highest, 9 lowest — so this range crosses digit for digit and a
/// value outside it is dropped, as a value outside a closed vocabulary is.
///
/// The two also agree that 0 and no value at all are the same state. That does
/// *not* make 0 something to leave off: an event whose `priority` the server
/// states as 0 is written `PRIORITY:0` and read straight back, so the round trip
/// is the identity and the save path has nothing to explain. What it does mean is
/// that clearing the field — `"priority": null` — and setting it to 0 ask a server
/// for the same thing.
const PRIORITIES: std::ops::RangeInclusive<i64> = 0..=9;

/// JSCalendar `privacy` values (RFC 8984 §4.4.3) and their iCalendar `CLASS`
/// spelling (RFC 5545 §3.8.1.3) — how much of the event may be shared with other
/// calendar users, which is Evolution's Options ▸ Classification.
///
/// The same three-step scale in both formats, in the same order: everything may
/// be shared, only the time may be, nothing may — so each value crosses to the
/// other format's spelling of itself. Neither vocabulary is *closed* (RFC 5545
/// admits an x-name or an iana-token, RFC 8984 a registered or vendor value) and
/// neither says how a value in one becomes a value in the other, so a value
/// outside this table is dropped rather than passed through in the other format's
/// clothes.
///
/// The two also agree about what a *missing* value means: both default to public.
/// That does not make public something to leave off — see [`PRIVACIES`]'s reader
/// [`read_privacy`] and the writer in [`vevent_of`], where an event the server
/// states as public is written `CLASS:PUBLIC` and read straight back. Evolution's
/// appointment editor sets `CLASS` on *every* save from its Classification menu,
/// so a baseline rendered without the line would differ from what EDS hands back
/// on every save of such an event rather than once.
const PRIVACIES: [(&str, &str); 3] = [
    ("public", "PUBLIC"),
    ("private", "PRIVATE"),
    ("secret", "CONFIDENTIAL"),
];

/// The RFC 8984 §4.4.6 `sendTo` method whose URI is what iCalendar puts on an
/// `ATTENDEE`: iMIP (RFC 6047) is scheduling by mail, and its address is the
/// `mailto:` an RFC 5545 §3.3.3 CAL-ADDRESS is in practice. A participant
/// reachable only some other way — a web form, a phone number — has no line to
/// go on, so it is left off.
const IMIP: &str = "imip";

/// The role that makes a participant the event's organizer. RFC 8984 §4.4.6 has
/// no `organizer` property: the owner of the event is a participant like any
/// other, holding this role, where RFC 5545 §3.8.4.3 states it on a line of its
/// own.
const OWNER_ROLE: &str = "owner";

/// What a participant has replied: RFC 8984 §4.4.6's `participationStatus` and
/// RFC 5545 §3.2.12's `PARTSTAT`, which for a `VEVENT` admit the same five
/// answers under the same names. A value outside them is dropped rather than
/// passed through in the other format's clothes.
const PARTICIPATION_STATUSES: [(&str, &str); 5] = [
    ("needs-action", "NEEDS-ACTION"),
    ("accepted", "ACCEPTED"),
    ("declined", "DECLINED"),
    ("tentative", "TENTATIVE"),
    ("delegated", "DELEGATED"),
];

/// The parts a participant plays that iCalendar has a `ROLE` for (RFC 8984
/// §4.4.6, RFC 5545 §3.2.16), **in precedence order**: RFC 8984's `roles` is a
/// set and a `ROLE` parameter is one value, so a guest who is both an attendee
/// and an optional one is written as the optional one — the narrower statement
/// is the one the user needs, and `attendee` is iCalendar's default anyway.
///
/// `owner` is not here: it is the [`ORGANIZER`](OWNER_ROLE) line rather than a
/// role on the guest list.
const PARTICIPANT_ROLES: [(&str, &str); 4] = [
    ("chair", "CHAIR"),
    ("informational", "NON-PARTICIPANT"),
    ("optional", "OPT-PARTICIPANT"),
    ("attendee", "REQ-PARTICIPANT"),
];

/// The ways of taking part in an event held online: RFC 8984 §4.2.6's
/// `features` and RFC 7986 §6.3's `FEATURE` parameter, which name the same seven
/// things in the same words and differ only in case. So each crosses to the
/// other format's spelling of itself, and a value outside the table is dropped
/// rather than passed through in the other format's clothes.
///
/// In this order on the line, whatever order the Set holds them in, so that a
/// re-rendering is stable — the save path diffs against a re-rendering of what
/// the server holds.
const CONFERENCE_FEATURES: [(&str, &str); 7] = [
    ("audio", "AUDIO"),
    ("chat", "CHAT"),
    ("feed", "FEED"),
    ("moderator", "MODERATOR"),
    ("phone", "PHONE"),
    ("screen", "SCREEN"),
    ("video", "VIDEO"),
];

/// The `rel` (RFC 8288) that makes a link a picture of the event rather than a
/// document attached to it — RFC 8984 §1.4.11 lets `display` be set only when
/// the relation is this one, and it is what sends a link to `IMAGE` (RFC 7986
/// §5.10) instead of `ATTACH`.
const ICON_REL: &str = "icon";

/// What a picture of the event is for: RFC 8984 §1.4.11's `display` and RFC 7986
/// §6.1's `DISPLAY` parameter, which name the same four intentions in the same
/// words and differ only in case — another crossing where nothing is lost.
///
/// A value outside the table is dropped rather than passed through in the other
/// format's clothes, and dropping it is the *safe* direction here: §6.1 requires
/// a reader that meets a `DISPLAY` it does not know to show no image at all,
/// where the absent parameter means its default of `BADGE`.
const LINK_DISPLAYS: [(&str, &str); 4] = [
    ("badge", "BADGE"),
    ("graphic", "GRAPHIC"),
    ("fullsize", "FULLSIZE"),
    ("thumbnail", "THUMBNAIL"),
];

/// The characters RFC 6838 §4.2 admits in a media type's name after the first,
/// which is what RFC 5545 §3.2.8's `FMTTYPE` is made of. Everything outside
/// them — a `;` that would start another parameter, a `:` that would end them
/// all, a space, a line break — makes the type unwritable.
const RESTRICTED_NAME_CHARS: [char; 9] = ['!', '#', '$', '&', '-', '^', '_', '.', '+'];

/// What sort of participant it is: RFC 8984 §4.4.6's `kind` and RFC 5545
/// §3.2.3's `CUTYPE`. The two vocabularies say the same four things and differ
/// in one word — a JSCalendar `location` is iCalendar's `ROOM`. iCalendar's
/// `UNKNOWN` is not written: an event that says nothing about a participant's
/// kind gets no parameter, which is the same state.
const PARTICIPANT_KINDS: [(&str, &str); 4] = [
    ("individual", "INDIVIDUAL"),
    ("group", "GROUP"),
    ("resource", "RESOURCE"),
    ("location", "ROOM"),
];

/// The JSCalendar properties this mapping covers, and therefore the only ones
/// a save may name in a `CalendarEvent/set` update patch.
///
/// Five are covered *conditionally* — see [`maps_locations`],
/// [`maps_virtual_locations`], [`maps_alerts`], [`maps_recurrence_rule`] and
/// [`maps_recurrence_override`], which say when a save may name them.
///
/// `locations`, `virtualLocations` and `links` are also the three properties
/// named *into* rather than replaced: a save patches `locations/<key>/name`,
/// `virtualLocations/<key>/uri` and `links/<key>/href`, so the rest of the entry
/// stays. See `X_JMAP_KEY`, which is how a line says which entry it was drawn
/// from. `links` is conditional too, per entry rather than per property — see
/// `jmap_cal_sync::patch`, which is where the condition lives, since it is about
/// which entry a patch path can reach.
pub const MAPPED_PROPERTIES: [&str; 17] = [
    "title",
    "description",
    "start",
    "timeZone",
    "duration",
    "showWithoutTime",
    "status",
    "freeBusyStatus",
    "priority",
    "privacy",
    "locations",
    "virtualLocations",
    "links",
    "keywords",
    "alerts",
    "recurrenceRules",
    "recurrenceOverrides",
];

/// The event properties an edited instance may restate, and therefore the only
/// keys — besides `excluded` — a `recurrenceOverrides` PatchObject may hold for
/// [`maps_recurrence_override`] to call it covered.
///
/// `timeZone` is here because iCalendar states a zone per property rather than
/// per document: the instance's own `DTSTART` carries its own `TZID`, so an
/// occurrence the user moved into another zone has a spelling of its own, and
/// one with no `TZID` at all is the floating instance a null asks for.
///
/// `keywords` is here because a `CATEGORIES` line is drawn whole and the
/// instance has one of its own, so the tags an occurrence is filed under are
/// stated where that occurrence is. `locations` is *not*, for the opposite
/// reason: it is shown in part and patched into (see [`maps_locations`]), and an
/// override's PatchObject would have to reach `locations/<key>/name` inside an
/// entry the instance does not have.
///
/// `alerts` is here for the same reason as `keywords`, one component down: an
/// instance's `VALARM`s are drawn whole on its own component, so the reminders one
/// occurrence has are stated where that occurrence is. It is also the only
/// restated property whose coverage the *series* decides — RFC 8984 §4.5.1's
/// `useDefaultAlerts` is not itself restatable, so it holds for every instance,
/// and an occurrence whose reminders nothing reads has none to write; see
/// [`maps_recurrence_override`], which is asked of the event for that one reason.
///
/// `showWithoutTime` is absent, one step further out — see
/// `shows_without_time`, which is decided once for the whole document.
pub const OVERRIDE_PROPERTIES: [&str; 11] = [
    "title",
    "description",
    "start",
    "timeZone",
    "duration",
    "status",
    "freeBusyStatus",
    "priority",
    "privacy",
    "keywords",
    "alerts",
];

/// Whether the places an event happens at survive the trip through iCalendar
/// well enough for a save to name the property.
///
/// A JSCalendar event holds a *map* of Locations (RFC 8984 §4.2.5), each with a
/// `description`, `coordinates`, `links`, `locationTypes` and a `timeZone`
/// besides its `name`; RFC 5545 §3.6.1 gives a `VEVENT` one `LOCATION` line of
/// text. So only the name of one place is ever shown — and the save path
/// answers that not by refusing to write, but by patching
/// `locations/<key>/name` and leaving the entry otherwise untouched. What it
/// needs from here is whether that path is safe to walk:
///
/// - **More than one place** cannot be shown, so the user was never given the
///   chance to edit the second; a save must not act on a field that stands for
///   only part of the property.
/// - **An entry that is not an object** has no `name` member to patch into.
///   RFC 8620 §5.3 makes patching *through* a non-object an error, and a
///   rejected `CalendarEvent/set` costs every other edit in the same save.
/// - **A `name` that is not a string** is a place the user cannot see, so a
///   patch naming it would overwrite what was never shown.
/// - **An empty key** names no member of the map at all. Any other key is
///   carried: `~` and `/` have RFC 6901 escapes, which the save path applies.
///
/// An event with no places at all passes: there is nothing to lose, and a
/// `LOCATION` the user just typed is a place to create.
pub fn maps_locations(locations: &BTreeMap<String, Value>) -> bool {
    let mut entries = locations.iter();
    let Some((key, location)) = entries.next() else {
        return true;
    };
    entries.next().is_none()
        && !key.is_empty()
        && location.is_object()
        && matches!(location.get("name"), None | Some(Value::String(_)))
}

/// Whether the places an event may be joined online survive the trip through
/// iCalendar well enough for a save to name the property.
///
/// The same rule as [`maps_locations`] and it answers differently, because
/// `CONFERENCE` is a better-matched property than `LOCATION`: RFC 7986 §5.11
/// admits it more than once, so a map of several places is several lines and a
/// second entry costs nothing. What a save may not do is send a difference from
/// a drawing that left something out — an entry with nowhere to join, a way of
/// taking part outside RFC 7986 §6.3's vocabulary, a name no `LABEL` can carry,
/// or a key no patch path can name.
///
/// A `description` is emphatically not such a thing. It has no room on the line
/// and is *why* the property is patched into rather than replaced: the save
/// names `virtualLocations/<key>/uri` and the members beside it are untouched.
pub fn maps_virtual_locations(locations: &BTreeMap<String, Value>) -> bool {
    locations.iter().all(|(key, location)| {
        !key.is_empty()
            && drawn_conference(key, location).is_some()
            && matches!(location.get("name"), None | Some(Value::String(_)))
            && match location.get("features") {
                None => true,
                Some(Value::Object(features)) => features.iter().all(|(feature, held)| {
                    held == &Value::Bool(true)
                        && CONFERENCE_FEATURES
                            .iter()
                            .any(|(jscalendar, _)| jscalendar == feature)
                }),
                Some(_) => false,
            }
    })
}

/// The place a component shows: the key of the entry it comes from, and the
/// name to write on the `LOCATION` line.
///
/// The first entry that has a name, in the map's own order, so a document is
/// stable across renderings — the save path diffs against a re-rendering of what
/// the server holds. Where there is more than one, [`maps_locations`] has
/// already said the property must not be written back; drawing the first is
/// still better than showing an event as happening nowhere.
fn drawn_place(event: &CalendarEvent) -> Option<(&String, &str)> {
    event
        .locations
        .iter()
        .flatten()
        .find_map(|(key, location)| Some((key, place_name(location)?)))
}

/// The `name` of one Location, or `None` when it has none this mapping can put
/// on a content line — no name, one that is not text, or an empty one, which
/// would write a `LOCATION` saying nothing.
fn place_name(location: &Value) -> Option<&str> {
    location
        .get("name")?
        .as_str()
        .filter(|name| !name.is_empty())
}

/// Whether one tag of `keywords` goes on the `CATEGORIES` line, and so whether
/// the user ever saw it.
///
/// This is the one mapped property that is a *set* rather than a scalar or a map
/// of objects: RFC 5545 §3.8.1.2's `CATEGORIES` is a list of TEXT values and an
/// RFC 8984 §4.2.9 keyword is a bare string, so there is nothing inside an entry
/// to preserve and no key to patch by — the property goes back replaced whole,
/// unlike `locations`, which is patched into.
///
/// Being replaced whole is what makes this question a per-*tag* one. A tag the
/// line could not carry is not merely unseen but absent from what the user
/// edited, so the save has to write it back by hand rather than read its absence
/// as a deletion; asking about the set as a whole could only answer "then write
/// nothing at all", which drops the edit the user did make. What is refused:
///
/// - **A value that is not `true`.** RFC 8984 §1.4.3 has every value of a Set be
///   `true`; drawing anything else would say the tag is set where the server said
///   it is not.
/// - **An empty, or whitespace-only, tag.** An empty part of a value list reads
///   back as nothing at all, so the tag would vanish between the drawing and the
///   save; a whitespace-only one fares no better, because drawing it writes an
///   unescaped `CATEGORIES: ` line and [`read_keywords`] reads that bare
///   whitespace back as nothing too (calcard's own parser trims it), so it would
///   vanish just as surely, one round trip later.
/// - **A tag holding a carriage return.** It is dropped on its way onto a content
///   line — see `syntax::fold_into`, where that is a security property and not
///   tidiness — so the tag would come back spelled differently and a save would
///   rename it. A line feed is not this case: it has an escape and survives.
///
/// The single point the save and `drawn_tags` agree through, so a tag cannot be
/// called shown and then left off the line.
pub fn maps_keyword(tag: &str, set: &Value) -> bool {
    set == &Value::Bool(true) && !tag.trim().is_empty() && !tag.contains('\r')
}

/// The tags to write on the `CATEGORIES` line, in the order the set holds them —
/// which is sorted, so a document is stable across renderings; the save path
/// diffs against a re-rendering of what the server holds.
fn drawn_tags(event: &CalendarEvent) -> Vec<&str> {
    event
        .keywords
        .iter()
        .flatten()
        .filter(|(tag, set)| maps_keyword(tag, set))
        .map(|(tag, _)| tag.as_str())
        .collect()
}

/// Whether the reminders an event carries survive the trip through iCalendar,
/// and so whether a save may name `alerts`.
///
/// This is the first mapped property drawn as a *child component* rather than as
/// a content line: RFC 8984 §4.5.2's Alerts become the `VALARM`s of RFC 5545
/// §3.6.6. Like `keywords` and unlike `locations` the property goes back
/// **replaced whole** — a `VALARM` has no key of its own for a PatchObject to
/// reach into beyond the RFC 9074 §6 `UID` the entry's key rides on — so, exactly
/// as there, an alert this mapping leaves off the document is an alert the next
/// save deletes. What `drawn_alert` refuses, this refuses:
///
/// - **An `action` other than `display`.** An `ACTION:EMAIL` alarm must carry a
///   `SUMMARY` and an `ATTENDEE` (RFC 5545 §3.6.6) that a JSCalendar Alert does
///   not hold — see `DISPLAY_ALERT`.
/// - **A trigger that is not an offset** — RFC 8984 §4.5.4's AbsoluteTrigger, or
///   an offset that is no signed duration (`stated_offset`), or a `relativeTo`
///   outside the two §4.5.3 admits.
/// - **Anything else on the alert or its trigger**, most importantly RFC 9074
///   §6.1's `acknowledged`: a reminder the user has already dismissed, which the
///   `VALARM` this writes cannot say, and which a property replaced whole would
///   therefore un-dismiss.
/// - **A key no `UID` can carry back**, since the key is what a replaced map
///   states its entries under; see `names_map_entry`.
///
/// Taken as the whole event rather than one entry, unlike [`maps_keyword`],
/// because a second property decides this one: RFC 8984 §4.5.1's
/// `useDefaultAlerts` says the `alerts` property is *ignored* and the user's own
/// default reminders fire instead. Drawing them would show reminders that never
/// go off, and patching the property would edit what nothing reads — so such an
/// event is drawn without alarms and the property is never written. What it would
/// take to honour a reminder the user adds to such an event is a save that clears
/// `useDefaultAlerts` in the same patch, which this mapping does not do.
///
/// An event with no alerts at all passes: there is nothing to lose, and a
/// `VALARM` the user just added is a reminder to create.
pub fn maps_alerts(event: &CalendarEvent) -> bool {
    !uses_default_alerts(event)
        && event
            .alerts
            .iter()
            .flatten()
            .all(|(key, alert)| drawn_alert(key, alert, None).is_some())
}

/// Whether the event says its reminders are the user's own defaults rather than
/// the ones it carries — RFC 8984 §4.5.1, read out of [`CalendarEvent::extra`]
/// because the property is not modeled: nothing here writes it, and the only
/// question asked of it is whether it is `true`.
fn uses_default_alerts(event: &CalendarEvent) -> bool {
    event.extra.get("useDefaultAlerts") == Some(&Value::Bool(true))
}

/// One entry of `alerts` as the `VALARM` that states it, or `None` for a reminder
/// this mapping cannot put on the document. The single point [`maps_alerts`] and
/// [`drawn_alarms`] agree through, so an alert cannot be called covered and then
/// left off.
///
/// `summary` is the event's own title, which is the only text a DISPLAY alarm has
/// to show: RFC 5545 §3.6.6 requires a `DESCRIPTION` on one and RFC 8984 gives an
/// Alert no message of its own. An event with no title writes no `DESCRIPTION`
/// either — there is nothing to put there, and the reminder still fires — so the
/// text puts no condition on whether the alert is covered, and [`maps_alerts`]
/// asks without it.
fn drawn_alert(key: &str, alert: &Value, summary: Option<&str>) -> Option<Component> {
    let alert = alert.as_object()?;
    if !names_map_entry(key) {
        return None;
    }
    // Every member of the object has to be one of the three drawn, because the
    // property goes back replaced whole: what is not on the document is what the
    // next save removes.
    if !alert
        .keys()
        .all(|member| matches!(member.as_str(), "@type" | "trigger" | "action"))
    {
        return None;
    }
    if !is_type(alert.get("@type"), "Alert") {
        return None;
    }
    let (jscalendar, ical) = DISPLAY_ALERT;
    if !alert
        .get("action")
        .is_none_or(|action| action.as_str().is_some_and(|action| action == jscalendar))
    {
        return None;
    }

    let (offset, related) = drawn_trigger(alert.get("trigger")?)?;
    let mut valarm = Component::new("VALARM")
        // RFC 9074 §6 gives a VALARM a UID, which is where the key of the entry
        // rides so that a save states the reminder the server holds under the
        // name the server gave it.
        .with(make_entry("UID", key))
        .with(make_entry("ACTION", ical))
        .with(make_entry("TRIGGER", &offset).with_named_params("RELATED", related));
    if let Some(summary) = summary.filter(|summary| !summary.is_empty()) {
        valarm = valarm.with(make_entry("DESCRIPTION", summary));
    }
    Some(valarm)
}

/// When one alert fires, as the value of a `TRIGGER` and the `RELATED` parameter
/// it needs — none for the start, which both formats default to.
///
/// Only RFC 8984 §4.5.3's OffsetTrigger. §4.5.4's AbsoluteTrigger states a
/// `when` as a UTCDateTime, which iCalendar spells as a `TRIGGER;VALUE=DATE-TIME`
/// and this mapping does not write in either direction yet; it is refused rather
/// than approximated, since an offset guessed from an instant would move the
/// reminder as soon as the event moved.
fn drawn_trigger(trigger: &Value) -> Option<(String, Vec<&'static str>)> {
    let trigger = trigger.as_object()?;
    if !trigger
        .keys()
        .all(|member| matches!(member.as_str(), "@type" | "offset" | "relativeTo"))
    {
        return None;
    }
    if !is_type(trigger.get("@type"), "OffsetTrigger") {
        return None;
    }
    let offset = stated_offset(trigger.get("offset")?.as_str()?)?;
    // The default said out loud is still the default, so `start` adds no
    // parameter; a value outside the two is a reminder this mapping would have to
    // guess the moment of, which is how a reminder ends up firing at the wrong
    // time.
    let related = match trigger.get("relativeTo") {
        None => Vec::new(),
        Some(value) => match value.as_str()? {
            "start" => Vec::new(),
            "end" => vec!["END"],
            _ => return None,
        },
    };
    Some((offset, related))
}

/// Whether an `@type` member is absent — RFC 8984 §1.4.1 makes it optional where
/// the type is implied by the property — or names the type it should.
fn is_type(value: Option<&Value>, name: &str) -> bool {
    value.is_none_or(|value| value.as_str() == Some(name))
}

/// The `VALARM`s to draw beside an event, one per reminder the document can
/// carry, in the order the map holds them — which is sorted, so a re-rendering is
/// stable; the save path diffs against a re-rendering of what the server holds.
fn drawn_alarms(event: &CalendarEvent) -> Vec<Component> {
    if uses_default_alerts(event) {
        return Vec::new();
    }
    event
        .alerts
        .iter()
        .flatten()
        .filter_map(|(key, alert)| drawn_alert(key, alert, event.title.as_deref()))
        .collect()
}

/// The guest list, as the lines a `VEVENT` states it on: an `ORGANIZER` for the
/// participant that owns the event, and an `ATTENDEE` for everyone attending it.
///
/// In the map's own order, so a document is stable across renderings. Only the
/// *first* owner gets an `ORGANIZER`: RFC 8984 §4.4.6 admits several — the role
/// is a set member like any other — where RFC 5545 §3.6.1 admits one line, and
/// showing the first is better than showing an event nobody called.
///
/// A participant whose only role this mapping can spell is `owner` gets the
/// `ORGANIZER` line alone: it called the meeting without coming to it, and an
/// `ATTENDEE` would say it is on the guest list. One that holds another role
/// besides — the usual shape of a meeting somebody called and comes to — gets
/// both, because iCalendar states the organizer beside the guest list rather
/// than instead of it.
fn drawn_participants(event: &CalendarEvent) -> Vec<ICalendarEntry> {
    let mut lines = Vec::new();
    let mut organizer_drawn = false;
    for participant in event.participants.iter().flatten().map(|(_, value)| value) {
        let Some(address) = calendar_address(participant) else {
            continue;
        };
        let owns = holds_role(participant, OWNER_ROLE);
        let role = spelled(&PARTICIPANT_ROLES, participant.get("roles"));
        if owns && !organizer_drawn {
            organizer_drawn = true;
            lines.push(
                make_entry("ORGANIZER", address).with_named_params("CN", stated_name(participant)),
            );
        }
        if owns && role.is_none() {
            continue;
        }
        lines.push(
            make_entry("ATTENDEE", address)
                .with_named_params("CN", stated_name(participant))
                .with_named_params(
                    "CUTYPE",
                    spelled(&PARTICIPANT_KINDS, participant.get("kind")),
                )
                .with_named_params("ROLE", role)
                .with_named_params(
                    "PARTSTAT",
                    spelled(
                        &PARTICIPATION_STATUSES,
                        participant.get("participationStatus"),
                    ),
                )
                .with_named_params("RSVP", expects_reply(participant).then_some("TRUE")),
        );
    }
    lines
}

/// The places the event may be joined online at, as the `CONFERENCE` lines a
/// `VEVENT` states them on — one per entry the document can carry, in the map's
/// own order so that a document is stable across renderings.
///
/// Unlike `LOCATION`, which RFC 5545 §3.6.1 allows once and this mapping
/// therefore shows one of (see [`drawn_place`]), RFC 7986 §5.11 states that the
/// property "can be specified multiple times", so a map of several places needs
/// nothing left out and nothing patched in place.
fn drawn_conferences(event: &CalendarEvent) -> Vec<ICalendarEntry> {
    event
        .virtual_locations
        .iter()
        .flatten()
        .filter_map(|(key, location)| drawn_conference(key, location))
        .collect()
}

/// One `CONFERENCE`, or `None` for a virtual location no line can name.
///
/// `VALUE=URI` is written because RFC 7986 §5.11's `confparam` makes it
/// REQUIRED — the one parameter in this mapping that says nothing the default
/// would not, and a reader that trusts the grammar is entitled to demand it.
///
/// [`X_JMAP_KEY`] rides along for the reason it does on a `LOCATION`: an edit
/// goes back as a patch of `virtualLocations/<key>`, so the line has to say
/// which entry of the server's map it is a drawing of. Position could not do
/// that job — an editor that drops a line it has no UI for would slide every
/// later conference onto the wrong entry.
///
/// The `uri` is the whole of the line, so a value that is not a URI leaves
/// nothing to write: RFC 8984 §4.2.6 makes it the one mandatory member of a
/// VirtualLocation, and a place with none is dropped rather than guessed at.
/// Such an entry is one the drawing left out, which is what
/// [`maps_virtual_locations`] refuses a save over.
fn drawn_conference(key: &str, location: &Value) -> Option<ICalendarEntry> {
    let uri = location
        .get("uri")?
        .as_str()
        .filter(|uri| names_a_uri(uri))?;
    Some(
        make_entry("CONFERENCE", uri)
            .with_named_param("VALUE", "URI")
            .with_named_params("FEATURE", joining_features(location))
            .with_named_params("LABEL", stated_name(location))
            .with_named_param(X_JMAP_KEY, key),
    )
}

/// The ways of taking part this place offers, in [`CONFERENCE_FEATURES`] order.
/// The values of a set are `true` (RFC 8984 §1.4.3); anything else says nothing
/// was set.
fn joining_features(location: &Value) -> Vec<&'static str> {
    let features = location.get("features");
    CONFERENCE_FEATURES
        .iter()
        .filter(|(jscalendar, _)| {
            features.and_then(|features| features.get(jscalendar)) == Some(&Value::Bool(true))
        })
        .map(|(_, ical)| *ical)
        .collect()
}

/// The external resources the event points at, as the lines a `VEVENT` states
/// them on — one per entry the document can carry, in the map's own order so
/// that a document is stable across renderings.
///
/// Two properties, because RFC 8984 §4.2.7 keeps in one map what iCalendar
/// splits in two: a document attached to the event is RFC 5545 §3.8.1.1's
/// `ATTACH`, and a *picture of* the event is RFC 7986 §5.10's `IMAGE`.
/// [`ICON_REL`] is what tells them apart, since it is the relation RFC 8984
/// §1.4.11 attaches `display` to. Both admit being stated more than once, so —
/// as with `CONFERENCE`, and unlike `LOCATION` — nothing is left out.
fn drawn_links(event: &CalendarEvent) -> Vec<ICalendarEntry> {
    event
        .links
        .iter()
        .flatten()
        .filter_map(|(key, link)| drawn_link(key, link))
        .collect()
}

/// One `ATTACH` or `IMAGE`, or `None` for a link no line can name.
///
/// The `href` is the whole of the line, so a value that is not a URI leaves
/// nothing to write: RFC 8984 §1.4.11 makes it the one mandatory member of a
/// Link, and a resource with no address is dropped rather than guessed at. The
/// media type and the size are informational (§1.4.11 calls the size an
/// estimate), so one this mapping cannot spell costs the parameter and not the
/// resource — the user can still open what the line points at.
///
/// An [`X_JMAP_KEY`] rides along for the reason it does on a `CONFERENCE`: an
/// edit goes back as a patch of `links/<key>/href`, so the line has to say which
/// entry of the server's map it is a drawing of. Position could not do that job —
/// an editor that drops a line it has no URI for would slide every later
/// resource onto the wrong entry.
fn drawn_link(key: &str, link: &Value) -> Option<ICalendarEntry> {
    let href = link
        .get("href")?
        .as_str()
        .filter(|href| names_a_uri(href))?;
    let media_type = media_type(link);
    if link.get("rel").and_then(Value::as_str) == Some(ICON_REL) {
        // `VALUE=URI` because RFC 7986 §5.10's `image` grammar makes it REQUIRED
        // on the URI alternative — the same demand §5.11 makes of a
        // `CONFERENCE`, and the reason both write a parameter that says only
        // what the default already says.
        return Some(
            make_entry("IMAGE", href)
                .with_named_param("VALUE", "URI")
                .with_named_params("DISPLAY", spelled(&LINK_DISPLAYS, link.get("display")))
                .with_named_params("FMTTYPE", media_type)
                .with_named_param(X_JMAP_KEY, key),
        );
    }
    // No `VALUE`: RFC 5545 §3.8.1.1 already makes `URI` the default value type
    // of an `ATTACH`, and nothing in its grammar demands the parameter be
    // stated. Also no `DISPLAY` — RFC 7986 §6.1 admits it on `IMAGE` alone, so a
    // link that asked to be displayed without saying it is an icon is taken at
    // its `rel`.
    Some(
        make_entry("ATTACH", href)
            .with_named_params("FMTTYPE", media_type)
            .with_named_params("SIZE", stated_size(link))
            .with_named_param(X_JMAP_KEY, key),
    )
}

/// The media type of a linked resource, or `None` when no `FMTTYPE` can carry
/// it.
///
/// RFC 5545 §3.2.8's `fmttypeparam` is a type-name, a `/` and a subtype-name,
/// each an RFC 6838 §4.2 restricted-name — and nothing else, so a type carrying
/// media-type parameters of its own (`text/plain; charset=utf-8`) has no
/// spelling here. Checking the grammar rather than trusting the server is also
/// what keeps a `;` or a `:` out of a parameter value, and a CR or an LF out of
/// the line.
fn media_type(link: &Value) -> Option<&str> {
    let media_type = link.get("contentType")?.as_str()?;
    let (name, subtype) = media_type.split_once('/')?;
    [name, subtype]
        .iter()
        .all(|part| restricted_name(part))
        .then_some(media_type)
}

/// Whether a string is an RFC 6838 §4.2 restricted-name: an alphanumeric, then
/// any of the alphanumerics and [`RESTRICTED_NAME_CHARS`]. The length limit the
/// production also states is not checked — a name of 200 characters is odd, not
/// dangerous, and refusing it would drop a type a reader would have understood.
fn restricted_name(name: &str) -> bool {
    name.starts_with(|first: char| first.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || RESTRICTED_NAME_CHARS.contains(&c))
}

/// The size of a linked resource in octets, as RFC 8607 §4.1's `SIZE` states it,
/// or `None` when the server named none this mapping can write.
///
/// RFC 8984 §1.4.11 makes `size` an UnsignedInt — the octets the user would
/// download — so a negative number, a fraction or a string is not one, and
/// stating it anyway would put a value outside §4.1's `1*DIGIT` on the line.
fn stated_size(link: &Value) -> Option<String> {
    Some(link.get("size")?.as_u64()?.to_string())
}

/// The address to reach one participant at, or `None` for one no `ATTENDEE`
/// line can name.
///
/// A CAL-ADDRESS is a URI (RFC 5545 §3.3.3), so a value that is not one is left
/// off rather than written: there is nothing to invent in its place, and a line
/// libical refuses costs every other field of the event with it. That is only
/// safe because the guest list is written and never read back — see
/// [`read_vevent`], where `participants` stays `None`.
fn calendar_address(participant: &Value) -> Option<&str> {
    participant
        .get("sendTo")?
        .get(IMIP)?
        .as_str()
        .filter(|address| names_a_uri(address))
}

/// Whether a value is a URI this mapping will put on a content line: RFC 3986
/// §3.1's scheme, a `:`, and something after it.
///
/// Whitespace is refused along with the rest — no URI holds any — which is also
/// what keeps a CR or an LF out of a value that skips [`syntax::escape`]. That
/// is belt and braces: `syntax::fold_into` drops both on the way out, for
/// exactly this reason.
fn names_a_uri(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once(':') else {
        return false;
    };
    !rest.is_empty()
        && !value.chars().any(char::is_whitespace)
        && scheme.starts_with(|first: char| first.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|part| part.is_ascii_alphanumeric() || matches!(part, '+' | '-' | '.'))
}

/// The `name` an object states, or `None` where it has none to put in a `CN` or
/// a `LABEL`.
///
/// A name that is empty counts as none: RFC 8984 §4.2.6 defaults a
/// VirtualLocation's to the empty string, and a parameter carrying it would say
/// the place is named nothing, where leaving it off says only that the value
/// speaks for itself.
fn stated_name(value: &Value) -> Option<&str> {
    value.get("name")?.as_str().filter(|name| !name.is_empty())
}

/// Whether a participant states this RFC 8984 §4.4.6 role. The values of a set
/// are `true` (RFC 8984 §1.4.3); anything else says nothing was set.
fn holds_role(participant: &Value, role: &str) -> bool {
    participant
        .get("roles")
        .and_then(|roles| roles.get(role))
        .is_some_and(|held| held == &Value::Bool(true))
}

/// Whether a reply is expected of a participant — RFC 8984 §4.4.6's
/// `expectReply`, which is RFC 5545 §3.2.17's `RSVP`. Both default to false, so
/// only a stated `true` is written.
fn expects_reply(participant: &Value) -> bool {
    participant.get("expectReply") == Some(&Value::Bool(true))
}

/// The iCalendar spelling one of this mapping's tables gives a JSCalendar value,
/// or `None` for a value it does not know.
///
/// Two shapes of value reach here: a string, which is looked up as itself, and a
/// Set, whose *first* member the table names wins — which is what makes
/// [`PARTICIPANT_ROLES`] an order and not just a list.
fn spelled(table: &[(&str, &'static str)], value: Option<&Value>) -> Option<&'static str> {
    let stated = |name: &str| match value? {
        Value::String(value) => value.eq_ignore_ascii_case(name).then_some(()),
        set => (set.get(name) == Some(&Value::Bool(true))).then_some(()),
    };
    table
        .iter()
        .find(|(jscalendar, _)| stated(jscalendar).is_some())
        .map(|(_, ical)| *ical)
}

/// A stated offset as a JSCalendar SignedDuration (RFC 8984 §1.4.7), or `None`
/// when the value states no offset a reminder can have.
///
/// The signed sibling of [`stated_duration`], and the same trade: the two formats
/// spell a duration identically, so a value that is one is handed over as
/// written. What differs is the sign — RFC 5545 §3.8.6.3's `TRIGGER` is a
/// *negative* duration for the usual case of a reminder *before* the event, which
/// RFC 8984 §1.4.6's Duration has no room for and §1.4.7's SignedDuration is
/// there for.
fn stated_offset(value: &str) -> Option<String> {
    match value.strip_prefix('-') {
        Some(magnitude) => stated_duration(magnitude).map(|duration| format!("-{duration}")),
        None => stated_duration(value),
    }
}

/// The reminders the component carries, as a JSCalendar `alerts` map.
///
/// Keyed by each `VALARM`'s RFC 9074 §6 `UID` where it has one this mapping would
/// draw back — that is the key the entry had server-side, so a round trip is the
/// identity. Evolution's appointment editor writes an `X-EVOLUTION-ALARM-UID`
/// instead, so a reminder the user has just added arrives with no key at all; the
/// ones invented for those are positional (`a1`, `a2`, …), skipping any a `UID`
/// already claimed so that two reminders cannot collapse into one entry.
///
/// Positional is what makes them *stable*, which is what a save needs: reading
/// the same component twice yields the same map, so an editor that strips the
/// `UID` costs one re-keying of the property and not one per save. Two `VALARM`s
/// naming the *same* `UID` do collapse — a map has one entry per key, and RFC 9074
/// §6 asks for the id to be unique — which loses a reminder no `alerts` map could
/// have held either.
///
/// `None` rather than an empty map for a component with no readable alarm, for the
/// reason [`read_locations`] gives: the save path reads an edit off a difference
/// from what was shown, and an empty map would claim the event reminds nobody
/// where the component made no claim at all.
fn read_alerts(
    vevent: &ICalendarComponent,
    components: &[ICalendarComponent],
) -> Option<BTreeMap<String, Value>> {
    let mut alerts: BTreeMap<String, Value> = BTreeMap::new();
    let mut nameless: Vec<Value> = Vec::new();
    for valarm in vevent
        .component_ids
        .iter()
        .filter_map(|id| components.get(*id as usize))
        .filter(|child| child.component_type.as_str().eq_ignore_ascii_case("VALARM"))
    {
        let Some(alert) = read_alert(valarm) else {
            continue;
        };
        match component_text(valarm, "UID").filter(|uid| names_map_entry(uid)) {
            Some(uid) => {
                alerts.insert(uid, alert);
            }
            None => nameless.push(alert),
        }
    }
    let mut n = 0;
    for alert in nameless {
        let key = loop {
            n += 1;
            let key = format!("{INVENTED_ALERT_KEY}{n}");
            if !alerts.contains_key(&key) {
                break key;
            }
        };
        alerts.insert(key, alert);
    }
    (!alerts.is_empty()).then_some(alerts)
}

/// One `VALARM` as a JSCalendar Alert, or `None` for an alarm this mapping has
/// nothing to read it as — a sound, a program, a mail, or a trigger that is not
/// an offset. Dropped rather than guessed at, like every other unreadable value:
/// an alarm Evolution wrote as a sound is a reminder RFC 8984 has no `action`
/// for, so there is nothing to send even in principle.
///
/// The `action` and the trigger's `@type` are written out rather than left to
/// their defaults, and the `relativeTo` of a reminder before the start is left
/// off: both directions have to agree about which, or a save would read its own
/// rendering as an edit. What decides is [`drawn_alert`], which reads back exactly
/// this shape.
fn read_alert(valarm: &ICalendarComponent) -> Option<Value> {
    let (jscalendar, ical) = DISPLAY_ALERT;
    if !component_text(valarm, "ACTION")?.eq_ignore_ascii_case(ical) {
        return None;
    }
    let property = component_entry(valarm, "TRIGGER")?;
    let offset = stated_offset(&entry_raw_value(property))?;
    let mut trigger = Map::from_iter([
        ("@type".to_owned(), json!("OffsetTrigger")),
        ("offset".to_owned(), json!(offset)),
    ]);
    // RFC 5545 §3.2.14 defaults `RELATED` to the start, and so does RFC 8984
    // §4.5.3's `relativeTo`, so only the end is stated. A value that is neither
    // is an alarm firing at a moment this mapping cannot name.
    match entry_param(property, "RELATED") {
        None => {}
        Some(related) if related.eq_ignore_ascii_case("START") => {}
        Some(related) if related.eq_ignore_ascii_case("END") => {
            trigger.insert("relativeTo".to_owned(), json!("end"));
        }
        Some(_) => return None,
    }
    Some(json!({
        "@type": "Alert",
        "trigger": Value::Object(trigger),
        "action": jscalendar,
    }))
}

/// Whether a recurrence rule survives the trip through iCalendar.
///
/// Every `BYxxx` part of RFC 8984 §4.3.3 is modeled, along with `frequency`,
/// `interval`, `count`, `until` and `firstDayOfWeek`; `rscale` and `skip` — which
/// count a rule in a calendar other than the Gregorian one — ride in
/// [`RecurrenceRule::extra`] and would be lost. A caller that patches
/// `recurrenceRules` for a rule this returns `false` for narrows the user's
/// recurrence behind their back.
///
/// A rule `rule_to_rrule` refuses outright fails this too, so the save path
/// never patches over a recurrence the user was not shown — as does one whose
/// days of the week, days of the month, days of the year, weeks of the year,
/// months of the year, time of day, position in the set or day the week starts on
/// the `RRULE` cannot carry, which `by_day_part`, `by_month_day_part`,
/// `by_year_day_part`, `by_week_no_part`, `by_month_part`,
/// `by_second_part`, `by_minute_part`, `by_hour_part`,
/// `by_set_position_part` and `weekday_token` decide and `rule_to_rrule`
/// then leaves off.
pub fn maps_recurrence_rule(rule: &RecurrenceRule) -> bool {
    rule.extra.is_empty()
        && writable(rule)
        && rule
            .by_day
            .as_ref()
            .is_none_or(|_| by_day_part(rule).is_some())
        && rule
            .by_month_day
            .as_ref()
            .is_none_or(|_| by_month_day_part(rule).is_some())
        && rule
            .by_year_day
            .as_ref()
            .is_none_or(|_| by_year_day_part(rule).is_some())
        && rule
            .by_week_no
            .as_ref()
            .is_none_or(|_| by_week_no_part(rule).is_some())
        && rule
            .by_month
            .as_ref()
            .is_none_or(|_| by_month_part(rule).is_some())
        && rule
            .by_second
            .as_ref()
            .is_none_or(|_| by_second_part(rule).is_some())
        && rule
            .by_minute
            .as_ref()
            .is_none_or(|_| by_minute_part(rule).is_some())
        && rule
            .by_hour
            .as_ref()
            .is_none_or(|_| by_hour_part(rule).is_some())
        // Asked with the same answer [`rule_to_rrule`] computes, because this
        // part is writable only beside another one that was written.
        && rule
            .by_set_position
            .as_ref()
            .is_none_or(|_| by_set_position_part(rule, !named_by_parts(rule).is_empty()).is_some())
        // Asked of the value rather than of the part, because this is the one
        // part whose absence from the `RRULE` is not a refusal: the default day
        // is left off deliberately — see [`first_day_of_week_part`].
        && rule
            .first_day_of_week
            .as_deref()
            .is_none_or(|day| weekday_token(day).is_some())
}

/// The end a rule states that this mapping could not turn into RFC 8984
/// §4.3.3's local time, and so kept as it stood — `None` for a rule whose end
/// converted, and for one that never had an end at all.
///
/// [`maps_recurrence_rule`] says only *whether* a rule can be written back. A
/// save that refuses a create over it has to tell the user which appointment to
/// change, and this is the one refusal there is anything actionable to say
/// about: the value is a UTC instant (RFC 5545 §3.3.10's spelling wherever
/// `DTSTART` names a zone) that the document gave no way of resolving, because
/// it names a zone it does not define or defines it in a shape this crate's
/// zone evaluator will not guess at. So the caller can name the instant and the
/// zone rather
/// than say that something, somewhere, did not map. See `jmap_cal_sync`'s
/// `save_component`, which phrases it.
///
/// A rule refused for any *other* reason — a `byMonth` the `RRULE` cannot
/// carry, a missing frequency — answers `None` here, so a caller cannot dress
/// an unrelated refusal up as a time-zone problem.
pub fn unstateable_until(rule: &RecurrenceRule) -> Option<&str> {
    rule.until
        .as_deref()
        .filter(|until| to_ical_date_time(until).is_none())
}

/// Whether a recurrence override survives the trip through iCalendar.
///
/// iCalendar names a single instance of a recurring event three ways, and
/// JSCalendar says all three with one `recurrenceOverrides` entry (RFC 8984
/// §4.3.4). `EXDATE` says the instance does not happen and `RDATE` says it does
/// (RFC 5545 §3.8.5.1, §3.8.5.2) — those are the empty patch and `excluded`.
/// The third, an instance that happens *differently*, is a `VEVENT` of its own
/// carrying a `RECURRENCE-ID`, and it can restate only the properties
/// [`OVERRIDE_PROPERTIES`] lists; a patch touching anything else — a
/// participant, a location — is a patch this mapping shows in part and must not
/// write back.
///
/// The `series` is checked too, for one property: `alerts` is drawn only where RFC
/// 8984 §4.5.1's `useDefaultAlerts` is not set, and the flag is not something an
/// override may restate, so it is the series that decides whether an occurrence's
/// own reminders reach the document at all. See [`maps_alerts`], which asks the
/// same question of the series' own set.
///
/// The `id` is checked as well as the patch. It is the instance's own start as a
/// LocalDateTime, and one no `EXDATE` can spell is an override that would vanish
/// from a property replaced whole.
///
/// An instance that is `excluded` may say nothing else: there is no occurrence
/// left to show an edited title on, so the `EXDATE` carries the exclusion and
/// drops the rest.
///
/// As with [`maps_recurrence_rule`], an override this returns `false` for is
/// still *drawn* as far as it can be — see [`event_to_ical`], which places an
/// override it cannot describe with a bare `RDATE` rather than hiding the
/// occurrence.
pub fn maps_recurrence_override(series: &CalendarEvent, id: &str, patch: &Value) -> bool {
    override_maps_by(series, id, patch, maps_override_field)
}

/// [`maps_recurrence_override`], asked by a save that can send a zone's
/// definition beside the identifier naming it.
///
/// The two differ over one field. `timeZone` is refused above because a
/// PatchObject naming `recurrenceOverrides` has no way to carry the RFC 8984
/// §4.7.2 entry a custom identifier is only legal beside (§1.4.9) — but a save
/// that patches `timeZones` in the same request does, and then the identifier
/// is as sendable as an IANA name is. So a caller willing to send the
/// definition asks this instead, and must actually send it: what makes the
/// override legal is the pair, not the check.
///
/// Which definition counts is the *series'*, because that is where a document
/// keeps them — one `VTIMEZONE` per `TZID` in the enclosing `VCALENDAR`,
/// whichever component names it — and it is the same judgement the drawing
/// makes, so an override this admits is one [`event_to_ical`] places in the zone
/// it names rather than at the series' clock. Callers: `jmap_cal_sync`'s
/// [`prune_time_zones`] neighbour, the patch path.
pub fn sends_recurrence_override(series: &CalendarEvent, id: &str, patch: &Value) -> bool {
    override_maps_by(series, id, patch, draws_override_field)
}

/// The shape both questions above have, with the per-field judgement left to the
/// caller: an id an `EXDATE` can spell, an exclusion that says nothing else, and
/// otherwise every restated field admitted by `field`.
fn override_maps_by(
    series: &CalendarEvent,
    id: &str,
    patch: &Value,
    field: fn(&CalendarEvent, &str, &Value) -> bool,
) -> bool {
    let Some(fields) = patch.as_object() else {
        return false;
    };
    if to_ical_date_time(id).is_none() {
        return false;
    }
    if excluded(patch) {
        return fields.len() == 1;
    }
    fields
        .iter()
        .all(|(name, value)| field(series, name, value))
}

/// Whether one field of an override's PatchObject reaches the component and
/// comes back meaning the same thing.
///
/// A PatchObject sets a property with a value and removes it with a null, and
/// the component says the removal by not carrying the line at all — so a null
/// round-trips wherever an absent property does. An *empty* string is neither:
/// the writer drops it like an absent value, so it would come back as a
/// removal, which is a different patch.
fn maps_override_field(series: &CalendarEvent, name: &str, value: &Value) -> bool {
    match name {
        "excluded" => value.is_boolean(),
        _ if !OVERRIDE_PROPERTIES.contains(&name) => false,
        // Outside the closed vocabulary there is no STATUS to write, so the
        // instance would come back at the series' status.
        "status" => value.is_null() || value.as_str().is_some_and(known_status),
        // The same, one closed vocabulary over: a `TRANSP` this mapping cannot
        // write leaves the instance blocking time however the series does, which
        // is what the override said it does *not*. A null is the instance set
        // back to the default, which the component says by carrying no line —
        // the state an event with no property is in anyway.
        "freeBusyStatus" => value.is_null() || value.as_str().is_some_and(known_transparency),
        // The same again, one type over: a `PRIORITY` this mapping cannot write
        // leaves the instance as important as the series is, which is what the
        // override said it is *not*. `as_i64` also refuses the number spelled as a
        // string or as a fraction, neither of which reaches a content line as the
        // integer that would come back.
        "priority" => value.is_null() || value.as_i64().is_some_and(known_priority),
        // And back to a string, one vocabulary over: a `CLASS` this mapping cannot
        // write leaves the instance as visible as the series is, which is what the
        // override said it is *not* — and this is the one property where that
        // matters beyond tidiness, since the instance the user hid would be drawn
        // at the series' classification. A null is the instance set back to the
        // default, which the component says by carrying no line.
        "privacy" => value.is_null() || value.as_str().is_some_and(known_privacy),
        // The one restated property that is a set. Its `CATEGORIES` line is drawn
        // whole, so a tag the line cannot show is a tag the next save deletes from
        // this occurrence — the same [`maps_keyword`] the series is drawn by, asked
        // of every tag rather than carried back. A null is the instance filed under
        // nothing, which the component says by carrying no line; the *empty* set is
        // refused, because it is written the same way and would come back as that
        // null, which is a different patch — the `title: ""` case one type over.
        "keywords" => {
            value.is_null()
                || value.as_object().is_some_and(|tags| {
                    !tags.is_empty() && tags.iter().all(|(tag, set)| maps_keyword(tag, set))
                })
        }
        // The set one component down, and the one property the *series* has a say
        // in: with RFC 8984 §4.5.1's `useDefaultAlerts` set, nothing reads the
        // property for the series or for any instance of it, so no `VALARM` is
        // drawn on either component and there is nothing a save could usefully
        // write — down to the null, which would be an edit to what nothing reads.
        // Otherwise the instance's `VALARM`s are drawn whole, so a reminder the
        // component cannot show is one the next save deletes from that occurrence:
        // asked of the same [`drawn_alert`] the series' own set is asked of, key
        // included, since a replaced map states its entries under their keys. The
        // empty map is refused for the reason the empty set of tags is — it is
        // written the same way as a removal and would come back as that null.
        "alerts" => {
            !uses_default_alerts(series)
                && (value.is_null()
                    || value.as_object().is_some_and(|alerts| {
                        !alerts.is_empty()
                            && alerts
                                .iter()
                                .all(|(key, alert)| drawn_alert(key, alert, None).is_some())
                    }))
        }
        // A start is required by RFC 8984, so a null says nothing, and the
        // value has to be one a DTSTART can carry.
        "start" => value.as_str().and_then(to_ical_date_time).is_some(),
        // A null is the floating instance, which a `DTSTART` with no `TZID`
        // says. A value has to be a name JSCalendar admits: the `TZID` this
        // reads back from is an iCalendar identifier (see [`names_time_zone`]),
        // and `recurrenceOverrides` goes back to the server replaced whole, so
        // one entry the server rejects costs every edit in the save.
        //
        // Which is why a custom identifier is refused here and *not* on the way
        // to the component — RFC 8984 §1.4.9 admits one only beside the
        // `timeZones` entry defining it, and this property's own patch has no way
        // to carry that entry along. A save willing to patch `timeZones` in the
        // same request asks [`sends_recurrence_override`] instead, which is
        // [`draws_override_field`]'s rule; this one is for a caller that will
        // send `recurrenceOverrides` alone.
        "timeZone" => value.is_null() || value.as_str().is_some_and(names_time_zone),
        // The only place an instance's own length is stated is its component's
        // DURATION, and one this mapping will not write there (see
        // [`vevent_of`]) leaves the instance at the series' length — which is
        // what the override said it was *not*.
        "duration" => value.is_null() || value.as_str().and_then(stated_duration).is_some(),
        // title, description.
        _ => value.is_null() || value.as_str().is_some_and(|text| !text.is_empty()),
    }
}

/// Whether one field of an override reaches the *component* — and, for the one
/// property where the two once diverged, whether it reaches a server that is
/// told what the identifier means.
///
/// [`maps_override_field`] is the same rule for a caller that will send
/// `recurrenceOverrides` and nothing else. A component states a custom
/// identifier perfectly well, beside the `VTIMEZONE` the same document defines
/// it with (see [`drawn_time_zones`]); a PatchObject naming only that one
/// property cannot, because the identifier would reach the server dangling.
/// Refusing it in the drawing as well drew the occurrence on the *series'*
/// clock — a different appointment, stated without saying so — and refusing it
/// in a save that *can* patch `timeZones` throws the user's move away, which is
/// what [`sends_recurrence_override`] asks this instead.
fn draws_override_field(series: &CalendarEvent, name: &str, value: &Value) -> bool {
    match name {
        "timeZone" => {
            value.is_null()
                || value
                    .as_str()
                    .is_some_and(|tzid| names_time_zone(tzid) || defines_time_zone(series, tzid))
        }
        _ => maps_override_field(series, name, value),
    }
}

/// Whether an override says its instance does not happen — the `EXDATE` half of
/// [`recurrence_dates`].
///
/// Anything that is not literally `"excluded": true` counts as an instance that
/// happens, including the `false` RFC 8984 §4.3.4 defaults to and a value of the
/// wrong type. That is the reading that cannot make an appointment disappear:
/// [`maps_recurrence_override`] refuses the malformed shape separately, so the
/// mistake cannot be written back to the server either way.
fn excluded(patch: &Value) -> bool {
    patch.get("excluded") == Some(&Value::Bool(true))
}

/// Whether an `RRULE` can carry this rule's frequency and its end — the two
/// parts that cannot be narrowed, only lost.
///
/// A rule that names no frequency has no `RRULE` spelling at all: an empty one
/// is rejected by libical and means nothing to a reader. A rule whose `until`
/// is not a date-time this mapping can write must be refused rather than
/// written without it, because a recurrence that ends and one that never does
/// are different events, and the unbounded one repeats into every week of the
/// user's calendar for ever.
fn writable(rule: &RecurrenceRule) -> bool {
    !rule.frequency.is_empty()
        && rule
            .until
            .as_deref()
            .is_none_or(|until| to_ical_date_time(until).is_some())
}

/// Render an event as an iCalendar object, ready for
/// `i_cal_component_new_from_string()`.
///
/// The series comes first, then one component per instance edited on its own —
/// see `modified_instances`.
pub fn event_to_ical(event: &CalendarEvent) -> String {
    let start = event.start.as_deref().and_then(to_ical_date_time);
    // Whether this event goes out as a date rather than a date-time, which is
    // the whole of `showWithoutTime` on this side. Decided once for the whole
    // document: `DTSTART`, `DURATION`, an `RRULE`'s `UNTIL`, the named instances
    // and every edited instance's own `RECURRENCE-ID` have to agree about it.
    let as_a_date = start
        .as_deref()
        .is_some_and(|start| shows_without_time(event, start));
    let zone = event.time_zone.as_deref();

    let mut vevent = vevent_of(event, as_a_date, zone, None);

    for rule in event.recurrence_rules.iter().flatten() {
        if let Some(entry) = rule_to_rrule(rule, Ends::In(Zoned::named(zone)), as_a_date)
            .and_then(|v| rrule_entry(&v))
        {
            vevent = vevent.with(entry);
        }
    }

    // The instances named one at a time. Both properties take a list, so each
    // kind is one content line however many instances it holds.
    for (name, is_excluded) in [("EXDATE", true), ("RDATE", false)] {
        let dates = recurrence_dates(event, is_excluded);
        if !dates.is_empty() {
            vevent = vevent.with(dated(name, &dates, as_a_date, zone));
        }
    }

    // Before the envelope, because which zones the document has to define is
    // decided by the components in it — an occurrence that moved into one of its
    // own names a second `TZID`. See [`drawn_time_zones`].
    let instances = modified_instances(event);

    let mut calendar = Component::new("VCALENDAR")
        .with(make_entry("VERSION", "2.0"))
        .with(make_entry("PRODID", PRODID));
    // Ahead of the events that refer to them, which is where a reader resolving a
    // `TZID` as it walks wants them.
    for vtimezone in drawn_time_zones(event, &instances, as_a_date) {
        calendar = calendar.with_child(vtimezone);
    }
    calendar = calendar.with_child(vevent);
    for (id, instance) in &instances {
        calendar = calendar.with_child(vevent_of(instance, as_a_date, zone, Some(id)));
    }
    calendar.to_ics()
}

/// The zones the document has to define itself, as `VTIMEZONE`s, in the order a
/// reader meets the components that refer to them.
///
/// RFC 5545 §3.2.19 says a `TZID` parameter names a `VTIMEZONE` in the *same*
/// object. The mapping leans on the reader for an IANA name — libical resolves
/// one out of its builtin table, and `jmap_backend_cal` puts that definition into
/// the object EDS caches, which is a better description of a zone than whatever a
/// server managed to state about it. What no reader can look up is RFC 8984
/// §1.4.9's other form of a `TimeZoneId`: a custom identifier beginning with a
/// solidus, which the event defines in its own `timeZones` (§4.7.2) and nowhere
/// else — what a server has to invent for the zone an Exchange invitation carries
/// its own `VTIMEZONE` for. A `DTSTART` naming one and defining nothing is a wall
/// clock time in no particular zone: libical floats it, and the appointment lands
/// hours from where the server put it, deterministically and silently.
///
/// So the identifiers this crate cannot name are exactly the ones it defines,
/// which is [`names_time_zone`] used the other way round.
///
/// Measured end to end, not assumed: `jmap-functional`'s third calendar leg seeds
/// such an event on the mock and holds a libecal consumer to the instant the server
/// means, two hours from where a floating start would land. What it also measured is
/// where the definition ends up — EDS gathers it into the calendar's own timezone
/// store rather than handing it back beside the event, so only a consumer that asks
/// the calendar for the zone resolves the identifier.
///
/// The `TZID`s a document carries are the series' zone and — for an occurrence
/// that moved into one of its own — that instance's, so more than one of them can
/// be a custom identifier and each needs its own definition. What must *not*
/// happen is one zone defined twice: two components naming the same custom
/// identifier is the ordinary case (an occurrence moved by an hour but not out of
/// its zone), and a second copy would be a duplicate `TZID` in one object. So the
/// identifiers are walked in the order their components appear and drawn once
/// each.
///
/// Only the drawing is this permissive. What the *save* path may state back to a
/// server is a narrower question for an override, and one this does not answer —
/// see [`maps_override_field`].
fn drawn_time_zones(
    event: &CalendarEvent,
    instances: &[(String, CalendarEvent)],
    as_a_date: bool,
) -> Vec<Component> {
    // A date-valued `DTSTART` takes no `TZID` at all (RFC 5545 §3.2.19), so a
    // document written that way refers to no zone and defines none.
    if as_a_date {
        return Vec::new();
    }
    let Some(definitions) = event.time_zones.as_ref() else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut drawn = Vec::new();
    for tzid in std::iter::once(event.time_zone.as_deref())
        .chain(
            instances
                .iter()
                .map(|(_, instance)| instance.time_zone.as_deref()),
        )
        .flatten()
    {
        if names_time_zone(tzid) || is_utc(tzid) || !seen.insert(tzid) {
            continue;
        }
        if let Some(vtimezone) =
            definition_of(definitions, tzid).and_then(|definition| vtimezone_of(tzid, definition))
        {
            drawn.push(vtimezone);
        }
    }
    drawn
}

/// The entry of a `timeZones` map that defines `tzid`, under either spelling of
/// its key.
///
/// RFC 8984 §4.7.2 types the map as `Id[TimeZone]` while §1.4.9 puts the solidus
/// on the identifier and §1.4.4's `Id` grammar has no solidus in it, so where
/// the prefix lives is genuinely ambiguous and both readings are in the wild.
/// Asking for either costs nothing, and a zone left undefined because the server
/// chose the other one is a silent hour.
fn definition_of<'a>(definitions: &'a BTreeMap<String, Value>, tzid: &str) -> Option<&'a Value> {
    definitions
        .get(tzid)
        .or_else(|| definitions.get(tzid.trim_start_matches('/')))
}

/// What `event` says the zone `tzid` is — its RFC 8984 §4.7.2 entry, or `None`
/// where the event defines no such identifier.
///
/// `definition_of` asked of a whole event, for a caller outside this crate:
/// `jmap_cal_sync`'s patch path, which sends the entry beside the identifier
/// naming it and has to know whether the server already holds one. Says nothing
/// about whether the definition can be *drawn* — that is
/// [`defines_time_zone`]'s question, and the two differ exactly where a server
/// states more than a `VTIMEZONE` has room for.
pub fn time_zone_definition<'a>(event: &'a CalendarEvent, tzid: &str) -> Option<&'a Value> {
    definition_of(event.time_zones.as_ref()?, tzid)
}

/// Whether the zone `event` is in is one a save may state to a server.
///
/// Three shapes are sendable and one is not. No zone at all is a floating event,
/// which is a real thing to save. An IANA name is RFC 8984 §1.4.9's first form
/// of a `TimeZoneId` and needs nothing beside it. A custom identifier — the
/// solidus-prefixed second form — is sendable exactly when the event *defines*
/// it, because §1.4.9 admits it only alongside the `timeZones` entry that says
/// which zone it is.
///
/// Everything else is not: a `TZID` off a document that neither names a zone nor
/// begins with a solidus is no identifier at all (Windows' `W. Europe Standard
/// Time` is the one that arrives), and one that begins with a solidus and
/// defines nothing is a dangling reference a server is entitled to reject —
/// which would cost the user every other edit in the same save.
///
/// "Defines it" means the definition can be drawn *whole*, which is
/// `vtimezone_of`'s judgement and not a second one: a definition this mapping
/// could only state in part describes a different zone, so the identifier is as
/// good as undefined. Callers: `jmap_cal_sync`'s create path, which files the
/// appointment floating rather than sending a zone a server cannot resolve.
pub fn maps_time_zone(event: &CalendarEvent) -> bool {
    let Some(tzid) = event.time_zone.as_deref() else {
        return true;
    };
    names_time_zone(tzid) || defines_time_zone(event, tzid)
}

/// Whether `event` says what the zone `tzid` is — RFC 8984 §1.4.9's second form
/// of a `TimeZoneId`, the solidus-prefixed identifier that resolves nowhere but
/// the object carrying it.
///
/// "Says what it is" means the §4.7.2 entry can be drawn *whole*, which is
/// `vtimezone_of`'s judgement asked directly rather than duplicated: a
/// definition this mapping could only state in part describes a different zone,
/// so the identifier is as good as undefined.
///
/// Public for `jmap_cal_sync`'s patch path, which may send such an identifier
/// exactly when it can send [`time_zone_definition`] beside it.
pub fn defines_time_zone(event: &CalendarEvent, tzid: &str) -> bool {
    tzid.starts_with('/')
        && event
            .time_zones
            .as_ref()
            .and_then(|definitions| definition_of(definitions, tzid))
            .and_then(|definition| vtimezone_of(tzid, definition))
            .is_some()
}

/// One RFC 8984 §4.7.2 TimeZone as a `VTIMEZONE`, or `None` for a definition this
/// mapping cannot draw whole.
///
/// Whole is the point. Every observance of a zone describes the offset between
/// the transitions the others name, so a `VTIMEZONE` missing one is not a
/// narrowed description of the zone — it is a *different* zone, and an event in
/// it is at a different instant. Half a definition is therefore worse than none:
/// none leaves the reader floating the event, which is visibly wrong, where half
/// of one is confidently wrong by an hour. The same goes for a rule an `RRULE`
/// cannot spell, which would draw a transition that happens once where the zone
/// moves every year.
///
/// RFC 5545 §3.6.5 requires at least one subcomponent, and libical refuses a
/// `VTIMEZONE` without one — which would cost the whole object rather than only
/// the zone — so a definition stating no observance is not drawn either.
///
/// What is left out: `aliases`, `url`, `validUntil` and the `comments` on a rule,
/// none of which a reader needs to resolve the identifier, and the
/// `recurrenceOverrides` of an observance, which would need a `RDATE` per
/// transition and describes a zone whose past was corrected rather than one whose
/// future differs.
fn vtimezone_of(tzid: &str, definition: &Value) -> Option<Component> {
    let mut vtimezone = Component::new("VTIMEZONE").with(make_entry("TZID", tzid));
    let mut observances = 0;
    for (name, member) in [("STANDARD", "standard"), ("DAYLIGHT", "daylight")] {
        // A zone that never moves states one of the two and not the other, which
        // is most of the world. A member that is there and is not a list of rules
        // is a definition this mapping cannot read, and gives up the whole zone
        // like any other part it cannot draw.
        let rules = match definition.get(member) {
            None | Some(Value::Null) => continue,
            Some(rules) => rules.as_array()?,
        };
        for rule in rules {
            vtimezone = vtimezone.with_child(observance(name, rule)?);
            observances += 1;
        }
    }
    (observances > 0).then_some(vtimezone)
}

/// One RFC 8984 §4.7.2 TimeZoneRule as the `STANDARD` or `DAYLIGHT` subcomponent
/// it is, or `None` for one that cannot be drawn — see [`vtimezone_of`], which
/// gives up on the whole zone when this does.
///
/// The three properties RFC 5545 §3.6.5 makes REQUIRED are the three JSCalendar
/// makes mandatory, so a rule missing one is not a rule. `DTSTART` is a local
/// time and has no `TZID`: §3.6.5 resolves it against `TZOFFSETFROM`, which is
/// why an observance can date itself in the zone it is defining.
fn observance(name: &str, rule: &Value) -> Option<Component> {
    let member = |name: &str| rule.get(name).and_then(Value::as_str);
    // The offset this observance's own local times are stated in, which its
    // `UNTIL` needs as well as its `DTSTART` — see [`Ends::At`].
    let offset_from = member("offsetFrom").and_then(utc_offset)?;
    let mut observance = Component::new(name)
        .with(make_entry(
            "DTSTART",
            &member("start").and_then(to_ical_date_time)?,
        ))
        .with(make_entry("TZOFFSETFROM", &offset_from))
        .with(make_entry(
            "TZOFFSETTO",
            &member("offsetTo").and_then(utc_offset)?,
        ));

    // When the transition repeats.
    //
    // Whole or not at all, which is [`maps_recurrence_rule`] asked ahead of the
    // spelling rather than after it: `rule_to_rrule` leaves off a `BYxxx` part it
    // cannot write and still yields a line, and here that line would be a zone
    // that moves on a *different* day — the last Sunday of every month, where the
    // rule said the last Sunday of October. On an event that is a recurrence the
    // user can see and correct; on a zone it is the same silent hour every other
    // part of this refuses to draw.
    for value in rule
        .get("recurrenceRules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let recurrence: RecurrenceRule = serde_json::from_value(value.clone()).ok()?;
        if !maps_recurrence_rule(&recurrence) {
            return None;
        }
        let rrule_str = rule_to_rrule(&recurrence, Ends::At(&offset_from), false)?;
        let entry = rrule_entry(&rrule_str)?;
        observance = observance.with(entry);
    }

    // What a reader shows for the offset — `CET`, `CEST`. RFC 8984 §4.7.2 keys
    // the names by locale-independent name; RFC 5545 §3.8.3.2 admits several
    // `TZNAME`s and distinguishes them by `LANGUAGE`, which the JSCalendar side
    // does not carry, so each name is drawn plainly.
    for named in rule
        .get("names")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, wanted)| *wanted == &Value::Bool(true))
        .map(|(name, _)| name)
    {
        observance = observance.with(make_entry("TZNAME", named));
    }
    Some(observance)
}

/// An RFC 5545 §3.3.14 UTC-OFFSET — `±hhmm[ss]` — or `None` for a value that is
/// no offset.
///
/// The one place the two formats may spell the same thing differently: RFC 8984
/// §4.7.2 states the same value iCalendar does, and JSON in the wild puts colons
/// in it. Both are read; what is written is iCalendar's, which is the format the
/// property this feeds is defined in.
///
/// `-0000` is picked out by the grammar itself: §3.3.14 forbids it, because the
/// sign says which side of UTC the zone is on and there is no negative zero.
fn utc_offset(value: &str) -> Option<String> {
    let (sign, digits) = value.split_at_checked(1)?;
    if !matches!(sign, "+" | "-") {
        return None;
    }
    let digits: String = digits.replace(':', "");
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let field = |at: usize| digits.get(at..at + 2)?.parse::<u32>().ok();
    let (hours, minutes) = (field(0)?, field(2)?);
    let seconds = match digits.len() {
        4 => 0,
        6 => field(4)?,
        _ => return None,
    };
    if hours > 23 || minutes > 59 || seconds > 60 {
        return None;
    }
    if sign == "-" && (hours, minutes, seconds) == (0, 0, 0) {
        return None;
    }
    // The seconds are written only when there are any: a whole number of minutes
    // is the short form, whichever way the server stated it.
    match seconds {
        0 => Some(format!("{sign}{hours:02}{minutes:02}")),
        _ => Some(format!("{sign}{hours:02}{minutes:02}{seconds:02}")),
    }
}

/// One `VEVENT`: the properties an event and an edited instance of it spell the
/// same way, plus the `RECURRENCE-ID` that makes it the latter.
///
/// The recurrence itself is *not* here. A series states its rules and its named
/// instances; an instance edited on its own states neither, because it is one
/// occurrence and RFC 5545 §3.8.4.4 already says which.
///
/// The two date-times this writes are in **two different zones** whenever an
/// instance moved into one of its own. `DTSTART` is in the event's own zone,
/// which for an instance is what its override said; `RECURRENCE-ID` is in
/// `series_zone`, because it names the occurrence the recurrence rules generated
/// and those run on the series' clock (RFC 5545 §3.8.4.4 — the value has to
/// match the series' `DTSTART`, or it points at an instant the series never
/// generated and the edit attaches to nothing). For the series itself the two
/// are the same zone.
fn vevent_of(
    event: &CalendarEvent,
    as_a_date: bool,
    series_zone: Option<&str>,
    recurrence_id: Option<&str>,
) -> Component {
    let mut vevent = Component::new("VEVENT");

    // EDS keys its cache on the iCalendar UID and passes it back to
    // load_component_sync()/remove_component_sync(), so it has to be the
    // identifier the JMAP methods take — the server-assigned id. The
    // JSCalendar uid, which is a different namespace, rides alongside; before
    // the first CalendarEvent/set there is no id and it stands in. An edited
    // instance carries the series' own, which is what ties the two together.
    if let Some(uid) = event
        .id
        .as_ref()
        .map(|id| id.as_str())
        .or(event.uid.as_deref())
    {
        vevent = vevent.with(make_entry("UID", uid));
    }
    if let Some(uid) = &event.uid {
        vevent = vevent.with(make_entry(X_JMAP_UID, uid));
    }
    if let Some(recurrence_id) = recurrence_id {
        vevent = vevent.with(dated(
            RECURRENCE_ID,
            std::slice::from_ref(&recurrence_id.to_owned()),
            as_a_date,
            series_zone,
        ));
    }

    // When the event was made and when it last changed. Written for whoever
    // reads the document and never read back — see [`to_utc_date_time`], and
    // `CalendarEvent::created`, which says whose the two instants are. DTSTAMP
    // carries `updated` as well because RFC 5545 §3.8.7.2 makes it REQUIRED on a
    // `VEVENT` and equivalent to `LAST-MODIFIED` in a calendar with no `METHOD`,
    // which is every calendar this crate emits; an event the server states no
    // `updated` for still gets no line, since the only value to invent would be
    // "now" and a rendering that changes between two runs is what the save path
    // cannot have.
    for (name, value) in [
        ("CREATED", &event.created),
        ("DTSTAMP", &event.updated),
        ("LAST-MODIFIED", &event.updated),
    ] {
        if let Some(stamp) = value.as_deref().and_then(to_utc_date_time) {
            vevent = vevent.with(make_entry(name, &stamp));
        }
    }

    for (name, value) in [
        ("SUMMARY", &event.title),
        ("DESCRIPTION", &event.description),
    ] {
        if let Some(value) = value.as_deref().filter(|value| !value.is_empty()) {
            vevent = vevent.with(make_entry(name, value));
        }
    }

    if let Some(start) = event.start.as_deref().and_then(to_ical_date_time) {
        vevent = vevent.with(dated(
            "DTSTART",
            std::slice::from_ref(&start),
            as_a_date,
            event.time_zone.as_deref(),
        ));
    }

    // Only a value that really is a length: the two formats spell an ISO 8601
    // duration identically, but not the same set of them, and a `DURATION` line
    // libical refuses costs the whole component — every field of the event, not
    // just its length. See [`stated_duration`], which is also what reads the
    // property back.
    if let Some(duration) = event.duration.as_deref().and_then(stated_duration) {
        vevent = vevent.with(make_entry("DURATION", &duration));
    }

    if let Some(status) = event.status.as_deref().and_then(ical_status) {
        vevent = vevent.with(make_entry("STATUS", status));
    }

    // Whether the event blocks the time it occupies. An event that says nothing
    // gets no line: the default the two formats share means the property is
    // absent from both, so there is nothing to state. See [`FREE_BUSY_STATUSES`].
    if let Some(transparency) = event
        .free_busy_status
        .as_deref()
        .and_then(ical_transparency)
    {
        vevent = vevent.with(make_entry("TRANSP", transparency));
    }

    // How important the event is. Only a number inside the range both formats
    // admit — see [`PRIORITIES`], which is also what reads the property back.
    if let Some(priority) = event.priority.filter(|priority| known_priority(*priority)) {
        vevent = vevent.with(
            ICalendarEntry::new(ICalendarProperty::Priority)
                .with_value(ICalendarValue::Integer(priority)),
        );
    }

    // How much of the event may be shared. Written out even for public, which is
    // the default both formats share — see [`PRIVACIES`] for why that is not the
    // `TRANSP` case.
    if let Some(privacy) = event.privacy.as_deref().and_then(ical_privacy) {
        vevent = vevent.with(make_entry("CLASS", privacy));
    }

    // One place of possibly several, by name, with the key it came from riding
    // alongside so a save can patch that entry rather than replace the property.
    // See [`maps_locations`] for what the drawing leaves out.
    if let Some((key, name)) = drawn_place(event) {
        vevent = vevent.with(make_entry("LOCATION", name).with_named_param(X_JMAP_KEY, key));
    }

    // Where the event may be joined online: a line per place, since RFC 7986
    // §5.11 admits several where `LOCATION` admits one. Written for the user to
    // read and never read back — see [`drawn_conferences`].
    for line in drawn_conferences(event) {
        vevent = vevent.with(line);
    }

    // What the event points at: the agenda as an `ATTACH`, the picture beside
    // its title as an `IMAGE`. Written for the user to read and never read back
    // — see [`drawn_links`].
    for line in drawn_links(event) {
        vevent = vevent.with(line);
    }

    // The whole set, on one line. An event whose every tag is one this mapping
    // cannot show gets no line at all rather than an empty one, which would state
    // a tag that is the empty string. See [`maps_keyword`].
    let tags = drawn_tags(event);
    if !tags.is_empty() {
        vevent = vevent.with(
            ICalendarEntry::new(ICalendarProperty::Categories).with_values(
                tags.into_iter()
                    .map(|t| ICalendarValue::Text(t.to_owned()))
                    .collect(),
            ),
        );
    }

    // Who called the meeting and who is invited to it. Written for the user to
    // read and never read back: changing the guest list, or what somebody
    // replied, is scheduling — an iTIP REQUEST or REPLY (RFC 5546) this backend
    // does not send — so `participants` is absent from [`MAPPED_PROPERTIES`] and
    // no save can name it. See [`drawn_participants`].
    for line in drawn_participants(event) {
        vevent = vevent.with(line);
    }

    // The reminders, each a component of its own rather than a line — so they
    // land after every property of the event whatever order they were added in,
    // which is what RFC 5545 §3.6's `eventc` grammar wants. See [`maps_alerts`]
    // for what is left out.
    for valarm in drawn_alarms(event) {
        vevent = vevent.with_child(valarm);
    }

    vevent
}

/// The instances that get a `VEVENT` of their own, each with the rendered
/// `RECURRENCE-ID` naming the occurrence it replaces.
///
/// In the map's chronological order, so a document is stable across renderings
/// — the save path diffs against a re-rendering of what the server holds, and a
/// difference that is only an order is a difference it would have to explain.
fn modified_instances(event: &CalendarEvent) -> Vec<(String, CalendarEvent)> {
    event
        .recurrence_overrides
        .iter()
        .flatten()
        .filter_map(|(id, patch)| {
            Some((to_ical_date_time(id)?, modified_instance(event, id, patch)?))
        })
        .collect()
}

/// The event one override describes, or `None` when the override says nothing
/// a `VEVENT` of its own would show.
///
/// The instance starts as the series with its recurrence dropped and its start
/// moved to the occurrence's own — RFC 8984 §4.3.4's rule that an override's key
/// *is* the instance's start unless the patch says otherwise. Then the patch is
/// applied, one property at a time, and a key or a value outside what
/// [`draws_override_field`] accepts is skipped rather than fatal: the instance is
/// still worth drawing at the series' title, and [`maps_recurrence_override`]
/// separately tells the save path that it was not seen whole.
///
/// An `excluded` instance yields `None`: it does not happen, so there is nothing
/// to draw, and the `EXDATE` [`recurrence_dates`] emits is the whole of it.
fn modified_instance(event: &CalendarEvent, id: &str, patch: &Value) -> Option<CalendarEvent> {
    if excluded(patch) {
        return None;
    }
    let fields = patch.as_object()?;
    let mut instance = CalendarEvent {
        id: event.id.clone(),
        uid: event.uid.clone(),
        // Inherited like everything else RFC 8984 §4.3.4 does not let an override
        // restate — and neither timestamp is restatable: they are the server's
        // record of the event, not of one of its occurrences. RFC 5545 asks for
        // the `DTSTAMP` on the instance's own component all the same.
        created: event.created.clone(),
        updated: event.updated.clone(),
        title: event.title.clone(),
        description: event.description.clone(),
        start: Some(id.to_owned()),
        time_zone: event.time_zone.clone(),
        duration: event.duration.clone(),
        show_without_time: event.show_without_time,
        status: event.status.clone(),
        free_busy_status: event.free_busy_status.clone(),
        priority: event.priority,
        privacy: event.privacy.clone(),
        // Inherited: RFC 8984 §4.3.4 has an instance hold every property its
        // override does not restate. Leaving them off would draw an occurrence of
        // a meeting as happening nowhere and belonging to nothing — and, since
        // both are drawn on the instance's own component, would read back as the
        // user having emptied them there. The same for the reminders, which are
        // `VALARM`s of the instance's own component: an occurrence drawn without
        // them is one nobody is reminded of. `locations` is inherited and no more —
        // an override may not name a place ([`OVERRIDE_PROPERTIES`]) — where
        // `keywords` and `alerts` are restated below.
        locations: event.locations.clone(),
        // Inherited and not restatable, like `participants` below: an occurrence
        // drawn without them is a meeting with nowhere to join it.
        virtual_locations: event.virtual_locations.clone(),
        // Inherited and not restatable for the same reason: an occurrence drawn
        // without them is a meeting whose agenda has gone missing.
        links: event.links.clone(),
        keywords: event.keywords.clone(),
        alerts: event.alerts.clone(),
        // Inherited for the same reason, and not restatable: RFC 8984 §4.4.6's
        // `participants` is not in [`OVERRIDE_PROPERTIES`], and an occurrence
        // drawn without them is a meeting nobody was invited to.
        participants: event.participants.clone(),
        // The properties this mapping does not model, inherited for the same
        // reason and needed for one of them: RFC 8984 §4.5.1's `useDefaultAlerts`
        // is read from here (see [`uses_default_alerts`]), and an instance that
        // arrived without it would be drawn with alarms the series beside it is
        // drawn without — reminders that never fire, and, read back, an occurrence
        // the user apparently set them on.
        extra: event.extra.clone(),
        ..CalendarEvent::default()
    };

    let mut modified = false;
    for (name, value) in fields {
        if !draws_override_field(event, name, value) {
            continue;
        }
        // The one restatable property that is not text. Checked above, so a null
        // clears the importance and anything else is an integer inside the range
        // both formats admit.
        if name == "priority" {
            instance.priority = value.as_i64();
            modified = true;
            continue;
        }
        // The one restatable property that is a set, and the one that replaces
        // rather than adds to what it inherited: a PatchObject sets the property to
        // the value it names, so an override naming one tag is an occurrence filed
        // under that tag alone. Checked above, so a null empties it and every entry
        // is one the `CATEGORIES` line carries.
        if name == "keywords" {
            instance.keywords = value.as_object().map(|tags| {
                tags.iter()
                    .map(|(tag, set)| (tag.clone(), set.clone()))
                    .collect()
            });
            modified = true;
            continue;
        }
        // The set that is drawn as components rather than as a line, replacing what
        // it inherited for the same reason: an override naming one reminder is an
        // occurrence with that reminder alone. Checked above, so a null leaves the
        // occurrence unreminded, every entry is one a `VALARM` states whole, and
        // the series does not hold the flag that would make all of them unread.
        if name == "alerts" {
            instance.alerts = value.as_object().map(|alerts| {
                alerts
                    .iter()
                    .map(|(key, alert)| (key.clone(), alert.clone()))
                    .collect()
            });
            modified = true;
            continue;
        }
        // Checked above: a null removes the property, and anything else here is
        // the non-empty string that sets it.
        let text = value.as_str().map(str::to_owned);
        match name.as_str() {
            "title" => instance.title = text,
            "description" => instance.description = text,
            "duration" => instance.duration = text,
            "status" => instance.status = text,
            "freeBusyStatus" => instance.free_busy_status = text,
            "privacy" => instance.privacy = text,
            "start" => instance.start = text,
            "timeZone" => instance.time_zone = text,
            // `excluded: false`, which says only that the instance happens —
            // and it happening is what an override with no other content
            // already means.
            _ => continue,
        }
        modified = true;
    }
    modified.then_some(instance)
}

/// A property carrying date-times in the one form this event spells them in.
///
/// `DTSTART` decides that form, and RFC 5545 obliges the properties that name
/// instances of it to agree: an `EXDATE` or `RDATE` in a different value type or
/// a different zone (§3.8.5.1, §3.8.5.2) resolves against another clock than the
/// occurrences it is meant to add to or remove from, so the exclusion misses and
/// the deleted appointment comes back. One function for all three is how they
/// are kept from drifting apart.
///
/// Each value is a rendered `YYYYMMDDTHHMMSS`; the caller has already decided
/// whether the event is written as a date, which is the only case where the time
/// may be dropped.
fn dated(name: &str, values: &[String], as_a_date: bool, zone: Option<&str>) -> ICalendarEntry {
    let prop = ICalendarProperty::parse(name.as_bytes())
        .unwrap_or_else(|| ICalendarProperty::Other(name.to_ascii_uppercase()));
    match (as_a_date, zone) {
        // A DATE value, RFC 5545 §3.6.1's other form of an event. The parameter
        // is required: these properties are DATE-TIME by default, and libical
        // refuses the whole component over a value that is not one.
        (true, _) => {
            let vals = values
                .iter()
                .map(|value| ICalendarValue::Text(value[..8].to_owned()))
                .collect();
            ICalendarEntry::new(prop)
                .with_values(vals)
                .with_named_param("VALUE", "DATE")
        }
        // Form 2, a UTC instant. Form 3 with TZID=Etc/UTC would be legal but
        // obliges us to ship a VTIMEZONE for it.
        (false, Some(zone)) if is_utc(zone) => {
            let vals = values
                .iter()
                .map(|value| ICalendarValue::Text(format!("{value}Z")))
                .collect();
            ICalendarEntry::new(prop).with_values(vals)
        }
        // Form 3. RFC 5545 §3.2.19 has the document define what a `TZID` refers
        // to; for an IANA name this is the one place the mapping leans on the
        // consumer instead, since libical resolves one from its built-in zone
        // table — measured rather than assumed: `jmap-functional`'s second
        // calendar leg reads such a start back through real EDS and holds a
        // libecal consumer to the instant it means, two hours from where a
        // floating one would land. A zone with no name to resolve is defined in
        // the document instead where the event says what it is (see
        // [`drawn_time_zone`]); one that is neither still falls back to floating
        // on the consumer's side, which is the same guess we would have to make —
        // measured too, in the same leg, as an appointment two hours from where
        // the server put it.
        (false, Some(zone)) => {
            let vals = values
                .iter()
                .map(|value| ICalendarValue::Text(value.clone()))
                .collect();
            ICalendarEntry::new(prop)
                .with_values(vals)
                .with_named_param("TZID", zone)
        }
        // Form 1, floating. Inventing UTC here would move the event.
        (false, None) => {
            let vals = values
                .iter()
                .map(|value| ICalendarValue::Text(value.clone()))
                .collect();
            ICalendarEntry::new(prop).with_values(vals)
        }
    }
}

/// The instances of `event` to name in an `EXDATE` (`is_excluded`) or an
/// `RDATE`, rendered and in chronological order.
///
/// An instance drawn as a component of its own is left out of the `RDATE`: the
/// component already places it, and the date would only repeat that it happens.
/// What is left in is every override with a writable id that
/// [`modified_instance`] found nothing to draw — a patch naming a property
/// outside [`OVERRIDE_PROPERTIES`], say. Placing that one at the series' title
/// is a narrowing of the same kind as an `RRULE` that had to drop its `byDay`,
/// and better than an occurrence the user cannot see at all. Where the rules
/// already generate that instant the `RDATE` is a duplicate, and RFC 5545
/// §3.8.5.2 has the recurrence set absorb it.
fn recurrence_dates(event: &CalendarEvent, is_excluded: bool) -> Vec<String> {
    event
        .recurrence_overrides
        .iter()
        .flatten()
        .filter(|(_, patch)| excluded(patch) == is_excluded)
        .filter(|(id, patch)| modified_instance(event, id, patch).is_none())
        .filter_map(|(id, _)| to_ical_date_time(id))
        .collect()
}

/// The iCalendar `STATUS` for a JSCalendar status, or `None` for one outside the
/// closed vocabulary the two share.
fn ical_status(status: &str) -> Option<&'static str> {
    STATUSES
        .iter()
        .find(|(jscalendar, _)| jscalendar.eq_ignore_ascii_case(status))
        .map(|(_, ical)| *ical)
}

fn known_status(status: &str) -> bool {
    ical_status(status).is_some()
}

/// The iCalendar `TRANSP` for a JSCalendar `freeBusyStatus`, or `None` for one
/// outside the closed vocabulary the two share.
fn ical_transparency(free_busy_status: &str) -> Option<&'static str> {
    FREE_BUSY_STATUSES
        .iter()
        .find(|(jscalendar, _)| jscalendar.eq_ignore_ascii_case(free_busy_status))
        .map(|(_, ical)| *ical)
}

fn known_transparency(free_busy_status: &str) -> bool {
    ical_transparency(free_busy_status).is_some()
}

/// The iCalendar `CLASS` for a JSCalendar `privacy`, or `None` for one outside
/// the three-value scale the two share — see [`PRIVACIES`].
fn ical_privacy(privacy: &str) -> Option<&'static str> {
    PRIVACIES
        .iter()
        .find(|(jscalendar, _)| jscalendar.eq_ignore_ascii_case(privacy))
        .map(|(_, ical)| *ical)
}

fn known_privacy(privacy: &str) -> bool {
    ical_privacy(privacy).is_some()
}

/// The JSCalendar `privacy` a `CLASS` states, or `None` where the component
/// states none this mapping can name — which is read as nothing said, like every
/// other unreadable value, rather than passed on for the server to reject.
///
/// Case is ignored, as it is for `STATUS` and `TRANSP`: RFC 5545 §3.1 makes an
/// enumerated property value case-insensitive, so `CLASS:confidential` is the
/// same classification as `CLASS:CONFIDENTIAL`. What it is *not* is a match for
/// JSCalendar's own spelling of a different value — the two vocabularies overlap
/// nowhere, so `CLASS:secret` is an x-name-shaped value this mapping has no
/// business reading as RFC 8984's `secret`.
fn read_privacy(vevent: &ICalendarComponent) -> Option<String> {
    let privacy = component_text(vevent, "CLASS")?;
    PRIVACIES
        .iter()
        .find(|(_, ical)| ical.eq_ignore_ascii_case(&privacy))
        .map(|(jscalendar, _)| (*jscalendar).to_owned())
}

/// Whether an importance is one both formats admit — see [`PRIORITIES`].
fn known_priority(priority: i64) -> bool {
    PRIORITIES.contains(&priority)
}

/// The JSCalendar `priority` a `PRIORITY` states, or `None` where the component
/// states none this mapping can carry — an integer outside the shared range, or
/// something that is no integer at all, which is read as nothing said like every
/// other unreadable value rather than passed on for the server to reject.
///
/// `parse` is deliberately strict about what an integer is: it refuses leading
/// space, a fraction and a second value after a comma, none of which is the RFC
/// 5545 §3.3.8 INTEGER the property is defined to carry.
fn read_priority(vevent: &ICalendarComponent) -> Option<i64> {
    component_text(vevent, "PRIORITY")?
        .parse()
        .ok()
        .filter(|priority| known_priority(*priority))
}

/// The JSCalendar `freeBusyStatus` a `TRANSP` states, or `None` where the
/// component states none this mapping can name — which is read as nothing said,
/// like every other unreadable value, rather than passed on for the server to
/// reject.
fn read_transparency(vevent: &ICalendarComponent) -> Option<String> {
    let transparency = component_text(vevent, "TRANSP")?;
    FREE_BUSY_STATUSES
        .iter()
        .find(|(_, ical)| ical.eq_ignore_ascii_case(&transparency))
        .map(|(jscalendar, _)| (*jscalendar).to_owned())
}

/// How deep component nesting may go before a document is refused whole.
pub const MAX_DEPTH: usize = 32;

/// Parse an iCalendar string into calcard's `ICalendar` structure, validating
/// envelope boundaries and component nesting depth limits.
pub fn parse_ical(text: &str) -> Result<ICalendar, ICalError> {
    check_structure(text)?;

    let mut parser = Parser::new(text);
    let calendar = match parser.entry() {
        Entry::ICalendar(calendar) => calendar,
        _ => return Err(ICalError::NotACalendar),
    };
    match parser.entry() {
        Entry::Eof => {}
        Entry::InvalidLine(line) => return Err(ICalError::Trailing(line)),
        _ => return Err(ICalError::Trailing("BEGIN:VCALENDAR".to_owned())),
    }

    let components = &calendar.components;
    let root = components.first().ok_or(ICalError::NotACalendar)?;
    if !root
        .component_type
        .as_str()
        .eq_ignore_ascii_case("VCALENDAR")
    {
        return Err(ICalError::NotACalendar);
    }
    check_depth(components)?;

    Ok(calendar)
}

fn check_structure(text: &str) -> Result<(), ICalError> {
    let mut open: Vec<String> = Vec::new();
    for line in unfold(text.strip_prefix('\u{feff}').unwrap_or(text)) {
        let Some((keyword, name)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_uppercase();
        if name.is_empty() {
            continue;
        }
        if keyword.eq_ignore_ascii_case("BEGIN") {
            open.push(name);
        } else if keyword.eq_ignore_ascii_case("END") {
            match open.pop() {
                Some(expected) if expected == name => {}
                Some(expected) => {
                    return Err(ICalError::Mismatched {
                        expected,
                        found: name,
                    });
                }
                None => {}
            }
        }
    }
    match open.pop() {
        Some(name) => Err(ICalError::Unterminated(name)),
        None => Ok(()),
    }
}

fn check_depth(components: &[ICalendarComponent]) -> Result<(), ICalError> {
    let mut pending = vec![(0usize, 1usize)];
    while let Some((index, depth)) = pending.pop() {
        let Some(component) = components.get(index) else {
            continue;
        };
        if depth > MAX_DEPTH {
            return Err(ICalError::TooDeep(
                component.component_type.as_str().to_ascii_uppercase(),
            ));
        }
        for child in &component.component_ids {
            pending.push((*child as usize, depth + 1));
        }
    }
    Ok(())
}

fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.replace("\r\n", "\n").split('\n') {
        match raw.strip_prefix([' ', '\t']) {
            Some(rest) if !lines.is_empty() => lines.last_mut().expect("non-empty").push_str(rest),
            _ => {
                let line = raw.trim_end_matches('\r');
                if !line.is_empty() {
                    lines.push(line.to_owned());
                }
            }
        }
    }
    lines
}

pub(crate) fn entry_text(entry: &ICalendarEntry) -> String {
    entry
        .values
        .iter()
        .filter_map(value_text_str)
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn entry_texts(entry: &ICalendarEntry) -> Vec<String> {
    entry.values.iter().filter_map(value_text_str).collect()
}

pub(crate) fn entry_raw_value(entry: &ICalendarEntry) -> String {
    entry_text(entry)
}

pub(crate) fn entry_param(entry: &ICalendarEntry, name: &str) -> Option<String> {
    entry
        .params
        .iter()
        .find(|param| param.name.as_str().eq_ignore_ascii_case(name))
        .map(|param| param_text(&param.value))
}

pub(crate) fn entry_param_values(entry: &ICalendarEntry, name: &str) -> Vec<String> {
    entry
        .params
        .iter()
        .filter(|param| param.name.as_str().eq_ignore_ascii_case(name))
        .map(|param| param_text(&param.value))
        .collect()
}

pub(crate) fn component_entry<'a>(
    component: &'a ICalendarComponent,
    name: &str,
) -> Option<&'a ICalendarEntry> {
    component
        .entries
        .iter()
        .find(|entry| entry.name.as_str().eq_ignore_ascii_case(name))
}

pub(crate) fn component_entries<'a>(
    component: &'a ICalendarComponent,
    name: &'a str,
) -> impl Iterator<Item = &'a ICalendarEntry> {
    component
        .entries
        .iter()
        .filter(move |entry| entry.name.as_str().eq_ignore_ascii_case(name))
}

pub(crate) fn component_text(component: &ICalendarComponent, name: &str) -> Option<String> {
    component_entry(component, name).map(entry_text)
}

pub(crate) fn value_text_str(value: &ICalendarValue) -> Option<String> {
    value_text(value).map(|(s, _)| s)
}

fn value_text(value: &ICalendarValue) -> Option<(String, bool)> {
    let typed = |value: String| Some((value, false));
    match value {
        ICalendarValue::Text(text) => Some((text.clone(), true)),
        ICalendarValue::PartialDateTime(stamp) => typed(date_time_text(stamp)),
        ICalendarValue::Duration(duration) => typed(duration.to_string()),
        ICalendarValue::RecurrenceRule(rule) => typed(rule.to_string()),
        ICalendarValue::Period(period) => typed(period.to_string()),
        ICalendarValue::Uri(Uri::Location(uri)) => typed(uri.clone()),
        ICalendarValue::Integer(number) => typed(number.to_string()),
        ICalendarValue::Float(number) => typed(number.to_string()),
        ICalendarValue::Boolean(true) => typed("TRUE".to_owned()),
        ICalendarValue::Boolean(false) => typed("FALSE".to_owned()),
        ICalendarValue::CalendarScale(scale) => typed(scale.as_str().to_owned()),
        ICalendarValue::Method(method) => typed(method.as_str().to_owned()),
        ICalendarValue::Classification(class) => typed(class.as_str().to_owned()),
        ICalendarValue::Status(status) => typed(status.as_str().to_owned()),
        ICalendarValue::Transparency(transparency) => typed(transparency.as_str().to_owned()),
        ICalendarValue::Action(action) => typed(action.as_str().to_owned()),
        ICalendarValue::BusyType(kind) => typed(kind.as_str().to_owned()),
        ICalendarValue::ParticipantType(kind) => typed(kind.as_str().to_owned()),
        ICalendarValue::ResourceType(kind) => typed(kind.as_str().to_owned()),
        ICalendarValue::Proximity(proximity) => typed(proximity.as_str().to_owned()),
        ICalendarValue::Binary(_) | ICalendarValue::Uri(Uri::Data(_)) => None,
    }
}

fn date_time_text(stamp: &PartialDateTime) -> String {
    let kind = match (stamp.year.is_some(), stamp.hour.is_some()) {
        (true, true) => ICalendarValueType::DateTime,
        (true, false) => ICalendarValueType::Date,
        (false, true) => ICalendarValueType::Time,
        (false, false) => ICalendarValueType::UtcOffset,
    };
    let mut out = String::new();
    let _ = stamp.format_as_ical(&mut out, &kind);
    match out == "-0000" {
        true => "+0000".to_owned(),
        false => out,
    }
}

fn param_text(value: &ICalendarParameterValue) -> String {
    match value {
        ICalendarParameterValue::Text(text) => text.clone(),
        ICalendarParameterValue::Integer(number) => number.to_string(),
        ICalendarParameterValue::Bool(true) => "TRUE".to_owned(),
        ICalendarParameterValue::Bool(false) => "FALSE".to_owned(),
        ICalendarParameterValue::Uri(Uri::Location(uri)) => uri.clone(),
        ICalendarParameterValue::Cutype(kind) => kind.as_str().to_owned(),
        ICalendarParameterValue::Fbtype(kind) => kind.as_str().to_owned(),
        ICalendarParameterValue::Partstat(status) => status.as_str().to_owned(),
        ICalendarParameterValue::Related(related) => related.as_str().to_owned(),
        ICalendarParameterValue::Reltype(kind) => kind.as_str().to_owned(),
        ICalendarParameterValue::Role(role) => role.as_str().to_owned(),
        ICalendarParameterValue::ScheduleAgent(agent) => agent.as_str().to_owned(),
        ICalendarParameterValue::ScheduleForceSend(send) => send.as_str().to_owned(),
        ICalendarParameterValue::Value(kind) => kind.as_str().to_owned(),
        ICalendarParameterValue::Display(display) => display.as_str().to_owned(),
        ICalendarParameterValue::Feature(feature) => feature.as_str().to_owned(),
        ICalendarParameterValue::Duration(duration) => duration.to_string(),
        ICalendarParameterValue::Linkrel(relation) => relation.as_str().to_owned(),
        ICalendarParameterValue::Uri(Uri::Data(_)) | ICalendarParameterValue::Null => String::new(),
    }
}

/// Read an iCalendar object into a calendar event.
///
/// The series is the `VEVENT` **without** a `RECURRENCE-ID`, found by that
/// rather than by position: EDS hands a save every instance of one uid it holds,
/// in no promised order, and taking the first component would read a single
/// edited day as if it were the whole series. The rest become
/// `recurrenceOverrides` entries — see `read_overrides`.
///
/// A document holding *nothing but* detached instances has no series to attach
/// them to, so the first component is read as the event it describes and the
/// others are dropped. There is nothing better available: JSCalendar says
/// "this instance differs" only relative to a series.
///
/// The `id` is whatever the component's `UID` says, which for an event
/// Evolution has just created is a locally invented string rather than a JMAP
/// id — the caller knows which case it is in and must drop it before sending
/// a create.
pub fn ical_to_event(text: &str) -> Result<CalendarEvent, ICalError> {
    let calendar = parse_ical(text)?;
    let components = &calendar.components;
    let vevents: Vec<&ICalendarComponent> = components
        .iter()
        .filter(|child| child.component_type.as_str().eq_ignore_ascii_case("VEVENT"))
        .collect();
    let series = *vevents
        .iter()
        .find(|vevent| component_entry(vevent, RECURRENCE_ID).is_none())
        .or_else(|| vevents.first())
        .ok_or(ICalError::NoEvent)?;

    let zones = stated_zones(components);
    let mut event = read_vevent(series, &zones, components);
    // A standalone Event MUST state its JSCalendar version (jscalendarbis
    // §3.1.2) — Fastmail refuses a create without it. The value is `"2.0"`:
    // draft-ietf-jmap-calendars-28 §1.4 defines CalendarEvent as a
    // *jscalendarbis* Event, and Fastmail rejects `"1.0"` in this context
    // (observed 2026-08-24 — both a version-less create and a `"1.0"` one
    // came back `invalidProperties: ["version"]`). Stamped here, not in
    // `read_vevent`, because embedded objects (recurrence-override
    // instances) must NOT carry it.
    event.version = Some("2.0".to_owned());
    event.recurrence_overrides = read_overrides(series, &vevents, &event, &zones, components);
    // After the overrides, because which definitions the document is carrying for
    // us is decided by which zones the event turned out to refer to — the series'
    // and one per occurrence that moved into a zone of its own. See
    // [`read_time_zones`].
    event.time_zones = read_time_zones(components, &event);
    Ok(event)
}

/// Whether a value is a time zone JSCalendar can carry — an RFC 8984 §1.4.9
/// `TimeZoneId`.
///
/// That type is a name in the IANA Time Zone Database, or a custom identifier
/// beginning with a solidus **that the same object defines in its `timeZones`
/// property**. This mapping does not carry `timeZones`, so the second form has
/// nowhere to be defined and is refused: a dangling identifier is an event a
/// server may reject outright, and rejecting a `CalendarEvent/set` costs the
/// user every edit in it, not just the zone.
///
/// Which matters because a solidus-prefixed identifier is exactly what arrives.
/// libical names its builtin zones `/freeassociation.sourceforge.net/<zone>`
/// and Evolution's appointment editor sets the start with the zone object, so
/// every zoned component a save hands back carries one — even one this crate
/// wrote spelling the zone plainly. `zone_names` is how it gets translated.
///
/// What is checked is the *shape* of a name, not membership of the database:
/// non-empty segments of letters, digits, `_`, `-` and `+` separated by `/`.
/// That admits `Europe/Berlin`, `America/Argentina/Buenos_Aires`, `Etc/GMT+5`
/// and the bare `UTC`, and refuses the solidus form (an empty first segment),
/// a Windows zone name like `W. Europe Standard Time`, and anything carrying a
/// character a content line could be broken with. Checking membership would
/// mean shipping a zone database this crate has no other use for; refusing a
/// zone the database does not have is the server's job, and it can do it
/// without the whole save failing.
pub fn names_time_zone(value: &str) -> bool {
    value.split('/').all(|segment| {
        !segment.is_empty()
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+'))
    })
}

/// What the document states about each zone it defines, by the `TZID` that
/// refers to it.
///
/// Two different questions are asked of the one map, which is why the whole
/// `VTIMEZONE` is kept rather than the answer to the first alone: which zone a
/// `TZID` names (see [`zone_of`]), and what the zone's offset was at a given
/// instant (see [`crate::zone`], and [`read_until`], its one caller).
type Zones<'a> = BTreeMap<String, Zone<'a>>;

/// One `VTIMEZONE`, and the zone it says it describes.
struct Zone<'a> {
    /// Only where the component answers the question: a `TZID` that already
    /// [`names_time_zone`] needs no translating, and an `X-LIC-LOCATION` that
    /// names no zone either is no better than what it would replace.
    name: Option<String>,
    observances: Vec<&'a ICalendarComponent>,
}

/// Every `VTIMEZONE` the document carries, by the `TZID` that refers to it.
fn stated_zones<'a>(components: &'a [ICalendarComponent]) -> Zones<'a> {
    components
        .iter()
        .filter(|child| {
            child
                .component_type
                .as_str()
                .eq_ignore_ascii_case("VTIMEZONE")
        })
        .filter_map(|vtimezone| {
            let tzid = component_text(vtimezone, "TZID")?;
            let name = component_text(vtimezone, X_LIC_LOCATION)
                .filter(|location| !names_time_zone(&tzid) && names_time_zone(location));
            let observances: Vec<&'a ICalendarComponent> = vtimezone
                .component_ids
                .iter()
                .filter_map(|id| components.get(*id as usize))
                .collect();
            Some((tzid, Zone { name, observances }))
        })
        .collect()
}

/// The definitions of the zones the event refers to, as the RFC 8984 §4.7.2
/// `timeZones` map that carries them — the inverse of [`drawn_time_zones`].
///
/// "Refers to" is the series' zone *and* every occurrence that moved into one of
/// its own: RFC 5545 §3.2.19 puts the zone on the property, so a detached
/// instance states its own `TZID` and need not share the series'. Looking for the
/// series' alone sent the server an occurrence naming a zone nothing defined —
/// a dangling `TimeZoneId` §1.4.9 does not admit, which a server may reject
/// outright and which a server that keeps it shows as one floating occurrence.
///
/// Only a custom identifier gets one, and only when the document defines it.
/// Which is the whole of the case: an IANA name resolves out of any zone
/// database, so a definition beside it is the reader's copy and not ours to
/// carry — see [`zone_of`], which translates what it can *into* a name and so
/// never reaches here. What is left is RFC 8984 §1.4.9's other form, the
/// solidus-prefixed identifier that resolves nowhere but the object it came in,
/// and there the definition is the zone: without it the identifier dangles and
/// the appointment is a wall clock time in no particular zone.
///
/// A `TZID` that is neither — Windows' `W. Europe Standard Time` — is left
/// undefined however complete the `VTIMEZONE` beside it, because there is no
/// identifier to file the definition under. Inventing one would mean this crate
/// deciding which zone Exchange meant, and the two honest answers are the
/// server's own zone (on an edit) or a floating event (on a create). See
/// [`maps_time_zone`], which is where that answer is given.
///
/// The definition is read only if it can be *drawn again*, which is
/// [`vtimezone_of`]'s judgement asked directly rather than duplicated: half a
/// zone is a different zone, so a `VTIMEZONE` this mapping cannot state whole is
/// read as no definition at all. It also makes the round trip exact — what comes
/// back out is byte for byte what came in.
fn read_time_zones(
    components: &[ICalendarComponent],
    event: &CalendarEvent,
) -> Option<BTreeMap<String, Value>> {
    let mut zones: BTreeMap<String, Value> = BTreeMap::new();
    for tzid in referred_zones(event) {
        if names_time_zone(tzid) || !tzid.starts_with('/') || zones.contains_key(tzid) {
            continue;
        }
        let Some(definition) = components
            .iter()
            .filter(|child| {
                child
                    .component_type
                    .as_str()
                    .eq_ignore_ascii_case("VTIMEZONE")
            })
            .find(|vtimezone| component_text(vtimezone, "TZID").as_deref() == Some(tzid))
            .and_then(|vtimezone| read_definition(vtimezone, components))
        else {
            continue;
        };
        // Half a zone is a different zone, so one this mapping could not draw
        // again is read as no definition at all — for that identifier alone,
        // leaving whatever the other components named still defined.
        if vtimezone_of(tzid, &definition).is_none() {
            continue;
        }
        zones.insert(tzid.to_owned(), definition);
    }
    (!zones.is_empty()).then_some(zones)
}

/// Every `TimeZoneId` the event names — the series' own, and one per occurrence
/// that moved into a zone of its own.
///
/// Naming and *defining* are different questions: this lists what is referred to,
/// whatever form the identifier takes, and leaves [`defines_time_zone`] to say which
/// of those references the event answers. [`read_time_zones`] collects the
/// definitions for them; [`prune_time_zones`] drops the definitions for
/// everything else.
fn referred_zones(event: &CalendarEvent) -> impl Iterator<Item = &str> {
    std::iter::once(event.time_zone.as_deref())
        .chain(
            event
                .recurrence_overrides
                .iter()
                .flatten()
                .map(|(_, patch)| patch.get("timeZone").and_then(Value::as_str)),
        )
        .flatten()
}

/// Drop every `timeZones` entry the event no longer refers to.
///
/// A §4.7.2 entry nothing names is a claim about a zone the event is not in, so
/// it is not sent — but "nothing names it" has to be asked of the whole event
/// rather than of its series alone. A caller that clears an unsendable
/// [`CalendarEvent::time_zone`] (see [`maps_time_zone`], and `jmap_cal_sync`'s
/// create path, which is the one caller) used to clear the map with it; that
/// took the definition of a zone one *occurrence* had been moved into, leaving
/// the override naming a `TimeZoneId` nothing resolved. RFC 8984 §1.4.9 does not
/// admit that identifier without its definition, and a server is entitled to
/// refuse the whole `CalendarEvent/set` over it — costing the user the
/// appointment rather than the series' zone.
///
/// Emptied completely, the map goes away rather than being sent as `{}`.
pub fn prune_time_zones(event: &mut CalendarEvent) {
    let referred: BTreeSet<String> = referred_zones(event).map(str::to_owned).collect();
    let Some(definitions) = event.time_zones.as_mut() else {
        return;
    };
    // Under either spelling of the key, for the reason [`definition_of`] reads
    // both: an entry kept under one and looked up under the other is a zone that
    // silently stops resolving.
    definitions.retain(|tzid, _| {
        referred
            .iter()
            .any(|referred| referred == tzid || referred.trim_start_matches('/') == tzid)
    });
    if definitions.is_empty() {
        event.time_zones = None;
    }
}

/// One `VTIMEZONE` as the RFC 8984 §4.7.2 TimeZone it describes, or `None` for
/// one this mapping cannot read — the inverse of [`vtimezone_of`].
///
/// Only the members [`vtimezone_of`] draws are read, because those are the ones
/// the document can hold: `aliases`, `url`, `validUntil` and an observance's
/// `recurrenceOverrides` have no iCalendar spelling this crate writes, so a
/// value invented for them here would be this side making a claim about the zone
/// rather than reporting one.
fn read_definition(
    vtimezone: &ICalendarComponent,
    components: &[ICalendarComponent],
) -> Option<Value> {
    let mut zone = Map::new();
    zone.insert("@type".to_owned(), json!("TimeZone"));
    zone.insert("tzId".to_owned(), json!(component_text(vtimezone, "TZID")?));
    let mut observances = 0;
    for (name, member) in [("STANDARD", "standard"), ("DAYLIGHT", "daylight")] {
        let rules = vtimezone
            .component_ids
            .iter()
            .filter_map(|id| components.get(*id as usize))
            .filter(|child| child.component_type.as_str().eq_ignore_ascii_case(name))
            .map(read_observance)
            .collect::<Option<Vec<Value>>>()?;
        if rules.is_empty() {
            continue;
        }
        observances += rules.len();
        zone.insert(member.to_owned(), Value::Array(rules));
    }
    // RFC 5545 §3.6.5 requires at least one subcomponent, and a zone that states
    // no observance says nothing about what time it is in it.
    (observances > 0).then_some(Value::Object(zone))
}

/// One `STANDARD` or `DAYLIGHT` subcomponent as the RFC 8984 §4.7.2
/// TimeZoneRule it is — the inverse of [`observance`].
///
/// The three properties RFC 5545 §3.6.5 makes REQUIRED are the three JSCalendar
/// makes mandatory, so an observance missing one is not read. `DTSTART` is a
/// local time here and carries no `TZID`: §3.6.5 resolves it against
/// `TZOFFSETFROM`, which is how an observance dates itself in the zone it is
/// defining, and it is why the value is read as a local date-time rather than
/// through the zone lookup every other `DTSTART` in the document goes through.
fn read_observance(component: &ICalendarComponent) -> Option<Value> {
    let mut rule = Map::new();
    rule.insert("@type".to_owned(), json!("TimeZoneRule"));
    rule.insert(
        "start".to_owned(),
        json!(to_local_date_time(&entry_raw_value(component_entry(
            component, "DTSTART"
        )?))?),
    );
    // Through `utc_offset` rather than verbatim: it is the same grammar on both
    // sides, so a value it refuses is one no reader resolves, and a zone
    // described by an offset nobody can read is not a zone.
    let offset_from = utc_offset(&component_text(component, "TZOFFSETFROM")?)?;
    rule.insert("offsetFrom".to_owned(), json!(offset_from));
    rule.insert(
        "offsetTo".to_owned(),
        json!(utc_offset(&component_text(component, "TZOFFSETTO")?)?),
    );

    let rules = component_entries(component, "RRULE")
        .map(|property| {
            // Not a zone but the offset beside it: an observance dates itself in
            // the zone it is defining rather than in one it refers to, so a UTC
            // `UNTIL` here converts with nothing but arithmetic — see
            // [`Ends::At`].
            let recurrence = rrule_to_rule(&entry_raw_value(property), Ends::At(&offset_from))?;
            // A rule that survives this but not the trip back — an end this
            // could not restate, a month `month_token` will not write — costs
            // the whole definition, and does so at [`read_time_zones`], which
            // draws every definition it reads and keeps only what came back.
            // Asking it a second time here would be the same question twice.
            serde_json::to_value(recurrence).ok()
        })
        .collect::<Option<Vec<Value>>>()?;
    if !rules.is_empty() {
        rule.insert("recurrenceRules".to_owned(), Value::Array(rules));
    }

    // What a reader shows for the offset — `CET`, `CEST`. RFC 8984 §4.7.2 keys
    // the names by locale-independent name and holds each as a `true`; RFC 5545
    // §3.8.3.2 distinguishes several `TZNAME`s by `LANGUAGE`, which has nowhere
    // to go, so each name is read plainly and a second spelling of one name is
    // one key.
    let names: Map<String, Value> = component_entries(component, "TZNAME")
        .map(entry_text)
        .filter(|name| !name.is_empty())
        .map(|name| (name, json!(true)))
        .collect();
    if !names.is_empty() {
        rule.insert("names".to_owned(), Value::Object(names));
    }
    Some(Value::Object(rule))
}

/// The zone a `TZID` refers to, as a JSCalendar `TimeZoneId` where the document
/// gives one.
///
/// A value that is already a name is taken at its word — the `VTIMEZONE` beside
/// it is then a description of a zone we already know the name of, and letting a
/// stray `X-LIC-LOCATION` overrule it could move the event to another continent.
/// Otherwise the document's own `VTIMEZONE` is asked, which is where RFC 5545
/// §3.6.5 says the zone a `TZID` refers to is defined.
///
/// A value neither route names is handed back **unchanged**, not dropped. The
/// save path has to be able to tell a zone this mapping cannot name from no
/// zone at all: the latter is a floating event, which is a real thing to save,
/// and reading the former as it would leave the server's zone cleared by an
/// edit that never touched it. Refusing to send it is that path's decision to
/// make, and it needs the value to make it — see `jmap_cal_sync::patch`.
fn zone_of(tzid: &str, zones: &Zones) -> String {
    match names_time_zone(tzid) {
        true => tzid.to_owned(),
        false => zones
            .get(tzid)
            .and_then(|zone| zone.name.clone())
            .unwrap_or_else(|| tzid.to_owned()),
    }
}

/// One `VEVENT` as an event, recurrence rules included and named instances not:
/// those are the document's, not the component's.
fn read_vevent(
    vevent: &ICalendarComponent,
    zones: &Zones,
    components: &[ICalendarComponent],
) -> CalendarEvent {
    let text = |name: &str| component_text(vevent, name).filter(|value| !value.is_empty());
    let (start, time_zone, show_without_time) = read_start(vevent, zones);

    // Only for a component that is actually *in* the zone: a `TZID` beside a
    // DATE value states no zone at all (RFC 5545 §3.2.19), and shifting a
    // floating event's `UNTIL` by it would move an end nothing had placed.
    let definition = time_zone
        .is_some()
        .then(|| {
            component_entry(vevent, "DTSTART")
                .and_then(|property| entry_param(property, "TZID"))
                .and_then(|tzid| zones.get(&tzid))
        })
        .flatten();
    let ends = Ends::In(Zoned {
        name: time_zone.as_deref(),
        observances: definition.map(|zone| zone.observances.as_slice()),
    });
    let rules: Vec<RecurrenceRule> = component_entries(vevent, "RRULE")
        .filter_map(|property| rrule_to_rule(&entry_raw_value(property), ends))
        .collect();

    CalendarEvent {
        id: text("UID").map(Into::into),
        // Membership follows from which EDS source is being served, not from
        // the component, so the backend fills it in on create.
        calendar_ids: None,
        event_type: Some("Event".to_owned()),
        // Only the standalone (top-level) object states its JSCalendar
        // version — jscalendarbis §3.1.2 forbids it on embedded objects, and
        // this constructor also builds recurrence-override instances. The
        // top level is stamped in [`ical_to_event`].
        version: None,
        uid: text(X_JMAP_UID),
        // Written onto the document and never read back off it: both instants
        // are the server's record of the event, so a value read here would be
        // this side proposing one — a guess, on a create, at when the server
        // first saw the event, and a claim, on a save, about when it last
        // changed it. The mirror of `DTEND`, which is read and never written.
        created: None,
        updated: None,
        title: text("SUMMARY"),
        description: text("DESCRIPTION"),
        start,
        time_zone,
        duration: read_duration(vevent),
        show_without_time,
        status: component_text(vevent, "STATUS").and_then(|status| {
            STATUSES
                .iter()
                .find(|(_, ical)| ical.eq_ignore_ascii_case(&status))
                .map(|(jscalendar, _)| (*jscalendar).to_owned())
        }),
        free_busy_status: read_transparency(vevent),
        priority: read_priority(vevent),
        privacy: read_privacy(vevent),
        locations: read_locations(vevent),
        // Read back the way `locations` is: what the line showed, under the key
        // it came out with, so that a save patches `virtualLocations/<key>` and
        // the `description` (RFC 8984 §4.2.6) the line had no room for stays
        // where the server put it. See [`read_virtual_locations`].
        virtual_locations: read_virtual_locations(vevent),
        keywords: read_keywords(vevent),
        alerts: read_alerts(vevent, components),
        // Drawn onto the document and never read back off it, like the two
        // timestamps above and for a heavier reason: who is invited, and what
        // each of them replied, is scheduling state. Changing it means an iTIP
        // REQUEST or REPLY going out to those people (RFC 5546), which this
        // backend does not send — so a save that patched `participants` would
        // rewrite the server's guest list while nobody was told. Reading nothing
        // here is what keeps that impossible: the property is not in
        // `MAPPED_PROPERTIES`, so no save can name it.
        participants: None,
        // Read back the way `virtualLocations` is, and for the same reason: a
        // Link (RFC 8984 §1.4.11) holds a `cid`, a `rel` and a `title` that no
        // `ATTACH` or `IMAGE` line has room for, so what the line showed comes
        // back under the key it came out with and a save patches
        // `links/<key>/href`. See [`read_links`].
        links: read_links(vevent),
        recurrence_rules: (!rules.is_empty()).then_some(rules),
        recurrence_overrides: None,
        // Filled in by [`ical_to_event`] once the event's own zone is known,
        // because whether the document is defining a zone *for us* depends on
        // which identifier the event ended up naming — see [`read_time_zones`].
        // A component read on its own is a component with no `VTIMEZONE` in
        // scope, so `None` is also the honest answer here.
        time_zones: None,
        extra: Default::default(),
    }
}

/// The place the component names, as a one-entry `locations` map.
///
/// The key is the one the `LOCATION` came out with — [`X_JMAP_KEY`], so that a
/// save reaches the server's own entry — or [`INVENTED_KEY`] for a line that
/// carries none, which is what Evolution's appointment editor writes and what
/// any other client's component looks like.
///
/// A key that is not an RFC 8984 §1.4.4 `Id` — up to 255 octets of letters,
/// digits, `-` and `_` — is treated as absent, because this is the one direction
/// where the key may be *created* server-side: a component whose event has no
/// place yet is saved with the property written whole, under the key read here.
/// A key the server would reject there costs the whole `CalendarEvent/set`, and
/// the value came off a content line, so it is not ours to trust. (A key the
/// *server* chose is used as it is, from the event the save is diffed against —
/// see `jmap_cal_sync::patch`.)
///
/// `None` rather than an empty map for a component that names no place: the save
/// path reads an edit off a difference from what was shown, and an empty map
/// would claim the event happens nowhere where the component made no claim.
fn read_locations(vevent: &ICalendarComponent) -> Option<BTreeMap<String, Value>> {
    let property = component_entry(vevent, "LOCATION")?;
    let name = entry_text(property);
    if name.is_empty() {
        return None;
    }
    let key = entry_param(property, X_JMAP_KEY)
        .filter(|key| names_map_entry(key))
        .unwrap_or_else(|| INVENTED_KEY.to_owned());
    Some([(key, json!({"@type": "Location", "name": name}))].into())
}

/// The places the component says the event may be joined online, as a
/// `virtualLocations` map.
///
/// Every `CONFERENCE` is read, since RFC 7986 §5.11 admits the property more
/// than once — and only what the line shows: the `uri`, the `name` a `LABEL`
/// carries and the `features` a `FEATURE` does. A `description` is neither
/// drawn nor read, which is exactly why the save path patches
/// `virtualLocations/<key>/uri` rather than replacing the property: the members
/// with no room on the line stay where the server put them.
///
/// The key is the one the line came out with — [`X_JMAP_KEY`], so that a save
/// reaches the server's own entry — or an invented one for a line carrying
/// none, which is what another client's component looks like. Invented keys
/// avoid the ones the document already named, because two conferences that
/// collided on a key would become one. A key that is not an RFC 8984 §1.4.4
/// `Id` is treated as absent, for the reason [`read_locations`] gives: on a
/// create the key is what the server is asked to file the entry under, and a
/// key it rejects costs the whole `CalendarEvent/set`.
///
/// A line with nowhere to join — no value, or one that is not a URI — is
/// dropped rather than read, since RFC 8984 §4.2.6 makes `uri` the one
/// mandatory member of a VirtualLocation.
///
/// `None` rather than an empty map for a component that names none, for the
/// reason [`read_locations`] gives: the save path reads an edit off a
/// difference from what was shown, and an empty map would claim the event is
/// joined nowhere where the component made no claim at all.
fn read_virtual_locations(vevent: &ICalendarComponent) -> Option<BTreeMap<String, Value>> {
    let lines: Vec<&ICalendarEntry> = component_entries(vevent, "CONFERENCE").collect();
    let keys: Vec<Option<String>> = lines
        .iter()
        .map(|line| entry_param(line, X_JMAP_KEY).filter(|key| names_map_entry(key)))
        .collect();

    let mut places = BTreeMap::new();
    let mut invented = 0;
    for (line, key) in lines.iter().zip(&keys) {
        let value = entry_raw_value(line);
        if !names_a_uri(&value) {
            continue;
        }
        let key = match key {
            Some(key) => key.clone(),
            None => loop {
                invented += 1;
                let key = format!("{INVENTED_CONFERENCE_KEY}{invented}");
                if !keys.iter().any(|k| k.as_deref() == Some(key.as_str())) {
                    break key;
                }
            },
        };
        let mut place = json!({"@type": "VirtualLocation", "uri": value});
        if let Some(name) = entry_param(line, "LABEL").filter(|name| !name.is_empty()) {
            place["name"] = Value::String(name);
        }
        let features: Map<String, Value> = entry_param_values(line, "FEATURE")
            .into_iter()
            .filter_map(|feature| {
                CONFERENCE_FEATURES
                    .iter()
                    .find(|(_, ical)| ical.eq_ignore_ascii_case(&feature))
                    .map(|(jscalendar, _)| ((*jscalendar).to_owned(), Value::Bool(true)))
            })
            .collect();
        if !features.is_empty() {
            place["features"] = Value::Object(features);
        }
        places.insert(key, place);
    }
    (!places.is_empty()).then_some(places)
}

/// The external resources the component points at, as a `links` map.
///
/// Both properties are read — every `ATTACH` (RFC 5545 §3.8.1.1) and every
/// `IMAGE` (RFC 7986 §5.10), which is the split RFC 8984 §4.2.7 keeps in one map
/// — and only what the line shows: the `href` it is made of, the `contentType`
/// an `FMTTYPE` carries, the `size` a `SIZE` does and, for an `IMAGE`, the
/// `display` of §6.1 and the [`ICON_REL`] the property name itself states. A
/// `cid` and a `title` are neither drawn nor read, which is why the save path
/// patches `links/<key>/href` rather than replacing the property: the members
/// with no room on the line stay where the server put them.
///
/// The key is the one the line came out with — [`X_JMAP_KEY`] — or an invented
/// one for a line carrying none, under the same rules
/// [`read_virtual_locations`] gives: invented keys avoid the ones the document
/// already named, and a key that is not an RFC 8984 §1.4.4 `Id` is treated as
/// absent, because on a create the key is what the server is asked to file the
/// entry under.
///
/// Two kinds of line are dropped rather than read, and both would otherwise send
/// the server somewhere it cannot go:
///
/// - **A value that is not a URI**, since RFC 8984 §1.4.11 makes `href` the one
///   mandatory member of a Link. An inline attachment is such a line: a
///   `VALUE=BINARY` `ATTACH` holds the file itself, which [`syntax`] has no text
///   for at all.
/// - **A `file:` URI**, which is where Evolution keeps an attachment the user
///   added from their own disk. Nobody else's client could fetch it, and the path
///   names the user's home directory — so filing it as a Link would put a local
///   path in a record every other client of the account can read. Sending the
///   file itself means uploading it as a blob, which this crate has no part in.
///
/// Neither dropped line is read as a deletion: a missing entry is one the save
/// path says nothing about (see `jmap_cal_sync::patch`).
///
/// `None` rather than an empty map for a component that points nowhere, for the
/// reason [`read_locations`] gives.
///
/// [`syntax`]: crate::syntax
fn read_links(vevent: &ICalendarComponent) -> Option<BTreeMap<String, Value>> {
    let lines: Vec<&ICalendarEntry> = vevent
        .entries
        .iter()
        .filter(|entry| {
            let name = entry.name.as_str();
            name.eq_ignore_ascii_case("ATTACH") || name.eq_ignore_ascii_case("IMAGE")
        })
        .collect();
    let keys: Vec<Option<String>> = lines
        .iter()
        .map(|line| entry_param(line, X_JMAP_KEY).filter(|key| names_map_entry(key)))
        .collect();

    let mut links = BTreeMap::new();
    let mut invented = 0;
    for (line, key) in lines.iter().zip(&keys) {
        let href = entry_raw_value(line);
        if !names_a_uri(&href) || fetched_locally(&href) {
            continue;
        }
        let key = match key {
            Some(key) => key.clone(),
            None => loop {
                invented += 1;
                let key = format!("{INVENTED_LINK_KEY}{invented}");
                if !keys.iter().any(|k| k.as_deref() == Some(key.as_str())) {
                    break key;
                }
            },
        };
        let mut link = json!({"@type": "Link", "href": href});
        if line.name.as_str().eq_ignore_ascii_case("IMAGE") {
            // The property is what says the resource is a picture of the event,
            // so the `rel` it stands for is read off the name — without it a
            // re-drawing would put the picture back on an `ATTACH`.
            link["rel"] = Value::String(ICON_REL.to_owned());
            if let Some(display) = entry_param(line, "DISPLAY").and_then(|display| {
                LINK_DISPLAYS
                    .iter()
                    .find(|(_, ical)| ical.eq_ignore_ascii_case(&display))
            }) {
                link["display"] = Value::String(display.0.to_owned());
            }
        } else if let Some(size) =
            entry_param(line, "SIZE").and_then(|size| size.parse::<u64>().ok())
        {
            // Only on an `ATTACH`: RFC 8607 §4.1 adds the parameter to that
            // property, and RFC 7986 §5.10 admits no `SIZE` on an `IMAGE`, so a
            // drawing never wrote one there.
            link["size"] = Value::from(size);
        }
        if let Some(media_type) = entry_param(line, "FMTTYPE").filter(|name| !name.is_empty()) {
            link["contentType"] = Value::String(media_type);
        }
        links.insert(key, link);
    }
    (!links.is_empty()).then_some(links)
}

/// Whether a URI names a file on this machine rather than a resource anybody
/// could fetch — RFC 8089's `file` scheme, which is what Evolution's attachment
/// store hands out. Compared case-insensitively, since RFC 3986 §3.1 makes a
/// scheme case-insensitive.
fn fetched_locally(href: &str) -> bool {
    href.split_once(':')
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("file"))
}

/// The tags the component carries, as a JSCalendar `keywords` Set.
///
/// Every `CATEGORIES` property is read, not just the first: RFC 5545 §3.8.1.2
/// admits the property more than once in a `VEVENT`, and each holds a `,`-
/// separated list. A set is what both sides mean, so a tag named twice — across
/// the lines or within one — is one member, and the map collapses it.
///
/// An empty value is dropped rather than carried as the empty tag: `CATEGORIES:`
/// and `CATEGORIES:a,,b` state nothing between their separators, which is not the
/// same as stating a tag whose name is nothing.
///
/// Leading and trailing whitespace is trimmed off each tag, and a
/// whitespace-only value is dropped the same way an empty one is, rather than
/// carried literally: writing a tag's edge whitespace back emits it unescaped
/// (`CATEGORIES:0 ` for a tag stated as `"0 "`), and calcard's own parser
/// trims exactly that bare edge whitespace on the next parse — so carrying it
/// here would only defer the trim by one round trip instead of applying it on
/// the first, which is what a fixed point requires. Trimming up front makes
/// what gets read match what a second read would produce anyway. (See
/// [`maps_keyword`], which refuses to draw a now-unreachable whitespace-only
/// tag for the matching reason.)
///
/// `None` rather than an empty map for a component with no tags, for the reason
/// [`read_locations`] gives: the save path reads an edit off a difference from
/// what was shown, and an empty set would claim the event is untagged where the
/// component made no claim at all.
fn read_keywords(vevent: &ICalendarComponent) -> Option<BTreeMap<String, Value>> {
    let tags: BTreeMap<String, Value> = component_entries(vevent, "CATEGORIES")
        .flat_map(entry_texts)
        .map(|tag| tag.trim().to_owned())
        .filter(|tag| !tag.is_empty())
        .map(|tag| (tag, Value::Bool(true)))
        .collect();
    (!tags.is_empty()).then_some(tags)
}

/// Whether a value is an RFC 8984 §1.4.4 `Id`: 1 to 255 octets of letters,
/// digits, `-` and `_`.
fn names_map_entry(value: &str) -> bool {
    (1..=255).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// The event's start as a JSCalendar LocalDateTime, its time zone, and whether
/// it is shown without a time — three answers because all three are read off
/// the one `DTSTART`, and a date-only one changes the other two.
///
/// A `VALUE=DATE` start is how iCalendar spells an all-day event, so it becomes
/// `showWithoutTime`; the start itself is still read as midnight, because that
/// is what the day begins at and dropping it would leave the event with no time
/// at all. Any `TZID` on such a property is ignored — RFC 5545 §3.2.19 says it
/// does not apply to a DATE value — which also keeps the pair symmetric with
/// [`shows_without_time`], the only shape the writer emits.
///
/// A timed start yields `None` rather than `Some(false)`: the RFC 8984 default
/// is false anyway, and the save path reads an edit off a difference from this,
/// so answering `false` where the server said nothing would invent one.
fn read_start(
    vevent: &ICalendarComponent,
    zones: &Zones,
) -> (Option<String>, Option<String>, Option<bool>) {
    let Some(property) = component_entry(vevent, "DTSTART") else {
        return (None, None, None);
    };
    let value = entry_raw_value(property);
    let Some(start) = to_local_date_time(&value) else {
        return (None, None, None);
    };
    // calcard renders a DATE value with no `T` in it, whatever the parameters
    // said, so the value is what decides — a `VALUE=DATE` this mapping did not
    // write and a bare `20260115` mean the same thing to a reader.
    if !value.contains(['T', 't']) {
        return (Some(start), None, Some(true));
    }
    let zone = match value.ends_with('Z') {
        true => Some(UTC.to_owned()),
        false => entry_param(property, "TZID")
            .filter(|zone| !zone.is_empty())
            .map(|zone| zone_of(&zone, zones)),
    };
    (Some(start), zone, None)
}

/// The instances the document names one at a time, as `recurrenceOverrides`.
///
/// An `RDATE` becomes an override that patches nothing — the instance happens as
/// the rules would have it — and an `EXDATE` one that is `excluded`. A value
/// neither property can be read as a date-time is skipped, like any other
/// unreadable value, and `None` rather than an empty map is the answer for a
/// document that names none: the save path reads an edit off a difference from
/// what was shown, so an empty map would be a claim that there are no overrides
/// where the document made no claim at all.
///
/// An `RDATE` of `VALUE=PERIOD` (RFC 5545 §3.8.5.2) states the instance's own
/// length as well as its start, which is iCalendar's way of saying "this extra
/// occurrence runs longer than the rest"; it becomes a `duration` patch, or an
/// empty one where the period is as long as the series already is. See
/// [`period_length`]. An `EXDATE` gets no such treatment: RFC 5545 §3.8.5.1
/// admits no period there, and an instance that does not happen has no length.
///
/// `EXDATE` is read after `RDATE` deliberately. A component naming one instant
/// in both properties contradicts itself, and taking it as excluded is the
/// reading that cannot invent an appointment; RFC 5545 §3.8.5.1 also has an
/// `EXDATE` win over the rest of the recurrence set.
///
/// The detached instances are read last, so a `VEVENT` describing an instant an
/// `RDATE` also names wins: they agree that it happens, and only one of them
/// says how. An `EXDATE` for the same instant is a document contradicting
/// itself the other way round, and the edit is the more specific statement.
///
/// Two are skipped rather than read. One whose `RECURRENCE-ID` is not a
/// date-time names no instance to attach to. One carrying `RANGE=THISANDFUTURE`
/// (RFC 5545 §3.2.13) stands for *every* instance from that one on, which
/// `recurrenceOverrides` has no single entry for; reading it as one would move
/// one day and silently drop the change to all the others.
fn read_overrides(
    series: &ICalendarComponent,
    vevents: &[&ICalendarComponent],
    event: &CalendarEvent,
    zones: &Zones,
    components: &[ICalendarComponent],
) -> Option<BTreeMap<String, Value>> {
    let mut overrides: BTreeMap<String, Value> = BTreeMap::new();
    let values = |name: &str| -> Vec<String> {
        component_entries(series, name)
            .flat_map(entry_texts)
            .collect()
    };
    for value in values("RDATE") {
        // A period is the only RDATE value with a `/` in it, and calcard renders
        // both its spellings that way, so the separator is what decides.
        let (start, length) = match value.split_once('/') {
            Some((start, end)) => (start, Some(period_length(start, end))),
            None => (value.as_str(), None),
        };
        let Some(date) = to_local_date_time(start) else {
            continue;
        };
        // Only a length that *differs* from the series' is an override, the
        // same rule [`instance_patch`] applies to a detached component; a
        // period restating the series' length says nothing new. An unreadable
        // one patches a `null`, which removes the duration rather than letting
        // the instance inherit a length the document did not give it.
        let patch = match length.filter(|length| *length != event.duration) {
            Some(length) => json!({ "duration": length }),
            None => Value::Object(Default::default()),
        };
        overrides.insert(date, patch);
    }
    for value in values("EXDATE") {
        if let Some(date) = to_local_date_time(&value) {
            overrides.insert(date, json!({"excluded": true}));
        }
    }

    for vevent in vevents {
        if std::ptr::eq(*vevent, series) {
            continue;
        }
        let Some(property) = component_entry(vevent, RECURRENCE_ID) else {
            continue;
        };
        if entry_param(property, "RANGE").is_some() {
            continue;
        }
        let Some(id) = to_local_date_time(&entry_raw_value(property)) else {
            continue;
        };
        let patch = instance_patch(event, &read_vevent(vevent, zones, components), &id);
        overrides.insert(id, patch);
    }

    (!overrides.is_empty()).then_some(overrides)
}

/// How a detached instance differs from the series it belongs to, as an RFC 8984
/// §4.3.4 PatchObject.
///
/// Only [`OVERRIDE_PROPERTIES`] are compared, because only those are restated on
/// the component; a property the instance carries and the mapping does not read
/// is invisible here exactly as it is everywhere else. A property the series has
/// and the instance does not comes back as a `null`, which is how a PatchObject
/// removes one — and how the component said it, by not carrying the line.
///
/// `start` is compared against `id` rather than against the series: an override's
/// key *is* its instance's start, so a `DTSTART` equal to it says nothing, and
/// one that differs is an occurrence the user moved.
fn instance_patch(series: &CalendarEvent, instance: &CalendarEvent, id: &str) -> Value {
    let mut patch = Map::new();
    for (name, was, now) in [
        ("title", &series.title, &instance.title),
        ("description", &series.description, &instance.description),
        // A zone belongs to the property that carries it, so this is a real
        // difference and not an inheritance: a component whose `DTSTART` has no
        // `TZID` floats however the series is written, which is the `null` a
        // PatchObject removes a property with.
        ("timeZone", &series.time_zone, &instance.time_zone),
        ("duration", &series.duration, &instance.duration),
        ("status", &series.status, &instance.status),
        (
            "freeBusyStatus",
            &series.free_busy_status,
            &instance.free_busy_status,
        ),
        ("privacy", &series.privacy, &instance.privacy),
    ] {
        if was != now {
            patch.insert(
                (*name).to_owned(),
                now.clone().map_or(Value::Null, Value::String),
            );
        }
    }
    // The one restated property that is not text, compared the same way: an
    // instance whose component carries no `PRIORITY` where the series does is an
    // occurrence the user made unimportant, which is the `null` a PatchObject
    // removes a property with.
    if series.priority != instance.priority {
        patch.insert(
            "priority".to_owned(),
            instance.priority.map_or(Value::Null, Value::from),
        );
    }
    // And the one that is a set, compared the same way: an instance carrying no
    // `CATEGORIES` where the series does is an occurrence the user unfiled, which
    // is the `null` a PatchObject removes a property with. The set is compared
    // whole rather than tag by tag, because that is how the property goes back —
    // an override states what the instance is filed under, not how it differs.
    if series.keywords != instance.keywords {
        patch.insert(
            "keywords".to_owned(),
            match &instance.keywords {
                // Serialising a set this crate's own reader built cannot fail: it
                // holds strings and `true`.
                Some(tags) => serde_json::to_value(tags).unwrap_or(Value::Null),
                None => Value::Null,
            },
        );
    }
    // And the map that is a set of components, compared the same way and whole for
    // the same reason: an instance carrying no `VALARM` where the series does is an
    // occurrence the user is no longer reminded of, which is the `null` a
    // PatchObject removes a property with. Where the series draws none — because
    // RFC 8984 §4.5.1 says nothing reads them — neither does the instance, so the
    // two agree and nothing is stated here at all.
    if series.alerts != instance.alerts {
        patch.insert(
            "alerts".to_owned(),
            match &instance.alerts {
                // Serialising a map this crate's own reader built cannot fail: it
                // holds two objects of strings.
                Some(alerts) => serde_json::to_value(alerts).unwrap_or(Value::Null),
                None => Value::Null,
            },
        );
    }
    if let Some(start) = instance.start.as_deref().filter(|start| *start != id) {
        patch.insert("start".to_owned(), Value::String(start.to_owned()));
    }
    Value::Object(patch)
}

/// How long the event lasts, as a JSCalendar Duration.
///
/// `DURATION` is what this crate writes and it is passed straight back — the
/// two formats spell an ISO 8601 duration identically. But it is *not* what
/// Evolution writes: the appointment editor calls `e_cal_component_set_dtend`,
/// and RFC 5545 §3.6.1 makes `DTEND` and `DURATION` mutually exclusive, so an
/// event a user just created says how long it is only through its end. Reading
/// `DURATION` alone left every such event with no duration at all, which is
/// `P0D` by RFC 8984 §4.2.2 — a zero-length blip in place of the meeting.
///
/// The difference is taken on the wall clock, which is also how JSCalendar
/// reads the answer: its `P1D` is a nominal day, the same time on the next
/// day, rather than 24 exact hours. The two agree for as long as both ends are
/// in one zone, which is the only shape Evolution writes. A `DTEND` in a
/// *different* zone than the `DTSTART` — legal, and not something this mapping
/// can resolve — comes out short or long by the offset between them; the
/// alternative is dropping the length of an event that plainly states it, and
/// a zone database is a dependency this crate does not carry (see
/// [`rule_to_rrule`] for the same trade made about `UNTIL`).
fn read_duration(vevent: &ICalendarComponent) -> Option<String> {
    if let Some(duration) = component_entry(vevent, "DURATION")
        .map(entry_raw_value)
        .and_then(|value| stated_duration(&value))
    {
        return Some(duration);
    }
    let start = instant(&entry_raw_value(component_entry(vevent, "DTSTART")?))?;
    let end = instant(&entry_raw_value(component_entry(vevent, "DTEND")?))?;
    to_duration(end - start)
}

/// A stated length as a JSCalendar Duration, or `None` when the value states no
/// length an event can have. The one check both directions use.
///
/// The two formats spell a length identically, so a value that *is* one is
/// handed over as written rather than re-rendered. But they do not spell the
/// same *set* of lengths: RFC 5545 §3.3.6 admits a sign, and a `DURATION:-PT1H`
/// — an event lasting minus an hour — has nothing to map onto in RFC 8984
/// §1.4.6, while RFC 5545 has no room for a value that is not a duration at all.
/// A value that does not pass is treated as absent, like every other unreadable
/// one, and what that costs differs by direction:
///
/// - **Reading**, [`read_duration`] falls through to `DTEND`, so an event ends
///   up without a length rather than with one nobody can use. Passing it on put
///   a value the server rejects into the save patch, which fails the whole
///   `CalendarEvent/set` and takes the user's real edits down with it.
/// - **Writing**, [`vevent_of`] leaves the property out. libical refusing a
///   content line costs the whole component, so an unwritable length would cost
///   the appointment — every field of it, not just how long it lasts.
/// - **Inside an override**, [`maps_override_field`] refuses it, which tells the
///   save path that this instance was seen in part and its
///   `recurrenceOverrides` must not be written back. The only place an
///   instance's own length is stated is its component's `DURATION`, so a length
///   that cannot go there comes back as the series', which is what the override
///   said it was *not*.
///
/// A leading `+` is the same length said RFC 5545's way, and is dropped rather
/// than handed to a format with nowhere to put it. That is the one value this
/// returns changed rather than verbatim, so an override stating it is still
/// covered: the length that comes back means the same thing, which is what
/// [`maps_override_field`] asks of a field.
///
/// The grammar accepted is deliberately a little looser than RFC 5545's, which
/// nests its units (an hour may be followed only by minutes, a week stands
/// alone): here any of `W D H M S` may be measured, each at most once and in
/// that order, at least one of them, with `T` before the first time unit. That
/// admits `PT1H15S` and `P1W2D`, which every reader adds up the same way and
/// some emitters write. The check exists to refuse values that are not lengths,
/// not to be a conformance test — refusing a length an event plainly states is
/// the failure it is here to avoid, not to cause.
///
/// What it sees is calcard's rendering of the value, not the octets the
/// component carried: a `P1DT` arrives already trimmed to `P1D`. Only what
/// survives *that* is judged here.
fn stated_duration(value: &str) -> Option<String> {
    let value = value.strip_prefix('+').unwrap_or(value);
    let mut rest = value.strip_prefix(['P', 'p'])?;
    let mut measured = false;
    for unit in ['W', 'D', 'T', 'H', 'M', 'S'] {
        if unit == 'T' {
            // Not a unit but the divider before the first of the time ones; it
            // stands only if something is measured after it.
            if let Some(after) = rest.strip_prefix(['T', 't']) {
                rest = after;
                measured = false;
            }
            continue;
        }
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            continue;
        }
        let Some(after) = rest[digits..].strip_prefix([unit, unit.to_ascii_lowercase()]) else {
            continue;
        };
        rest = after;
        measured = true;
    }
    (measured && rest.is_empty()).then(|| value.to_owned())
}

/// How long a period lasts, as a JSCalendar Duration, given its two halves.
///
/// RFC 5545 §3.3.9 spells a period either way — `19960403T020000Z/PT3H` or
/// `19960403T020000Z/19960403T050000Z` — and the two halves of this mapping
/// answer them exactly as [`read_duration`] answers `DURATION` and `DTEND`: a
/// stated duration goes through [`stated_duration`], because both formats spell
/// an ISO 8601 duration identically but not the same set of them, and a stated
/// end is measured on the wall clock.
///
/// `None` is "this period states no length an occurrence could have": a period
/// that ends at or before it starts, and a half that is not a length — RFC 5545
/// §3.3.9 requires the end to be after the start, and RFC 8984 §1.4.6 has no
/// negative duration to map one onto. The negative case falls out of the second
/// branch for free: `-PT1H` is not a `P`, and it is not a date-time either.
///
/// A duration stated as zero is *not* caught, because catching it would mean
/// parsing the value rather than passing it through — `PT0S`, `P0D` and
/// `PT0H0M0S` all spell it. It comes back as written, which RFC 8984 §4.2.2
/// reads as the same zero length the `None` answer leaves behind; the two
/// spellings differ on paper and not in the calendar.
fn period_length(start: &str, end: &str) -> Option<String> {
    if end.starts_with(['P', 'p']) {
        return stated_duration(end);
    }
    to_duration(instant(end)? - instant(start)?)
}

/// A `DTSTART`/`DTEND` value as seconds from 1970-01-01T00:00:00 on its own
/// wall clock — a number to subtract, not an instant on any timeline.
fn instant(value: &str) -> Option<i64> {
    let local = to_local_date_time(value)?;
    let fields: Vec<i64> = local
        .split(['-', 'T', ':'])
        .filter_map(|field| field.parse().ok())
        .collect();
    // Six fields, and each parses, because `to_local_date_time` wrote them.
    let [year, month, day, hour, minute, second] = fields[..] else {
        return None;
    };
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Days from 1970-01-01 to a proleptic Gregorian date, by Howard Hinnant's
/// `days_from_civil`. Exact for every year either format can spell.
pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    // Count the year as starting in March, which puts a leap day at the end of
    // it and so needs no special case.
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// `5400` → `PT1H30M`, and `86400` → `P1D`.
///
/// Whole days are named as days rather than as 24 hours each, because that is
/// what was measured: the difference came off a wall clock, and JSCalendar's
/// day is the nominal one that survives a daylight saving change.
///
/// A length that is zero or negative yields nothing. Zero is the RFC 8984
/// default anyway, and a negative duration — an event ending before it begins
/// — is a component saying nothing usable about its length, which is better
/// answered with silence than with a value the server would reject or, worse,
/// accept.
fn to_duration(seconds: i64) -> Option<String> {
    if seconds <= 0 {
        return None;
    }
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let mut duration = String::from("P");
    if days > 0 {
        duration.push_str(&format!("{days}D"));
    }
    if rest > 0 {
        duration.push('T');
        for (amount, unit) in [(rest / 3_600, 'H'), (rest / 60 % 60, 'M'), (rest % 60, 'S')] {
            if amount > 0 {
                duration.push_str(&format!("{amount}{unit}"));
            }
        }
    }
    Some(duration)
}

/// Whether this event can be written the way iCalendar writes an all-day one:
/// a `DTSTART` of `VALUE=DATE`, with no time anywhere on the component.
///
/// `showWithoutTime` asks for it, but cannot on its own get it. RFC 8984 §4.1.5
/// says an event shown without a time starts at midnight and lasts whole days;
/// a server is free to send otherwise, and RFC 5545 has no way to write the
/// result — a DATE value has no time to hold 09:00, takes no `TZID`
/// (§3.2.19), and stands only beside a duration of whole days (§3.6.1) and an
/// `UNTIL` that is itself a DATE (§3.3.10). So each of those is a condition
/// here, and an event failing any of them is written as the timed event it
/// half is: wrong about its day-ness, right about when it happens, and — since
/// the save path diffs against this same rendering — not read back as the user
/// having cleared the flag.
///
/// `start` is the already-rendered `DTSTART` value, `YYYYMMDDTHHMMSS`.
fn shows_without_time(event: &CalendarEvent, start: &str) -> bool {
    event.show_without_time == Some(true)
        && event.time_zone.is_none()
        && at_midnight(start)
        && event
            .duration
            .as_deref()
            .filter(|duration| !duration.is_empty())
            .is_none_or(whole_days)
        && event
            .recurrence_rules
            .iter()
            .flatten()
            .filter(|rule| writable(rule))
            .all(|rule| {
                rule.until
                    .as_deref()
                    .and_then(to_ical_date_time)
                    .is_none_or(|until| at_midnight(&until))
                    // §3.3.10 forbids `BYHOUR`, `BYMINUTE` and `BYSECOND` beside
                    // a DATE-valued `DTSTART`, and a rule naming a time of day is
                    // not something a day with no clock can hold. Asked of the
                    // parts [`rule_to_rrule`] would actually write, not of the
                    // properties: a time it refuses is not on the component to
                    // contradict the DATE form, and such an event keeps its
                    // day-ness.
                    && !names_a_time_of_day(rule)
            })
        // An instance named at 09:00 cannot be truncated to its date without
        // excluding — or adding — a different occurrence than the server named.
        && event
            .recurrence_overrides
            .iter()
            .flatten()
            .all(|(id, patch)| instance_shows_without_time(event, id, patch))
}

/// Whether the `RRULE` this rule is written as carries a time of day — a
/// `BYSECOND`, a `BYMINUTE` or a `BYHOUR`, which RFC 5545 §3.3.10 forbids beside a
/// `DTSTART` of value type DATE.
///
/// Asked of the parts rather than of the properties, for the reason
/// [`shows_without_time`] gives at the call site.
fn names_a_time_of_day(rule: &RecurrenceRule) -> bool {
    by_second_part(rule).is_some() || by_minute_part(rule).is_some() || by_hour_part(rule).is_some()
}

/// Whether one override can be named — and, where it is drawn as a component of
/// its own, written whole — without a time.
///
/// An id no property could carry is dropped rather than written, so it puts no
/// condition on the form. An id that *is* written has to land on a day, and an
/// instance with a component of its own has to meet the same conditions the
/// series does: a start at midnight, a length in whole days and no zone of its
/// own, since RFC 5545 §3.6.1 lets nothing else stand beside a DATE-valued
/// `DTSTART` and §3.2.19 gives such a value no `TZID` to carry a zone on.
fn instance_shows_without_time(event: &CalendarEvent, id: &str, patch: &Value) -> bool {
    let Some(rendered) = to_ical_date_time(id) else {
        return true;
    };
    if !at_midnight(&rendered) {
        return false;
    }
    let Some(instance) = modified_instance(event, id, patch) else {
        return true;
    };
    instance.time_zone.is_none()
        && instance
            .start
            .as_deref()
            .and_then(to_ical_date_time)
            .is_none_or(|start| at_midnight(&start))
        && instance
            .duration
            .as_deref()
            .filter(|duration| !duration.is_empty())
            .is_none_or(whole_days)
}

/// Whether a rendered `YYYYMMDDTHHMMSS` names the top of its day.
fn at_midnight(value: &str) -> bool {
    value.ends_with("T000000")
}

/// Whether an ISO 8601 duration is a whole number of days — RFC 5545's
/// `dur-day`/`dur-week`, the only lengths that may stand beside a DATE start.
///
/// Anything after the designator's `T` is a time component, so its absence is
/// the whole test; a negative duration (a leading `-`) is not a length an event
/// can have and fails here with the rest.
fn whole_days(duration: &str) -> bool {
    let Some(parts) = duration.strip_prefix(['P', 'p']) else {
        return false;
    };
    !parts.is_empty() && !parts.contains(['T', 't'])
}

fn is_utc(zone: &str) -> bool {
    zone.eq_ignore_ascii_case(UTC) || zone.eq_ignore_ascii_case("UTC")
}

/// `2026-01-15T13:00:00Z` → `20260115T130000Z`, or `None` for a value that names
/// no instant.
///
/// The UTC-only sibling of [`to_ical_date_time`]: RFC 8984 §1.4.5's UTCDateTime
/// and the UTC form of RFC 5545 §3.3.5's DATE-TIME are the same instant written
/// two ways, and the `Z` is what makes each of them one — so a value without it
/// is a local time, which these properties have no zone to resolve against and
/// are refused rather than guessed at. A sub-second fraction, which neither
/// format's DATE-TIME carries, falls out of [`strip`]'s digit count and is
/// refused the same way.
pub(crate) fn to_utc_date_time(value: &str) -> Option<String> {
    let local = value.strip_suffix(['Z', 'z'])?;
    to_ical_date_time(local).map(|stamp| format!("{stamp}Z"))
}

/// `2026-01-15T13:00:00` → `20260115T130000`.
fn to_ical_date_time(local: &str) -> Option<String> {
    let (date, time) = local.split_once('T')?;
    let date: String = strip(date, '-', 8)?;
    let time: String = strip(time, ':', 6)?;
    exists(&date, &time).then(|| format!("{date}T{time}"))
}

/// Whether `value` has the shape of a DATE-TIME or DATE (`YYYYMMDD`, an
/// optional `T` and `HHMMSS`, an optional trailing `Z`) — digits in all the
/// right places, whether or not they name an instant that exists. Splits
/// "not shaped like a date-time at all" (`"whenever"`) from "shaped like one
/// but naming no real instant" (`"20261315T000000Z"`, month 13): the two get
/// different treatment in [`rrule_to_rule`] and [`to_local_date_time`] means
/// the latter either way, so this is the shape check factored out of it.
fn date_time_digits(value: &str) -> Option<(&str, &str)> {
    let value = value.strip_suffix(['Z', 'z']).unwrap_or(value);
    let (date, time) = match value.split_once('T') {
        Some((date, time)) => (date, time),
        None => (value, "000000"),
    };
    if date.len() != 8 || time.len() < 6 || !date.is_char_boundary(8) || !time.is_char_boundary(6) {
        return None;
    }
    // Sub-second precision is legal in neither format's DATE-TIME, but a
    // trailing fraction is easy to ignore and hard to guess at.
    let time = &time[..6];
    if !date.bytes().chain(time.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((date, time))
}

/// `20260115T130000`, `20260115T130000Z` or `20260115` (`VALUE=DATE`) →
/// `2026-01-15T13:00:00`. A date without a time is read as midnight:
/// `showWithoutTime` is not modeled yet, and an all-day event that lost its
/// start entirely would be worse than one pinned to the top of the day.
pub(crate) fn to_local_date_time(value: &str) -> Option<String> {
    let (date, time) = date_time_digits(value)?;
    if !exists(date, time) {
        return None;
    }
    Some(format!(
        "{}-{}-{}T{}:{}:{}",
        &date[..4],
        &date[4..6],
        &date[6..],
        &time[..2],
        &time[2..4],
        &time[4..],
    ))
}

/// Remove `separator` and check that exactly `digits` digits are left.
pub(crate) fn strip(value: &str, separator: char, digits: usize) -> Option<String> {
    let stripped: String = value.chars().filter(|c| *c != separator).collect();
    (stripped.len() == digits && stripped.bytes().all(|b| b.is_ascii_digit())).then_some(stripped)
}

/// Whether `YYYYMMDD` and `HHMMSS` digits name an instant that exists.
///
/// Digits in the right places are not a date, and neither format's reader
/// checks which: calcard reads `20261315T250000` into a date-time whose month
/// is 13, and libical is asked for the value only after this mapping has
/// written it. So the check is here, and a month of 13 is treated the same way
/// as a value that cannot be read at all — as absent — because both directions
/// are worse than losing the property. Handed to libical, an impossible
/// `DTSTART` costs the whole component and with it every field of the event;
/// sent to the server, `"start": "2026-13-15T25:00:00"` is not a JSCalendar
/// LocalDateTime and fails the entire `CalendarEvent/set`, so the user's edit
/// to the title is lost along with it.
///
/// Both callers have already established that the arguments are 8 and 6 ASCII
/// digits.
pub(crate) fn exists(date: &str, time: &str) -> bool {
    let field = |value: &str| value.parse::<u32>().unwrap_or(u32::MAX);
    let (year, month, day) = (field(&date[..4]), field(&date[4..6]), field(&date[6..8]));
    let (hour, minute, second) = (field(&time[..2]), field(&time[2..4]), field(&time[4..6]));
    (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && hour <= 23
        && minute <= 59
        // RFC 5545 §3.3.12 and RFC 3339 §5.6 both allow the leap second, and a
        // server that stores one has to get it back unchanged.
        && second <= 60
}

/// The length of a month, in the proleptic Gregorian calendar both formats use.
pub(crate) fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}

/// `("1973-04-29T07:00:00", "-0500")` → `1973-04-29T02:00:00`: the instant a
/// UTC date-time names, restated where the offset from UTC is fixed at
/// `offset`.
fn at_offset(utc: &str, offset: &str) -> Option<String> {
    moved(utc, offset_seconds(offset)?)
}

/// The inverse: the UTC date-time a local time in a fixed-offset zone names.
fn from_offset(local: &str, offset: &str) -> Option<String> {
    moved(local, -offset_seconds(offset)?)
}

/// An RFC 5545 §3.3.14 UTC offset as a count of seconds east of UTC, through
/// [`utc_offset`] so that only a spelling both formats admit is read at all.
pub(crate) fn offset_seconds(offset: &str) -> Option<i64> {
    let offset = utc_offset(offset)?;
    let (sign, digits) = offset.split_at_checked(1)?;
    let field = |at: usize| digits.get(at..at + 2)?.parse::<i64>().ok();
    let magnitude = field(0)? * 3600 + field(2)? * 60 + field(4).unwrap_or(0);
    Some(match sign {
        "-" => -magnitude,
        _ => magnitude,
    })
}

/// A LocalDateTime moved by `seconds`, in the proleptic Gregorian calendar both
/// formats count in.
///
/// The carry is a day either way and no more, because [`utc_offset`] holds an
/// offset under 24 hours — which is why this is [`days_in_month`] and a borrow
/// rather than a conversion to a count of days since some epoch and back. A
/// leap second is carried into the following minute, the shift being arithmetic
/// on the instant; and a result outside the four-digit years RFC 5545 §3.3.4
/// admits is no date-time either format can state, so it is refused rather than
/// written with a year no reader would parse.
fn moved(local: &str, seconds: i64) -> Option<String> {
    let (date, time) = local.split_once('T')?;
    let field = |value: Option<&str>| value?.parse::<i64>().ok();
    let (mut year, mut month, mut day) = (
        field(date.get(..4))?,
        field(date.get(5..7))?,
        field(date.get(8..10))?,
    );
    let of_day =
        field(time.get(..2))? * 3600 + field(time.get(3..5))? * 60 + field(time.get(6..8))?;

    const DAY: i64 = 24 * 60 * 60;
    let of_day = match of_day + seconds {
        moved if moved < 0 => {
            (year, month, day) = match (month, day) {
                (1, 1) => (year - 1, 12, 31),
                (_, 1) => (year, month - 1, days_in_month_of(year, month - 1)?),
                _ => (year, month, day - 1),
            };
            moved + DAY
        }
        moved if moved >= DAY => {
            (year, month, day) = match day == days_in_month_of(year, month)? {
                false => (year, month, day + 1),
                true if month == 12 => (year + 1, 1, 1),
                true => (year, month + 1, 1),
            };
            moved - DAY
        }
        moved => moved,
    };

    (0..=9999).contains(&year).then(|| {
        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
            of_day / 3600,
            of_day / 60 % 60,
            of_day % 60,
        )
    })
}

/// [`days_in_month`] on the signed fields [`moved`] carries, refusing the year
/// that has stepped outside what a date-time can state rather than wrapping it.
fn days_in_month_of(year: i64, month: i64) -> Option<i64> {
    let length = days_in_month(u32::try_from(year).ok()?, u32::try_from(month).ok()?);
    (length > 0).then_some(i64::from(length))
}

/// An `RRULE` value, or `None` for a rule [`writable`] refuses.
///
/// A rule carrying unmodeled parts in `extra` *is* written, narrowed to what an
/// `RRULE` holds — showing a weekly event on the wrong days beats showing none
/// — and [`maps_recurrence_rule`] is how the save path knows not to write that
/// narrowing back.
fn rule_to_rrule(rule: &RecurrenceRule, ends: Ends, as_a_date: bool) -> Option<String> {
    if !writable(rule) {
        return None;
    }
    let mut parts = vec![format!("FREQ={}", rule.frequency.to_ascii_uppercase())];
    // The end comes before the interval, which is the order libical writes the
    // two in (measured in `jmap-backend-cal/tests/marshal.rs`) — the same reason
    // the `BYxxx` parts below are emitted in its order rather than the RFC's
    // grammar order.
    if let Some(count) = rule.count {
        parts.push(format!("COUNT={count}"));
    }
    if let Some(until) = rule.until.as_deref() {
        // JSCalendar's `until` is a local time in the event's own zone, so it
        // is spelled the way DTSTART is — which RFC 5545 §3.3.10 also requires
        // of its value *type*, hence the date-only form for an event written as
        // a date. The time dropped there is midnight, because
        // [`shows_without_time`] does not choose that form otherwise.
        //
        // §3.3.10 asks for a UTC instant when DTSTART carries a TZID;
        // converting one would need a zone database, which this crate
        // deliberately does not depend on, so a zoned event's UNTIL stays
        // local. It round-trips, and libical reads it in the event's zone.
        //
        // An observance's does convert, and is converted: RFC 5545 §3.6.5's own
        // examples state one as a UTC instant, which is what every producer of
        // a `VTIMEZONE` writes — tzdata's, libical's, an Exchange invitation's
        // — so drawing the local form §3.3.10's value-type rule would also
        // admit means handing readers a spelling they may never have met, for
        // nothing. See [`Ends::At`].
        let (stated, suffix) = match ends {
            Ends::At(offset) => (from_offset(until, offset), "Z"),
            Ends::In(zone) if zone.name.is_some_and(is_utc) => (Some(until.to_owned()), "Z"),
            Ends::In(_) => (Some(until.to_owned()), ""),
        };
        // Only the conversion can fail here — [`writable`] has already read the
        // value — and a rule that loses the end it was given is one that never
        // stops, so the whole line goes rather than an unbounded one.
        let until = stated.as_deref().and_then(to_ical_date_time)?;
        parts.push(match as_a_date {
            true => format!("UNTIL={}", &until[..8]),
            false => format!("UNTIL={until}{suffix}"),
        });
    }
    // INTERVAL=1 is the RFC 5545 default and only makes the line longer.
    if let Some(interval) = rule.interval.filter(|interval| *interval != 1) {
        parts.push(format!("INTERVAL={interval}"));
    }
    // Last, where RFC 5545's own examples put them, and in the order libical
    // writes them — so a rule that went out this way and came back through EDS's
    // own cache compares equal to itself.
    let named = named_by_parts(rule);
    // `BYSETPOS` selects out of what the parts above expand to, so whether it
    // may be written depends on whether any of them was — see
    // [`by_set_position_part`].
    let selects_from_a_set = !named.is_empty();
    parts.extend(named);
    parts.extend(by_set_position_part(rule, selects_from_a_set));
    parts.extend(first_day_of_week_part(rule));
    Some(parts.join(";"))
}

/// The `BYxxx` parts of a rule's `RRULE` that name a date or a time of day, in
/// the order libical writes them — everything but `BYSETPOS`, which names
/// neither and picks from what these produce.
///
/// The three that name a time of day come first, finest unit outwards, ahead of
/// the days: that is where libical puts them (measured in
/// `jmap-backend-cal/tests/marshal.rs`), and it is not where the parts added
/// before them went.
fn named_by_parts(rule: &RecurrenceRule) -> Vec<String> {
    [
        by_second_part(rule),
        by_minute_part(rule),
        by_hour_part(rule),
        by_day_part(rule),
        by_month_day_part(rule),
        by_year_day_part(rule),
        by_week_no_part(rule),
        by_month_part(rule),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The `BYHOUR` part of a rule's `RRULE`, or `None` when the rule names no hours
/// — and, as with [`by_day_part`], when it names ones this mapping will not
/// write. RFC 5545 §3.3.10's `hour` is 0 to 23.
fn by_hour_part(rule: &RecurrenceRule) -> Option<String> {
    time_of_day_part("BYHOUR", rule.by_hour.as_deref(), 23)
}

/// The `BYMINUTE` part, on the same terms. §3.3.10's `minutes` is 0 to 59.
fn by_minute_part(rule: &RecurrenceRule) -> Option<String> {
    time_of_day_part("BYMINUTE", rule.by_minute.as_deref(), 59)
}

/// The `BYSECOND` part, on the same terms. §3.3.10's `seconds` is 0 to **60**,
/// one more than a minute holds: the sixtieth is the leap second UTC
/// occasionally inserts, and libical accepts it (measured in
/// `jmap-backend-cal/tests/marshal.rs`), so this does too.
fn by_second_part(rule: &RecurrenceRule) -> Option<String> {
    time_of_day_part("BYSECOND", rule.by_second.as_deref(), 60)
}

/// One of the three parts that name a time of day, written as `NAME=v,v,v` — or
/// `None` for a set this mapping will not write, which is what
/// [`maps_recurrence_rule`] reads the answer for.
///
/// It is all the values or none of them, for the reason [`by_day_part`] gives: a
/// part holding a subset names *different* times rather than fewer of these. A
/// value above `largest` is outside the range RFC 5545 §3.3.10 gives the part, and
/// costs libical the **whole** `RRULE` rather than the one value — the event would
/// reach EDS's cache as a single appointment with the user's series gone.
///
/// There is no frequency gate. §3.3.10 defines all three at every frequency —
/// expanding a longer period into several times within it, and limiting the
/// occurrences a shorter one produces. The gate they do need is on the *event*,
/// not the rule: §3.3.10 forbids them beside a DATE-valued `DTSTART`, and
/// [`shows_without_time`] is where that is honoured, by drawing an all-day event
/// whose rule names a time as a timed one. libical keeps the contradiction rather
/// than objecting to it (`jmap-backend-cal/tests/marshal.rs`), so nothing below
/// this mapping would.
fn time_of_day_part(name: &str, values: Option<&[u32]>, largest: u32) -> Option<String> {
    let values = values?;
    // An empty part names no time — and libical reads `BYHOUR=` as `BYHOUR=0`,
    // moving every occurrence of the series, which is worse than a part left off.
    if values.is_empty() || values.iter().any(|value| *value > largest) {
        return None;
    }
    let tokens: Vec<String> = values.iter().map(u32::to_string).collect();
    Some(format!("{name}={}", tokens.join(",")))
}

/// The `BYDAY` part of a rule's `RRULE`, or `None` when the rule names no days
/// — and also when it names days this mapping will not write, which is what
/// [`maps_recurrence_rule`] reads the answer for.
///
/// It is all the days or none of them. A `BYDAY` holding a subset is a
/// *different* recurrence, not a narrower view of one: dropping `2MO` from
/// `BYDAY=2MO,TH` leaves an event that no longer happens on the Monday at all,
/// which is a worse lie than an event shown on every day of the week the series
/// starts on.
fn by_day_part(rule: &RecurrenceRule) -> Option<String> {
    let days = rule.by_day.as_ref()?;
    // `BYDAY=` names no day and is not a rule part any reader can use.
    if days.is_empty() {
        return None;
    }
    let tokens: Option<Vec<String>> = days
        .iter()
        .map(|day| by_day_token(day, &rule.frequency))
        .collect();
    Some(format!("BYDAY={}", tokens?.join(",")))
}

/// One weekday as an `RRULE` writes it — `TH`, `2WE`, `-1FR` — or `None` for
/// an NDay no `BYDAY` can carry.
///
/// The ordinal is refused outright unless the frequency gives it a period to
/// count within: RFC 5545 §3.3.10 says `BYDAY` MUST NOT carry a numeric value
/// when `FREQ` is not `MONTHLY` or `YEARLY`, and a content line libical refuses
/// costs the whole component — every field of the event, not just its
/// recurrence. Writing the weekday without its ordinal is not the fallback,
/// because "the second Monday" and "every Monday" are different events and the
/// second one fills the user's calendar.
fn by_day_token(day: &NDay, frequency: &str) -> Option<String> {
    if !day.extra.is_empty() {
        return None;
    }
    let weekday = WEEKDAYS
        .iter()
        .find(|weekday| weekday.eq_ignore_ascii_case(&day.day))?;
    match day.nth_of_period {
        None => Some((*weekday).to_owned()),
        // RFC 8984 §4.3.3 forbids zero, and RFC 5545's ordwk starts at 1.
        Some(0) => None,
        Some(nth) if counts_within_a_period(frequency) => Some(format!("{nth}{weekday}")),
        Some(_) => None,
    }
}

/// Whether a frequency gives an `nthOfPeriod` a period to count within — the
/// `MONTHLY`/`YEARLY` of RFC 5545 §3.3.10.
fn counts_within_a_period(frequency: &str) -> bool {
    ["monthly", "yearly"]
        .iter()
        .any(|period| period.eq_ignore_ascii_case(frequency))
}

/// The `BYMONTHDAY` part of a rule's `RRULE`, or `None` when the rule names no
/// days of the month — and, as with [`by_day_part`], when it names ones this
/// mapping will not write.
///
/// It is all the days or none of them, for the same reason: a `BYMONTHDAY`
/// holding a subset is a different recurrence, not a narrower view of one.
fn by_month_day_part(rule: &RecurrenceRule) -> Option<String> {
    let days = rule.by_month_day.as_ref()?;
    // `BYMONTHDAY=` names no day, and a week is not a period a day of the month
    // sits inside: RFC 5545 §3.3.10 says the part MUST NOT be specified when
    // `FREQ` is `WEEKLY`, and a content line libical refuses costs the whole
    // component.
    if days.is_empty() || "weekly".eq_ignore_ascii_case(&rule.frequency) {
        return None;
    }
    let tokens: Option<Vec<String>> = days.iter().copied().map(month_day_token).collect();
    Some(format!("BYMONTHDAY={}", tokens?.join(",")))
}

/// One day of the month as an `RRULE` writes it — `15`, `-1` — or `None` for a
/// value no `BYMONTHDAY` can carry.
fn month_day_token(day: i32) -> Option<String> {
    match day {
        // RFC 5545's `ordmoday` is 1 to 31, which RFC 8984 §4.3.3 counts
        // backwards from the end of the month as well. Zero is no day of any
        // month, and neither format admits it.
        -31..=-1 | 1..=31 => Some(day.to_string()),
        _ => None,
    }
}

/// The `BYYEARDAY` part of a rule's `RRULE`, or `None` when the rule names no
/// days of the year — and, as with [`by_day_part`], when it names ones this
/// mapping will not write.
///
/// It is all the days or none of them, for the same reason: a `BYYEARDAY`
/// holding a subset is a different recurrence, not a narrower view of one.
///
/// The frequency gate names the three periods RFC 5545 §3.3.10 excludes rather
/// than allowing `YEARLY` alone: `BYYEARDAY` MUST NOT be specified beside
/// `DAILY`, `WEEKLY` or `MONTHLY` — none of which holds a year — but it *is*
/// defined beside `HOURLY`, `MINUTELY` and `SECONDLY`, limiting the occurrences
/// those expand to.
fn by_year_day_part(rule: &RecurrenceRule) -> Option<String> {
    let days = rule.by_year_day.as_ref()?;
    // `BYYEARDAY=` names no day, and a content line libical refuses costs the
    // whole component — every field of the event, not just its recurrence.
    if days.is_empty() || !holds_a_year(&rule.frequency) {
        return None;
    }
    let tokens: Option<Vec<String>> = days.iter().copied().map(year_day_token).collect();
    Some(format!("BYYEARDAY={}", tokens?.join(",")))
}

/// Whether a frequency leaves room for a day of the year — everything outside
/// the `DAILY`/`WEEKLY`/`MONTHLY` column of RFC 5545 §3.3.10's table.
fn holds_a_year(frequency: &str) -> bool {
    !["daily", "weekly", "monthly"]
        .iter()
        .any(|period| period.eq_ignore_ascii_case(frequency))
}

/// One day of the year as an `RRULE` writes it — `100`, `-1` — or `None` for a
/// value no `BYYEARDAY` can carry.
fn year_day_token(day: i32) -> Option<String> {
    match day {
        // RFC 5545's `yeardaynum` is 1 to 366 — 366 for the leap day — which
        // RFC 8984 §4.3.3 counts backwards from 31 December as well. Zero is no
        // day of any year, and neither format admits it.
        -366..=-1 | 1..=366 => Some(day.to_string()),
        _ => None,
    }
}

/// The `BYWEEKNO` part of a rule's `RRULE`, or `None` when the rule names no
/// weeks of the year — and, as with [`by_day_part`], when it names ones this
/// mapping will not write.
///
/// It is all the weeks or none of them, for the same reason: a `BYWEEKNO`
/// holding a subset is a different recurrence, not a narrower view of one.
///
/// The frequency gate is the narrowest of any part here, and names the one
/// frequency that is *allowed* rather than the ones that are not: RFC 5545
/// §3.3.10 says `BYWEEKNO` MUST NOT be specified when `FREQ` is anything other
/// than `YEARLY` — not even beside `HOURLY`, where [`by_year_day_part`] is
/// defined. A content line libical refuses costs the whole component.
///
/// Which days each week holds is decided by the rule's `firstDayOfWeek`, which
/// [`first_day_of_week_part`] carries; §3.3.10 numbers the weeks by ISO 8601
/// from that day. Carrying this part while that one was unmodeled would have
/// shown weeks counted from a day the server never named.
fn by_week_no_part(rule: &RecurrenceRule) -> Option<String> {
    let weeks = rule.by_week_no.as_ref()?;
    // `BYWEEKNO=` names no week, and a content line libical refuses costs the
    // whole component — every field of the event, not just its recurrence.
    if weeks.is_empty() || !"yearly".eq_ignore_ascii_case(&rule.frequency) {
        return None;
    }
    let tokens: Option<Vec<String>> = weeks.iter().copied().map(week_no_token).collect();
    Some(format!("BYWEEKNO={}", tokens?.join(",")))
}

/// One week of the year as an `RRULE` writes it — `20`, `-1` — or `None` for a
/// value no `BYWEEKNO` can carry.
fn week_no_token(week: i32) -> Option<String> {
    match week {
        // RFC 5545's `ordwk` is 1 to 53 — 53 for the week a long year has and a
        // short one does not — which RFC 8984 §4.3.3 counts backwards from the
        // end of the year as well. Zero is no week of any year, and neither
        // format admits it.
        -53..=-1 | 1..=53 => Some(week.to_string()),
        _ => None,
    }
}

/// The `BYMONTH` part of a rule's `RRULE`, or `None` when the rule names no
/// months of the year — and, as with [`by_day_part`], when it names ones this
/// mapping will not write.
///
/// It is all the months or none of them, for the same reason: a `BYMONTH`
/// holding a subset is a different recurrence, not a narrower view of one.
///
/// There is no frequency gate. RFC 5545 §3.3.10 defines `BYMONTH` at every
/// frequency — limiting the occurrences a shorter period expands to, rather than
/// expanding them — unlike `BYMONTHDAY`, which a week has no room for.
fn by_month_part(rule: &RecurrenceRule) -> Option<String> {
    let months = rule.by_month.as_ref()?;
    // `BYMONTH=` names no month and is not a rule part any reader can use.
    if months.is_empty() {
        return None;
    }
    let tokens: Option<Vec<&str>> = months.iter().map(|month| month_token(month)).collect();
    Some(format!("BYMONTH={}", tokens?.join(",")))
}

/// One month of the year as an `RRULE` writes it, or `None` for a value no
/// `BYMONTH` this mapping is willing to write can carry.
///
/// The value that comes back is the caller's own string, because the only
/// spelling accepted is the one iCalendar writes back unchanged: RFC 5545's
/// `monthnum` also admits a leading zero (`03`), which both libical and calcard
/// re-render as `3` — a rule written that way would return spelled differently
/// and read as an edit the user never made.
///
/// A leap month — RFC 8984 §4.3.3's `5L`, the reason `byMonth` holds strings at
/// all — is refused rather than written: iCalendar can only name one under
/// RFC 7529's `RSCALE`, and this mapping does not model an event's calendar
/// system, so `BYMONTH=5L` beside a Gregorian series would name a month that
/// series does not have.
///
/// Every other refusal is a month no year has, and libical answers those two
/// ways — dropping the whole `RRULE`, or keeping a rule that can never occur.
/// `jmap-backend-cal/tests/marshal.rs` records which is which.
fn month_token(month: &str) -> Option<&str> {
    match month.parse::<u32>() {
        Ok(number @ 1..=12) if month == number.to_string() => Some(month),
        _ => None,
    }
}

/// The `BYSETPOS` part of a rule's `RRULE`, or `None` when the rule selects no
/// occurrence — and, as with [`by_day_part`], when it selects ones this mapping
/// will not write.
///
/// It is all the positions or none of them, for the same reason the other parts
/// are all-or-nothing: a `BYSETPOS` holding a subset picks *different*
/// occurrences out of the set rather than showing fewer of these.
///
/// The gate has no counterpart in the parts above, because this is the one part
/// that names no date of its own: RFC 5545 §3.3.10 says `BYSETPOS` MUST only be
/// used together with another `BYxxx` part, since it selects out of the set
/// those expand to. So `selects_from_a_set` is asked of the parts
/// [`rule_to_rrule`] actually *wrote*, not of the ones the rule holds — a
/// `byWeekNo` beside a monthly frequency is left off, and a `BYSETPOS` written
/// next to it would be selecting from a set no reader of the line can see.
/// Alone, `BYSETPOS=1` would be a no-op and `BYSETPOS=2` a series that never
/// happens again, so the part is left off and [`maps_recurrence_rule`] tells the
/// save path the rule was seen in part.
///
/// There is no frequency gate: §3.3.10 defines the part at every frequency, and
/// libical keeps it beside each one.
fn by_set_position_part(rule: &RecurrenceRule, selects_from_a_set: bool) -> Option<String> {
    let positions = rule.by_set_position.as_ref()?;
    // `BYSETPOS=` selects nothing, and a content line libical refuses costs the
    // whole component — every field of the event, not just its recurrence.
    if positions.is_empty() || !selects_from_a_set {
        return None;
    }
    let tokens: Option<Vec<String>> = positions.iter().copied().map(set_position_token).collect();
    Some(format!("BYSETPOS={}", tokens?.join(",")))
}

/// One occurrence of the set as an `RRULE` writes it — `1`, `-1` — or `None`
/// for a value no `BYSETPOS` can carry.
fn set_position_token(position: i32) -> Option<String> {
    match position {
        // RFC 5545's `setposday` is spelled as `yeardaynum` is, so 1 to 366 —
        // the most occurrences a `BYYEARDAY` can put in one interval's set —
        // which RFC 8984 §4.3.3 counts backwards from the last as well. Zero
        // selects no occurrence, and neither format admits it.
        -366..=-1 | 1..=366 => Some(position.to_string()),
        _ => None,
    }
}

/// The `WKST` part of a rule's `RRULE`, or `None` when the rule names no first
/// day of the week — **and when it names Monday**, which is the one part this
/// mapping leaves off a rule it is perfectly able to write.
///
/// Monday is RFC 5545 §3.3.10's default, and libical drops `WKST=MO` from a rule
/// it reads (measured in `jmap-backend-cal/tests/marshal.rs`). A rule written with
/// it would therefore come back out of EDS's cache without it, and the save path
/// would read that as the user removing `firstDayOfWeek` — the same reason
/// `INTERVAL=1` is left off. So an absent `WKST` is not a refusal here, and
/// [`maps_recurrence_rule`] asks [`weekday_token`] about the value instead of
/// asking this function.
///
/// The cost of that: a save which patches `recurrenceRules` for some *other*
/// reason drops an explicit `firstDayOfWeek: "mo"` the server held. That is the
/// value the property defaults to, so the rule still names the same dates.
///
/// There is no frequency gate. §3.3.10 says only where the part is
/// *significant* — a fortnightly series' weeks, a `BYWEEKNO` — which is a reader's
/// business, and libical keeps the day beside every frequency.
fn first_day_of_week_part(rule: &RecurrenceRule) -> Option<String> {
    let day = weekday_token(rule.first_day_of_week.as_deref()?)?;
    (day != "MO").then(|| format!("WKST={day}"))
}

/// One weekday as an `RRULE` writes it — `SU` — or `None` for a value no `WKST`
/// can carry.
///
/// Only RFC 8984 §4.3.3's own lowercase spelling is accepted, for the reason
/// [`month_token`] accepts only `3` and not `03`: a value in another case is one
/// this mapping would hand back respelled, and a rule that comes back spelled
/// differently reads as an edit the user never made. Anything else is a day no
/// week starts on, and `WKST=XX` costs libical the whole `RRULE` — every field of
/// the event, not just its recurrence.
fn weekday_token(day: &str) -> Option<&'static str> {
    if !day.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return None;
    }
    WEEKDAYS
        .iter()
        .copied()
        .find(|weekday| weekday.eq_ignore_ascii_case(day))
}

/// A rule's `UNTIL` as RFC 8984 §4.3.3's `until`, which is a local time in the
/// zone [`Ends`] names.
///
/// RFC 5545 §3.3.10 requires the value to be a UTC instant whenever `DTSTART`
/// carries a `TZID`, so a `Z` here is what every conformant producer writes —
/// an invitation, an imported `.ics`. Reading its digits as a local time would
/// move the end of the series by the zone's offset, which is a recurrence the
/// user never edited being shortened on the server.
///
/// Converting it properly needs the offset in force at that instant, which is
/// normally a zone database's job and this crate carries none. It does not have
/// to: RFC 5545 §3.6.5 makes the document itself define the zone its `TZID`
/// refers to, so the answer is in the file wherever the `VTIMEZONE` is — which
/// is every component Evolution writes and every invitation worth the name.
/// [`crate::zone`] reads it out, and that is the ordinary case.
///
/// What is left is a document that names a zone it does not define, or defines
/// it in a shape [`crate::zone`] will not guess at. There the instant is kept as
/// it was stated, `Z` and all: that is not a LocalDateTime, so [`writable`]
/// refuses it and [`maps_recurrence_rule`] tells the save path to leave
/// `recurrenceRules` alone.
///
/// [`Ends::At`] is the one case where the shift needs no definition at all, an
/// offset being a number rather than a zone — see that variant.
///
/// The two cases with no offset to shift by are read as they always were: an
/// event whose zone *is* UTC states the same digits either way, and a floating
/// one has no zone to resolve an instant against — RFC 5545 admits no `Z`
/// beside a floating `DTSTART` at all, so its digits are the best reading of a
/// producer being loose.
///
/// Every other failure — a value that names no instant, a shift that steps off
/// the years RFC 5545 §3.3.4 admits — keeps the value **verbatim** for the same
/// reason, and never yields nothing: a rule that arrives with an end and is read
/// without one is one that never stops, and it would be sent to the server
/// looking like a recurrence the user had deliberately left unbounded. What is
/// kept is no LocalDateTime either, so [`writable`] refuses it and
/// [`maps_recurrence_rule`] is how each caller learns the end did not survive.
fn read_until(value: &str, ends: Ends) -> String {
    let Some(local) = to_local_date_time(value) else {
        return value.to_owned();
    };
    if !value.ends_with(['Z', 'z']) {
        return local;
    }
    match ends {
        Ends::At(offset) => at_offset(&local, offset).unwrap_or_else(|| value.to_owned()),
        Ends::In(zone) => match zone.offset_at(&local) {
            Some(offset) => moved(&local, offset).unwrap_or_else(|| value.to_owned()),
            None if zone.name.is_some_and(|name| !is_utc(name)) => format!("{local}Z"),
            None => local,
        },
    }
}

/// The zone a component's own times are in, as much of it as is known: the
/// `TimeZoneId` [`read_start`] resolved, and — where the document defines the
/// zone rather than merely naming it — the `VTIMEZONE` that says when its
/// offset changes.
///
/// The definition is what [`read_until`] needs and nothing else does, so it is
/// `None` on the drawing side, which has only a name to go on.
#[derive(Clone, Copy)]
struct Zoned<'a> {
    name: Option<&'a str>,
    observances: Option<&'a [&'a ICalendarComponent]>,
}

impl<'a> Zoned<'a> {
    /// A zone named and not defined: what the drawing side has, and what a
    /// document carrying no `VTIMEZONE` for it gives the reading side.
    fn named(name: Option<&'a str>) -> Self {
        Self {
            name,
            observances: None,
        }
    }

    /// The offset from UTC in force in this zone at `utc`, where the document
    /// said enough for it to be worked out.
    fn offset_at(&self, utc: &str) -> Option<i64> {
        crate::zone::offset_at(self.observances?, utc)
    }
}

/// What a recurrence rule's `UNTIL` is stated against — the one thing mapping a
/// rule needs beyond the rule itself, in either direction.
#[derive(Clone, Copy)]
enum Ends<'a> {
    /// In the zone the component naming the rule is in, as `read_start`
    /// resolved it — a zone with no name at all being a floating component.
    In(Zoned<'a>),
    /// At a fixed offset from UTC, which is a `VTIMEZONE` observance: RFC 5545
    /// §3.6.5 dates one in the zone it is defining, resolving its local
    /// `DTSTART` against the `TZOFFSETFROM` in the same component, and RFC 8984
    /// §4.7.2 does the same for the TimeZoneRule's `start`. An `UNTIL` bounds
    /// that same series of local times, so it is stated the same way on each
    /// side — and unlike a `TZID`, an offset is a number of seconds rather than
    /// a zone whose observance rules have to be evaluated, so the two spellings
    /// convert into one another with nothing but arithmetic.
    At(&'a str),
}

/// The reverse. Parts outside the modeled set are dropped rather than parked
/// in `extra`: an `RSCALE=CHINESE` copied verbatim into JSCalendar would be
/// rejected by the server, whose `rscale` is a lowercase calendar-system name.
///
/// `ends` is what only `UNTIL` needs — see [`read_until`].
fn rrule_to_rule(value: &str, ends: Ends) -> Option<RecurrenceRule> {
    let mut rule = RecurrenceRule::default();
    for part in value.split(';') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        match key.to_ascii_uppercase().as_str() {
            "FREQ" => rule.frequency = value.to_ascii_lowercase(),
            "INTERVAL" => rule.interval = value.parse().ok(),
            "COUNT" => rule.count = value.parse().ok(),
            // A value with no shape of a DATE-TIME at all (not merely one that
            // is shaped like one but names no real instant, e.g. month 13 —
            // `a_rules_unreadable_until_is_kept_rather_than_read_as_no_end_at_all`
            // keeps *that* case verbatim on purpose, for `maps_recurrence_rule`
            // to flag) takes the rest of the rule with it — see this function's
            // own canary test, `an_until_no_parser_can_read_never_reaches_this_
            // mapping`. Used to fall out of this crate for free: `calcard`
            // itself would drop such a rule down to its `FREQ` alone before
            // `entry_raw_value` ever saw the trailing parts. A later `calcard`
            // became more lenient and started handing the whole raw text
            // through instead, so the truncation has to happen here now.
            "UNTIL" if date_time_digits(value).is_none() => break,
            "UNTIL" => rule.until = Some(read_until(value, ends)),
            "BYDAY" => rule.by_day = Some(value.split(',').map(to_nday).collect()),
            "BYMONTHDAY" => {
                rule.by_month_day = Some(value.split(',').map(to_month_day).collect());
            }
            // Read like a day of the month, and for the same reasons — see
            // [`to_month_day`], whose spelling `yeardaynum` shares.
            "BYYEARDAY" => {
                rule.by_year_day = Some(value.split(',').map(to_month_day).collect());
            }
            // Likewise: `weeknum` is spelled as `monthdaynum` is, and a token
            // [`to_month_day`] cannot read becomes the zero no week can be.
            "BYWEEKNO" => {
                rule.by_week_no = Some(value.split(',').map(to_month_day).collect());
            }
            // Each token verbatim: JSCalendar holds a month as the string
            // iCalendar spells it with, so one this mapping will not write back —
            // a thirteenth month, a leap month — is carried as itself and refused
            // by [`month_token`] on the way out, which is what
            // [`maps_recurrence_rule`] then reads.
            "BYMONTH" => rule.by_month = Some(value.split(',').map(str::to_owned).collect()),
            // A time of day needs a sentinel of its own: zero is midnight — and
            // the zeroth minute, and the zeroth second — a real value this has to
            // be able to read, so it cannot double as "unreadable" the way it does
            // for a day of the month. See [`to_time_of_day`].
            "BYSECOND" => rule.by_second = Some(value.split(',').map(to_time_of_day).collect()),
            "BYMINUTE" => rule.by_minute = Some(value.split(',').map(to_time_of_day).collect()),
            "BYHOUR" => rule.by_hour = Some(value.split(',').map(to_time_of_day).collect()),
            // Likewise again: `setposday` is spelled as `monthdaynum` is, and a
            // token [`to_month_day`] cannot read becomes the zero that selects
            // no occurrence.
            "BYSETPOS" => {
                rule.by_set_position = Some(value.split(',').map(to_month_day).collect());
            }
            // RFC 5545's `weekday` is upper case where RFC 8984 §4.3.3's is
            // lower, and iCalendar is case-insensitive besides, so the day is
            // lowered rather than matched: a token that is no weekday at all
            // arrives as itself, is refused by [`weekday_token`] on the way out
            // and so flagged by [`maps_recurrence_rule`]. In practice the parser
            // drops such a token before this sees it (`WKST=XX` arrives as no
            // `WKST` at all), which is a narrowing below this crate — the same
            // one an unreadable `BYDAY` token gets.
            "WKST" => rule.first_day_of_week = Some(value.to_ascii_lowercase()),
            _ => {}
        }
    }
    if rule.frequency.is_empty() {
        return None;
    }
    rule.rule_type = Some("RecurrenceRule".to_owned());
    Some(rule)
}

/// One `BYDAY` token as the NDay it names — `TH`, `2WE`, `-1FR`, and RFC 5545
/// §3.3.10's signed spelling `+3TU`, whose plus JSCalendar has no room for.
///
/// A token this cannot take apart keeps its whole self as the `day`, which is
/// outside the closed vocabulary and so is refused by [`by_day_token`] on the
/// way back out and flagged by [`maps_recurrence_rule`]. Reading it as the
/// weekday alone would drop an ordinal the rule was written with and repeat the
/// event on every one of that weekday instead.
fn to_nday(token: &str) -> NDay {
    let unsigned = token.strip_prefix(['+', '-']).unwrap_or(token);
    let digits = unsigned.len()
        - unsigned
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .len();
    if digits == 0 {
        return NDay::new(&token.to_ascii_lowercase());
    }
    let (ordinal, weekday) = token.split_at(token.len() - unsigned.len() + digits);
    match ordinal.parse::<i32>() {
        // Zero is no occurrence of a weekday, and a value too large for the
        // ordinal is one this cannot hand back unchanged; both keep the token.
        Ok(nth) if nth != 0 => NDay {
            nth_of_period: Some(nth),
            ..NDay::new(&weekday.to_ascii_lowercase())
        },
        _ => NDay::new(&token.to_ascii_lowercase()),
    }
}

/// One `BYMONTHDAY` token as the number JSCalendar holds — RFC 5545 §3.3.10's
/// signed `monthdaynum`, whose leading plus `str::parse` accepts and JSCalendar
/// has no room for.
///
/// A token this cannot read becomes zero, which is a day no month has: it is
/// refused by [`month_day_token`] on the way back out and so flagged by
/// [`maps_recurrence_rule`], exactly as a `BYDAY` token [`to_nday`] cannot take
/// apart is. Dropping it instead would leave a *smaller* set of days looking
/// like the whole rule, and a save would then delete whichever day the server
/// really held there.
fn to_month_day(token: &str) -> i32 {
    token.parse().unwrap_or(0)
}

/// One `BYSECOND`, `BYMINUTE` or `BYHOUR` token as the number JSCalendar holds —
/// RFC 5545 §3.3.10's `seconds`, `minutes` and `hour`, all three unsigned.
///
/// A token this cannot read becomes [`u32::MAX`] rather than the zero
/// [`to_month_day`] uses, for the reason [`time_of_day_part`] gives: zero is
/// midnight, and the zeroth minute, and the zeroth second — a value a rule may
/// legitimately name, so it cannot also mean "no value". What matters is only that
/// it be one [`time_of_day_part`] refuses, so that the set is carried whole and
/// flagged by [`maps_recurrence_rule`] rather than handed back a member short.
fn to_time_of_day(token: &str) -> u32 {
    token.parse().unwrap_or(u32::MAX)
}
