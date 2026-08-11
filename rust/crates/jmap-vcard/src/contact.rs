// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSContact [`ContactCard`] ↔ vCard 3.0.
//!
//! The mapped set is deliberately the one the address book backend needs to
//! be useful — UID, FN, N, EMAIL, TEL, ADR, LABEL, ORG, TITLE, ROLE, NOTE,
//! BDAY — and no more. Everything else on a card (nicknames, keywords, …) is
//! *dropped*, which is only safe because saving goes back to the server as a
//! PatchObject naming the mapped properties: a property we never mapped is a
//! property we never overwrite.
//!
//! `ORG` is the one property whose *value* is a list rather than a field: RFC
//! 2426 §3.5.5 states the organisation's name and then the units within it,
//! in the order RFC 9553 §2.2.3's `units` gives them, so an entry crosses as
//! one line with as many components as it has units. What does not cross is
//! the entry's `sortAs` and `contexts`, which `ORG` has no component and no
//! parameter for — hence the [`X_JMAP_KEY`] this side already writes on an
//! `EMAIL`, and a save that patches `organizations/<key>/name` in place.
//!
//! `titles` is the one property of which only *some* entries cross. RFC 9553
//! §2.2.4 keeps the job title and the role played in one map, told apart by
//! `kind`, and allows vendor kinds besides those two; vCard 3.0 has exactly
//! `TITLE` and `ROLE`. An entry of any other kind is therefore dropped rather
//! than written on a line that would misstate it — and, as with every other
//! dropped thing here, the save path must then leave it alone.
//!
//! `addresses` is lossy the same way one level down, and is the one property
//! that crosses on two lines. RFC 2426 §3.2.1's `ADR` has seven fields; RFC
//! 9553 §2.5.1 builds an address out of named components, sixteen kinds of
//! them. Seven of those kinds have a field of their own
//! ([`ADDRESS_COMPONENTS`]) and one more, the house `number`, shares the
//! street's ([`JOINED_COMPONENTS`]); the rest — `floor`, `room`, `landmark` —
//! have nowhere to go, and are left off the line rather than written into a
//! field that would say something else about them. Beside the `ADR`
//! goes RFC 2426 §3.2.2's `LABEL`, the address written out as it should be
//! printed, which is RFC 9553's `full` and what EDS keeps in its three
//! synthetic address-label fields. An address may have either line, or both:
//! `full` stands on its own for an address "even if the individual address
//! components are not known", and an address stated only in components vCard
//! has no field for has neither line and is invisible — so `addresses` too is
//! a map of which the vCard states only some entries.
//!
//! `notes` is the plainest of them and lossy only around the value: RFC 2426
//! §3.6.2's `NOTE` is free text, so an entry's own text crosses whole, while
//! RFC 9553 §2.8.3's `created` and `author` — when the note was written and
//! by whom — have no component and no parameter to sit in, and so ride along
//! in the entry's `extra` for the save to patch around. An entry saying
//! nothing at all gets no line, which is the same invisibility again.
//!
//! `anniversaries` is lossy in its *value*, which is new: RFC 9553 §2.8.1
//! dates a memorable event either as a `PartialDate`, which may state as
//! little as a year, or as a `Timestamp`, which states a point in time. A
//! vCard date line states one calendar day and nothing else, so a date that
//! names no single day gets no line — not because the line has nowhere to put
//! it, but because EDS reads anything short of a whole date as *no* date and
//! would show the user 1000-01-01. A point in time crosses as the day it
//! falls on, leaving the hour behind for the save to patch around
//! ([`states_a_point_in_time`]). Of the three kinds, `birth` goes on RFC 2426
//! §3.1.5's `BDAY` and `wedding` on the line EDS reads `E_CONTACT_ANNIVERSARY`
//! off; `death` has no line at all, so `anniversaries` too is a map of which
//! the vCard states only some entries.

use std::collections::BTreeMap;

use jmap_proto::contacts::{
    Address, AddressComponent, Anniversary, ContactCard, ContactEmail, ContactPhone, Name,
    NameComponent, Note, OrgUnit, Organization, Title,
};
use serde_json::{Map, Value, json};

use crate::error::VCardError;
use crate::syntax::{self, Property};

