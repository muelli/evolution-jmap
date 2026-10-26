// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Converts an edited vCard into a `ContactCard/set` PatchObject.
//!
//! The save patches the card and does not replace it, because a vCard is a
//! lossy view of a JSContact card. The mapping keeps UID, FN, N, NICKNAME,
//! EMAIL, TEL, ADR, LABEL, ORG, TITLE, ROLE, NOTE, URL, CALURI, FBURL, PHOTO,
//! CATEGORIES, the instant-messaging `X-` lines, the spouse line and the two
//! date lines. It drops all other properties (for example preferred languages,
//! speak-to-as and crypto keys). A whole-card replace would delete them.
//!
//! The mapped properties are also lossy:
//!
//! - `emails`/`phones` are keyed maps. The key crosses in `X-JMAP-KEY`, so an
//!   entry is patched by its key (`emails/work/address`). Without the key, an
//!   edit becomes a remove and an add, and the entry loses its unmapped
//!   properties and its server-side identity.
//! - `contexts` and `features` are boolean maps, and vCard 3.0 can spell only
//!   some members. A `school` context has no `TYPE`, so the save merges it
//!   back. On an `ADR` and a `TEL`, the line also omits one of the two
//!   contexts vCard can spell, so the merged members depend on the entry (see
//!   `slotted_context`).
//! - `pref` is a rank from 1 to 100, but vCard 3.0 has only a flag. A
//!   preferred entry keeps its rank. The flag can add or remove a preference,
//!   but it cannot change the rank.
//! - `name.components` is a list with no keys, so it goes back whole and is
//!   merged like an address's components (`merge_named`). A component that `N`
//!   did not state stays. A component with an unchanged value keeps its
//!   unmapped members, mainly its `phonetic` spelling. The server's order
//!   stays, so that opening and closing a contact writes nothing. Before the
//!   merge, components that shared one `N` field are split again (for
//!   example, a double-barrelled given name).
//! - `organizations` entries hold a `sortAs` and `contexts` that `ORG` cannot
//!   carry, so the save patches each member (`organizations/work/name`).
//!   `units` is a list, so it goes back whole. A unit keeps its members when
//!   its name is unchanged, matched by name and not by position.
//! - `addresses` entries cross on two lines: the `ADR` and the `LABEL` with
//!   the written-out address. The save reads them back as one entry and
//!   patches `addresses/<key>/full` with the components.
//! - `nicknames` entries have a `contexts` and a `pref` that `NICKNAME` cannot
//!   carry, so the save patches only the name. The key survives Evolution: EDS
//!   rewrites the line value in place and keeps the parameters.
//! - `links` entries are patched by their URI only, for the same reason. The
//!   key survives Evolution in the same way.
//! - `calendars` entries are patched by their URI only, and the key survives
//!   Evolution. The line name states the `kind`, but the user cannot edit it:
//!   a move to the other field arrives as a deletion and an addition.
//! - `media` entries have no surviving key. The save compares what the `PHOTO`
//!   line states, not the members. The user can edit only the first picture
//!   (see `diff_media`).
//! - `anniversaries` entries have no surviving key: EDS rebuilds a birthday
//!   line from a structured field and drops `X-JMAP-KEY`. The save finds the
//!   entry by the date kind and patches the date member by member. This keeps
//!   a `calendarScale` or an untouched point in time.
//! - `onlineServices` entries are patched by their handle, the only part an
//!   `X-JABBER` line states. The `TYPE` on the line is the EDS slot, not the
//!   entry's `contexts`. The handle is the entry's `user`, or the handle in
//!   its `uri`. A rename goes back on the same member. If that is not
//!   possible, the save drops the old `uri`: this is the only place where a
//!   save removes a member the vCard did not show (see `diff_online_services`).
//! - `relatedTo` is keyed by the related entity, so the line shows the key to
//!   the user. An edit withdraws a relation from one entity and adds it to
//!   another. Both changes patch the relation set, so an unstated relation
//!   survives (see `diff_related_to`).
//! - `keywords` is a set and goes back replaced whole, because a tag has no
//!   key and no members. The save adds each tag that `CATEGORIES` could not
//!   carry, so the save does not delete it (see `diff_keywords`).
//! - The vCard states only some entries of each keyed map. These have no line:
//!   a title of a `kind` other than `title` and `role`, an address with no
//!   `ADR` field and no written-out form, an organisation with no name and no
//!   unit, an email with no address, a phone with no number, an empty note, a
//!   date with no single day, a link of a kind vCard 3.0 cannot state, a
//!   calendar of neither line kind, and a handle at a service EDS has no field
//!   for. The save must not delete these entries for their absence. It must
//!   also not overwrite them with an addition whose key the reader made by
//!   counting only the visible entries. `diff_entries` and the `states_*`
//!   predicates prevent this. The predicates are next to the emitter, so they
//!   match what the emitter omits.
//!
//! RFC 8620 §5.3 requires every path segment before the last to exist on the
//! object already. Thus the save writes a property whole when the server does
//! not have it.

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::contacts::{
    Address, AddressComponent, Anniversary, Calendar, ContactCard, ContactEmail, ContactPhone,
    Link, Media, Name, Nickname, Note, OnlineService, OrgUnit, Organization, Relation, Title,
};
use jmap_vcard::{
    address_label, anniversary_date, maps_context, maps_phone_feature, online_service_handle,
    online_service_uri, restore_address_components, restore_name_components, same_photo,
    same_service, states_a_point_in_time, states_address, states_address_component,
    states_anniversary, states_assistant, states_calendar, states_context, states_email,
    states_keyword, states_link, states_manager, states_media, states_name, states_name_component,
    states_nickname, states_note, states_online_service, states_org_unit, states_organization,
    states_phone, states_phone_feature, states_spouse, states_title, title_kind,
};
use serde_json::{Map, Value, json};

type RelationPredicate = fn(&str, &Relation) -> bool;

