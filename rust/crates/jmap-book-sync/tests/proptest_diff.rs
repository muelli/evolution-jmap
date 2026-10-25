// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Property tests for the keyed-map properties of `patch::diff` (RFC 8620
//! §5.3).
//!
//! The invariant: take a pair of cards (`current`, `edited`) where the vCard
//! mapping states every entry (`states_*` is true). Applying `diff(current,
//! edited)` to `current` with plain PatchObject semantics gives `edited`, and
//! `diff(current, current)` is empty. This invariant covers the properties
//! whose diff is the generic add/modify/remove shape of `diff_entries`, with no
//! further merge or rekey rule. These are nicknames, notes, titles, links,
//! calendars and the whole-set-replace `keywords`. It also covers
//! `organizations`, whose entries are patched member by member and whose
//! `units` list is merged by name (`merge_units`).
//!
//! `anniversaries` and `media` use a different invariant. Their keys never
//! survive Evolution: EDS rebuilds the date line and the first `PHOTO` line,
//! without parameters, every time. Thus literal equality with the edited card
//! is the wrong check, because the reader makes that key, not the user.
//! Instead, `rekey_keyless` promises that the content goes back under the key
//! of *current* when it pairs. An anniversary pairs by `kind`, and a photo
//! pairs with the first unclaimed visible entry. Only a true addition (nothing
//! to pair with) keeps the key of the edited card. Thus `arb_anniversary_cards`
//! and `arb_media_cards` build a third, explicit "expected" card.
//! `anniversaries_round_trip` and `media_round_trip` compare with that card,
//! not with `edited`. Out of scope for anniversaries: a `kind` change on a key
//! that survives literally (this needs a key collision that the generator does
//! not make). Also out of scope: the `Timestamp` (point-in-time) date shape and
//! its whole-value-replace branch. Out of scope for media: `data:` URI photos
//! (so `mediaType` and the base64/subtype comparison in `same_photo` stay
//! untested). A photo URI with a scheme other than `http` or `https` is
//! hidden (see `jmap_vcard::states_media`). The generator makes both, and
//! `expected` keeps a hidden current photo, because the user did not see it.
//! Also out of scope: a second photo that is visible at the same
//! time (the "first unclaimed" order of `rekey_keyless`). Also out of scope:
//! non-photo entries (`logo`, `sound`) that pass through `diff_entries`
//! unchanged.
//!
//! Two tests cover `online_services`. `online_services_service_round_trip` is
//! the plain `diff_entries` shape: a service rename with a constant handle.
//! `online_services_handle_round_trip` checks the shape-preserving case that
//! the module doc of `patch.rs` names. A `vcard_to_card` parse always builds an
//! entry as `{user: Some(handle), uri: None}` (see its `IMPP` and `X-`
//! branches), so `edited` always has that shape. But an earlier whole-value
//! write can leave `current` with a handle drawn from `uri`. A handle rename on
//! that shape rebuilds the `uri` around the new handle and does not change to
//! `user`. This test also uses a third, explicit "expected" card, for the same
//! reason as anniversaries and media. Both tests keep each field that the vCard
//! mapping does not read back (`contexts`, `pref`, `label`, `extra`) at its
//! default on every entry. Only `user`, `uri` and `service` vary.
//!
//! `addresses_components_round_trip` covers `merge_components` through the
//! per-`ADR`-field-kind slots of `arb_address_components_pair`.
//! `merge_components` is `restore_address_components` plus `merge_named`, the
//! list merge by value that `units` also uses. Unlike in `anniversaries`,
//! `media` and `online_services`, the key of an address survives Evolution (the
//! `ADR` line keeps `X-JMAP-KEY`). Thus this test is a literal round trip with
//! `edited` and needs no "expected" card. Out of scope: the joined
//! `number`/`name` field case (the splice of `restore_shared_fields`, which
//! matters only when two component kinds share one `ADR` field). Also out of
//! scope: `full` (`LABEL`), `contexts` and `pref`, which stay at their default
//! on both sides, as the non-core fields of `online_services` do.
//!
//! The generator gives a title `kind` of `Some("title")` or `Some("role")`,
//! never `None`. `title_kind()` treats an absent `kind` as "title".
//! `diff_titles` writes that default as a literal string when the (defaulted)
//! kind changes. Thus a round trip from `None` can end at `Some("title")`, not
//! at `None`. `title_kind()` treats these as equal, but `PartialEq` does not.
//! The two explicit spellings test the same comparison without that false
//! alarm.
//!
//! The units of `organizations` need a restriction too, for a similar reason.
//! `read_organization` (jmap-vcard) never sets `sortAs` or `extra` on a parsed
//! `OrgUnit`, so a parsed edited unit is always bare. Only the current side
//! from the server can carry them. `merge_units` uses the whole struct of
//! current for a unit with an unchanged name, and the bare edited struct
//! otherwise. Thus the generator makes the edited value of each kept unit an
//! exact clone of the current value. A real vCard parse cannot give a different
//! `sortAs` for an unchanged name. Out of scope: invisible (empty-name) current
//! units, whose reinsertion follows an anchor and position rule that the
//! generator does not model. Also out of scope: simultaneous renames matched by
//! position, not by name. Here the two are observably identical. With no
//! invisible unit at stake, the merged output does not depend on the slot that
//! a failed name match pairs with. The organization-level `sortAs` and
//! `contexts` are out of scope too: `diff_organizations` never patches them,
//! and `tests/save.rs` covers their preservation.
//!
//! `name_components_round_trip` covers the same `merge_named` logic through the
//! component-list branch of `diff_name` (`restore_name_components` plus the
//! merge). A `Name` is one struct, not a keyed map, so it has no entry key.
//! `diff_name` always writes the whole merged list under `name/components`, so
//! this test is a literal round trip with `edited`, as for addresses. The one
//! joined-field case here is the splice of `restore_shared_fields`: two `given`
//! components that share one `N` field (a double-barrelled given name). The
//! generator avoids it by construction: it makes one component per kind.
//! `full`, `isOrdered`, `sortAs` and `extra` stay at their default on both
//! sides, as `full`, `contexts` and `pref` do for addresses.
//!
//! `related_to_round_trip` covers `diff_related_to`. This is the one property
//! keyed by the name of the related entity, not by an id. Thus an edit is a
//! withdrawal from one key and a claim on another key, not a change to an
//! entry. `arb_related_to_cards` builds several independent named slots, with
//! one mapped kind (`spouse`) at a time. A bare withdrawal deletes the whole
//! entry. A withdrawal that leaves an unmapped relation type (`kin`) or an
//! extra member removes only `relation/spouse`. The effect of a claim depends
//! on what the server already held under that name. It creates a whole new
//! entry or writes into the `relation/spouse` of an existing entry. For an
//! existing entry without a `relation` field, it replaces the whole field. A
//! withdrawal on one slot and a claim on another are independent adds and
//! removes keyed by name. Thus the rename that the module doc of `patch.rs`
//! describes needs no special case. Out of scope: two mapped kinds that change
//! on the same name at once (`is_gaining_other_kind` in `diff_related_to`). The
//! unit test `a_new_relation_stating_two_mapped_kinds_at_once_is_written_once`
//! in `patch.rs` covers that case.

