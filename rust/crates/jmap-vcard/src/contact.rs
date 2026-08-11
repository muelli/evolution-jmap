// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSContact [`ContactCard`] ↔ vCard 3.0.
//!
//! The mapped set is deliberately the one the address book backend needs to
//! be useful — UID, FN, N, EMAIL, TEL, ORG — and no more. Everything else on
//! a card (addresses, notes, nicknames, …) is *dropped*, which is only
//! safe because saving goes back to the server as a PatchObject naming the
//! mapped properties: a property we never mapped is a property we never
//! overwrite.
//!
//! `ORG` is the one property whose *value* is a list rather than a field: RFC
//! 2426 §3.5.5 states the organisation's name and then the units within it,
//! in the order RFC 9553 §2.2.3's `units` gives them, so an entry crosses as
//! one line with as many components as it has units. What does not cross is
//! the entry's `sortAs` and `contexts`, which `ORG` has no component and no
//! parameter for — hence the [`X_JMAP_KEY`] this side already writes on an
//! `EMAIL`, and a save that patches `organizations/<key>/name` in place.

use std::collections::BTreeMap;

use jmap_proto::contacts::{
    ContactCard, ContactEmail, ContactPhone, Name, NameComponent, OrgUnit, Organization,
};
use serde_json::{Map, Value};

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
        if email.address.is_empty() {
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
        if phone.number.is_empty() {
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

    for (key, organization) in card.organizations.iter().flatten() {
        let Some(components) = organization_components(organization) else {
            continue;
        };
        properties.push(Property::structured("ORG", components).with_param(X_JMAP_KEY, key));
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
    let mut organizations = BTreeMap::new();

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
            "ORG" => {
                let Some(organization) = read_organization(property) else {
                    continue;
                };
                organizations.insert(entry_key(property, "o", &organizations), organization);
            }
            _ => {}
        }
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
        organizations: (!organizations.is_empty()).then_some(organizations),
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
