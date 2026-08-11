// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning an edited vCard back into a `ContactCard/set` PatchObject.
//!
//! The whole point of patching rather than replacing is that a vCard is a
//! lossy view of a JSContact card. The mapping keeps UID, FN, N, NICKNAME,
//! EMAIL, TEL, ADR, LABEL, ORG, TITLE, ROLE, NOTE, URL, CATEGORIES, the
//! instant-messaging `X-` lines and the two date lines, and drops everything
//! else, so a save that sent the parsed card back whole would silently delete
//! the properties it could not represent — preferred languages, what the
//! contact is spoken to as, the media a card carries — none of which the user
//! ever saw, let alone asked to remove.
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
//! - `nicknames` entries carry a `contexts` and a `pref` a `NICKNAME` line has
//!   no parameter for, so the entry is patched by its name alone. Its key does
//!   survive Evolution, unlike a date's: EDS rewrites the value of that line in
//!   place and leaves the parameters where they were.
//! - `links` entries are patched by their URI alone, for the same reason a
//!   nickname's is: what the resource is and how strongly it is preferred have
//!   no parameter on a `URL` line. Their key survives Evolution — EDS rewrites
//!   the value of that line in place and leaves the parameters where they were.
//! - `anniversaries` entries have no key to be patched by at all: EDS keeps a
//!   birthday in a structured field and rebuilds the line out of it, dropping
//!   the `X-JMAP-KEY`, so the entry an edited date belongs to is found by what
//!   kind of date it is. The date itself is patched member by member, which
//!   is what keeps a `calendarScale` — or a point in time the user did not
//!   touch — from being flattened into the day the line showed.
//! - `onlineServices` entries are patched by their handle, which is the only
//!   part of them an `X-JABBER` line states; where the handle is used and how
//!   strongly it is preferred have no parameter on it, and the `TYPE` it does
//!   carry is the slot EDS files the handle in rather than the entry's
//!   `contexts`. Renaming the handle also *drops* the entry's `uri`, which named
//!   the handle the user has just replaced — see [`diff_online_services`], the
//!   one place here where a save removes a member the vCard never showed.
//! - `keywords` is the one mapped property that is a *set*, and the one that
//!   goes back **replaced whole**: a tag is a bare string with no key and no
//!   members, so there is nothing to reach into. That makes a tag the
//!   `CATEGORIES` line could not carry a tag a save would *delete* rather than
//!   merely one the user cannot see, so [`maps_keywords`] freezes the whole
//!   property for such a card — the one place here where the user's edit is
//!   dropped rather than merged. See [`diff_keywords`].
//! - *Every* keyed map is one of which the vCard states only **some**
//!   entries. A title of a `kind` outside `title` and `role` has no vCard
//!   property; an address with neither an `ADR` field nor a written-out form
//!   to put on a `LABEL`, an organisation with neither a name nor a unit, an
//!   email with no address, a phone with no number, a note that says nothing,
//!   a date naming no single day, a link of a kind vCard 3.0 cannot state and a
//!   handle at a service EDS has no field for all have no line to be written
//!   on. Each is dropped on the way
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
    Address, AddressComponent, Anniversary, ContactCard, ContactEmail, ContactPhone, Link, Name,
    Nickname, Note, OnlineService, OrgUnit, Organization, Title,
};
use jmap_vcard::{
    address_label, anniversary_date, maps_address_component, maps_context, maps_keywords,
    maps_name_component, maps_phone_feature, restore_address_components, same_service,
    states_a_point_in_time, states_address, states_anniversary, states_email, states_link,
    states_nickname, states_note, states_online_service, states_organization, states_phone,
    states_title, title_kind,
};
use serde_json::{Map, Value};

/// The patch that turns the card the server holds into the card Evolution
/// just saved. Empty when the edit changed nothing this mapping can see.
pub fn diff(current: &ContactCard, edited: &ContactCard) -> Map<String, Value> {
    let mut patch = Map::new();
    diff_name(&mut patch, current.name.as_ref(), edited.name.as_ref());
    diff_nicknames(
        &mut patch,
        current.nicknames.as_ref(),
        edited.nicknames.as_ref(),
    );
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
    diff_anniversaries(
        &mut patch,
        current.anniversaries.as_ref(),
        edited.anniversaries.as_ref(),
    );
    diff_links(&mut patch, current.links.as_ref(), edited.links.as_ref());
    diff_online_services(
        &mut patch,
        current.online_services.as_ref(),
        edited.online_services.as_ref(),
    );
    diff_keywords(&mut patch, current, edited);
    patch
}