use std::collections::{BTreeMap, BTreeSet};

use jmap_book_sync::patch::diff;
use jmap_proto::contacts::{
    Address, AddressComponent, Anniversary, Calendar, ContactCard, Link, Media, Name,
    NameComponent, Nickname, Note, OnlineService, OrgUnit, Organization, Relation, Title,
};
use jmap_vcard::states_media;
use proptest::prelude::*;
use serde_json::{Map, Value, json};

/// Applies a patch per RFC 8620 §5.3: a `null` value at the end of a path
/// removes that member, and any other value sets it. A missing parent segment
/// becomes an empty object.
fn apply_patch(target: &mut Value, patch: &Map<String, Value>) {
    for (path, value) in patch {
        let segments: Vec<String> = path.split('/').map(unescape_pointer).collect();
        apply_at(target, &segments, value.clone());
    }
}

fn apply_at(target: &mut Value, segments: &[String], value: Value) {
    let Value::Object(map) = target else {
        return;
    };
    if segments.len() == 1 {
        if value.is_null() {
            map.remove(&segments[0]);
        } else {
            map.insert(segments[0].clone(), value);
        }
        return;
    }
    let child = map
        .entry(segments[0].clone())
        .or_insert_with(|| Value::Object(Map::new()));
    apply_at(child, &segments[1..], value);
}

/// RFC 6901 §3, reversed.
fn unescape_pointer(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

fn round_trips(current_card: &ContactCard, edited_card: &ContactCard) -> Result<(), TestCaseError> {
    prop_assert!(diff(current_card, current_card).is_empty());
    let patch = diff(current_card, edited_card);
    let mut value = serde_json::to_value(current_card).expect("ContactCard always serializes");
    apply_patch(&mut value, &patch);
    let applied: ContactCard =
        serde_json::from_value(value).expect("a patched ContactCard always parses");
    prop_assert_eq!(&applied, edited_card, "patch was: {:?}", patch);
    Ok(())
}

/// Builds a `(current, edited)` pair of keyed maps from four independent
/// groups. The groups are unchanged entries (`s*`), changed entries (`m*`),
/// edited-only entries (`a*`) and current-only entries (`r*`). A changed entry
/// is on both sides, with only its diffable fields changed. The prefixes keep
/// the keys of the groups disjoint. Thus the add, remove and keep branches of
/// `diff_entries` run without accidental key collisions.
fn entry_sets<T: Clone + std::fmt::Debug>(
    stable: impl Strategy<Value = T>,
    modified_pair: impl Strategy<Value = (T, T)>,
    added: impl Strategy<Value = T>,
    removed: impl Strategy<Value = T>,
) -> impl Strategy<Value = (BTreeMap<String, T>, BTreeMap<String, T>)> {
    (
        proptest::collection::vec(stable, 0..=2),
        proptest::collection::vec(modified_pair, 0..=2),
        proptest::collection::vec(added, 0..=2),
        proptest::collection::vec(removed, 0..=2),
    )
        .prop_map(|(stable, modified, added, removed)| {
            let mut current = BTreeMap::new();
            let mut edited = BTreeMap::new();
            for (index, entry) in stable.into_iter().enumerate() {
                current.insert(format!("s{index}"), entry.clone());
                edited.insert(format!("s{index}"), entry);
            }
            for (index, (old, new)) in modified.into_iter().enumerate() {
                current.insert(format!("m{index}"), old);
                edited.insert(format!("m{index}"), new);
            }
            for (index, entry) in added.into_iter().enumerate() {
                edited.insert(format!("a{index}"), entry);
            }
            for (index, entry) in removed.into_iter().enumerate() {
                current.insert(format!("r{index}"), entry);
            }
            (current, edited)
        })
}

fn arb_nickname() -> impl Strategy<Value = Nickname> {
    "[a-zA-Z ]{1,12}".prop_map(|name| {
        let mut v = Nickname::default();
        v.name = name;
        v
    })
}

fn arb_nickname_pair() -> impl Strategy<Value = (Nickname, Nickname)> {
    (arb_nickname(), "[a-zA-Z ]{1,12}").prop_map(|(old, name)| {
        let new = {
            let mut v = old.clone();
            v.name = name;
            v
        };
        (old, new)
    })
}

fn card_with_nicknames(map: BTreeMap<String, Nickname>) -> ContactCard {
    {
        let mut card = ContactCard::default();
        card.nicknames = (!map.is_empty()).then_some(map);
        card
    }
}

fn arb_note() -> impl Strategy<Value = Note> {
    "[a-zA-Z0-9 .,]{1,20}".prop_map(|note| {
        let mut v = Note::default();
        v.note = note;
        v
    })
}

fn arb_note_pair() -> impl Strategy<Value = (Note, Note)> {
    (arb_note(), "[a-zA-Z0-9 .,]{1,20}").prop_map(|(old, note)| {
        let new = {
            let mut v = old.clone();
            v.note = note;
            v
        };
        (old, new)
    })
}

fn card_with_notes(map: BTreeMap<String, Note>) -> ContactCard {
    {
        let mut card = ContactCard::default();
        card.notes = (!map.is_empty()).then_some(map);
        card
    }
}

fn arb_title_kind() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(Some("title".to_owned())),
        Just(Some("role".to_owned())),
    ]
}

