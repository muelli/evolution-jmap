// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning an edited vCard back into a `ContactCard/set` PatchObject.
//!
//! The whole point of patching rather than replacing is that a vCard is a
//! lossy view of a JSContact card. The mapping keeps UID, FN, N, NICKNAME,
//! EMAIL, TEL, ADR, LABEL, ORG, TITLE, ROLE, NOTE, URL, CALURI, FBURL, PHOTO,
//! CATEGORIES, the
//! instant-messaging `X-` lines, the spouse line and the two date lines, and
//! drops everything
//! else, so a save that sent the parsed card back whole would silently delete
//! the properties it could not represent — preferred languages, what the
//! contact is spoken to as, the crypto keys a card lists — none of which the
//! user ever saw, let alone asked to remove.
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
//!   back in rather than being replaced away. On an `ADR` and a `TEL` one of
//!   the two contexts vCard *can* spell is left off the line as well, so which
//!   members are merged back depends on the entry — see `slotted_context`.
//! - `pref` is a rank from 1 to 100 and vCard 3.0 has only a flag. An
//!   address that was already preferred keeps its rank; the flag can only
//!   introduce or remove a preference, never renumber one.
//! - `name.components` are a *list* with no keys to patch by, so they go back
//!   whole and are merged the way an address's components are (`merge_named`):
//!   kinds the `N` value has no field for are carried across the replacement,
//!   and a component that still says what it said keeps the members that value
//!   had no field for either — its `phonetic` spelling above all. Their order
//!   is the server's, so that opening a contact and closing it again writes
//!   nothing even when the `N` fields state them in another order. And as with
//!   an address, matching by value first needs the components that shared one
//!   field told apart again: both halves of a double-barrelled given name are
//!   written into `N`'s second field and come back as one component holding
//!   them both.
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
//! - `calendars` entries are patched by their URI alone, as a link's are, and
//!   their key survives Evolution the same way. Their `kind` is stated — it is
//!   which of the two lines the URI is on — but cannot have been *edited*:
//!   moving an address from the Calendar field to the Free/Busy one is a
//!   deletion and an addition, and arrives here as exactly that.
//! - `media` entries have no surviving key either, and are patched by what the
//!   `PHOTO` line states rather than by their members — a picture read back off
//!   a line is not the entry that produced it. Only the first of them is ever
//!   the one the user edited. See `diff_media`.
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
//!   `contexts`. The handle is the entry's `user`, or the one its `uri` states
//!   for a service whose URI scheme says nothing else — and a rename goes back
//!   on whichever of the two it was drawn from. Where it cannot, the `uri` that
//!   named the replaced handle is *dropped*: see `diff_online_services`, the
//!   one place here where a save removes a member the vCard never showed.
//! - `relatedTo` is the one mapped property keyed by *who the entry is about*
//!   rather than by an id of whoever wrote it, so it is the one whose key the
//!   line shows the user: there is nothing on the entry to patch, and an edit is
//!   a marriage withdrawn from one entity and claimed of another. Both halves
//!   reach into the relation *set* rather than replacing the entry, so that a
//!   relation the line never stated survives either way. See
//!   `diff_related_to`.
//! - `keywords` is the one mapped property that is a *set*, and the one that
//!   goes back **replaced whole**: a tag is a bare string with no key and no
//!   members, so there is nothing to reach into. A tag the `CATEGORIES` line
//!   could not carry is therefore one a save would *delete* rather than merely
//!   one the user cannot see — so the save carries it onto the set it writes,
//!   which is how a set keeps the rule the keyed maps keep by leaving an entry
//!   unnamed. See `diff_keywords`.
//! - *Every* keyed map is one of which the vCard states only **some**
//!   entries. A title of a `kind` outside `title` and `role` has no vCard
//!   property; an address with neither an `ADR` field nor a written-out form
//!   to put on a `LABEL`, an organisation with neither a name nor a unit, an
//!   email with no address, a phone with no number, a note that says nothing,
//!   a date naming no single day, a link of a kind vCard 3.0 cannot state, a
//!   calendaring resource naming neither of the two kinds that have a line, and
//!   a handle at a service EDS has no field for all have no line to be written
//!   on. Each is dropped on the way
//!   out and must
//!   therefore be invisible to the save in both directions — neither deleted
//!   for being absent from the edited card, nor overwritten by an addition
//!   whose key the reader invented by counting only the entries it could
//!   see. That is what `diff_entries` and the `states_*` predicates it
//!   takes are for; the predicates live next to the emitter, so what the
//!   save calls invisible is what the emitter actually left out.
//!
//! RFC 8620 §5.3 requires every path segment before the last to exist on the
//! object already, which is why a property that is absent server-side is
//! written whole instead of being reached into.

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::contacts::{
    Address, AddressComponent, Anniversary, Calendar, ContactCard, ContactEmail, ContactPhone,
    Link, Media, Name, Nickname, Note, OnlineService, OrgUnit, Organization, Relation, Title,
};
use jmap_vcard::{
    address_label, anniversary_date, maps_address_component, maps_context, maps_name_component,
    maps_phone_feature, online_service_handle, online_service_uri, restore_address_components,
    restore_name_components, same_photo, same_service, states_a_point_in_time, states_address,
    states_anniversary, states_calendar, states_context, states_email, states_keyword, states_link,
    states_media, states_nickname, states_note, states_nothing_but_the_marriage,
    states_online_service, states_organization, states_phone, states_phone_feature, states_spouse,
    states_title, title_kind,
};
use serde_json::{Map, Value, json};

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

