// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Property tests for `patch::diff`'s keyed-map properties (RFC 8620 §5.3).
//!
//! The invariant: for a pair of cards (`current`, `edited`) built so that
//! every entry is one the vCard mapping actually states (`states_*` true),
//! applying `diff(current, edited)` to `current` under plain PatchObject
//! semantics yields `edited`, and `diff(current, current)` is empty. This
//! covers the properties whose diff logic is the generic add/modify/remove
//! shape (`diff_entries`) with no further merge or rekey heuristic:
//! nicknames, notes, titles, links, calendars, and the whole-set-replace
//! `keywords`; plus `organizations`, whose entries are patched member by
//! member and whose `units` list is merged by name (`merge_units`).
//! `patch.rs`'s own module doc explains why the rest (name components,
//! addresses, related_to) deliberately keep entries the vCard never showed
//! untouched, or match entries by value rather than by key: those need
//! generators that model that lossiness on purpose, not a card built from
//! entries that are all stateable, and are left for a later increment.
//!
//! `anniversaries` and `media` are covered here too, but not by the same
//! invariant: their keys never survive Evolution at all (EDS rebuilds the
//! date line, and the first `PHOTO` line, dropping parameters, every time),
//! so literal equality with the edited card is the wrong thing to check —
//! that key is an artifact of the reader, not anything the user chose or the
//! save should reproduce. What `rekey_keyless` promises instead is that the
//! content lands back under *current's* key when it pairs (by `kind` for an
//! anniversary, with the first unclaimed visible entry for a photo), and
//! only a genuine addition (nothing to pair with) keeps the edited card's
//! own key, so `arb_anniversary_cards`/`arb_media_cards` build a third,
//! explicit "expected" card to say so, and `anniversaries_round_trip`/
//! `media_round_trip` check against that rather than against `edited`
//! directly. Scoped out, left for later: for anniversaries, a `kind` change
//! on a literally-surviving key (needs a key collision this generator does
//! not construct) and the `Timestamp` (point-in-time) date shape, whose
//! whole-value-replace branch this increment does not exercise; for media,
//! `data:` URI photos (so `mediaType` and the base64/subtype comparison in
//! `same_photo` stay untested), a second simultaneously-visible photo
//! (`rekey_keyless`'s "first unclaimed" ordering rather than its
//! single-candidate case), and non-photo (`logo`/`sound`) entries passing
//! through `diff_entries` untouched.
//!
//! `online_services` is covered by two tests: `online_services_service_round_trip`
//! is the plain `diff_entries` shape (a service rename with the handle held
//! constant), and `online_services_handle_round_trip` checks the one
//! shape-preserving case the module doc calls out — a `vcard_to_card` parse
//! always builds an entry as `{user: Some(handle), uri: None}` (see its
//! `IMPP`/`X-` branches), so `edited` is always that shape, but `current` can
//! be `uri`-drawn from an earlier whole-value write, and a handle rename on
//! that shape rewrites the `uri` around the new handle rather than switching
//! to `user`. That is a third, explicit "expected" card again, not literal
//! equality with `edited`, for the same reason anniversaries' and media's is.
//! Both tests hold every field the vCard mapping does not read back
//! (`contexts`, `pref`, `label`, `extra`) at its default on every entry, so
//! equality on those fields is never in question; only `user`/`uri`/`service`
//! vary.
//!
//! Title's `kind` is kept to `Some("title")`/`Some("role")` here rather than
//! also generating `None`: `title_kind()` treats an absent `kind` as
//! "title", and `diff_titles` writes that default back out as the literal
//! string whenever the (defaulted) kind changes, so a round trip that started
//! from `None` lands on `Some("title")` rather than `None` again — a
//! representation difference `title_kind()` calls equivalent but
//! `PartialEq` does not. Restricting the generator to the two explicit
//! spellings exercises the same comparison without that false alarm.
//!
//! `organizations`' units need a restriction of their own, for the same kind
//! of reason. `read_organization` (jmap-vcard) never sets a parsed
//! `OrgUnit`'s `sortAs` or `extra`, so a freshly-parsed edited unit is
//! always bare; only the server-held current side ever carries one.
//! `merge_units` reuses current's whole struct for a unit whose name is
//! unchanged, and the edited (bare) struct otherwise, so the generator
//! keeps every kept unit's edited-side value an exact clone of current's
//! rather than inventing a different `sortAs` for an unchanged name, which
//! a real vCard parse could never produce anyway. Out of scope here, left
//! for later: invisible (empty-name) current units, whose reinsertion
//! follows an anchor/position rule this generator does not model, and
//! simultaneous renames matched by position rather than by name (the two
//! are observably identical here, since nothing in the merged output
//! depends on which original slot a failed name match was paired with once
//! no invisible unit's position is at stake). Organization-level
//! `sortAs`/`contexts` are out of scope too: `diff_organizations` never
//! patches them, so their preservation is not this property's job, and is
//! already covered by `tests/save.rs`.