fn arb_title() -> impl Strategy<Value = Title> {
    ("[a-zA-Z ]{1,12}", arb_title_kind()).prop_map(|(name, kind)| {
        let mut title = Title::default();
        title.name = name;
        title.kind = kind;
        title
    })
}

fn arb_title_pair() -> impl Strategy<Value = (Title, Title)> {
    (arb_title(), "[a-zA-Z ]{1,12}", arb_title_kind()).prop_map(|(old, name, kind)| {
        let new = {
            let mut title = old.clone();
            title.name = name;
            title.kind = kind;
            title
        };
        (old, new)
    })
}

fn card_with_titles(map: BTreeMap<String, Title>) -> ContactCard {
    {
        let mut card = ContactCard::default();
        card.titles = (!map.is_empty()).then_some(map);
        card
    }
}

fn arb_uri() -> impl Strategy<Value = String> {
    "[a-z]{3,8}://[a-z0-9]{1,10}"
}

fn arb_link_kind() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(None),
        Just(Some("blog".to_owned())),
        Just(Some("video".to_owned())),
    ]
}

fn arb_link() -> impl Strategy<Value = Link> {
    (arb_uri(), arb_link_kind()).prop_map(|(uri, kind)| {
        let mut v = Link::default();
        v.uri = uri;
        v.kind = kind;
        v
    })
}

fn arb_link_pair() -> impl Strategy<Value = (Link, Link)> {
    (arb_link(), arb_uri(), arb_link_kind()).prop_map(|(old, uri, kind)| {
        let new = {
            let mut v = old.clone();
            v.uri = uri;
            v.kind = kind;
            v
        };
        (old, new)
    })
}

fn card_with_links(map: BTreeMap<String, Link>) -> ContactCard {
    {
        let mut card = ContactCard::default();
        card.links = (!map.is_empty()).then_some(map);
        card
    }
}

fn arb_calendar_kind() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(Some("calendar".to_owned())),
        Just(Some("freeBusy".to_owned())),
    ]
}

fn arb_calendar() -> impl Strategy<Value = Calendar> {
    (arb_uri(), arb_calendar_kind()).prop_map(|(uri, kind)| {
        let mut calendar = Calendar::default();
        calendar.uri = uri;
        calendar.kind = kind;
        calendar
    })
}

fn arb_calendar_pair() -> impl Strategy<Value = (Calendar, Calendar)> {
    (arb_calendar(), arb_uri(), arb_calendar_kind()).prop_map(|(old, uri, kind)| {
        let new = {
            let mut calendar = old.clone();
            calendar.uri = uri;
            calendar.kind = kind;
            calendar
        };
        (old, new)
    })
}

fn card_with_calendars(map: BTreeMap<String, Calendar>) -> ContactCard {
    {
        let mut card = ContactCard::default();
        card.calendars = (!map.is_empty()).then_some(map);
        card
    }
}

fn arb_tag() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9]{1,10}"
}

fn card_with_keywords(tags: BTreeSet<String>) -> ContactCard {
    let map: BTreeMap<String, Value> = tags
        .into_iter()
        .map(|tag| (tag, Value::Bool(true)))
        .collect();
    {
        let mut card = ContactCard::default();
        card.keywords = (!map.is_empty()).then_some(map);
        card
    }
}

fn arb_keyword_cards() -> impl Strategy<Value = (ContactCard, ContactCard)> {
    (
        proptest::collection::btree_set(arb_tag(), 0..=4),
        proptest::collection::btree_set(arb_tag(), 0..=4),
    )
        .prop_map(|(current, edited)| (card_with_keywords(current), card_with_keywords(edited)))
}

fn arb_org_name() -> impl Strategy<Value = String> {
    "[a-zA-Z]{1,10}"
}

fn arb_organization_with_name() -> impl Strategy<Value = Organization> {
    arb_org_name().prop_map(Organization::new)
}

fn arb_organization_name_pair() -> impl Strategy<Value = (Organization, Organization)> {
    (arb_organization_with_name(), arb_org_name()).prop_map(|(old, name)| {
        let new = Organization::new(name);
        (old, new)
    })
}

fn card_with_organizations(map: BTreeMap<String, Organization>) -> ContactCard {
    {
        let mut card = ContactCard::default();
        card.organizations = (!map.is_empty()).then_some(map);
        card
    }
}

fn arb_sort_as() -> impl Strategy<Value = Option<String>> {
    prop_oneof![Just(None), "[a-zA-Z]{1,6}".prop_map(Some)]
}

fn bare_unit(name: String) -> OrgUnit {
    OrgUnit::new(&name)
}

/// A `(current, edited)` pair of units lists. `kept` units have the same name
/// and `sortAs` on both sides. A real vCard round trip can change only the name
/// of a unit, so it never changes `sortAs` for an unchanged name. `removed`
/// names are current-only. `added` names are edited-only and bare, like a
/// parsed unit. No name occurs in two groups, so `merge_units` matches each
/// edited unit by exact name or not at all, never by position.
fn arb_units_pair() -> impl Strategy<Value = (Vec<OrgUnit>, Vec<OrgUnit>)> {
    (
        proptest::collection::vec(arb_sort_as(), 0..=2),
        0..=2usize,
        0..=2usize,
    )
        .prop_map(|(kept_sort_as, added, removed)| {
            let mut current = Vec::new();
            let mut edited = Vec::new();
            for (index, sort_as) in kept_sort_as.into_iter().enumerate() {
                let mut unit = OrgUnit::new(&format!("k{index}"));
                unit.sort_as = sort_as;
                current.push(unit.clone());
                edited.push(unit);
            }
            for index in 0..removed {
                current.push(bare_unit(format!("r{index}")));
            }
            for index in 0..added {
                edited.push(bare_unit(format!("a{index}")));
            }
            (current, edited)
        })
}

const UNITS_ORG_NAME: &str = "Acme";

fn organization_with_units(units: Vec<OrgUnit>) -> Organization {
    let mut v = Organization::new(UNITS_ORG_NAME);
    v.units = (!units.is_empty()).then_some(units);
    v
}