/// Who else the contact is related to — the one mapped property whose *key* is
/// what the line shows, and so the one where an edit is not a change to an entry
/// but a withdrawal and a claim.
///
/// RFC 9553 §2.1.8 keys `relatedTo` by the related entity itself, and RFC 9555
/// §2.9.5 is what allows that key to be a person's name rather than a `uid`. So
/// there is nothing on the entry to patch: a name the user respells names another
/// entity, and the line said exactly one thing about the old one — that it is a
/// spouse. That one thing is all the save may withdraw:
///
/// - a name gone from the field loses the marriage, and the entry with it when
///   the marriage was all it said ([`states_nothing_but_the_marriage`]). Where the
///   server also called that entity `kin`, the entry stays and only the marriage
///   is struck off — the `kin` was never on the line and is not the user's to have
///   deleted.
/// - a name arrived in the field *gains* the marriage. If the card already relates
///   to somebody of that name, that is the same entity — the key says so — so the
///   type is added to the set rather than replacing it, and a relation the user
///   cannot see survives being married.
///
/// Which also means this property needs no [`diff_entries`]: there are no keys
/// this side invented for the reader to collide with. A name is a name on both
/// sides, and an entry the vCard never showed is either keyed by something no
/// field can produce — a URI — or is the very entity the user just named.
fn diff_related_to(
    patch: &mut Map<String, Value>,
    current: Option<&BTreeMap<String, Relation>>,
    edited: Option<&BTreeMap<String, Relation>>,
) {
    let empty = BTreeMap::new();
    let current = current.unwrap_or(&empty);
    let shown = spouses(current);
    let wanted = spouses(edited.unwrap_or(&empty));
    if shown.keys().eq(wanted.keys()) {
        return;
    }

    // RFC 8620 §5.3 requires every path segment before the last to exist on the
    // object already, and a card relating to nobody has no `relatedTo` to reach
    // into, so the property is written whole. Nothing is lost by that: there are
    // no entries to keep.
    if current.is_empty() {
        patch.insert("relatedTo".to_owned(), json_of(&wanted));
        return;
    }

    let withdrawn: Vec<(&&String, &&Relation)> = shown
        .iter()
        .filter(|(key, _)| !wanted.contains_key(**key))
        .collect();
    let dropped = |relation: &Relation| states_nothing_but_the_marriage(relation);
    // Every entry the card holds was a marriage, and the field now names none:
    // the property goes rather than being left as the empty map, which is a
    // different thing to store than §2.1.8's default of no relations.
    if wanted.is_empty()
        && withdrawn.len() == current.len()
        && withdrawn.iter().all(|(_, relation)| dropped(relation))
    {
        patch.insert("relatedTo".to_owned(), Value::Null);
        return;
    }

    for (key, relation) in withdrawn {
        let path = format!("relatedTo/{}", escape(key));
        match dropped(relation) {
            true => drop(patch.insert(path, Value::Null)),
            false => drop(patch.insert(format!("{path}/relation/spouse"), Value::Null)),
        }
    }
    for (key, relation) in wanted.iter().filter(|(key, _)| !shown.contains_key(**key)) {
        let path = format!("relatedTo/{}", escape(key));
        match current.get(*key) {
            // The same entity, said one more thing about.
            Some(existing) => {
                // §5.3 again: the set has to be there to be added to, and an
                // entry stating no type at all — RFC 9555 §2.9.5 reads a
                // `RELATED` line carrying no `TYPE` into exactly that — has no
                // `relation` member for a path to end in.
                match existing.relation.is_some() {
                    true => patch.insert(format!("{path}/relation/spouse"), Value::Bool(true)),
                    false => patch.insert(format!("{path}/relation"), json!({"spouse": true})),
                };
            }
            None => drop(patch.insert(path, json_of(relation))),
        }
    }
}