/// The mapped relation types that have dedicated vCard property lines in EDS.
const MAPPED_RELATIONS: &[(&str, RelationPredicate)] = &[
    ("spouse", states_spouse),
    ("manager", states_manager),
    ("assistant", states_assistant),
];

/// Returns the patch from the server's card to the card that Evolution saved.
/// The patch is empty when the edit changed nothing that the mapping shows.
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
    diff_calendars(
        &mut patch,
        current.calendars.as_ref(),
        edited.calendars.as_ref(),
    );
    diff_media(&mut patch, current.media.as_ref(), edited.media.as_ref());
    diff_online_services(
        &mut patch,
        current.online_services.as_ref(),
        edited.online_services.as_ref(),
    );
    diff_related_to(
        &mut patch,
        current.related_to.as_ref(),
        edited.related_to.as_ref(),
    );
    diff_keywords(&mut patch, current, edited);
    patch
}

/// Returns [`diff`] from `current` to `edited`, limited to the properties that
/// differ between `base` and `edited`.
///
/// `base` is the card that the user edited. A property that only another client
/// changed since then keeps the server's value.
pub fn diff_since(
    base: &ContactCard,
    current: &ContactCard,
    edited: &ContactCard,
) -> Map<String, Value> {
    diff_since_except(base, current, edited, &BTreeSet::new())
}

/// Returns [`diff_since`] without the properties in `kept`, which keep the server's value.
pub fn diff_since_except(
    base: &ContactCard,
    current: &ContactCard,
    edited: &ContactCard,
    kept: &BTreeSet<String>,
) -> Map<String, Value> {
    let touched = diff(base, edited);
    let mut patch = diff(current, edited);
    patch.retain(|path, _| {
        let name = property(path);
        !kept.contains(name) && touched.keys().any(|edit| property(edit) == name)
    });
    patch
}

/// Returns the properties that the user and a different client both changed
/// after `base`, to different values.
pub fn conflicts(
    base: &ContactCard,
    current: &ContactCard,
    edited: &ContactCard,
) -> BTreeSet<String> {
    let remote = diff(current, base);
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

/// Patches the mapped relations (`spouse`, `manager`, `assistant`). The line
/// shows the key, so an edit withdraws a relation and adds a relation.
///
/// RFC 9553 §2.1.8 keys `relatedTo` by the related entity. RFC 9555 §2.9.5
/// lets that key be a person's name and not a `uid`. Thus a respelled name is
/// a different entity, and the line stated only the relation type of the old
/// entity. The save withdraws only that relation type:
///
/// - A name removed from the field loses that relation. The entry goes too if
///   that relation was all it held. If the entry also has an unmapped relation
///   (for example `kin`), the entry stays and only the mapped relation goes.
/// - A name added to the field gains the relation. If the card already has an
///   entry with that name, it is the same entity, so the save adds the type to
///   the set. A relation that the user cannot see stays.
fn diff_related_to(
    patch: &mut Map<String, Value>,
    current: Option<&BTreeMap<String, Relation>>,
    edited: Option<&BTreeMap<String, Relation>>,
) {
    let empty = BTreeMap::new();
    let current = current.unwrap_or(&empty);
    let edited = edited.unwrap_or(&empty);

    let mut changes_found = false;
    for &(_, predicate) in MAPPED_RELATIONS {
        let shown: BTreeSet<&String> = current
            .iter()
            .filter(|(key, rel)| predicate(key, rel))
            .map(|(key, _)| key)
            .collect();
        let wanted: BTreeSet<&String> = edited
            .iter()
            .filter(|(key, rel)| predicate(key, rel))
            .map(|(key, _)| key)
            .collect();
        if shown != wanted {
            changes_found = true;
            break;
        }
    }

    if !changes_found {
        return;
    }

    // RFC 8620 §5.3: a card with no `relatedTo` has no path to patch into, so
    // write the property whole.
    if current.is_empty() {
        let wanted_map: BTreeMap<&String, &Relation> = edited
            .iter()
            .filter(|(key, rel)| {
                MAPPED_RELATIONS
                    .iter()
                    .any(|&(_, predicate)| predicate(key, rel))
            })
            .collect();
        if !wanted_map.is_empty() {
            patch.insert("relatedTo".to_owned(), json_of(&wanted_map));
        }
        return;
    }

    // The edit removed every mapped relation, and the server holds only mapped
    // relations: remove the whole property.
    let any_wanted = edited.iter().any(|(key, rel)| {
        MAPPED_RELATIONS
            .iter()
            .any(|&(_, predicate)| predicate(key, rel))
    });
    let all_current_only_mapped = current.values().all(|relation| {
        relation.extra.keys().all(|member| member == "@type")
            && relation
                .relation
                .iter()
                .flatten()
                .all(|(kind, _)| MAPPED_RELATIONS.iter().any(|&(mapped, _)| mapped == kind))
    });

    if !any_wanted && all_current_only_mapped {
        patch.insert("relatedTo".to_owned(), Value::Null);
        return;
    }

    for &(kind, predicate) in MAPPED_RELATIONS {
        let shown: BTreeMap<&String, &Relation> = current
            .iter()
            .filter(|(key, rel)| predicate(key, rel))
            .collect();
        let wanted: BTreeMap<&String, &Relation> = edited
            .iter()
            .filter(|(key, rel)| predicate(key, rel))
            .collect();

        if shown.keys().eq(wanted.keys()) {
            continue;
        }

        // Relations of this kind that the edit withdrew.
        for (&key, relation) in shown.iter().filter(|(key, _)| !wanted.contains_key(*key)) {
            let path = format!("relatedTo/{}", escape(key));
            let is_only_this_kind = relation.extra.keys().all(|member| member == "@type")
                && relation.relation.iter().flatten().all(|(k, _)| k == kind);

            let is_gaining_other_kind = MAPPED_RELATIONS
                .iter()
                .filter(|&(other_kind, _)| *other_kind != kind)
                .any(|&(_, other_pred)| {
                    edited.get(key).is_some_and(|r| other_pred(key, r))
                        && !current.get(key).is_some_and(|r| other_pred(key, r))
                });

            if is_only_this_kind && !is_gaining_other_kind {
                patch.insert(path, Value::Null);
            } else {
                patch.insert(format!("{path}/relation/{kind}"), Value::Null);
            }
        }

        // Relations of this kind that the edit added.
        for (&key, relation) in wanted.iter().filter(|(key, _)| !shown.contains_key(*key)) {
            let path = format!("relatedTo/{}", escape(key));
            match current.get(key) {
                Some(existing) => match existing.relation.is_some() {
                    true => {
                        patch.insert(format!("{path}/relation/{kind}"), Value::Bool(true));
                    }
                    false => {
                        patch.insert(format!("{path}/relation"), json!({kind: true}));
                    }
                },
                None => {
                    // An earlier mapped kind can have written this entity whole
                    // from the same `edited` value, with this kind included. A
                    // patch into a sub-path of that create is forbidden by
                    // RFC 8620 §5.3, so do not write again.
                    if !patch.contains_key(&path) {
                        patch.insert(path, json_of(relation));
                    }
                }
            }
        }
    }
}

/// Patches the tags. This is the only mapped property that goes back replaced
/// whole and not entry by entry.
///
/// A `CATEGORIES` line holds the whole set, and a keyword is a bare string with
/// no key and no members. A cleared field gives `"keywords": null`, which asks
/// for the RFC 9553 §2.8.2 default of no tags. An empty map is a different
/// value, and EDS also removes the attribute fully.
///
/// [`states_keyword`] checks the server's set. The user did not see a tag that
/// `CATEGORIES` could not carry, so its absence is not a removal. A set has no
/// key to omit from the patch, so the save adds that tag to the written set.
/// The rest of the edit still applies.
///
/// A carried tag keeps the server's value, also a value that RFC 9553 §1.4.3
/// does not admit: a change to it is not the user's edit. Where both sides name
/// the same tag, the user's set wins.
///
/// The edited side needs no check: each tag on it comes from a content line,
/// and RFC 9553 admits any string as a keyword.
fn diff_keywords(patch: &mut Map<String, Value>, current: &ContactCard, edited: &ContactCard) {
    // Compare an empty server set as no tags, as the vCard shows it. Otherwise
    // each save patches it to null.
    let tags = |card: &ContactCard| {
        card.keywords
            .clone()
            .filter(|keywords| !keywords.is_empty())
            .unwrap_or_default()
    };
    let current = tags(current);
    let mut wanted: BTreeMap<String, Value> = current
        .iter()
        .filter(|(tag, set)| !states_keyword(tag, set))
        .map(|(tag, set)| (tag.clone(), set.clone()))
        .collect();
    wanted.extend(tags(edited));
    if wanted == current {
        return;
    }
    patch.insert(
        "keywords".to_owned(),
        if wanted.is_empty() {
            Value::Null
        } else {
            // Serialisation cannot fail: the set holds strings and values from
            // the reader and the server.
            serde_json::to_value(wanted).unwrap_or(Value::Null)
        },
    );
}

fn has_unmapped_name_metadata(name: &Name) -> bool {
    name.is_ordered.is_some()
        || name.sort_as.is_some()
        || name.extra.keys().any(|k| k != "fileAs" && k != "file_as")
        || name
            .components
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|c| !states_name_component(c))
}

