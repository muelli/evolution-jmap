// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning an edited vCard back into a `ContactCard/set` PatchObject.
//!
//! The whole point of patching rather than replacing is that a vCard is a
//! lossy view of a JSContact card. The mapping keeps UID, FN, N, EMAIL, TEL,
//! ADR, LABEL, ORG, TITLE, ROLE and NOTE and drops everything else, so a save
//! that sent the parsed card back whole would silently delete the properties
//! it could not represent — nicknames, anniversaries, preferred languages —
//! none of which the user ever saw, let alone asked to remove.
//!
//! The same lossiness recurs *inside* the properties that are mapped, and
//! that is the subtler half of this module:
//!
//! - `emails`/`phones` are keyed maps, so an entry is patched by its key
//!   (`emails/work/address`). The key survives the trip through vCard in
//!   `X-JMAP-KEY`; without it every edit would be a remove-and-re-add, which
//!   loses the entry's unmapped properties and its identity server-side.
//! - `contexts` and `features` are boolean maps of which vCard 3.0 can spell
//!   only some members. A context of `school` has no `TYPE`, so it is merged
//!   back in rather than being replaced away.
//! - `pref` is a rank from 1 to 100 and vCard 3.0 has only a flag. An
//!   address that was already preferred keeps its rank; the flag can only
//!   introduce or remove a preference, never renumber one.
//! - `name.components` can hold kinds the `N` value has no field for, which
//!   are carried across the replacement.
//! - `organizations` entries hold a `sortAs` and `contexts` the `ORG` line
//!   has nowhere to put, so the entry is patched member by member
//!   (`organizations/work/name`) rather than replaced. Its `units` are a
//!   *list*, which has no keys to patch by, so they are written whole — and
//!   a unit that kept its name keeps the members it was carrying, matched by
//!   that name rather than by position, so that dissolving one department
//!   does not renumber the sorting hints of the others.
//! - `addresses` entries cross on two lines rather than one — the `ADR` and
//!   the `LABEL` that writes the same address out for an envelope — so a
//!   save reads them back as one entry and patches `addresses/<key>/full`
//!   beside the components.
//! - *Every* keyed map is one of which the vCard states only **some**
//!   entries. A title of a `kind` outside `title` and `role` has no vCard
//!   property; an address with neither an `ADR` field nor a written-out form
//!   to put on a `LABEL`, an organisation with neither a name nor a unit, an
//!   email with no address, a phone with no number and a note that says
//!   nothing all have no line to be written on. Each is dropped on the way
//!   out and must
//!   therefore be invisible to the save in both directions — neither deleted
//!   for being absent from the edited card, nor overwritten by an addition
//!   whose key the reader invented by counting only the entries it could
//!   see. That is what [`diff_entries`] and the `states_*` predicates it
//!   takes are for; the predicates live next to the emitter, so what the
//!   save calls invisible is what the emitter actually left out.
//!
//! RFC 8620 §5.3 requires every path segment before the last to exist on the
//! object already, which is why a property that is absent server-side is
//! written whole instead of being reached into.

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::contacts::{
    Address, AddressComponent, ContactCard, ContactEmail, ContactPhone, Name, Note, OrgUnit,
    Organization, Title,
};
use jmap_vcard::{
    address_label, maps_address_component, maps_context, maps_name_component, maps_phone_feature,
    restore_address_components, states_address, states_email, states_note, states_organization,
    states_phone, states_title, title_kind,
};
use serde_json::{Map, Value};

/// The patch that turns the card the server holds into the card Evolution
/// just saved. Empty when the edit changed nothing this mapping can see.
pub fn diff(current: &ContactCard, edited: &ContactCard) -> Map<String, Value> {
    let mut patch = Map::new();
    diff_name(&mut patch, current.name.as_ref(), edited.name.as_ref());
    diff_emails(&mut patch, current.emails.as_ref(), edited.emails.as_ref());
    diff_phones(&mut patch, current.phones.as_ref(), edited.phones.as_ref());
    diff_organizations(
        &mut patch,
        current.organizations.as_ref(),
        edited.organizations.as_ref(),
    );
    diff_titles(&mut patch, current.titles.as_ref(), edited.titles.as_ref());
    diff_addresses(
        &mut patch,
        current.addresses.as_ref(),
        edited.addresses.as_ref(),
    );
    diff_notes(&mut patch, current.notes.as_ref(), edited.notes.as_ref());
    patch
}