/// The entries of a `relatedTo` map that reach the spouse line, in the order the
/// map holds them.
fn spouses(entries: &BTreeMap<String, Relation>) -> BTreeMap<&String, &Relation> {
    entries
        .iter()
        .filter(|(key, relation)| states_spouse(key, relation))
        .collect()
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
/// [`states_keyword`] is asked of the server's own set, for the reason the other
/// properties ask their `states_*` predicate: a tag the `CATEGORIES` line could
/// not carry was never shown, so its absence from the edited card is not the
/// user asking for it to go. Where a keyed map answers that by leaving the entry
/// unnamed in the patch, a set has no key to leave alone — so the tag is carried
/// onto the set the save writes instead. That is the same rule reached by the
/// only means a set allows, and it is why an unstatable tag costs the sight of
/// it and nothing more; the edit around it still lands.
///
/// A tag is carried back exactly as the server stated it, value included — even
/// the value RFC 9553 §1.4.3 does not admit, because the server is the one who
/// said it and rewriting it here would be this mapping inventing a change. The
/// user's own set wins where the two name the same tag: a tag they typed is a
/// tag they mean to be set, whatever the server had against that name.
///
/// The *edited* side needs no such check: every tag on it was read off a content
/// line, and any string is a keyword RFC 9553 admits.
fn diff_keywords(patch: &mut Map<String, Value>, current: &ContactCard, edited: &ContactCard) {
    // An empty set server-side is compared as no tags, because that is what it
    // was drawn as: without this a card holding one would be patched to a null
    // by every save, an edit nobody made.
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
            // Serialising a set this crate's own reader built cannot fail: it
            // holds strings and values the server itself sent.
            serde_json::to_value(wanted).unwrap_or(Value::Null)
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

    // The component list is written back whole — a name has no keys to patch
    // by — so it is merged the way an address's components are: a component
    // that still says what it says keeps the members the `N` value had no
    // field for, its `phonetic` spelling above all, and one of a kind that
    // value cannot state at all is not the user's to have deleted. And as
    // there, the matching by value only works once the components that shared
    // one `N` field have been told apart again — a double-barrelled given name
    // comes back as one component holding both halves, and would otherwise read
    // as both halves deleted and replaced by their own concatenation.
    let current_components = current.components.as_deref().unwrap_or_default();
    let merged = merge_named(
        current_components,
        &restore_name_components(
            current_components,
            edited.components.as_deref().unwrap_or_default(),
        ),
        |component| (&component.kind, &component.value),
        maps_name_component,
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
            diff_pref(patch, path, old.pref, new.pref);
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
                slotted_context(&old.contexts),
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

/// The contact's calendaring resources, which cross on two lines of different
/// names rather than on one.
///
/// Patched by their URI alone, for the reason a link's is: what the resource
/// is, where it is used and how strongly it is preferred have no parameter on a
/// `CALURI` or an `FBURL`. The `kind` cannot have been edited either, though it
/// *is* stated — it is the line's own name, and moving a URI from one field to
/// the other in Evolution is deleting it from one and typing it into the other,
/// which arrives here as exactly that.
///
/// Their key survives Evolution: measured against libebook-contacts 3.52, a set
/// on `E_CONTACT_CALENDAR_URI` or `E_CONTACT_FREEBUSY_URL` rewrites the value of
/// the first line of that name in place and leaves its parameters — the
/// `X-JMAP-KEY` included — where they were, exactly as a set on the home page
/// does. Only that first line is the one the user can edit; any further line of
/// the same name passes through untouched and is matched by the key it kept.
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
        },
    );
}