fn arb_organization_units_stable() -> impl Strategy<Value = Organization> {
    proptest::collection::vec(arb_sort_as(), 0..=2).prop_map(|sort_as_list| {
        let units = sort_as_list
            .into_iter()
            .enumerate()
            .map(|(index, sort_as)| {
                let mut unit = OrgUnit::new(&format!("u{index}"));
                unit.sort_as = sort_as;
                unit
            })
            .collect();
        organization_with_units(units)
    })
}

fn arb_organization_units_pair() -> impl Strategy<Value = (Organization, Organization)> {
    arb_units_pair().prop_map(|(current_units, edited_units)| {
        (
            organization_with_units(current_units),
            organization_with_units(edited_units),
        )
    })
}

fn arb_address_value() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 ]{1,10}"
}

/// The vCard-mapped `ADR` component kinds, without `number`. `number` shares
/// its field with `name` (the splice of `restore_shared_fields`), which is out
/// of scope here. Thus each kind here matches one `ADR` field on its own.
const ADDRESS_COMPONENT_KINDS: [&str; 7] = [
    "postOfficeBox",
    "apartment",
    "name",
    "locality",
    "region",
    "postcode",
    "country",
];

fn arb_address_with_components() -> impl Strategy<Value = Address> {
    arb_address_value().prop_map(|value| {
        let mut address_val = Address::default();
        address_val.components = Some(vec![AddressComponent::new("locality", value)]);
        address_val
    })
}

/// One component kind's slot across (current, edited). The slot is absent on
/// both sides, edited only (a true addition), or current only (removed). Or it
/// is on both sides, unchanged (kept) or with different values (modified).
#[derive(Clone, Debug)]
enum AddressComponentSlot {
    Absent,
    Kept(String),
    Added(String),
    Removed(String),
    Modified(String, String),
}

fn arb_address_component_slot() -> impl Strategy<Value = AddressComponentSlot> {
    prop_oneof![
        Just(AddressComponentSlot::Absent),
        arb_address_value().prop_map(AddressComponentSlot::Kept),
        arb_address_value().prop_map(AddressComponentSlot::Added),
        arb_address_value().prop_map(AddressComponentSlot::Removed),
        (arb_address_value(), arb_address_value())
            .prop_filter("value must actually change", |(old, new)| old != new)
            .prop_map(|(old, new)| AddressComponentSlot::Modified(old, new)),
    ]
}

/// Builds a `(current, edited)` address pair, one slot per
/// `ADDRESS_COMPONENT_KINDS` kind, as a model of `merge_components`. A `Kept`
/// component is the server copy. The merge matches it by `(kind, value)` and
/// puts it where `current` had it. A `Removed` component is dropped: its kind
/// is stated (mapped and not empty), so its absence from the line means that
/// the user deleted it. The new value of an `Added` or `Modified` kind is a
/// `spare` entry with no match in `current`. The merge appends it after all
/// kept components, in the order of `edited`. `edited` states it at the same
/// place, because the generator builds both sides with kept components first
/// and new components after. Thus the result of `merge_components` matches
/// `edited` literally and needs no "expected" card. `anniversaries`, `media`
/// and `online_services` need one, because their keys do not survive Evolution.
///
/// `full`, `contexts` and `pref` stay at their default on both sides, as the
/// non-core fields of `online_services` do.
fn arb_address_components_pair() -> impl Strategy<Value = (Address, Address)> {
    proptest::collection::vec(arb_address_component_slot(), ADDRESS_COMPONENT_KINDS.len())
        .prop_filter(
            "at least one current-side component, to exercise the merge",
            |slots| {
                slots.iter().any(|slot| {
                    matches!(
                        slot,
                        AddressComponentSlot::Kept(_)
                            | AddressComponentSlot::Removed(_)
                            | AddressComponentSlot::Modified(_, _)
                    )
                })
            },
        )
        .prop_map(|slots| {
            let mut current = Vec::new();
            let mut current_tail = Vec::new();
            let mut edited = Vec::new();
            let mut edited_tail = Vec::new();
            for (kind, slot) in ADDRESS_COMPONENT_KINDS.into_iter().zip(slots) {
                match slot {
                    AddressComponentSlot::Absent => {}
                    AddressComponentSlot::Kept(value) => {
                        let component = AddressComponent::new(kind, value);
                        current.push(component.clone());
                        edited.push(component);
                    }
                    AddressComponentSlot::Added(value) => {
                        edited_tail.push(AddressComponent::new(kind, value));
                    }
                    AddressComponentSlot::Removed(value) => {
                        current_tail.push(AddressComponent::new(kind, value));
                    }
                    AddressComponentSlot::Modified(old, new) => {
                        current_tail.push(AddressComponent::new(kind, old));
                        edited_tail.push(AddressComponent::new(kind, new));
                    }
                }
            }
            current.extend(current_tail);
            edited.extend(edited_tail);
            let to_address = |components: Vec<AddressComponent>| {
                let mut address_val = Address::default();
                address_val.components = (!components.is_empty()).then_some(components);
                address_val
            };
            (to_address(current), to_address(edited))
        })
}

fn card_with_addresses(map: BTreeMap<String, Address>) -> ContactCard {
    {
        let mut card = ContactCard::default();
        card.addresses = (!map.is_empty()).then_some(map);
        card
    }
}

fn arb_name_value() -> impl Strategy<Value = String> {
    "[a-zA-Z]{1,10}"
}

/// The vCard-mapped `N` component kinds. Unlike `ADDRESS_COMPONENT_KINDS`, no
/// kind needs exclusion: each kind has its own `N` field. This includes `given`
/// and `given2`: per RFC 9553 §2.2.1, both halves of a double-barrelled given
/// name have the kind `given`. Thus one component per kind never shares a field
/// with another kind. A shared field occurs only with two components of the
/// *same* kind, which this generator does not make.
const NAME_COMPONENT_KINDS: [&str; 5] = ["title", "given", "given2", "surname", "credential"];