use std::collections::{BTreeMap, BTreeSet};

use jmap_book_sync::patch::diff;
use jmap_proto::contacts::{
    Anniversary, Calendar, ContactCard, Link, Media, Nickname, Note, OnlineService, OrgUnit,
    Organization, Title,
};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

/// RFC 8620 §5.3: a `null` value at the end of a path removes that member,
/// anything else sets it; every path segment before the last already exists.
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

/// Builds a `(current, edited)` pair of keyed maps out of four independent
/// groups: entries unchanged on both sides (`s*`), entries present on both
/// sides with only their diffable field(s) changed (`m*`), entries only on
/// the edited side (`a*`), and entries only on the current side (`r*`). The
/// prefixes keep every group's keys disjoint, so `diff_entries`' add/remove/
/// keep branches are exercised without any accidental key collision.
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
    "[a-zA-Z ]{1,12}".prop_map(|name| Nickname {
        name,
        ..Nickname::default()
    })
}

fn arb_nickname_pair() -> impl Strategy<Value = (Nickname, Nickname)> {
    (arb_nickname(), "[a-zA-Z ]{1,12}").prop_map(|(old, name)| {
        let new = Nickname {
            name,
            ..old.clone()
        };
        (old, new)
    })
}

fn card_with_nicknames(map: BTreeMap<String, Nickname>) -> ContactCard {
    ContactCard {
        nicknames: (!map.is_empty()).then_some(map),
        ..ContactCard::default()
    }
}

fn arb_note() -> impl Strategy<Value = Note> {
    "[a-zA-Z0-9 .,]{1,20}".prop_map(|note| Note {
        note,
        ..Note::default()
    })
}

fn arb_note_pair() -> impl Strategy<Value = (Note, Note)> {
    (arb_note(), "[a-zA-Z0-9 .,]{1,20}").prop_map(|(old, note)| {
        let new = Note {
            note,
            ..old.clone()
        };
        (old, new)
    })
}

fn card_with_notes(map: BTreeMap<String, Note>) -> ContactCard {
    ContactCard {
        notes: (!map.is_empty()).then_some(map),
        ..ContactCard::default()
    }
}

fn arb_title_kind() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(Some("title".to_owned())),
        Just(Some("role".to_owned())),
    ]
}

fn arb_title() -> impl Strategy<Value = Title> {
    ("[a-zA-Z ]{1,12}", arb_title_kind()).prop_map(|(name, kind)| Title {
        name,
        kind,
        ..Title::default()
    })
}

fn arb_title_pair() -> impl Strategy<Value = (Title, Title)> {
    (arb_title(), "[a-zA-Z ]{1,12}", arb_title_kind()).prop_map(|(old, name, kind)| {
        let new = Title {
            name,
            kind,
            ..old.clone()
        };
        (old, new)
    })
}

fn card_with_titles(map: BTreeMap<String, Title>) -> ContactCard {
    ContactCard {
        titles: (!map.is_empty()).then_some(map),
        ..ContactCard::default()
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
    (arb_uri(), arb_link_kind()).prop_map(|(uri, kind)| Link {
        uri,
        kind,
        ..Link::default()
    })
}

fn arb_link_pair() -> impl Strategy<Value = (Link, Link)> {
    (arb_link(), arb_uri(), arb_link_kind()).prop_map(|(old, uri, kind)| {
        let new = Link {
            uri,
            kind,
            ..old.clone()
        };
        (old, new)
    })
}

fn card_with_links(map: BTreeMap<String, Link>) -> ContactCard {
    ContactCard {
        links: (!map.is_empty()).then_some(map),
        ..ContactCard::default()
    }
}

fn arb_calendar_kind() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(Some("calendar".to_owned())),
        Just(Some("freeBusy".to_owned())),
    ]
}

fn arb_calendar() -> impl Strategy<Value = Calendar> {
    (arb_uri(), arb_calendar_kind()).prop_map(|(uri, kind)| Calendar {
        uri,
        kind,
        ..Calendar::default()
    })
}