/// The picture of the contact, which is the one media kind a vCard 3.0 `PHOTO`
/// line states and therefore the one an Evolution user can change.
///
/// Two things here are the `PHOTO` line's own. First, the entry a chosen picture
/// belongs to cannot be found by its key: EDS rebuilds the line out of the photo
/// it holds and drops the parameters, as it does for a date — so a keyless
/// picture is paired with the one it replaced ([`rekey_keyless`]), which is
/// enough because Evolution edits exactly one of them. Measured against
/// libebook-contacts 3.52: `E_CONTACT_PHOTO` reports the *first* `PHOTO` line,
/// a `set` rewrites that line in place and leaves the rest — keys and all —
/// where they were, and clearing the field removes that one line only. So a card
/// carrying several pictures has only its first edited, and the others come back
/// wearing their keys and are matched by them.
///
/// Second, what changed is asked of the *line* rather than of the members
/// ([`same_photo`]): the entry read back off a line is not the entry that
/// produced it — a `data:` URI may have been spelled without its base64 padding,
/// and a media type it stated arrives as the entry's own `mediaType` — so
/// comparing members would rewrite a picture nobody touched on every save.
/// Once the line really has changed, both members go back, because between them
/// they are what the new picture *is*.
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
            // The line the picture is stated on *is* its kind — a `PHOTO` is the
            // photo — so a sound or a logo cannot arrive here, and `kind` has
            // nothing to be patched to.
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
            // What the line stated, on either side — not the `user`, because an
            // entry the server stated as a URI is drawn from that URI and comes
            // back as a `user` saying the same thing. Comparing the members
            // would call that an edit and rewrite the entry on every save.
            let handle = online_service_handle(new);
            if online_service_handle(old) != handle {
                // The rename goes back on the member it was drawn from. An entry
                // that stated only a URI keeps that shape, rebuilt around the new
                // handle — the scheme that let the handle be read is the one that
                // writes it.
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
                    // Otherwise the handle goes on the `user`, and the URI that
                    // named the old one goes with it: nothing here can rebuild
                    // it — either the service has no scheme this mapping knows,
                    // or no URI states what the user typed — and a URI for the
                    // old handle is not one for the new. The same judgement
                    // `merge_units` makes about a renamed unit's `sortAs`.
                    None => {
                        patch.insert(format!("{path}/user"), value_or_null(new.user.as_ref()));
                        if old.uri.is_some() {
                            patch.insert(format!("{path}/uri"), Value::Null);
                        }
                    }
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
    // The entry a keyless date belongs to is found by what kind of date it is,
    // which is enough because Evolution has exactly one field per kind: the
    // birthday it hands back is the birthday the card already had.
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

/// The edited entries of a keyed map whose key does not survive Evolution, under
/// the keys the server holds them by.
///
/// Most mapped maps cross with their key in `X-JMAP-KEY` and come back wearing
/// it. Two do not — the dates and the pictures — because EDS keeps each in a
/// structured field and rebuilds the line out of what it holds, dropping the
/// parameters: verified against libebook-contacts 3.52, where an untouched line
/// keeps them and a rewritten one does not. So a keyless entry is paired with an
/// entry the server holds that `pairs_with` accepts — by what kind of date it is,
/// or, for a picture, with whichever comes first, since Evolution edits the first
/// `PHOTO` line and no other.
///
/// Candidates are paired in order, and an entry whose key *did* survive keeps it
/// and is not paired against — otherwise a card carrying two birthdays, of which
/// Evolution shows the first and passes the second through untouched, would have
/// them swapped by every save.
///
/// Only *visible* entries are candidates, and a key an invisible entry holds is
/// not treated as surviving: the reader invents keys by counting the entries it
/// can see, so the key it gives a picture can be the key of a logo the user was
/// never shown. Pairing past that collision keeps the edit on the entry it
/// belongs to; [`diff_entries`] is what then keeps the invisible entry's key from
/// being taken.
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

/// The component list a save writes: what the `ADR` line now states, with
/// everything it had no field for left where it was.
///
/// Like [`merge_units`] this is a list with no keys to patch by, so it is
/// [`merge_named`] that does the merging — a component is recognised by what it
/// says, same kind and same value, and one that still says it keeps the members
/// the line could not carry, its `phonetic` spelling above all.
///
/// What is this side's own is the step before: matching by value only works
/// once the components a single `ADR` field was
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
    let merged = merge_named(
        current,
        &edited,
        |component| (&component.kind, &component.value),
        maps_address_component,
    );
    (!merged.is_empty()).then_some(merged)
}