fn diff_name(patch: &mut Map<String, Value>, current: Option<&Name>, edited: Option<&Name>) {
    let current_stated = current.is_some_and(states_name);
    let edited_stated = edited.is_some_and(states_name);

    if !current_stated && !edited_stated {
        return;
    }

    let (Some(current), Some(edited)) = (current, edited) else {
        match (current, edited) {
            (Some(current), None) => {
                if has_unmapped_name_metadata(current) {
                    if current.full.is_some() {
                        patch.insert("name/full".to_owned(), Value::Null);
                    }
                    // Clear only the components that `N` showed. Keep the others,
                    // as an edit does.
                    let components = current.components.as_deref().unwrap_or_default();
                    let kept: Vec<_> = components
                        .iter()
                        .filter(|c| !states_name_component(c))
                        .collect();
                    if kept.len() != components.len() {
                        patch.insert(
                            "name/components".to_owned(),
                            if kept.is_empty() {
                                Value::Null
                            } else {
                                json_of(&kept)
                            },
                        );
                    }
                } else {
                    patch.insert("name".to_owned(), Value::Null);
                }
            }
            (None, Some(edited)) => {
                patch.insert("name".to_owned(), json_of(edited));
            }
            _ => {}
        }
        return;
    };

    let current_full = current.full.as_deref().filter(|full| !full.is_empty());
    let edited_full = edited.full.as_deref().filter(|full| !full.is_empty());
    if current_full != edited_full {
        patch.insert(
            "name/full".to_owned(),
            edited_full.map_or(Value::Null, |full| Value::String(full.to_owned())),
        );
    }

    // The component list has no keys, so it goes back whole and is merged like
    // an address's components. First split the components that shared one `N`
    // field (for example, a double-barrelled given name). Otherwise the merge
    // reads both halves as deleted and replaced by their join.
    let current_components = current.components.as_deref().unwrap_or_default();
    let merged = merge_named(
        current_components,
        &restore_name_components(
            current_components,
            edited.components.as_deref().unwrap_or_default(),
        ),
        |component| (&component.kind, &component.value),
        states_name_component,
    );
    let merged = (!merged.is_empty()).then_some(merged);
    if current.components.as_ref().filter(|c| !c.is_empty()) != merged.as_ref() {
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
            // Only the name crosses the vCard, so only the name can change.
            if old.name != new.name {
                patch.insert(format!("{path}/name"), Value::String(new.name.clone()));
            }
        },
    );
}