/// The tags the contact is filed under — the one mapped property that goes back
/// **replaced whole** rather than patched entry by entry.
///
/// A `CATEGORIES` line holds the whole set and a JSContact keyword is a bare
/// string, so there is nothing inside an entry to preserve and no key to reach
/// into: the line states what was shown, and the difference from it is the set
/// the user now wants. Clearing the field is `"keywords": null`, which is how a
/// PatchObject asks for RFC 9553 §2.8.2's default of no tags — an empty map
/// would be a different thing to store, and is also what EDS does *not* leave
/// behind: it removes the attribute outright.
///
/// [`maps_keywords`] is asked of the server's own set, for the reason the other
/// properties ask their `states_*` predicate: a tag the `CATEGORIES` line could
/// not carry was never shown, and replacing the set whole would delete it. The
/// cost is that the user's edit to the field is dropped for such a card — the
/// only property here where an edit can be lost rather than merged — which is
/// the trade a set with no keys forces. The *edited* side needs no such check:
/// every tag on it was read off a content line, and any string is a keyword RFC
/// 9553 admits.
fn diff_keywords(patch: &mut Map<String, Value>, current: &ContactCard, edited: &ContactCard) {
    let empty = BTreeMap::new();
    let tags = |card: &ContactCard| {
        card.keywords
            .clone()
            .filter(|keywords| !keywords.is_empty())
    };
    if !maps_keywords(current.keywords.as_ref().unwrap_or(&empty)) {
        return;
    }
    // An empty set server-side is compared as no tags, because that is what it
    // was drawn as: without this a card holding one would be patched to a null
    // by every save, an edit nobody made.
    if tags(current) == tags(edited) {
        return;
    }
    patch.insert(
        "keywords".to_owned(),
        match tags(edited) {
            // Serialising a set this crate's own reader built cannot fail: it
            // holds strings and `true`.
            Some(tags) => serde_json::to_value(tags).unwrap_or(Value::Null),
            None => Value::Null,
        },
    );
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

fn diff_nicknames(
    patch: &mut Map<String, Value>,
    current: Option<&BTreeMap<String, Nickname>>,
    edited: Option<&BTreeMap<String, Nickname>>,
) {
    diff_entries(
        patch,
        "nicknames",
        current,
        edited,
        states_nickname,
        |patch, path, old, new| {
            // Only the name can have been edited: the context the nickname is
            // used in and how strongly it is preferred never reached the
            // vCard, so they are patched around rather than through.
            if old.name != new.name {
                patch.insert(format!("{path}/name"), Value::String(new.name.clone()));
            }
        },
    );
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

fn diff_links(
    patch: &mut Map<String, Value>,
    current: Option<&BTreeMap<String, Link>>,
    edited: Option<&BTreeMap<String, Link>>,
) {
    diff_entries(
        patch,
        "links",
        current,
        edited,
        states_link,
        |patch, path, old, new| {
            // Only the URI can have been edited: what the resource is, where
            // it is used, how strongly it is preferred and what to call it
            // never reached the vCard, so they are patched around rather than
            // through. Nor can the `kind` have changed — a `URL` states the
            // one kind that has no name, and an entry of any other kind has no
            // line to be edited on.
            if old.uri != new.uri {
                patch.insert(format!("{path}/uri"), Value::String(new.uri.clone()));
            }
        },
    );
}

fn diff_online_services(
    patch: &mut Map<String, Value>,
    current: Option<&BTreeMap<String, OnlineService>>,
    edited: Option<&BTreeMap<String, OnlineService>>,
) {
    diff_entries(
        patch,
        "onlineServices",
        current,
        edited,
        states_online_service,
        |patch, path, old, new| {
            if old.user != new.user {
                patch.insert(format!("{path}/user"), value_or_null(new.user.as_ref()));
                // The URI stated the handle that has just been replaced, and
                // nothing here can rebuild it from the new one: that needs the
                // service's URI scheme, which this mapping does not know and
                // must not guess at. So it goes with the name it belonged to,
                // for the reason `merge_units` drops a renamed unit's `sortAs` —
                // a URI for the old handle is not one for the new.
                if old.uri.is_some() {
                    patch.insert(format!("{path}/uri"), Value::Null);
                }
            }
            // Which the line does not state — it states the service by *being*
            // the property EDS keeps that service's handles on — so this can
            // only be a key that arrived on another service's line. Compared as
            // RFC 9553 §2.3.2 asks, so the spelling the server chose is left
            // alone rather than replaced by the one this side reads back.
            if !same_service(old.service.as_deref(), new.service.as_deref()) {
                patch.insert(
                    format!("{path}/service"),
                    value_or_null(new.service.as_ref()),
                );
            }
        },
    );
}

fn diff_anniversaries(
    patch: &mut Map<String, Value>,
    current: Option<&BTreeMap<String, Anniversary>>,
    edited: Option<&BTreeMap<String, Anniversary>>,
) {
    let edited = rekey_anniversaries(current, edited);
    diff_entries(
        patch,
        "anniversaries",
        current,
        edited.as_ref(),
        states_anniversary,
        |patch, path, old, new| {
            // The line the date is stated on *is* its kind, so this can only
            // be a birthday that became a wedding day or the reverse.
            if old.kind != new.kind {
                patch.insert(format!("{path}/kind"), Value::String(new.kind.clone()));
            }
            if anniversary_date(old) == anniversary_date(new) {
                return;
            }
            let Some(date) = new.date.as_ref() else {
                return;
            };
            // A day is patched member by member, so that whatever else the
            // server hung off the date — a `calendarScale`, a member this
            // version has never heard of — stays where it is. A point in time
            // cannot be mended that way: the user typed a day, and a day is a
            // different kind of object, so it replaces the old one whole.
            if states_a_point_in_time(old) {
                patch.insert(format!("{path}/date"), date.clone());
                return;
            }
            for member in ["year", "month", "day"] {
                let Some(value) = date.get(member) else {
                    continue;
                };
                if old.date.as_ref().and_then(|old| old.get(member)) != Some(value) {
                    patch.insert(format!("{path}/date/{member}"), value.clone());
                }
            }
        },
    );
}

/// The edited anniversaries under the keys the server holds them by.
///
/// Every other keyed map crosses with its key in `X-JMAP-KEY` and comes back
/// wearing it. The dates do not: EDS keeps the birthday in a structured field
/// and rebuilds the line out of it, dropping the parameters — verified against
/// libebook-contacts 3.52, where an untouched line keeps them and a rewritten
/// one does not. So the entry a keyless date belongs to is found by what kind
/// of date it is, which is enough because Evolution has exactly one field per
/// kind: the birthday it hands back is the birthday the card already had.
///
/// Entries of one kind are paired in order, and an entry whose key *did*
/// survive keeps it and is not paired against — otherwise a card carrying two
/// birthdays, of which Evolution shows the first and passes the second
/// through untouched, would have them swapped by every save.
fn rekey_anniversaries(
    current: Option<&BTreeMap<String, Anniversary>>,
    edited: Option<&BTreeMap<String, Anniversary>>,
) -> Option<BTreeMap<String, Anniversary>> {
    let edited = edited?;
    let empty = BTreeMap::new();
    let current = current.unwrap_or(&empty);
    let mut unclaimed: Vec<(&String, &Anniversary)> = current
        .iter()
        .filter(|(key, entry)| states_anniversary(entry) && !edited.contains_key(*key))
        .collect();

    let mut rekeyed = BTreeMap::new();
    for (key, entry) in edited {
        let key = match current.contains_key(key) {
            true => key.clone(),
            false => match unclaimed.iter().position(|(_, old)| old.kind == entry.kind) {
                Some(index) => unclaimed.remove(index).0.clone(),
                None => key.clone(),
            },
        };
        rekeyed.insert(key, entry.clone());
    }
    Some(rekeyed)
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
