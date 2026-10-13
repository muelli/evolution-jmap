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
//! `keywords`. `patch.rs`'s own module doc explains why the rest (name
//! components, addresses, organizations, media, anniversaries,
//! online_services, related_to) deliberately keep entries the vCard never
//! showed untouched, or match entries by value rather than by key — those
//! need generators that model that lossiness on purpose, not a card built
//! from entries that are all stateable, and are left for a later increment.
//!
//! Title's `kind` is kept to `Some("title")`/`Some("role")` here rather than
//! also generating `None`: `title_kind()` treats an absent `kind` as
//! "title", and `diff_titles` writes that default back out as the literal
//! string whenever the (defaulted) kind changes, so a round trip that started
//! from `None` lands on `Some("title")` rather than `None` again — a
//! representation difference `title_kind()` calls equivalent but
//! `PartialEq` does not. Restricting the generator to the two explicit
//! spellings exercises the same comparison without that false alarm.

use std::collections::{BTreeMap, BTreeSet};

use jmap_book_sync::patch::diff;
use jmap_proto::contacts::{Calendar, ContactCard, Link, Nickname, Note, Title};
use proptest::prelude::*;
use serde_json::{Map, Value};

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
}