fn arb_calendar_pair() -> impl Strategy<Value = (Calendar, Calendar)> {
    (arb_calendar(), arb_uri(), arb_calendar_kind()).prop_map(|(old, uri, kind)| {
        let new = Calendar {
            uri,
            kind,
            ..old.clone()
        };
        (old, new)
    })
}

fn card_with_calendars(map: BTreeMap<String, Calendar>) -> ContactCard {
    ContactCard {
        calendars: (!map.is_empty()).then_some(map),
        ..ContactCard::default()
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
    ContactCard {
        keywords: (!map.is_empty()).then_some(map),
        ..ContactCard::default()
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
    arb_org_name().prop_map(|name| Organization {
        name: Some(name),
        ..Organization::default()
    })
}

fn arb_organization_name_pair() -> impl Strategy<Value = (Organization, Organization)> {
    (arb_organization_with_name(), arb_org_name()).prop_map(|(old, name)| {
        let new = Organization {
            name: Some(name),
            ..Organization::default()
        };
        (old, new)
    })
}

fn card_with_organizations(map: BTreeMap<String, Organization>) -> ContactCard {
    ContactCard {
        organizations: (!map.is_empty()).then_some(map),
        ..ContactCard::default()
    }
}

fn arb_sort_as() -> impl Strategy<Value = Option<String>> {
    prop_oneof![Just(None), "[a-zA-Z]{1,6}".prop_map(Some)]
}

fn bare_unit(name: String) -> OrgUnit {
    OrgUnit {
        name,
        sort_as: None,
        extra: BTreeMap::new(),
    }
}

/// A `(current, edited)` units list pair. `kept` units carry the same name
/// and `sortAs` on both sides (a real vCard round trip can only touch a
/// unit's name, so an unchanged name never comes back with a different
/// `sortAs`); `removed` names are current-only; `added` names are
/// edited-only and bare, matching a freshly-parsed unit. No name collides
/// across the three groups, so `merge_units` resolves every edited unit by
/// an exact name match or not at all, never by its position fallback.
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
                let unit = OrgUnit {
                    name: format!("k{index}"),
                    sort_as,
                    extra: BTreeMap::new(),
                };
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
    Organization {
        name: Some(UNITS_ORG_NAME.to_owned()),
        units: (!units.is_empty()).then_some(units),
        ..Organization::default()
    }
}

fn arb_organization_units_stable() -> impl Strategy<Value = Organization> {
    proptest::collection::vec(arb_sort_as(), 0..=2).prop_map(|sort_as_list| {
        let units = sort_as_list
            .into_iter()
            .enumerate()
            .map(|(index, sort_as)| OrgUnit {
                name: format!("u{index}"),
                sort_as,
                extra: BTreeMap::new(),
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

/// One `kind`'s slot across (current, edited): absent on both sides, present
/// only on the edited side (a genuine addition), present only on current
/// (removed), or present on both with independently-chosen dates (a
/// modification, possibly a no-op one).
#[derive(Clone, Debug)]
enum AnniversarySlot {
    Absent,
    Added(Value),
    Removed(Value),
    Modified(Value, Value),
}

/// A `PartialDate` (RFC 9553 §2.8.1) safely inside the range a vCard 3.0 date
/// line can state and read back exactly: year 1000-9999 keeps clear of
/// `Day::survives_the_field_it_lands_in`'s clamp, already covered by
/// `eds-sys/tests/contacts.rs`, and day 1-28 keeps every month valid without
/// having to model which months are short.
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
    ContactCard {
        anniversaries: (!map.is_empty()).then_some(map),
        ..ContactCard::default()
    }
}

/// Builds (current, edited, expected) for the two kinds a vCard anniversary
/// can be (`birth`, `wedding`), each with its own slot. `current` and
/// `edited` always use different keys per kind (`c-<kind>` / `e-<kind>`),
/// modelling that an anniversary's key is never the same one twice; `expected`
/// is what `rekey_keyless` should produce: a `Modified` slot lands under
/// current's key (the pairing this increment exists to check), an `Added`
/// one keeps edited's own key, and a `Removed` one leaves no entry at all.
fn arb_anniversary_cards() -> impl Strategy<Value = (ContactCard, ContactCard, ContactCard)> {
    (arb_anniversary_slot(), arb_anniversary_slot()).prop_map(|(birth, wedding)| {
        let mut current = BTreeMap::new();
        let mut edited = BTreeMap::new();
        let mut expected = BTreeMap::new();
        for (kind, slot) in [("birth", birth), ("wedding", wedding)] {
            let current_key = format!("c-{kind}");
            let edited_key = format!("e-{kind}");
            let entry = |date: Value| Anniversary {
                kind: kind.to_owned(),
                date: Some(date),
                extra: BTreeMap::new(),
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

/// One photo's slot across (current, edited), same shape as
/// `AnniversarySlot`: absent, a genuine addition, a removal, or present on
/// both with independently-chosen URIs (a modification, possibly a no-op
/// one).
#[derive(Clone, Debug)]
enum MediaSlot {
    Absent,
    Added(String),
    Removed(String),
    Modified(String, String),
}

/// A non-`data:` URI, so `same_photo` reduces to comparing this string:
/// `data:` URIs and `mediaType` are left for a later increment.
fn arb_photo_uri() -> impl Strategy<Value = String> {
    "[a-z]{3,8}://[a-z0-9]{1,10}\\.jpg"
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
    ContactCard {
        media: (!map.is_empty()).then_some(map),
        ..ContactCard::default()
    }
}

/// Builds (current, edited, expected) for the one photo Evolution shows and
/// can edit. Like `arb_anniversary_cards`, `current` and `edited` always use
/// different keys (`c-photo` / `e-photo`), modelling that a photo's key is
/// never the same one twice; `expected` is what `rekey_keyless` should
/// produce: a `Modified` slot lands under current's key (the pairing this
/// increment exists to check), an `Added` one keeps edited's own key, and a
/// `Removed` one leaves no entry at all.
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
                current.insert(current_key, entry(uri));
            }
            MediaSlot::Modified(old, new) => {
                current.insert(current_key.clone(), entry(old));
                edited.insert(edited_key, entry(new.clone()));
                expected.insert(current_key, entry(new));
            }
        }
        (
            card_with_media(current),
            card_with_media(edited),
            card_with_media(expected),
        )
    })
}

/// Plain `[a-zA-Z0-9]{1,10}`: non-empty and free of `/?#%` and whitespace, so
/// it is always `plain_handle` (drawable from, and rebuildable into, a URI)
/// and always `states_online_service`-visible.
fn arb_handle() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9]{1,10}"
}

/// Two service names from `SERVICE_SCHEMES`/`ONLINE_SERVICES` with exactly one
/// scheme each, so a handle drawn from or rebuilt into a URI is unambiguous.
const SERVICE_A: &str = "Skype";
const SERVICE_B: &str = "Yahoo";

/// The shape every real `vcard_to_card` parse builds an online-service entry
/// in: a plain `user`, never a `uri`.
fn user_stated(service: &str, handle: String) -> OnlineService {
    OnlineService {
        service: Some(service.to_owned()),
        user: Some(handle),
        uri: None,
        ..OnlineService::default()
    }
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
    ContactCard {
        online_services: (!map.is_empty()).then_some(map),
        ..ContactCard::default()
    }
}

/// The two shapes a server-held entry's handle can be stated in: a plain
/// `user`, or drawn from a `uri` with no `user` of its own (from an earlier
/// whole-value write — never from a `vcard_to_card` parse, which only ever
/// produces the first).
#[derive(Clone, Copy, Debug)]
enum CurrentShape {
    UserStated,
    UriDrawn,
}

fn shaped(shape: CurrentShape, handle: String) -> OnlineService {
    match shape {
        CurrentShape::UserStated => user_stated(SERVICE_A, handle),
        CurrentShape::UriDrawn => OnlineService {
            service: Some(SERVICE_A.to_owned()),
            user: None,
            uri: Some(format!("skype:{handle}")),
            ..OnlineService::default()
        },
    }
}

/// One entry's (current, edited, expected) triple for a handle rename.
/// `edited` is always `user`-shaped, as every real parse builds it; `current`
/// varies. `expected` is what the save should produce: a `user`-shaped
/// current keeps stating the new handle as a `user`, and a `uri`-drawn
/// current keeps the `uri` shape, rebuilt around the new handle — the member
/// the old handle was drawn from is the one the rename goes back on.
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

/// Like [`round_trips`], but against an `expected` card distinct from
/// `edited`: the right check for a property whose key `rekey_keyless`
/// reassigns rather than preserves. See `arb_anniversary_cards`.
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
}