fn diff_name(patch: &mut Map<String, Value>, current: Option<&Name>, edited: Option<&Name>) {
    let (Some(current), Some(edited)) = (current, edited) else {
        match (current, edited) {
            (Some(_), None) => drop(patch.insert("name".to_owned(), Value::Null)),
            (None, Some(edited)) => {
                patch.insert("name".to_owned(), json_of(edited));
            }
            _ => {}
        }
        return;
    };

    if current.full != edited.full {
        patch.insert("name/full".to_owned(), value_or_null(edited.full.as_ref()));
    }

    // Components of a kind the `N` value has no field for are not the user's
    // to have deleted, so they are carried over. Their position among the
    // mapped ones is not preserved — JSContact only ascribes meaning to the
    // order when `isOrdered` is set, and the alternative is guessing.
    let mut merged: Vec<_> = edited.components.clone().unwrap_or_default();
    merged.extend(
        current
            .components
            .iter()
            .flatten()
            .filter(|component| !maps_name_component(&component.kind))
            .cloned(),
    );
    let merged = (!merged.is_empty()).then_some(merged);
    if current.components != merged {
        patch.insert(
            "name/components".to_owned(),
            merged.map_or(Value::Null, |components| json_of(&components)),
        );
    }
}

fn diff_emails(
    patch: &mut Map<String, Value>,
    current: Option<&BTreeMap<String, ContactEmail>>,
    edited: Option<&BTreeMap<String, ContactEmail>>,
) {
    diff_entries(
        patch,
        "emails",
        current,
        edited,
        states_email,
        |patch, path, old, new| {
            if old.address != new.address {
                patch.insert(
                    format!("{path}/address"),
                    Value::String(new.address.clone()),
                );
            }
            diff_flags(
                patch,
                path,
                "contexts",
                &old.contexts,
                &new.contexts,
                maps_context,
            );
            diff_pref(patch, path, old.pref, new.pref);
        },
    );
}

fn diff_phones(
    patch: &mut Map<String, Value>,
    current: Option<&BTreeMap<String, ContactPhone>>,
    edited: Option<&BTreeMap<String, ContactPhone>>,
) {
    diff_entries(
        patch,
        "phones",
        current,
        edited,
        states_phone,
        |patch, path, old, new| {
            if old.number != new.number {
                patch.insert(format!("{path}/number"), Value::String(new.number.clone()));
            }
            diff_flags(
                patch,
                path,
                "contexts",
                &old.contexts,
                &new.contexts,
                maps_context,
            );
            diff_flags(
                patch,
                path,
                "features",
                &old.features,
                &new.features,
                maps_phone_feature,
            );
        },
    );
}

fn diff_organizations(
    patch: &mut Map<String, Value>,
    current: Option<&BTreeMap<String, Organization>>,
    edited: Option<&BTreeMap<String, Organization>>,
) {
    diff_entries(
        patch,
        "organizations",
        current,
        edited,
        states_organization,
        |patch, path, old, new| {
            if old.name != new.name {
                patch.insert(format!("{path}/name"), value_or_null(new.name.as_ref()));
            }
            let units = merge_units(old.units.as_deref(), new.units.as_deref());
            if old.units.as_deref() != units.as_deref() {
                patch.insert(
                    format!("{path}/units"),
                    units.map_or(Value::Null, |units| json_of(&units)),
                );
            }
        },
    );
}

fn diff_titles(
    patch: &mut Map<String, Value>,
    current: Option<&BTreeMap<String, Title>>,
    edited: Option<&BTreeMap<String, Title>>,
) {
    diff_entries(
        patch,
        "titles",
        current,
        edited,
        states_title,
        |patch, path, old, new| {
            if old.name != new.name {
                patch.insert(format!("{path}/name"), Value::String(new.name.clone()));
            }
            // Only a TITLE that became a ROLE or the reverse can be seen
            // here, and the kinds are compared with the default filled in so
            // that a card which never named one is not made to name it.
            let kind = title_kind(new.kind.as_deref());
            if title_kind(old.kind.as_deref()) != kind {
                patch.insert(format!("{path}/kind"), Value::String(kind.to_owned()));
            }
        },
    );
}