fn entry_pref(pref: Option<u32>, extra: &BTreeMap<String, Value>) -> Option<u32> {
    pref.or_else(|| {
        extra
            .get("pref")
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok())
    })
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
                maps_context,
            );
            diff_pref(
                patch,
                path,
                entry_pref(old.pref, &old.extra),
                entry_pref(new.pref, &new.extra),
            );
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
                slotted_context(&old.contexts),
            );
            diff_flags(
                patch,
                path,
                "features",
                &old.features,
                &new.features,
                maps_phone_feature,
                slotted_feature(&old.features),
            );
            diff_pref(
                patch,
                path,
                entry_pref(old.pref, &old.extra),
                entry_pref(new.pref, &new.extra),
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
            let old_name = old.name.as_deref().filter(|name| !name.is_empty());
            let new_name = new.name.as_deref().filter(|name| !name.is_empty());
            if old_name != new_name {
                patch.insert(
                    format!("{path}/name"),
                    new_name.map_or(Value::Null, |name| Value::String(name.to_owned())),
                );
            }
            let units = merge_units(old.units.as_deref(), new.units.as_deref());
            if old.units.as_deref().filter(|u| !u.is_empty()) != units.as_deref() {
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
            // A TITLE that became a ROLE, or the reverse. Compare with the
            // default kind, so that a card with no kind does not get one.
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
            if old.components.as_deref().filter(|c| !c.is_empty()) != components.as_deref() {
                patch.insert(
                    format!("{path}/components"),
                    components.map_or(Value::Null, |components| json_of(&components)),
                );
            }
            // Compare the `LABEL` line as the emitter writes it. An empty
            // written-out address has no line, so no label is not a clear.
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
                slotted_context(&old.contexts),
            );
            diff_pref(
                patch,
                path,
                entry_pref(old.pref, &old.extra),
                entry_pref(new.pref, &new.extra),
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
            // Only the text crosses the vCard, so only the text can change.
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
            if old.uri != new.uri {
                patch.insert(format!("{path}/uri"), Value::String(new.uri.clone()));
            }
            if old.kind != new.kind {
                patch.insert(format!("{path}/kind"), value_or_null(new.kind.as_ref()));
            }
        },
    );
}

/// Patches the calendaring resources, which cross on two lines (`CALURI` and
/// `FBURL`).
///
/// The save patches only the URI, as for a link: the other members have no
/// parameter on these lines. The line name states the `kind`, but the user
/// cannot edit it. A move to the other field arrives as a deletion and an
/// addition.
///
/// The key survives Evolution. In libebook-contacts 3.52, a set on
/// `E_CONTACT_CALENDAR_URI` or `E_CONTACT_FREEBUSY_URL` rewrites the value of
/// the first line of that name in place. It keeps the parameters, also
/// `X-JMAP-KEY`, as a set on the home page does. The user can edit only that
/// first line. Other lines of the same name pass through and keep their keys.
fn diff_calendars(
    patch: &mut Map<String, Value>,
    current: Option<&BTreeMap<String, Calendar>>,
    edited: Option<&BTreeMap<String, Calendar>>,
) {
    diff_entries(
        patch,
        "calendars",
        current,
        edited,
        states_calendar,
        |patch, path, old, new| {
            if old.uri != new.uri {
                patch.insert(format!("{path}/uri"), Value::String(new.uri.clone()));
            }
            if old.kind != new.kind {
                patch.insert(format!("{path}/kind"), value_or_null(new.kind.as_ref()));
            }
        },
    );
}