/// Same as [`AddressComponentSlot`] for the components of a `Name`: the same
/// shapes (kept, added, removed, modified) over [`NameComponent`], not
/// [`AddressComponent`].
#[derive(Clone, Debug)]
enum NameComponentSlot {
    Absent,
    Kept(String),
    Added(String),
    Removed(String),
    Modified(String, String),
}

fn arb_name_component_slot() -> impl Strategy<Value = NameComponentSlot> {
    prop_oneof![
        Just(NameComponentSlot::Absent),
        arb_name_value().prop_map(NameComponentSlot::Kept),
        arb_name_value().prop_map(NameComponentSlot::Added),
        arb_name_value().prop_map(NameComponentSlot::Removed),
        (arb_name_value(), arb_name_value())
            .prop_filter("value must actually change", |(old, new)| old != new)
            .prop_map(|(old, new)| NameComponentSlot::Modified(old, new)),
    ]
}

/// Builds a `(current, edited)` pair of `Name.components`, one slot per
/// [`NAME_COMPONENT_KINDS`] kind, in the same way as
/// [`arb_address_components_pair`]. A `Kept` component is the server copy,
/// matched by `(kind, value)`. A `Removed` component is dropped: its kind is
/// stated, so its absence means that the user deleted it. The new value of an
/// `Added` or `Modified` kind has no match in `current`. Thus the merge appends
/// it after all kept components, in the same order on both sides. The result of
/// `merge_named` then matches `edited` literally and needs no "expected" card.
///
/// `full`, `isOrdered`, `sortAs` and `extra` stay at their default on both
/// sides, as the non-core fields of `online_services` do.
fn arb_name_components_pair() -> impl Strategy<Value = (Name, Name)> {
    proptest::collection::vec(arb_name_component_slot(), NAME_COMPONENT_KINDS.len())
        .prop_filter(
            "at least one current-side component, to exercise the merge",
            |slots| {
                slots.iter().any(|slot| {
                    matches!(
                        slot,
                        NameComponentSlot::Kept(_)
                            | NameComponentSlot::Removed(_)
                            | NameComponentSlot::Modified(_, _)
                    )
                })
            },
        )
        .prop_map(|slots| {
            let mut current = Vec::new();
            let mut current_tail = Vec::new();
            let mut edited = Vec::new();
            let mut edited_tail = Vec::new();
            for (kind, slot) in NAME_COMPONENT_KINDS.into_iter().zip(slots) {
                match slot {
                    NameComponentSlot::Absent => {}
                    NameComponentSlot::Kept(value) => {
                        let component = NameComponent::new(kind, value);
                        current.push(component.clone());
                        edited.push(component);
                    }
                    NameComponentSlot::Added(value) => {
                        edited_tail.push(NameComponent::new(kind, value));
                    }
                    NameComponentSlot::Removed(value) => {
                        current_tail.push(NameComponent::new(kind, value));
                    }
                    NameComponentSlot::Modified(old, new) => {
                        current_tail.push(NameComponent::new(kind, old));
                        edited_tail.push(NameComponent::new(kind, new));
                    }
                }
            }
            current.extend(current_tail);
            edited.extend(edited_tail);
            let to_name = |components: Vec<NameComponent>| {
                let mut name_val = Name::default();
                name_val.components = (!components.is_empty()).then_some(components);
                name_val
            };
            (to_name(current), to_name(edited))
        })
}

fn card_with_name(name: Name) -> ContactCard {
    {
        let mut card = ContactCard::default();
        card.name = Some(name);
        card
    }
}

/// One `kind`'s slot across (current, edited). The slot is absent on both
/// sides, edited only (a true addition) or current only (removed). Or it is on
/// both sides with independent dates (a modification, which can be a no-op).
#[derive(Clone, Debug)]
enum AnniversarySlot {
    Absent,
    Added(Value),
    Removed(Value),
    Modified(Value, Value),
}

/// A `PartialDate` (RFC 9553 §2.8.1) well inside the range that a vCard 3.0
/// date line can state and read back exactly. Years 1000-9999 avoid the clamp
/// of `Day::survives_the_field_it_lands_in`, which `eds-sys/tests/contacts.rs`
/// covers. Days 1-28 are valid in every month, so the generator does not need
/// to model short months.
fn arb_anniversary_date() -> impl Strategy<Value = Value> {
    (1000u32..=9999, 1u32..=12, 1u32..=28).prop_map(|(year, month, day)| {
        json!({"@type": "PartialDate", "year": year, "month": month, "day": day})
    })
}

fn arb_anniversary_slot() -> impl Strategy<Value = AnniversarySlot> {
    prop_oneof![
        Just(AnniversarySlot::Absent),
        arb_anniversary_date().prop_map(AnniversarySlot::Added),
        arb_anniversary_date().prop_map(AnniversarySlot::Removed),
        (arb_anniversary_date(), arb_anniversary_date())
            .prop_map(|(old, new)| AnniversarySlot::Modified(old, new)),
    ]
}

fn card_with_anniversaries(map: BTreeMap<String, Anniversary>) -> ContactCard {
    {
        let mut card = ContactCard::default();
        card.anniversaries = (!map.is_empty()).then_some(map);
        card
    }
}

/// Builds (current, edited, expected) for the two kinds that a vCard
/// anniversary can have (`birth`, `wedding`), with one slot each. `current` and
/// `edited` always use different keys per kind (`c-<kind>`, `e-<kind>`),
/// because an anniversary never gets the same key twice. `expected` is the
/// result that `rekey_keyless` must give. A `Modified` slot goes under the key
/// of current: this is the pairing under test. An `Added` slot keeps the key of
/// edited, and a `Removed` slot leaves no entry.
fn arb_anniversary_cards() -> impl Strategy<Value = (ContactCard, ContactCard, ContactCard)> {
    (arb_anniversary_slot(), arb_anniversary_slot()).prop_map(|(birth, wedding)| {
        let mut current = BTreeMap::new();
        let mut edited = BTreeMap::new();
        let mut expected = BTreeMap::new();
        for (kind, slot) in [("birth", birth), ("wedding", wedding)] {
            let current_key = format!("c-{kind}");
            let edited_key = format!("e-{kind}");
            let entry = |date: Value| {
                let mut v = Anniversary::default();
                v.kind = kind.to_owned();
                v.date = Some(date);
                v
            };
            match slot {
                AnniversarySlot::Absent => {}
                AnniversarySlot::Added(date) => {
                    edited.insert(edited_key.clone(), entry(date.clone()));
                    expected.insert(edited_key, entry(date));
                }
                AnniversarySlot::Removed(date) => {
                    current.insert(current_key, entry(date));
                }
                AnniversarySlot::Modified(old, new) => {
                    current.insert(current_key.clone(), entry(old));
                    edited.insert(edited_key, entry(new.clone()));
                    expected.insert(current_key, entry(new));
                }
            }
        }
        (
            card_with_anniversaries(current),
            card_with_anniversaries(edited),
            card_with_anniversaries(expected),
        )
    })
}