/// Carries the JSContact `uid` when the vCard `UID` is taken by the JMAP id.
const X_JMAP_UID: &str = "X-JMAP-UID";
/// Carries the JSContact map key of an `emails`/`phones` entry.
const X_JMAP_KEY: &str = "X-JMAP-KEY";

/// JSContact name component kinds in reading order, paired with their
/// position in the vCard `N` value (`family;given;additional;prefix;suffix`).
const NAME_COMPONENTS: [(&str, usize); 5] = [
    ("title", 3),
    ("given", 1),
    ("given2", 2),
    ("surname", 0),
    ("credential", 4),
];

/// JSContact `contexts` keys and their vCard `TYPE` spelling.
const CONTEXTS: [(&str, &str); 2] = [("work", "WORK"), ("private", "HOME")];

/// JSContact phone `features` and their vCard `TYPE` spelling.
const PHONE_FEATURES: [(&str, &str); 5] = [
    ("voice", "VOICE"),
    ("fax", "FAX"),
    ("mobile", "CELL"),
    ("pager", "PAGER"),
    ("video", "VIDEO"),
];

/// JSContact address component kinds, paired with their position in the
/// vCard `ADR` value (RFC 2426 §3.2.1: post office box, extended address,
/// street, locality, region, postal code, country), listed in that order —
/// which is the order a reader gives the components it finds.
const ADDRESS_COMPONENTS: [(&str, usize); 7] = [
    ("postOfficeBox", 0),
    ("apartment", 1),
    ("name", 2),
    ("locality", 3),
    ("region", 4),
    ("postcode", 5),
    ("country", 6),
];

/// JSContact address component kinds that share a vCard `ADR` field with
/// another kind instead of having one of their own, paired with the kind
/// whose field they join.
///
/// RFC 2426 §3.2.1 gives the street address one field, while RFC 9553 §2.5.1
/// lets a card name the street and the house number separately. Leaving the
/// number off the line would take the house out of the address the user
/// reads, so it goes on the street field beside the street name, in the order
/// the card lists its components — which is the only thing that says whether
/// the number is read before the street name (`1 Main Street`) or after it
/// (`Hauptstraße 1`).
const JOINED_COMPONENTS: [(&str, &str); 1] = [("number", "name")];

/// JSContact title `kind` values and the vCard property stating each.
const TITLE_KINDS: [(&str, &str); 2] = [("title", "TITLE"), ("role", "ROLE")];

/// The line EDS keeps `E_CONTACT_ANNIVERSARY` on — the field Evolution's
/// contact editor labels "Anniversary".
///
/// vCard 3.0 has no property for a wedding day: RFC 6474's `ANNIVERSARY` is
/// vCard 4.0, which `e_contact_new_from_vcard()` is not given. Writing the
/// date on any other line would keep it out of the only field that shows it.
const X_EVOLUTION_ANNIVERSARY: &str = "X-EVOLUTION-ANNIVERSARY";

/// JSContact anniversary `kind` values and the vCard property stating each.
///
/// RFC 9553 §2.8.1's third kind, `death`, is missing on purpose: no vCard 3.0
/// property and no EDS field states it, and putting the date on a `BDAY`
/// would tell the user it is a birthday.
const ANNIVERSARY_KINDS: [(&str, &str); 2] =
    [("birth", "BDAY"), ("wedding", X_EVOLUTION_ANNIVERSARY)];

/// RFC 9553 §2.2.4's default `kind` for a title that names none.
const DEFAULT_TITLE_KIND: &str = "title";

/// The kind of a JSContact title, with the default filled in.
///
/// The save path has to agree with this side about what an unsaid kind
/// means, or it will patch a `kind` onto every card that left it out.
pub fn title_kind(kind: Option<&str>) -> &str {
    kind.unwrap_or(DEFAULT_TITLE_KIND)
}

/// Whether the vCard mapping covers a JSContact title of this `kind`.
fn maps_title_kind(kind: Option<&str>) -> bool {
    TITLE_KINDS
        .iter()
        .any(|(mapped, _)| *mapped == title_kind(kind))
}

/// Whether the vCard mapping covers a JSContact `name.components` kind.
///
/// Anything that saves a card back to the server has to know exactly which
/// JSContact fields a vCard can carry, or it will overwrite the ones it
/// silently dropped on the way in. The predicates below are that knowledge,
/// kept next to the tables they answer for.
pub fn maps_name_component(kind: &str) -> bool {
    NAME_COMPONENTS.iter().any(|(mapped, _)| *mapped == kind)
}