/// Patches the contact's photo, the only media kind that a vCard 3.0 `PHOTO`
/// line states and an Evolution user can change.
///
/// The key does not survive: EDS rebuilds the line from its photo and drops
/// the parameters, as for a date. Thus [`rekey_keyless`] pairs a keyless
/// picture with the picture it replaced. In libebook-contacts 3.52,
/// `E_CONTACT_PHOTO` reports the first `PHOTO` line. A `set` rewrites that line
/// in place and keeps the other lines and their keys. A clear removes only that
/// line. Thus the other pictures come back with their keys.
///
/// [`same_photo`] compares the lines, not the members. A read-back entry can
/// differ from the original: a `data:` URI can lose its base64 padding, and a
/// stated media type becomes `mediaType`. A member comparison would rewrite an
/// untouched picture on each save. When the line changed, the save patches
/// each of `uri` and `mediaType` that differs.
fn diff_media(
    patch: &mut Map<String, Value>,
    current: Option<&BTreeMap<String, Media>>,
    edited: Option<&BTreeMap<String, Media>>,
) {
    let edited = rekey_keyless(current, edited, states_media, |_, _| true);
    diff_entries(
        patch,
        "media",
        current,
        edited.as_ref(),
        states_media,
        |patch, path, old, new| {
            // A `PHOTO` line always states a photo, so `kind` cannot change.
            if same_photo(old, new) {
                return;
            }
            if old.uri != new.uri {
                patch.insert(format!("{path}/uri"), Value::String(new.uri.clone()));
            }
            if old.media_type != new.media_type {
                patch.insert(
                    format!("{path}/mediaType"),
                    value_or_null(new.media_type.as_ref()),
                );
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
            // Compare the stated handle, not `user`. A handle read from a `uri`
            // comes back as `user`, and a member comparison sees an edit.
            let handle = online_service_handle(new);
            if online_service_handle(old) != handle {
                // Write the rename on its source member. An entry with only a
                // URI gets a new URI with the same scheme.
                let rebuilt = match (&old.user, handle) {
                    (None, Some(handle)) => new
                        .service
                        .as_deref()
                        .or(old.service.as_deref())
                        .and_then(|service| online_service_uri(service, handle)),
                    _ => None,
                };
                match rebuilt {
                    Some(uri) => {
                        patch.insert(format!("{path}/uri"), Value::String(uri));
                    }
                    // Otherwise write `user` and drop the old URI. The URI cannot
                    // be rebuilt, and it names the old handle. `merge_units`
                    // drops a renamed unit's `sortAs` for the same reason.
                    None => {
                        patch.insert(format!("{path}/user"), value_or_null(new.user.as_ref()));
                        if old.uri.is_some() {
                            patch.insert(format!("{path}/uri"), Value::Null);
                        }
                    }
                }
            }
            // The property name states the service, so a change means the key
            // arrived on a different service's line. Compare per RFC 9553 §2.3.2,
            // so the server's spelling stays.
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
    // Pair a keyless date by its kind. Evolution has one field per kind.
    let edited = rekey_keyless(current, edited, states_anniversary, |old, new| {
        old.kind == new.kind
    });
    diff_entries(
        patch,
        "anniversaries",
        current,
        edited.as_ref(),
        states_anniversary,
        |patch, path, old, new| {
            // The line states the kind: a birthday became a wedding day, or the
            // reverse.
            if old.kind != new.kind {
                patch.insert(format!("{path}/kind"), Value::String(new.kind.clone()));
            }
            if anniversary_date(old) == anniversary_date(new) {
                return;
            }
            let Some(date) = new.date.as_ref() else {
                return;
            };
            // Patch a day member by member, so other members (for example
            // `calendarScale`) stay. A day replaces a point in time whole,
            // because they are different object types.
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

/// Returns the edited entries of a keyed map under the server's keys, for maps
/// whose key does not survive Evolution.
///
/// Most maps cross with their key in `X-JMAP-KEY`. The dates and the pictures
/// do not: EDS rebuilds those lines from structured fields and drops the
/// parameters (libebook-contacts 3.52 keeps them only on untouched lines).
/// Thus a keyless entry pairs with a server entry that `pairs_with` accepts:
/// the same date kind, or for a picture the first one. Evolution edits only
/// the first `PHOTO` line.
///
/// Candidates pair in order. An entry with a surviving key keeps it and is not
/// a candidate. Otherwise every save swaps two birthdays, of which Evolution
/// shows only the first.
///
/// Only visible entries are candidates, and an invisible entry's key does not
/// count as surviving. The reader makes keys by counting visible entries, so a
/// picture can get the key of a hidden logo. [`diff_entries`] then keeps the
/// invisible entry's key free.
fn rekey_keyless<T: Clone>(
    current: Option<&BTreeMap<String, T>>,
    edited: Option<&BTreeMap<String, T>>,
    is_visible: impl Fn(&T) -> bool,
    pairs_with: impl Fn(&T, &T) -> bool,
) -> Option<BTreeMap<String, T>> {
    let edited = edited?;
    let empty = BTreeMap::new();
    let current = current.unwrap_or(&empty);
    let survived = |key: &String| current.get(key).is_some_and(&is_visible);
    let mut unclaimed: Vec<(&String, &T)> = current
        .iter()
        .filter(|(key, entry)| is_visible(entry) && !edited.contains_key(*key))
        .collect();

    let mut rekeyed = BTreeMap::new();
    for (key, entry) in edited {
        let key = match survived(key) {
            true => key.clone(),
            false => match unclaimed.iter().position(|(_, old)| pairs_with(old, entry)) {
                Some(index) => unclaimed.remove(index).0.clone(),
                None => key.clone(),
            },
        };
        rekeyed.insert(key, entry.clone());
    }
    Some(rekeyed)
}

/// Returns the address components to write: the `ADR` values, plus the
/// components that `ADR` has no field for.
///
/// A list has no keys, so [`merge_named`] merges by kind and value. A kept
/// component keeps its unmapped members, mainly its `phonetic` spelling.
/// [`restore_address_components`] first splits the values that one `ADR` field
/// joined (a street name and its house number).
///
/// An empty edited list is also merged and is not a clear. An `ADR` with all
/// fields cleared, and an address shown only on `LABEL`, both arrive empty.
fn merge_components(
    current: Option<&[AddressComponent]>,
    edited: Option<&[AddressComponent]>,
) -> Option<Vec<AddressComponent>> {
    let current = current.unwrap_or_default();
    let edited = restore_address_components(current, edited.unwrap_or_default());
    let merged = merge_named(
        current,
        &edited,
        |component| (&component.kind, &component.value),
        states_address_component,
    );
    (!merged.is_empty()).then_some(merged)
}

/// Merges a keyless list of named parts (address or name components), which
/// the save writes whole.
///
/// `named` gives the pair that identifies a part. A part with an unchanged
/// pair stays as the server's copy, with its unmapped members. The merge walks
/// the server's list, so an invisible part keeps its position. Thus opening
/// and closing a contact writes nothing. New parts follow in vCard order.
///
/// `was_stated` has the same function as in [`diff_flags`]: a missing part is
/// a deletion only if the line stated it. It checks the whole part, not the
/// kind, because the line omits an empty part even when it has a field.
fn merge_named<T: Clone>(
    current: &[T],
    edited: &[T],
    named: impl Fn(&T) -> (&str, &str),
    was_stated: impl Fn(&T) -> bool,
) -> Vec<T> {
    let mut spare: Vec<&T> = edited.iter().collect();
    let mut merged: Vec<T> = Vec::new();
    for part in current {
        let same = |candidate: &&T| named(candidate) == named(part);
        match spare.iter().position(same) {
            // The part is still on the line: keep the server's copy.
            Some(index) => {
                spare.remove(index);
                merged.push(part.clone());
            }
            // The part is not on the line: keep it if the line never stated it.
            None if !was_stated(part) => merged.push(part.clone()),
            None => {}
        }
    }
    merged.extend(spare.into_iter().cloned());
    merged
}

/// Returns the units to write: the names that `ORG` states, each with the
/// other members of the server's unit of that name.
///
/// A list has no keys, so the merge matches by name. A unit's `sortAs` follows
/// its name. A renamed unit loses its `sortAs`, because the hint is for the
/// old name.
///
/// A missing unit is a deletion only if the line stated it (the `was_stated`
/// rule of [`merge_named`]). A unit with no name has no `ORG` component, so a
/// name match cannot find it. Each such unit goes back after the surviving
/// visible unit that it followed, or at the front. Thus opening and closing a
/// contact writes nothing. The original index is not used, because it moves
/// when an earlier unit is deleted.
///
/// A renamed unit pairs in order with a visible unit that has no other match.
/// It takes that unit's position for the unnamed units, but not its `sortAs`.
///
/// Thus an empty edited list is also merged and is not a deletion of all
/// units. An organisation with only unnamed units states none of them.
fn merge_units(current: Option<&[OrgUnit]>, edited: Option<&[OrgUnit]>) -> Option<Vec<OrgUnit>> {
    let current = current.unwrap_or_default();
    let edited = edited.unwrap_or_default();

    let mut spare: Vec<usize> = current
        .iter()
        .enumerate()
        .filter(|(_, unit)| states_org_unit(unit))
        .map(|(index, _)| index)
        .collect();
    let mut origin_of: Vec<Option<usize>> = Vec::with_capacity(edited.len());
    for unit in edited {
        let origin = spare
            .iter()
            .position(|&index| current[index].name == unit.name)
            .map(|position| spare.remove(position));
        origin_of.push(origin);
    }
    let unresolved_edited: Vec<usize> = origin_of
        .iter()
        .enumerate()
        .filter(|(_, origin)| origin.is_none())
        .map(|(position, _)| position)
        .collect();
    for (edited_position, original_index) in unresolved_edited.into_iter().zip(spare) {
        origin_of[edited_position] = Some(original_index);
    }

    let mut merged: Vec<OrgUnit> = Vec::with_capacity(edited.len());
    let mut merged_index_of: BTreeMap<usize, usize> = BTreeMap::new();
    for (unit, origin) in edited.iter().zip(origin_of) {
        let resolved = match origin {
            Some(original_index) => {
                merged_index_of.insert(original_index, merged.len());
                if current[original_index].name == unit.name {
                    current[original_index].clone()
                } else {
                    unit.clone()
                }
            }
            None => unit.clone(),
        };
        merged.push(resolved);
    }

    let mut anchor: Option<usize> = None;
    let mut shift = 0;
    let mut offset = 0;
    for (index, unit) in current.iter().enumerate() {
        if states_org_unit(unit) {
            if let Some(&position) = merged_index_of.get(&index) {
                anchor = Some(position + shift);
                offset = 0;
            }
        } else {
            let insert_at = anchor
                .map_or(offset, |position| position + 1 + offset)
                .min(merged.len());
            merged.insert(insert_at, unit.clone());
            shift += 1;
            offset += 1;
        }
    }
    (!merged.is_empty()).then_some(merged)
}

/// Patches a keyed map over the entries that the vCard stated. The save writes
/// added entries whole and sets removed entries to null. `diff_entry` compares
/// kept entries member by member.
///
/// The user did not see an invisible entry, so its absence is not an edit:
///
/// - The save never sets it to null.
/// - The save writes a first visible entry by path into the property, because
///   the property exists on the server.
/// - The save moves an addition to a free key if an invisible entry holds its
///   key. The reader makes keys by counting visible entries, so a new title
///   can get the key of a hidden title.
/// - The save writes nothing for an edited entry that is equal to the
///   invisible entry under its key. Thus `diff(card, card)` stays empty.
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
            None if current
                .get(key)
                .is_some_and(|hidden| json_of(hidden) == json_of(entry)) => {}
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

/// Returns the key for an added entry: its own key, or the first free variant.
fn free_key(wanted: &str, taken: &BTreeSet<String>) -> String {
    if !taken.contains(wanted) {
        return wanted.to_owned();
    }
    (2..)
        .map(|index| format!("{wanted}-{index}"))
        .find(|candidate| !taken.contains(candidate))
        .expect("an unbounded sequence has a free element")
}

/// Returns whether the line stated this `contexts` key. [`diff_flags`] uses it
/// before it reads an absent context as a removal.
///
/// vCard 3.0 has no `TYPE` for a `school` context (see [`maps_context`]). Of
/// the two contexts it has, an `ADR` or a `TEL` states only one. EDS puts a
/// line with both into two editor fields, and an edit of one rewrites the
/// line. The omitted context depends on the entry. See
/// [`jmap_vcard::states_context`].
///
/// `EMAIL` uses only [`maps_context`]. EDS files an email line by position
/// (`E_CONTACT_EMAIL_1` to `_4`), not by `TYPE`, so both contexts cross.
fn slotted_context(contexts: &Option<Value>) -> impl Fn(&str) -> bool + '_ {
    move |key| maps_context(key) && states_context(contexts.as_ref(), key)
}

/// Returns whether the `TEL` line stated this `features` key, as
/// [`slotted_context`] does for contexts.
///
/// EDS also picks the phone field by `TYPE`. A voice and fax number fills two
/// separate fields, and with no context it fills no field. See
/// [`jmap_vcard::states_phone_feature`].
fn slotted_feature(features: &Option<Value>) -> impl Fn(&str) -> bool + '_ {
    move |key| maps_phone_feature(key) && states_phone_feature(features.as_ref(), key)
}

/// Patches a boolean map: replaces the members that the mapping can spell and
/// keeps the others.
///
/// The two predicates are different. `was_stated` tells if the line stated the
/// member, so its absence is a removal. `is_mapped` tells if the mapping can
/// read the member, so its presence is an addition. They differ where a line
/// states one of several members ([`slotted_context`], [`slotted_feature`]).
/// If a user moves a number from Home to Business, the old line did not state
/// the new context. A `was_stated` gate on additions then leaves no context.
fn diff_flags(
    patch: &mut Map<String, Value>,
    path: &str,
    property: &str,
    current: &Option<Value>,
    edited: &Option<Value>,
    is_mapped: impl Fn(&str) -> bool,
    was_stated: impl Fn(&str) -> bool,
) {
    let mut merged: Map<String, Value> = match current {
        Some(Value::Object(flags)) => flags.clone(),
        _ => Map::new(),
    };
    merged.retain(|key, _| !was_stated(key));
    if let Some(Value::Object(flags)) = edited {
        merged.extend(
            flags
                .iter()
                .filter(|(key, _)| is_mapped(key))
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }

    let merged = (!merged.is_empty()).then_some(Value::Object(merged));
    let current_normalized = match current {
        Some(Value::Object(flags)) if !flags.is_empty() => current.as_ref(),
        _ => None,
    };
    if current_normalized != merged.as_ref() {
        patch.insert(format!("{path}/{property}"), merged.unwrap_or(Value::Null));
    }
}

/// Patches `pref`. vCard 3.0 has only a flag, so an existing server rank stays.
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

/// Escapes a map key for a JSON Pointer (RFC 6901 §3).
fn escape(key: &str) -> String {
    jmap_proto::text::escape_pointer_segment(key)
}

fn value_or_null(value: Option<&String>) -> Value {
    value.map_or(Value::Null, |value| Value::String(value.clone()))
}

/// Serialisation cannot fail: a type built from a vCard holds only strings,
/// numbers and maps of them.
fn json_of<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_key_keeps_the_wanted_key_when_nothing_holds_it() {
        let taken: BTreeSet<String> = ["t1".to_owned()].into_iter().collect();
        assert_eq!(free_key("t2", &taken), "t2");
    }

    #[test]
    fn free_key_counts_up_past_every_variant_already_taken() {
        let taken: BTreeSet<String> = ["t2".to_owned(), "t2-2".to_owned()].into_iter().collect();
        assert_eq!(free_key("t2", &taken), "t2-3");
    }

    /// A new relation with two mapped kinds (one name in both the Manager and
    /// Assistant fields) is one whole-object create. RFC 8620 §5.3 forbids a
    /// patch into a sub-path that the same patch creates.
    #[test]
    fn a_new_relation_stating_two_mapped_kinds_at_once_is_written_once() {
        let current: BTreeMap<String, Relation> = [("Alex Spouse".to_owned(), {
            let mut v = Relation::default();
            v.relation = Some([("spouse".to_owned(), json!(true))].into());
            v
        })]
        .into();
        let mut edited = current.clone();
        edited.insert("Jordan Multi".to_owned(), {
            let mut v = Relation::default();
            v.relation = Some(
                [
                    ("manager".to_owned(), json!(true)),
                    ("assistant".to_owned(), json!(true)),
                ]
                .into(),
            );
            v
        });

        let mut patch = Map::new();
        diff_related_to(&mut patch, Some(&current), Some(&edited));

        let new_entity_keys: Vec<&String> = patch
            .keys()
            .filter(|key| key.contains("Jordan Multi"))
            .collect();
        assert_eq!(
            new_entity_keys,
            vec!["relatedTo/Jordan Multi"],
            "a create and a reach into its own not-yet-existing sub-path in \
             the same patch: {patch:?}"
        );
        assert_eq!(
            patch["relatedTo/Jordan Multi"]["relation"],
            json!({"manager": true, "assistant": true}),
            "{patch:?}"
        );
    }

    #[derive(Clone, serde::Serialize)]
    struct Entry {
        visible: bool,
        text: &'static str,
    }

    fn is_visible(entry: &Entry) -> bool {
        entry.visible
    }

    fn overwrite(patch: &mut Map<String, Value>, path: &str, _current: &Entry, edited: &Entry) {
        patch.insert(path.to_owned(), json_of(edited));
    }

    /// The reader counts only visible entries, so a new title can get the key
    /// of a hidden title (see [`diff_entries`]). The addition moves to a free
    /// key and does not overwrite the hidden entry.
    #[test]
    fn an_addition_that_lands_on_a_hidden_entrys_key_is_moved_off_it() {
        let current: BTreeMap<String, Entry> = [
            (
                "t1".to_owned(),
                Entry {
                    visible: true,
                    text: "CEO",
                },
            ),
            (
                "t2".to_owned(),
                Entry {
                    visible: false,
                    text: "Secret",
                },
            ),
        ]
        .into_iter()
        .collect();
        let edited: BTreeMap<String, Entry> = [
            (
                "t1".to_owned(),
                Entry {
                    visible: true,
                    text: "CEO",
                },
            ),
            (
                "t2".to_owned(),
                Entry {
                    visible: true,
                    text: "New title",
                },
            ),
        ]
        .into_iter()
        .collect();

        let mut patch = Map::new();
        diff_entries(
            &mut patch,
            "titles",
            Some(&current),
            Some(&edited),
            is_visible,
            overwrite,
        );

        assert_eq!(
            patch.get("titles/t2-2"),
            Some(&json!({"visible": true, "text": "New title"}))
        );
        assert_eq!(
            patch.get("titles/t2"),
            None,
            "the hidden entry must not be overwritten"
        );
        assert!(patch.contains_key("titles/t1"));
    }

    /// A card that keeps a hidden photo under its own key has no diff to
    /// itself. The save neither copies the photo to a free key nor touches it.
    #[test]
    fn a_hidden_photo_unchanged_under_its_own_key_patches_nothing() {
        let card: ContactCard = serde_json::from_value(json!({
            "@type": "Card",
            "version": "1.0",
            "uid": "u1",
            "media": {"p1": {"@type": "Media", "kind": "photo", "uri": "aaa://0.jpg"}},
        }))
        .expect("a ContactCard");
        assert!(!states_media(&card.media.as_ref().expect("media")["p1"]));

        assert_eq!(diff(&card, &card), Map::new());
    }

    fn relations(entries: &[(&str, &[&str])]) -> BTreeMap<String, Relation> {
        entries
            .iter()
            .map(|(key, kinds)| {
                ((*key).to_owned(), {
                    let mut v = Relation::default();
                    v.relation = Some(
                        kinds
                            .iter()
                            .map(|k| ((*k).to_owned(), json!(true)))
                            .collect(),
                    );
                    v
                })
            })
            .collect()
    }

    /// A change to all mapped relations at the same time is still a change.
    #[test]
    fn respelling_every_mapped_relation_at_once_patches_all_three() {
        let current = relations(&[
            ("Alex", &["spouse"]),
            ("Blair", &["manager"]),
            ("Casey", &["assistant"]),
        ]);
        let edited = relations(&[
            ("Alexa", &["spouse"]),
            ("Blake", &["manager"]),
            ("Cass", &["assistant"]),
        ]);

        let mut patch = Map::new();
        diff_related_to(&mut patch, Some(&current), Some(&edited));

        let keys: BTreeSet<&str> = patch.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "relatedTo/Alex",
                "relatedTo/Alexa",
                "relatedTo/Blair",
                "relatedTo/Blake",
                "relatedTo/Casey",
                "relatedTo/Cass",
            ]
            .into_iter()
            .collect(),
            "{patch:?}"
        );
    }

    /// A name moved from Spouse to Manager keeps the entry. Only the relation
    /// type changes.
    #[test]
    fn a_spouse_who_becomes_the_manager_keeps_the_entry() {
        let current = relations(&[("Kim", &["spouse"])]);
        let edited = relations(&[("Kim", &["manager"])]);

        let mut patch = Map::new();
        diff_related_to(&mut patch, Some(&current), Some(&edited));

        assert_eq!(
            Value::Object(patch),
            json!({
                "relatedTo/Kim/relation/spouse": null,
                "relatedTo/Kim/relation/manager": true,
            })
        );
    }

    fn name(value: Value) -> Name {
        serde_json::from_value(value).expect("a Name")
    }

    fn name_patch(current: Option<&Name>, edited: Option<&Name>) -> Value {
        let mut patch = Map::new();
        diff_name(&mut patch, current, edited);
        Value::Object(patch)
    }

    /// A cleared name that holds only `FN` and `N` data is removed whole.
    #[test]
    fn clearing_a_plain_name_removes_it_whole() {
        let plain = [
            json!({"full": "Hans Oldenburg"}),
            json!({"components": [{"kind": "surname", "value": "Oldenburg"}]}),
            json!({"full": "Hans", "fileAs": "Oldenburg, Hans"}),
            json!({"full": "Hans", "file_as": "Oldenburg, Hans"}),
        ];
        for current in plain {
            assert_eq!(
                name_patch(Some(&name(current.clone())), None),
                json!({"name": null}),
                "{current}"
            );
        }
    }

    /// A cleared name keeps its unmapped members. Only the shown members go.
    #[test]
    fn clearing_a_name_keeps_the_members_vcard_never_showed() {
        let carrying = [
            json!({"full": "Hans", "isOrdered": true}),
            json!({"full": "Hans", "sortAs": {"surname": "Oldenburg"}}),
            json!({"full": "Hans", "phonetic": "hans"}),
        ];
        for current in carrying {
            assert_eq!(
                name_patch(Some(&name(current.clone())), None),
                json!({"name/full": null}),
                "{current}"
            );
        }
    }

    /// A new name on a card with no name is written whole.
    #[test]
    fn typing_a_name_into_a_card_without_one_writes_it_whole() {
        let edited = name(json!({"full": "Hans Oldenburg"}));
        assert_eq!(
            name_patch(None, Some(&edited)),
            json!({"name": {"full": "Hans Oldenburg"}})
        );
    }

    /// A cleared name keeps the components that `N` has no field for.
    #[test]
    fn clearing_a_name_keeps_the_components_n_cannot_show() {
        let current = name(json!({
            "full": "Hans Oldenburg",
            "components": [
                {"kind": "surname", "value": "Oldenburg"},
                {"kind": "generation", "value": "Jr."},
            ],
        }));
        assert_eq!(
            name_patch(Some(&current), None),
            json!({
                "name/full": null,
                "name/components": [{"kind": "generation", "value": "Jr."}],
            })
        );
    }

    /// A cleared name keeps a component list that `N` cannot show.
    #[test]
    fn clearing_a_name_leaves_a_wholly_unshown_component_list_alone() {
        let current = name(json!({
            "full": "Hans Oldenburg",
            "components": [{"kind": "generation", "value": "Jr."}],
        }));
        assert_eq!(name_patch(Some(&current), None), json!({"name/full": null}));
    }

    /// An edited entry does not pair with a hidden entry of the same kind, and
    /// does not take its key.
    #[test]
    fn rekey_keyless_never_pairs_an_edit_with_a_hidden_entry() {
        let current: BTreeMap<String, (bool, &str)> = [("a1".to_owned(), (false, "birth"))].into();
        let edited: BTreeMap<String, (bool, &str)> = [("x1".to_owned(), (true, "birth"))].into();

        let rekeyed = rekey_keyless(
            Some(&current),
            Some(&edited),
            |entry| entry.0,
            |old, new| old.1 == new.1,
        );

        assert_eq!(rekeyed, Some(edited));
    }

    /// Each unit with no name goes back after the named unit it followed, in
    /// the server's order.
    #[test]
    fn merge_units_puts_every_nameless_unit_back_where_it_was() {
        let unit = |unit: Value| -> OrgUnit { serde_json::from_value(unit).expect("an OrgUnit") };
        let current = [
            unit(json!({"name": "Sales"})),
            unit(json!({"sortAs": "h1"})),
            unit(json!({"name": "EMEA"})),
            unit(json!({"sortAs": "h2"})),
            unit(json!({"sortAs": "h3"})),
        ];
        let edited = [
            unit(json!({"name": "Sales"})),
            unit(json!({"name": "EMEA"})),
        ];

        let merged = merge_units(Some(&current), Some(&edited));

        assert_eq!(merged.as_deref(), Some(&current[..]));
    }
}