fn diff_addresses(
    patch: &mut Map<String, Value>,
    current: Option<&BTreeMap<String, Address>>,
    edited: Option<&BTreeMap<String, Address>>,
) {
    diff_entries(
        patch,
        "addresses",
        current,
        edited,
        states_address,
        |patch, path, old, new| {
            let components = merge_components(old.components.as_deref(), new.components.as_deref());
            if old.components.as_deref() != components.as_deref() {
                patch.insert(
                    format!("{path}/components"),
                    components.map_or(Value::Null, |components| json_of(&components)),
                );
            }
            // The `LABEL` line, compared as the emitter sees it: an address
            // written out as the empty string has no line either, so reading
            // no label back is not the user having cleared one.
            let label = address_label(new);
            if address_label(old) != label {
                patch.insert(
                    format!("{path}/full"),
                    label.map_or(Value::Null, |full| Value::String(full.to_owned())),
                );
            }
            diff_flags(
                patch,
                path,
                "contexts",
                &old.contexts,
                &new.contexts,
                maps_context,
            );
        },
    );
}

fn diff_notes(
    patch: &mut Map<String, Value>,
    current: Option<&BTreeMap<String, Note>>,
    edited: Option<&BTreeMap<String, Note>>,
) {
    diff_entries(
        patch,
        "notes",
        current,
        edited,
        states_note,
        |patch, path, old, new| {
            // Only the text can have been edited: when the note was written
            // and by whom never reached the vCard, so they are patched
            // around rather than through.
            if old.note != new.note {
                patch.insert(format!("{path}/note"), Value::String(new.note.clone()));
            }
        },
    );
}

/// The component list a save writes: what the `ADR` line now states, with
/// everything it had no field for left where it was.
///
/// Like [`merge_units`] this is a list with no keys to patch by, so a
/// component is recognised by what it says — same kind, same value — and one
/// that still says it keeps the members the line could not carry, its
/// `phonetic` spelling above all. Walking the server's list rather than the
/// edited one is what keeps an invisible component in its place instead of
/// shuffling it to the end, so that opening a contact and closing it again
/// writes nothing.
///
/// Matching by value only works once the components a single `ADR` field was
/// built from have been told apart again, which is
/// [`restore_address_components`]' job: a street name and its house number
/// come back from the vCard as one street, and would otherwise both read as
/// deleted and be replaced by their own concatenation.
fn merge_components(
    current: Option<&[AddressComponent]>,
    edited: Option<&[AddressComponent]>,
) -> Option<Vec<AddressComponent>> {
    let current = current.unwrap_or_default();
    let edited = restore_address_components(current, edited?);
    let mut spare: Vec<&AddressComponent> = edited.iter().collect();
    let mut merged: Vec<AddressComponent> = Vec::new();
    for component in current {
        let same = |candidate: &&AddressComponent| {
            candidate.kind == component.kind && candidate.value == component.value
        };
        match spare.iter().position(same) {
            // Still on the line, so the server's copy is the one to keep.
            Some(index) => {
                spare.remove(index);
                merged.push(component.clone());
            }
            // Gone from the line — which says the user deleted it only if
            // the line had a field for it in the first place.
            None if !maps_address_component(&component.kind) => merged.push(component.clone()),
            None => {}
        }
    }
    merged.extend(spare.into_iter().cloned());
    (!merged.is_empty()).then_some(merged)
}

/// The unit list a save writes: the names the `ORG` line now states, each
/// keeping whatever the server's unit of that name was carrying besides it.
///
/// A list has no keys to patch by, so this is the same merging `diff_flags`
/// does for a boolean map, done by name: a unit's `sortAs` is a hint about
/// how to file *that* unit, so it follows the name wherever it moved to and
/// is left behind when the name is gone. Renaming a unit therefore drops its
/// hint — which is right, because a hint for the old name is not one for the
/// new.
fn merge_units(current: Option<&[OrgUnit]>, edited: Option<&[OrgUnit]>) -> Option<Vec<OrgUnit>> {
    let edited = edited?;
    let mut spare: Vec<&OrgUnit> = current.unwrap_or_default().iter().collect();
    let merged: Vec<OrgUnit> = edited
        .iter()
        .map(
            |unit| match spare.iter().position(|old| old.name == unit.name) {
                Some(index) => spare.remove(index).clone(),
                None => unit.clone(),
            },
        )
        .collect();
    (!merged.is_empty()).then_some(merged)
}