/// One photo's slot across (current, edited), with the same shapes as
/// `AnniversarySlot`. The slot is absent, a true addition or a removal. Or it
/// is on both sides with independent URIs (a modification, which can be a
/// no-op).
#[derive(Clone, Debug)]
enum MediaSlot {
    Absent,
    Added(String),
    Removed(String),
    Modified(String, String),
}

/// A URI that is not a `data:` URI, so `same_photo` compares only this string.
/// `data:` URIs and `mediaType` are out of scope. Half of the URIs are visible
/// (`http`, `https`), the other half are hidden.
fn arb_photo_uri() -> impl Strategy<Value = String> {
    prop_oneof![
        "https?://[a-z0-9]{1,10}\\.jpg",
        "[a-z]{3,8}://[a-z0-9]{1,10}\\.jpg"
            .prop_filter("hidden scheme", |uri| !uri.starts_with("http")),
    ]
}

fn arb_media_slot() -> impl Strategy<Value = MediaSlot> {
    prop_oneof![
        Just(MediaSlot::Absent),
        arb_photo_uri().prop_map(MediaSlot::Added),
        arb_photo_uri().prop_map(MediaSlot::Removed),
        (arb_photo_uri(), arb_photo_uri()).prop_map(|(old, new)| MediaSlot::Modified(old, new)),
    ]
}

fn card_with_media(map: BTreeMap<String, Media>) -> ContactCard {
    {
        let mut card = ContactCard::default();
        card.media = (!map.is_empty()).then_some(map);
        card
    }
}

/// Builds (current, edited, expected) for the one photo that Evolution shows
/// and can edit. As in `arb_anniversary_cards`, `current` and `edited` always
/// use different keys (`c-photo`, `e-photo`), because a photo never gets the
/// same key twice. `expected` is the result that `rekey_keyless` must give. A
/// `Modified` slot goes under the key of current: this is the pairing under
/// test. An `Added` slot keeps the key of edited, and a `Removed` slot leaves
/// no entry. A hidden current photo never pairs and is never removed, so it
/// stays under its key, and a `Modified` slot then adds the new photo under the
/// key of edited.
fn arb_media_cards() -> impl Strategy<Value = (ContactCard, ContactCard, ContactCard)> {
    arb_media_slot().prop_map(|slot| {
        let mut current = BTreeMap::new();
        let mut edited = BTreeMap::new();
        let mut expected = BTreeMap::new();
        let current_key = "c-photo".to_owned();
        let edited_key = "e-photo".to_owned();
        let entry = |uri: String| Media {
            kind: Some("photo".to_owned()),
            uri,
            ..Media::default()
        };
        match slot {
            MediaSlot::Absent => {}
            MediaSlot::Added(uri) => {
                edited.insert(edited_key.clone(), entry(uri.clone()));
                expected.insert(edited_key, entry(uri));
            }
            MediaSlot::Removed(uri) => {
                if !states_media(&entry(uri.clone())) {
                    expected.insert(current_key.clone(), entry(uri.clone()));
                }
                current.insert(current_key, entry(uri));
            }
            MediaSlot::Modified(old, new) => {
                current.insert(current_key.clone(), entry(old.clone()));
                edited.insert(edited_key.clone(), entry(new.clone()));
                if states_media(&entry(old.clone())) {
                    expected.insert(current_key, entry(new));
                } else {
                    expected.insert(current_key, entry(old));
                    expected.insert(edited_key, entry(new));
                }
            }
        }
        (
            card_with_media(current),
            card_with_media(edited),
            card_with_media(expected),
        )
    })
}

/// Plain `[a-zA-Z0-9]{1,10}`: not empty, and free of `/?#%` and whitespace.
/// Thus it is always a `plain_handle` (the code can draw it from a URI and
/// rebuild a URI around it). It is also always visible to
/// `states_online_service`.
fn arb_handle() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9]{1,10}"
}

/// Two service names from `SERVICE_SCHEMES` and `ONLINE_SERVICES`. `SERVICE_A`
/// has exactly one scheme, so a handle drawn from or rebuilt into its URI is
/// unambiguous. `SERVICE_B` has two schemes and occurs only in `user`-stated
/// entries, never in a URI.
const SERVICE_A: &str = "Skype";
const SERVICE_B: &str = "Yahoo";

/// The shape in which every real `vcard_to_card` parse builds an online-service
/// entry: a plain `user`, never a `uri`.
fn user_stated(service: &str, handle: String) -> OnlineService {
    OnlineService::new().with_service(service).with_user(handle)
}

fn arb_online_service() -> impl Strategy<Value = OnlineService> {
    arb_handle().prop_map(|handle| user_stated(SERVICE_A, handle))
}

fn arb_online_service_service_pair() -> impl Strategy<Value = (OnlineService, OnlineService)> {
    arb_handle().prop_map(|handle| {
        (
            user_stated(SERVICE_A, handle.clone()),
            user_stated(SERVICE_B, handle),
        )
    })
}

fn card_with_online_services(map: BTreeMap<String, OnlineService>) -> ContactCard {
    {
        let mut card = ContactCard::default();
        card.online_services = (!map.is_empty()).then_some(map);
        card
    }
}

/// The two shapes that can state the handle of a server-held entry: a plain
/// `user`, or a `uri` with no `user`. The second shape comes from an earlier
/// whole-value write, never from a `vcard_to_card` parse.
#[derive(Clone, Copy, Debug)]
enum CurrentShape {
    UserStated,
    UriDrawn,
}