/// Merging a list of named parts — an address's components or a name's — that
/// the save has to write back whole because there are no keys to patch by.
///
/// A part is recognised by what it says, `named` giving the pair that says it,
/// and one that still says it comes back as the server's own copy so that
/// whatever the vCard had no field for rides along. Walking the server's list
/// rather than the edited one keeps an invisible part in its place instead of
/// shuffling it to the end, so that opening a contact and closing it again
/// writes nothing; parts the vCard added follow in the order it stated them.
fn merge_named<T: Clone>(
    current: &[T],
    edited: &[T],
    named: impl Fn(&T) -> (&str, &str),
    mapped: impl Fn(&str) -> bool,
) -> Vec<T> {
    let mut spare: Vec<&T> = edited.iter().collect();
    let mut merged: Vec<T> = Vec::new();
    for part in current {
        let same = |candidate: &&T| named(candidate) == named(part);
        match spare.iter().position(same) {
            // Still on the line, so the server's copy is the one to keep.
            Some(index) => {
                spare.remove(index);
                merged.push(part.clone());
            }
            // Gone from the line — which says the user deleted it only if
            // the line had a field for it in the first place.
            None if !mapped(named(part).0) => merged.push(part.clone()),
            None => {}
        }
    }
    merged.extend(spare.into_iter().cloned());
    merged
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

/// Whether the line the user was handed said anything about this JSContact
/// `contexts` key — the test [`diff_flags`] applies before reading a context's
/// absence as a removal.
///
/// Two reasons it might not have. vCard 3.0 has no `TYPE` for a context of
/// `school` at all, which is [`maps_context`]; and of the two it *does* have,
/// an `ADR` or a `TEL` states only one, because EDS reads a line wearing both
/// into two of the contact editor's per-context fields at once and the next
/// edit of either rewrites the single line behind them. Which one is left off
/// depends on the entry, so the predicate does too. See
/// [`jmap_vcard::states_context`].
///
/// `EMAIL` is not narrowed this way and keeps the plain [`maps_context`]: EDS
/// files an email line by its position (`E_CONTACT_EMAIL_1` to `_4`) rather
/// than by its `TYPE`, so both contexts cross and both come back.
fn slotted_context(contexts: &Option<Value>) -> impl Fn(&str) -> bool + '_ {
    move |key| maps_context(key) && states_context(contexts.as_ref(), key)
}

/// [`slotted_context`] for a phone's `features`: whether the `TEL` the user
/// was handed said this one.
///
/// A `TEL` states one feature for the reason an `ADR` states one context — EDS
/// picks the phone field by `TYPE` too, so a number that is both a voice line
/// and a fax fills two fields the user edits separately, and with no context
/// it fills none at all. See [`jmap_vcard::states_phone_feature`].
fn slotted_feature(features: &Option<Value>) -> impl Fn(&str) -> bool + '_ {
    move |key| maps_phone_feature(key) && states_phone_feature(features.as_ref(), key)
}

/// Replace the members of a boolean map this mapping can spell, keep the
/// rest.
///
/// The two predicates are not the same question, and conflating them loses
/// data. `was_stated` says whether the line the user was handed *said* this
/// member, and so whether its absence from the edited line is a removal;
/// `is_mapped` says whether the mapping can read the member back at all, and
/// so whether its presence is an addition. They differ exactly where a line
/// states only one of several members it holds — [`slotted_context`] and
/// [`slotted_feature`] — and there the narrower one must not also gate the
/// addition: a user moving a number from the Home field to the Business one
/// types in a context the old line never stated, and gating on `was_stated`
/// would drop the one they removed *and* ignore the one they typed, leaving
/// the number with no context at all.
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