/// Whether the vCard mapping covers a JSContact `contexts` key.
pub fn maps_context(key: &str) -> bool {
    CONTEXTS.iter().any(|(mapped, _)| *mapped == key)
}

/// Whether the vCard mapping covers a JSContact phone `features` key.
pub fn maps_phone_feature(key: &str) -> bool {
    PHONE_FEATURES.iter().any(|(mapped, _)| *mapped == key)
}

/// Whether the vCard mapping covers a JSContact address component kind.
pub fn maps_address_component(kind: &str) -> bool {
    address_field(kind).is_some()
}

/// The `ADR` field a component of this kind is stated in, whether it has one
/// to itself or shares it with another kind.
fn address_field(kind: &str) -> Option<usize> {
    if let Some((_, index)) = ADDRESS_COMPONENTS
        .iter()
        .find(|(mapped, _)| *mapped == kind)
    {
        return Some(*index);
    }
    let (_, onto) = JOINED_COMPONENTS
        .iter()
        .find(|(mapped, _)| *mapped == kind)?;
    ADDRESS_COMPONENTS
        .iter()
        .find(|(mapped, _)| mapped == onto)
        .map(|(_, index)| *index)
}

/// Whether an address reaches the user at all — whether it has anything an
/// `ADR` or a `LABEL` line can state.
///
/// This is the emitter's own decision, asked of it by name, so that the save
/// path cannot drift from what [`card_to_vcard`] actually wrote. Every keyed
/// map the mapping carries has one of these, because every one of them has
/// entries a vCard leaves out; a save that decided for itself which those
/// were would eventually decide differently, and delete an entry the user
/// never saw.
pub fn states_address(address: &Address) -> bool {
    address_fields(address).is_some() || address_label(address).is_some()
}

/// The text a `LABEL` line states for an address, or `None` for one written
/// out as nothing — which says no more than an `EMAIL:` with no address does,
/// and gets no line either.
pub fn address_label(address: &Address) -> Option<&str> {
    address.full.as_deref().filter(|full| !full.is_empty())
}

/// Whether a note reaches the user at all — whether it says anything a
/// `NOTE` line could state.
pub fn states_note(note: &Note) -> bool {
    !note.note.is_empty()
}

/// Whether an email address reaches the user at all. An entry with no
/// address states nothing, so it gets no `EMAIL` line.
pub fn states_email(email: &ContactEmail) -> bool {
    !email.address.is_empty()
}

/// Whether a phone number reaches the user at all.
pub fn states_phone(phone: &ContactPhone) -> bool {
    !phone.number.is_empty()
}

/// Whether a title reaches the user at all: the mapping must have a property
/// for its `kind` *and* the entry must name something.
///
/// The kind alone is not the question. A title of kind `title` that names
/// nothing has no `TITLE` line either, and asking only [`maps_title_kind`]
/// would call it visible and let a save delete it.
pub fn states_title(title: &Title) -> bool {
    !title.name.is_empty() && maps_title_kind(title.kind.as_deref())
}

/// Whether an organisation reaches the user at all — whether the `ORG` line
/// has a name or a unit to state. An entry holding only a `sortAs` has
/// neither.
pub fn states_organization(organization: &Organization) -> bool {
    organization_components(organization).is_some()
}

/// Whether an anniversary reaches the user at all: the mapping must have a
/// property for its `kind` *and* its date must name one calendar day.
pub fn states_anniversary(anniversary: &Anniversary) -> bool {
    anniversary_property(&anniversary.kind).is_some() && anniversary_date(anniversary).is_some()
}

/// The date a vCard line states for an anniversary, or `None` for a date no
/// single day can be read out of.
///
/// This is what the save compares by, rather than the JSON: the two shapes
/// RFC 9553 §2.8.1 allows can name the same day, so a card whose birthday is
/// a `Timestamp` must not look edited merely because it came back as the day
/// the user was shown.
pub fn anniversary_date(anniversary: &Anniversary) -> Option<String> {
    let date = anniversary.date.as_ref()?;
    // A `Timestamp` states a point in time. The day it falls on is read in
    // UTC, which is the only zone the card names.
    if let Some(utc) = date.get("utc").and_then(Value::as_str) {
        return read_day(utc).map(|day| day.text());
    }
    let day = Day {
        year: member(date, "year")?,
        month: member(date, "month")?,
        day: member(date, "day")?,
    };
    day.is_a_date().then(|| day.text())
}