fn shaped(shape: CurrentShape, handle: String) -> OnlineService {
    match shape {
        CurrentShape::UserStated => user_stated(SERVICE_A, handle),
        CurrentShape::UriDrawn => OnlineService::new()
            .with_service(SERVICE_A)
            .with_uri(format!("skype:{handle}")),
    }
}

/// One entry's (current, edited, expected) triple for a handle rename. `edited`
/// always has the `user` shape, as every real parse builds it; `current`
/// varies. `expected` is the result that the save must give. A `user`-shaped
/// current keeps the new handle in `user`. A `uri`-drawn current keeps the
/// `uri` shape, rebuilt around the new handle: the rename writes into the
/// member that held the old handle.
fn arb_online_service_handle_cards()
-> impl Strategy<Value = (ContactCard, ContactCard, ContactCard)> {
    (
        prop_oneof![Just(CurrentShape::UserStated), Just(CurrentShape::UriDrawn)],
        arb_handle(),
        arb_handle(),
    )
        .prop_map(|(shape, old_handle, new_handle)| {
            let key = "s0".to_owned();
            let current = shaped(shape, old_handle);
            let edited = user_stated(SERVICE_A, new_handle.clone());
            let expected = shaped(shape, new_handle);
            (
                card_with_online_services(BTreeMap::from([(key.clone(), current)])),
                card_with_online_services(BTreeMap::from([(key.clone(), edited)])),
                card_with_online_services(BTreeMap::from([(key, expected)])),
            )
        })
}

/// Like [`round_trips`], but compares with an `expected` card that differs from
/// `edited`. Use it for a property whose key `rekey_keyless` reassigns. See
/// `arb_anniversary_cards`.
fn round_trips_to(
    current_card: &ContactCard,
    edited_card: &ContactCard,
    expected_card: &ContactCard,
) -> Result<(), TestCaseError> {
    prop_assert!(diff(current_card, current_card).is_empty());
    let patch = diff(current_card, edited_card);
    let mut value = serde_json::to_value(current_card).expect("ContactCard always serializes");
    apply_patch(&mut value, &patch);
    let applied: ContactCard =
        serde_json::from_value(value).expect("a patched ContactCard always parses");
    prop_assert_eq!(&applied, expected_card, "patch was: {:?}", patch);
    Ok(())
}

/// The one mapped relation kind that this generator uses. `diff_related_to`
/// treats `spouse`, `manager` and `assistant` identically, so one kind covers
/// the withdraw and claim shapes. The generator does not make the
/// two-kinds-at-once case, which the unit tests of `patch.rs` cover.
const RELATION_KIND: &str = "spouse";

fn bare_relation() -> Relation {
    let mut v = Relation::default();
    v.relation = Some([(RELATION_KIND.to_owned(), json!(true))].into());
    v
}

fn card_with_related_to(map: BTreeMap<String, Relation>) -> ContactCard {
    {
        let mut card = ContactCard::default();
        card.related_to = (!map.is_empty()).then_some(map);
        card
    }
}

/// One name's slot in `arb_related_to_cards`. The variants cover each shape
/// that `diff_related_to` separates for one mapped kind.
#[derive(Clone, Debug)]
enum RelatedSlot {
    /// States the kind on the same name on both sides: no patch for this key.
    Kept,
    /// Current states only the kind: the withdrawal deletes the whole entry.
    RemovedBare,
    /// Current states the kind with an unmapped relation type (`kin`). The
    /// withdrawal removes only `relation/spouse`; the entry and `kin` stay.
    RemovedKeepsUnmappedKind,
    /// Current states the kind with an extra member that vCard has no field
    /// for. This is the same partial withdrawal, through the other guard.
    RemovedKeepsExtra(String),
    /// Edited states the kind on a name that the server never held: a
    /// whole-entry create.
    AddedNew,
    /// Edited states the kind on a name that the server held with an unmapped
    /// relation type: the patch writes into `relation/spouse` of that entry.
    AddedExistingWithRelation,
    /// Edited states the kind on a name that the server held with only
    /// extra members. The patch replaces the whole (absent) `relation`
    /// field.
    AddedExistingNoRelation(String),
}

fn arb_related_slot() -> impl Strategy<Value = RelatedSlot> {
    prop_oneof![
        Just(RelatedSlot::Kept),
        Just(RelatedSlot::RemovedBare),
        Just(RelatedSlot::RemovedKeepsUnmappedKind),
        "[a-z]{1,8}".prop_map(RelatedSlot::RemovedKeepsExtra),
        Just(RelatedSlot::AddedNew),
        Just(RelatedSlot::AddedExistingWithRelation),
        "[a-z]{1,8}".prop_map(RelatedSlot::AddedExistingNoRelation),
    ]
}