/// Shared shape of the keyed maps: added entries are written whole, dropped
/// entries are nulled, and surviving entries are handed to `diff_entry` to be
/// compared field by field — over the entries the vCard actually stated.
///
/// An entry the predicate calls invisible never reached the user, so the save
/// must not read an edit into its absence. Three things follow, and each is
/// a way the plain shape above would destroy it:
///
/// - it is never nulled for being missing from the edited card;
/// - a first visible entry is written *into* the property by path rather than
///   replacing it, because the property does exist server-side however empty
///   it looks from here;
/// - an addition is moved off its key if an invisible entry already holds it.
///   The reader invents keys by counting the entries it can see, so `t1` for
///   a title the user just typed can be the key of one it never showed.
fn diff_entries<T: serde::Serialize>(
    patch: &mut Map<String, Value>,
    property: &str,
    current: Option<&BTreeMap<String, T>>,
    edited: Option<&BTreeMap<String, T>>,
    is_visible: impl Fn(&T) -> bool,
    diff_entry: impl Fn(&mut Map<String, Value>, &str, &T, &T),
) {
    let empty = BTreeMap::new();
    let current = current.unwrap_or(&empty);
    let edited = edited.unwrap_or(&empty);
    let visible: BTreeMap<&String, &T> = current
        .iter()
        .filter(|(_, entry)| is_visible(entry))
        .collect();
    let hides_something = visible.len() < current.len();

    if visible.is_empty() && edited.is_empty() {
        return;
    }
    if visible.is_empty() && !hides_something {
        patch.insert(property.to_owned(), json_of(edited));
        return;
    }
    if edited.is_empty() {
        if hides_something {
            for key in visible.keys() {
                patch.insert(format!("{property}/{}", escape(key)), Value::Null);
            }
        } else {
            patch.insert(property.to_owned(), Value::Null);
        }
        return;
    }

    let mut taken: BTreeSet<String> = current.keys().cloned().collect();
    for (key, entry) in edited {
        match visible.get(key) {
            Some(existing) => {
                let path = format!("{property}/{}", escape(key));
                diff_entry(patch, &path, existing, entry);
            }
            None => {
                let key = free_key(key, &taken);
                patch.insert(format!("{property}/{}", escape(&key)), json_of(entry));
                taken.insert(key);
            }
        }
    }
    for key in visible.keys().filter(|key| !edited.contains_key(**key)) {
        patch.insert(format!("{property}/{}", escape(key)), Value::Null);
    }
}

/// The key an added entry is written under: the one it came with, or the
/// first free variant of it when that is somebody else's.
fn free_key(wanted: &str, taken: &BTreeSet<String>) -> String {
    if !taken.contains(wanted) {
        return wanted.to_owned();
    }
    (2..)
        .map(|index| format!("{wanted}-{index}"))
        .find(|candidate| !taken.contains(candidate))
        .expect("an unbounded sequence has a free element")
}

/// Replace the members of a boolean map this mapping can spell, keep the
/// rest.
fn diff_flags(
    patch: &mut Map<String, Value>,
    path: &str,
    property: &str,
    current: &Option<Value>,
    edited: &Option<Value>,
    is_mapped: impl Fn(&str) -> bool,
) {
    let mut merged: Map<String, Value> = match current {
        Some(Value::Object(flags)) => flags.clone(),
        _ => Map::new(),
    };
    merged.retain(|key, _| !is_mapped(key));
    if let Some(Value::Object(flags)) = edited {
        merged.extend(
            flags
                .iter()
                .filter(|(key, _)| is_mapped(key))
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }

    let merged = (!merged.is_empty()).then_some(Value::Object(merged));
    if current.as_ref() != merged.as_ref() {
        patch.insert(format!("{path}/{property}"), merged.unwrap_or(Value::Null));
    }
}

/// vCard 3.0 knows only that an entry is preferred, not how strongly, so a
/// rank the server already has is left alone.
fn diff_pref(
    patch: &mut Map<String, Value>,
    path: &str,
    current: Option<u32>,
    edited: Option<u32>,
) {
    let wanted = edited.and(current.or(edited));
    if wanted != current {
        patch.insert(
            format!("{path}/pref"),
            wanted.map_or(Value::Null, |rank| Value::Number(rank.into())),
        );
    }
}

/// JSON Pointer escaping (RFC 6901 §3) for a map key we did not choose.
fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn value_or_null(value: Option<&String>) -> Value {
    value.map_or(Value::Null, |value| Value::String(value.clone()))
}

/// Serialising a type built from a vCard cannot fail: it holds strings,
/// numbers and maps of them.
fn json_of<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}