/// Whether an anniversary is dated by a point in time (RFC 9553 §2.8.1's
/// `Timestamp`) rather than by a calendar day (its `PartialDate`).
///
/// The save asks because the two are patched differently: a day's members can
/// be reached into one at a time, leaving whatever else the object carries in
/// place, while a point in time the user has retyped as a day is a different
/// kind of object and has to be written whole.
pub fn states_a_point_in_time(anniversary: &Anniversary) -> bool {
    anniversary
        .date
        .as_ref()
        .is_some_and(|date| date.get("utc").is_some())
}

/// The vCard property an anniversary of this `kind` is stated on.
fn anniversary_property(kind: &str) -> Option<&'static str> {
    ANNIVERSARY_KINDS
        .iter()
        .find(|(mapped, _)| *mapped == kind)
        .map(|(_, name)| *name)
}

/// One calendar day: the whole of what a vCard 3.0 date line can state.
struct Day {
    year: u32,
    month: u32,
    day: u32,
}

impl Day {
    /// The day as RFC 2426 §3.1.5 asks for it — ISO 8601's extended form,
    /// which is also the one `e_contact_date_to_string()` writes back.
    fn text(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// Whether the numbers name a day of a kind the calendar has. Which
    /// months are 30 days long is left to the server that stated the date;
    /// what is refused here is a date no month could have.
    fn is_a_date(&self) -> bool {
        (1..=9999).contains(&self.year)
            && (1..=12).contains(&self.month)
            && (1..=31).contains(&self.day)
    }

    /// The day as the `PartialDate` a save writes when the user retyped one.
    fn json(&self) -> Value {
        json!({
            "@type": "PartialDate",
            "year": self.year,
            "month": self.month,
            "day": self.day,
        })
    }
}

/// The day a date line states, or `None` for text that names none.
///
/// Both ISO 8601 forms are read — `1964-03-27` and `19640327` — because
/// `e_contact_date_from_string()` reads both, and so a vCard that has been
/// through another client may carry either. A time after the date is dropped
/// rather than refused, for the same reason.
fn read_day(text: &str) -> Option<Day> {
    let digits: String = text
        .split(['T', 't'])
        .next()?
        .chars()
        .filter(|character| *character != '-')
        .collect();
    if digits.len() != 8 || !digits.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let day = Day {
        year: digits[0..4].parse().ok()?,
        month: digits[4..6].parse().ok()?,
        day: digits[6..8].parse().ok()?,
    };
    day.is_a_date().then_some(day)
}

/// One numeric member of a JSContact date object.
fn member(date: &Value, name: &str) -> Option<u32> {
    date.get(name)?.as_u64()?.try_into().ok()
}

/// Render a contact card as a vCard 3.0 string, ready for
/// `e_contact_new_from_vcard()`.
pub fn card_to_vcard(card: &ContactCard) -> String {
    let mut properties = vec![Property::new("VERSION", "3.0")];

    // EDS keys its cache on the vCard UID and passes it back to
    // load_contact_sync()/remove_contact_sync(), so it has to be the
    // identifier the JMAP methods take — the server-assigned id. The
    // JSContact uid, which is a different namespace, rides alongside.
    if let Some(uid) = card
        .id
        .as_ref()
        .map(|id| id.as_str())
        .or(card.uid.as_deref())
    {
        properties.push(Property::new("UID", uid));
    }
    if let Some(uid) = &card.uid {
        properties.push(Property::new(X_JMAP_UID, uid));
    }

    if let Some(name) = &card.name {
        if let Some(full) = name.full.clone().or_else(|| derive_full(name)) {
            properties.push(Property::new("FN", &full));
        }
        if let Some(fields) = name_fields(name) {
            properties.push(Property::structured("N", fields));
        }
    }

    for (key, email) in card.emails.iter().flatten() {
        if !states_email(email) {
            continue;
        }
        let mut types = type_names(&CONTEXTS, email.contexts.as_ref());
        if email.pref.is_some() {
            // vCard 3.0 has no ranking, only a preferred flag.
            types.push("PREF");
        }
        properties.push(
            Property::new("EMAIL", &email.address)
                .with_param(X_JMAP_KEY, key)
                .with_params("TYPE", types),
        );
    }

    for (key, phone) in card.phones.iter().flatten() {
        if !states_phone(phone) {
            continue;
        }
        let mut types = type_names(&CONTEXTS, phone.contexts.as_ref());
        types.extend(type_names(&PHONE_FEATURES, phone.features.as_ref()));
        properties.push(
            Property::new("TEL", &phone.number)
                .with_param(X_JMAP_KEY, key)
                .with_params("TYPE", types),
        );
    }

    for (key, address) in card.addresses.iter().flatten() {
        let types = type_names(&CONTEXTS, address.contexts.as_ref());
        if let Some(fields) = address_fields(address) {
            properties.push(
                Property::structured("ADR", fields)
                    .with_param(X_JMAP_KEY, key)
                    .with_params("TYPE", types.clone()),
            );
        }
        // The same address written out for an envelope, on the line RFC 2426
        // §3.2.2 gives it — directly after its own `ADR`, and on its own when
        // the components are not known and there is no `ADR` to follow.
        if let Some(full) = address_label(address) {
            properties.push(
                Property::new("LABEL", full)
                    .with_param(X_JMAP_KEY, key)
                    .with_params("TYPE", types),
            );
        }
    }

    for (key, organization) in card.organizations.iter().flatten() {
        let Some(components) = organization_components(organization) else {
            continue;
        };
        properties.push(Property::structured("ORG", components).with_param(X_JMAP_KEY, key));
    }

    for (key, title) in card.titles.iter().flatten() {
        if !states_title(title) {
            continue;
        }
        let Some((_, name)) = TITLE_KINDS
            .iter()
            .find(|(kind, _)| *kind == title_kind(title.kind.as_deref()))
        else {
            continue;
        };
        properties.push(Property::new(name, &title.name).with_param(X_JMAP_KEY, key));
    }

    for (key, note) in card.notes.iter().flatten() {
        if !states_note(note) {
            continue;
        }
        properties.push(Property::new("NOTE", &note.note).with_param(X_JMAP_KEY, key));
    }

    for (key, anniversary) in card.anniversaries.iter().flatten() {
        let (Some(name), Some(date)) = (
            anniversary_property(&anniversary.kind),
            anniversary_date(anniversary),
        ) else {
            continue;
        };
        properties.push(Property::new(name, &date).with_param(X_JMAP_KEY, key));
    }

    syntax::write(&properties)
}

/// Read a vCard 3.0 string into a contact card.
///
/// The `id` is whatever the vCard's `UID` says, which for a contact
/// Evolution has just created is a locally invented string rather than a
/// JMAP id — the caller knows which case it is in and must drop it before
/// sending a create.
pub fn vcard_to_card(vcard: &str) -> Result<ContactCard, VCardError> {
    let properties = syntax::parse(vcard)?;
    let text = |name: &str| {
        properties
            .iter()
            .find(|property| property.name == name)
            .map(Property::text)
            .filter(|value| !value.is_empty())
    };

    let name = read_name(&properties);
    let mut emails = BTreeMap::new();
    let mut phones = BTreeMap::new();
    let mut addresses = BTreeMap::new();
    let mut organizations = BTreeMap::new();
    let mut titles = BTreeMap::new();
    let mut notes = BTreeMap::new();
    let mut anniversaries = BTreeMap::new();

    for property in &properties {
        match property.name.as_str() {
            "EMAIL" => {
                let address = property.text();
                if address.is_empty() {
                    continue;
                }
                let email = ContactEmail {
                    address,
                    contexts: read_flags(&CONTEXTS, property),
                    pref: property.has_type("PREF").then_some(1),
                    ..ContactEmail::default()
                };
                emails.insert(entry_key(property, "e", &emails), email);
            }
            "TEL" => {
                let number = property.text();
                if number.is_empty() {
                    continue;
                }
                let phone = ContactPhone {
                    number,
                    contexts: read_flags(&CONTEXTS, property),
                    features: read_flags(&PHONE_FEATURES, property),
                    ..ContactPhone::default()
                };
                phones.insert(entry_key(property, "p", &phones), phone);
            }
            "ADR" => {
                let Some(address) = read_address(property) else {
                    continue;
                };
                addresses.insert(entry_key(property, "a", &addresses), address);
            }
            "ORG" => {
                let Some(organization) = read_organization(property) else {
                    continue;
                };
                organizations.insert(entry_key(property, "o", &organizations), organization);
            }
            "TITLE" | "ROLE" => {
                let Some(title) = read_title(property) else {
                    continue;
                };
                titles.insert(entry_key(property, "t", &titles), title);
            }
            "NOTE" => {
                let note = Note {
                    note: property.text(),
                    extra: BTreeMap::new(),
                };
                if !states_note(&note) {
                    continue;
                }
                notes.insert(entry_key(property, "n", &notes), note);
            }
            "BDAY" | X_EVOLUTION_ANNIVERSARY => {
                let Some(anniversary) = read_anniversary(property) else {
                    continue;
                };
                anniversaries.insert(entry_key(property, "y", &anniversaries), anniversary);
            }
            _ => {}
        }
    }

    // The `LABEL` lines after the `ADR` ones, because a label states an
    // address the card may already have named and has to find it first.
    for property in properties
        .iter()
        .filter(|property| property.name == "LABEL")
    {
        let full = property.text();
        if full.is_empty() {
            continue;
        }
        let contexts = read_flags(&CONTEXTS, property);
        let key = label_entry(property, contexts.as_ref(), &addresses);
        addresses
            .entry(key)
            .or_insert_with(|| Address {
                contexts,
                ..Address::default()
            })
            .full = Some(full);
    }

    Ok(ContactCard {
        id: text("UID").map(Into::into),
        // Membership follows from which EDS source is being served, not from
        // the contact, so the backend fills it in on create.
        address_book_ids: None,
        card_type: Some("Card".to_owned()),
        version: Some("1.0".to_owned()),
        uid: text(X_JMAP_UID),
        name,
        emails: (!emails.is_empty()).then_some(emails),
        phones: (!phones.is_empty()).then_some(phones),
        addresses: (!addresses.is_empty()).then_some(addresses),
        organizations: (!organizations.is_empty()).then_some(organizations),
        titles: (!titles.is_empty()).then_some(titles),
        notes: (!notes.is_empty()).then_some(notes),
        anniversaries: (!anniversaries.is_empty()).then_some(anniversaries),
        extra: BTreeMap::new(),
    })
}

/// The title a `TITLE` or `ROLE` line states, or `None` for a line with no
/// text on it.
///
/// The kind is left unsaid when it is the default, so that reading back a
/// card that never named one produces the card that was there — a save then
/// has nothing to patch.
fn read_title(property: &Property) -> Option<Title> {
    let name = property.text();
    if name.is_empty() {
        return None;
    }
    let kind = TITLE_KINDS
        .iter()
        .find(|(_, mapped)| *mapped == property.name)
        .map(|(kind, _)| *kind)
        .filter(|kind| *kind != DEFAULT_TITLE_KIND);
    Some(Title {
        name,
        kind: kind.map(str::to_owned),
        extra: BTreeMap::new(),
    })
}

/// The anniversary a date line states, or `None` for a line no calendar day
/// can be read out of.
///
/// The kind is the line's own: a `BDAY` states a birthday and nothing else,
/// so unlike a title's it is never guessed at and never left unsaid.
fn read_anniversary(property: &Property) -> Option<Anniversary> {
    let (kind, _) = ANNIVERSARY_KINDS
        .iter()
        .find(|(_, name)| *name == property.name)?;
    Some(Anniversary {
        kind: (*kind).to_owned(),
        date: Some(read_day(&property.text())?.json()),
        extra: BTreeMap::new(),
    })
}

/// The seven `ADR` fields for an address, or `None` for one with nothing to
/// put in any of them — an address stated only in components vCard has no
/// field for, which is then invisible to the user and to the save.
///
/// Empty fields are kept: a field's position is what says which part of the
/// address it is.
fn address_fields(address: &Address) -> Option<Vec<String>> {
    let components = address.components.as_ref()?;
    let mut fields = vec![String::new(); ADDRESS_COMPONENTS.len()];
    let mut any = false;
    for component in components {
        let Some(index) = address_field(&component.kind) else {
            continue;
        };
        if component.value.is_empty() {
            continue;
        }
        // Components that share a field — a street named on two lines, or a
        // street name and the house number standing on it — are written into
        // it one after another, in the order the card lists them.
        if !fields[index].is_empty() {
            fields[index].push(' ');
        }
        fields[index].push_str(&component.value);
        any = true;
    }
    any.then_some(fields)
}

/// The components an edited `ADR` line states, with every field that still
/// says exactly what the server built it from given those parts back.
///
/// A field built from several components is read back as one component of the
/// field's own kind, because nothing in `Hauptstraße 1` says where the street
/// name ends and the house number begins, and a guess would be wrong in half
/// the world's addresses. Left at that, opening a contact and closing it again
/// would flatten the parts the server had stated separately — so the save asks
/// this first: if the field still reads as the parts joined, it is those
/// parts, unedited, and they are put back in the order and shape they went out
/// in. If it does not, the user retyped the field, and it stays the one
/// component they typed — the parts it was built from cannot be recovered, and
/// keeping the old ones would leave a house number standing on a street that
/// is no longer there.
pub fn restore_address_components(
    current: &[AddressComponent],
    edited: &[AddressComponent],
) -> Vec<AddressComponent> {
    let mut restored = edited.to_vec();
    for (_, index) in ADDRESS_COMPONENTS {
        let parts: Vec<&AddressComponent> = current
            .iter()
            .filter(|component| {
                address_field(&component.kind) == Some(index) && !component.value.is_empty()
            })
            .collect();
        if parts.is_empty() {
            continue;
        }
        let joined = parts
            .iter()
            .map(|component| component.value.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let mut stated = restored
            .iter()
            .enumerate()
            .filter(|(_, component)| address_field(&component.kind) == Some(index));
        let at = match (stated.next(), stated.next()) {
            (Some((at, component)), None) if component.value == joined => at,
            _ => continue,
        };
        restored.splice(at..=at, parts.into_iter().cloned());
    }
    restored
}

/// The address an `ADR` line states, or `None` when every field of it is
/// empty — the same "nothing was said" an `EMAIL:` with no address is.
fn read_address(property: &Property) -> Option<Address> {
    let fields = property.components();
    let mut components = Vec::new();
    for (kind, index) in ADDRESS_COMPONENTS {
        let Some(value) = fields.get(index).filter(|value| !value.is_empty()) else {
            continue;
        };
        components.push(AddressComponent::new(kind, value));
    }
    if components.is_empty() {
        return None;
    }
    Some(Address {
        components: Some(components),
        contexts: read_flags(&CONTEXTS, property),
        // Filled in by the `LABEL` line, if the card has one for this address.
        full: None,
        extra: BTreeMap::new(),
    })
}

/// The `ORG` components for an organisation — its name, then its units — or
/// `None` for an entry that names neither and so has no line to be written on.
///
/// An organisation with units and no name keeps the empty first component:
/// the name's meaning is its position, so letting a unit slide into it would
/// say the department is the employer.
fn organization_components(organization: &Organization) -> Option<Vec<String>> {
    let name = organization.name.clone().unwrap_or_default();
    let units: Vec<String> = organization
        .units
        .iter()
        .flatten()
        .filter(|unit| !unit.name.is_empty())
        .map(|unit| unit.name.clone())
        .collect();
    if name.is_empty() && units.is_empty() {
        return None;
    }
    let mut components = vec![name];
    components.extend(units);
    Some(components)
}

/// The organisation an `ORG` line states, or `None` when every component of
/// it is empty — the same "nothing was said" an `EMAIL:` with no address is.
fn read_organization(property: &Property) -> Option<Organization> {
    let components = property.components();
    let name = components.first().filter(|name| !name.is_empty()).cloned();
    let units: Vec<OrgUnit> = components
        .iter()
        .skip(1)
        .filter(|unit| !unit.is_empty())
        .map(|unit| OrgUnit::new(unit))
        .collect();
    if name.is_none() && units.is_empty() {
        return None;
    }
    Some(Organization {
        name,
        units: (!units.is_empty()).then_some(units),
        extra: BTreeMap::new(),
    })
}

/// The `addresses` entry a `LABEL` line states: the one it names, the one it
/// matches, or a new one of its own.
///
/// An address stated only in `full` has no `ADR` line, so its key crosses on
/// the `LABEL` and nowhere else — which is why a key naming no address yet is
/// taken at its word rather than being replaced by an invented one.
///
/// Failing a key there is the `TYPE`, which is how RFC 2426 §3.2.2 has a
/// `LABEL` say which `ADR` it is the written-out form of. That fallback is
/// not decoration: `E_CONTACT_ADDRESS_LABEL_HOME` is one of EDS's synthetic
/// fields, so EDS rebuilds the line from the text alone and the `X-JMAP-KEY`
/// this side wrote does not survive the trip through Evolution. Without the
/// fallback every save would then file the label as a second address.
fn label_entry(
    property: &Property,
    contexts: Option<&Value>,
    addresses: &BTreeMap<String, Address>,
) -> String {
    let unlabelled = |address: &Address| address.full.is_none();
    if let Some(key) = property.param(X_JMAP_KEY).filter(|key| !key.is_empty())
        && addresses.get(key).is_none_or(unlabelled)
    {
        return key.to_owned();
    }
    if let Some((key, _)) = addresses
        .iter()
        .find(|(_, address)| unlabelled(address) && address.contexts.as_ref() == contexts)
    {
        return key.clone();
    }
    entry_key(property, "a", addresses)
}

/// The JSContact map key for an entry: the one we round-tripped, or the
/// first free `e1`, `e2`, … for a vCard that never had one.
fn entry_key<T>(property: &Property, prefix: &str, taken: &BTreeMap<String, T>) -> String {
    if let Some(key) = property.param(X_JMAP_KEY).filter(|key| !key.is_empty())
        && !taken.contains_key(key)
    {
        return key.to_owned();
    }
    (1..)
        .map(|index| format!("{prefix}{index}"))
        .find(|candidate| !taken.contains_key(candidate))
        .expect("an unbounded sequence has a free element")
}

/// The five vCard `N` fields, or `None` if the card names no components.
fn name_fields(name: &Name) -> Option<Vec<String>> {
    let components = name.components.as_ref()?;
    let mut fields = vec![String::new(); 5];
    let mut any = false;
    for component in components {
        let Some((_, index)) = NAME_COMPONENTS
            .iter()
            .find(|(kind, _)| *kind == component.kind)
        else {
            continue;
        };
        if component.value.is_empty() {
            continue;
        }
        // Two components of the same kind (a double-barrelled given name)
        // share one vCard field.
        if !fields[*index].is_empty() {
            fields[*index].push(' ');
        }
        fields[*index].push_str(&component.value);
        any = true;
    }
    any.then_some(fields)
}

/// A display name assembled from the components, for a card that has none.
fn derive_full(name: &Name) -> Option<String> {
    let components = name.components.as_ref()?;
    let mut parts = Vec::new();
    for (kind, _) in NAME_COMPONENTS {
        for component in components {
            if component.kind == kind && !component.value.is_empty() {
                parts.push(component.value.as_str());
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn read_name(properties: &[Property]) -> Option<Name> {
    let find = |name: &str| properties.iter().find(|property| property.name == name);

    let full = find("FN").map(Property::text).filter(|f| !f.is_empty());
    let fields = find("N").map(Property::components).unwrap_or_default();

    let mut components = Vec::new();
    for (kind, index) in NAME_COMPONENTS {
        let Some(value) = fields.get(index).filter(|value| !value.is_empty()) else {
            continue;
        };
        components.push(NameComponent {
            kind: kind.to_owned(),
            value: value.clone(),
        });
    }

    // No FN and no usable N: the vCard simply does not name anybody. Note
    // that a missing N is never guessed at by splitting FN — a wrong guess
    // would be written back to the server on the next save.
    if full.is_none() && components.is_empty() {
        return None;
    }
    Some(Name {
        components: (!components.is_empty()).then_some(components),
        full,
        extra: BTreeMap::new(),
    })
}

/// vCard `TYPE` values for the JSContact boolean map `value`.
fn type_names(table: &[(&str, &'static str)], value: Option<&Value>) -> Vec<&'static str> {
    let Some(Value::Object(flags)) = value else {
        return Vec::new();
    };
    table
        .iter()
        .filter(|(key, _)| flags.get(*key) == Some(&Value::Bool(true)))
        .map(|(_, type_name)| *type_name)
        .collect()
}

/// The JSContact boolean map for the `TYPE` values present on `property`.
fn read_flags(table: &[(&str, &str)], property: &Property) -> Option<Value> {
    let flags: Map<String, Value> = table
        .iter()
        .filter(|(_, type_name)| property.has_type(type_name))
        .map(|(key, _)| ((*key).to_owned(), Value::Bool(true)))
        .collect();
    (!flags.is_empty()).then_some(Value::Object(flags))
}