/// Builds (current, edited, expected) across several independent named slots. A
/// withdrawal on one slot and a claim on another make the rename that the
/// module doc of `patch.rs` describes. The key is the name, not an id, so the
/// rename is an independent remove and an independent add.
fn arb_related_to_cards() -> impl Strategy<Value = (ContactCard, ContactCard, ContactCard)> {
    proptest::collection::vec(arb_related_slot(), 0..=4)
        .prop_filter(
            "at least one slot that changes something, to exercise the withdraw/claim logic",
            |slots| slots.iter().any(|slot| !matches!(slot, RelatedSlot::Kept)),
        )
        .prop_map(|slots| {
            let mut current = BTreeMap::new();
            let mut edited = BTreeMap::new();
            let mut expected = BTreeMap::new();
            for (i, slot) in slots.into_iter().enumerate() {
                match slot {
                    RelatedSlot::Kept => {
                        let key = format!("same-{i}");
                        let value = bare_relation();
                        current.insert(key.clone(), value.clone());
                        edited.insert(key.clone(), value.clone());
                        expected.insert(key, value);
                    }
                    RelatedSlot::RemovedBare => {
                        current.insert(format!("gone-{i}"), bare_relation());
                    }
                    RelatedSlot::RemovedKeepsUnmappedKind => {
                        let key = format!("kept-kind-{i}");
                        current.insert(key.clone(), {
                            let mut v = Relation::default();
                            v.relation = Some(
                                [
                                    (RELATION_KIND.to_owned(), json!(true)),
                                    ("kin".to_owned(), json!(true)),
                                ]
                                .into(),
                            );
                            v
                        });
                        expected.insert(key, {
                            let mut v = Relation::default();
                            v.relation = Some([("kin".to_owned(), json!(true))].into());
                            v
                        });
                    }
                    RelatedSlot::RemovedKeepsExtra(note) => {
                        let key = format!("kept-extra-{i}");
                        current.insert(key.clone(), {
                            let mut v = Relation::default();
                            v.relation = Some([(RELATION_KIND.to_owned(), json!(true))].into());
                            v.extra = [("note".to_owned(), json!(note))].into();
                            v
                        });
                        expected.insert(key, {
                            let mut v = Relation::default();
                            v.relation = Some(BTreeMap::new());
                            v.extra = [("note".to_owned(), json!(note))].into();
                            v
                        });
                    }
                    RelatedSlot::AddedNew => {
                        let key = format!("brand-new-{i}");
                        let value = bare_relation();
                        edited.insert(key.clone(), value.clone());
                        expected.insert(key, value);
                    }
                    RelatedSlot::AddedExistingWithRelation => {
                        let key = format!("existing-rel-{i}");
                        current.insert(key.clone(), {
                            let mut v = Relation::default();
                            v.relation = Some([("kin".to_owned(), json!(true))].into());
                            v
                        });
                        edited.insert(key.clone(), bare_relation());
                        expected.insert(key, {
                            let mut v = Relation::default();
                            v.relation = Some(
                                [
                                    ("kin".to_owned(), json!(true)),
                                    (RELATION_KIND.to_owned(), json!(true)),
                                ]
                                .into(),
                            );
                            v
                        });
                    }
                    RelatedSlot::AddedExistingNoRelation(note) => {
                        let key = format!("existing-norel-{i}");
                        current.insert(key.clone(), {
                            let mut v = Relation::default();
                            v.relation = None;
                            v.extra = [("note".to_owned(), json!(note))].into();
                            v
                        });
                        edited.insert(key.clone(), bare_relation());
                        expected.insert(key, {
                            let mut v = Relation::default();
                            v.relation = Some([(RELATION_KIND.to_owned(), json!(true))].into());
                            v.extra = [("note".to_owned(), json!(note))].into();
                            v
                        });
                    }
                }
            }
            (
                card_with_related_to(current),
                card_with_related_to(edited),
                card_with_related_to(expected),
            )
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn nicknames_round_trip(
        (current, edited) in entry_sets(arb_nickname(), arb_nickname_pair(), arb_nickname(), arb_nickname())
    ) {
        round_trips(&card_with_nicknames(current), &card_with_nicknames(edited))?;
    }

    #[test]
    fn notes_round_trip(
        (current, edited) in entry_sets(arb_note(), arb_note_pair(), arb_note(), arb_note())
    ) {
        round_trips(&card_with_notes(current), &card_with_notes(edited))?;
    }

    #[test]
    fn titles_round_trip(
        (current, edited) in entry_sets(arb_title(), arb_title_pair(), arb_title(), arb_title())
    ) {
        round_trips(&card_with_titles(current), &card_with_titles(edited))?;
    }

    #[test]
    fn links_round_trip(
        (current, edited) in entry_sets(arb_link(), arb_link_pair(), arb_link(), arb_link())
    ) {
        round_trips(&card_with_links(current), &card_with_links(edited))?;
    }

    #[test]
    fn calendars_round_trip(
        (current, edited) in entry_sets(arb_calendar(), arb_calendar_pair(), arb_calendar(), arb_calendar())
    ) {
        round_trips(&card_with_calendars(current), &card_with_calendars(edited))?;
    }

    #[test]
    fn keywords_round_trip((current_card, edited_card) in arb_keyword_cards()) {
        round_trips(&current_card, &edited_card)?;
    }

    #[test]
    fn organizations_name_round_trip(
        (current, edited) in entry_sets(
            arb_organization_with_name(),
            arb_organization_name_pair(),
            arb_organization_with_name(),
            arb_organization_with_name(),
        )
    ) {
        round_trips(&card_with_organizations(current), &card_with_organizations(edited))?;
    }

    #[test]
    fn organizations_units_round_trip(
        (current, edited) in entry_sets(
            arb_organization_units_stable(),
            arb_organization_units_pair(),
            arb_organization_units_stable(),
            arb_organization_units_stable(),
        )
    ) {
        round_trips(&card_with_organizations(current), &card_with_organizations(edited))?;
    }

    #[test]
    fn anniversaries_round_trip(
        (current_card, edited_card, expected_card) in arb_anniversary_cards()
    ) {
        round_trips_to(&current_card, &edited_card, &expected_card)?;
    }

    #[test]
    fn media_round_trip(
        (current_card, edited_card, expected_card) in arb_media_cards()
    ) {
        round_trips_to(&current_card, &edited_card, &expected_card)?;
    }

    #[test]
    fn online_services_service_round_trip(
        (current, edited) in entry_sets(
            arb_online_service(),
            arb_online_service_service_pair(),
            arb_online_service(),
            arb_online_service(),
        )
    ) {
        round_trips(&card_with_online_services(current), &card_with_online_services(edited))?;
    }

    #[test]
    fn online_services_handle_round_trip(
        (current_card, edited_card, expected_card) in arb_online_service_handle_cards()
    ) {
        round_trips_to(&current_card, &edited_card, &expected_card)?;
    }

    #[test]
    fn addresses_components_round_trip(
        (current, edited) in entry_sets(
            arb_address_with_components(),
            arb_address_components_pair(),
            arb_address_with_components(),
            arb_address_with_components(),
        )
    ) {
        round_trips(&card_with_addresses(current), &card_with_addresses(edited))?;
    }

    #[test]
    fn name_components_round_trip(
        (current, edited) in arb_name_components_pair()
    ) {
        round_trips(&card_with_name(current), &card_with_name(edited))?;
    }

    #[test]
    fn related_to_round_trip(
        (current_card, edited_card, expected_card) in arb_related_to_cards()
    ) {
        round_trips_to(&current_card, &edited_card, &expected_card)?;
    }
}
