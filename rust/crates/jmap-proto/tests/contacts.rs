// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Round-trip tests for the RFC 9610 contact types (JSContact, RFC 9553).

#![cfg(feature = "contacts")]

use jmap_proto::contacts::{AddressBook, ContactCard};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn roundtrip<T>(value: &Value) -> Value
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let typed: T = serde_json::from_value(value.clone()).expect("deserialize");
    serde_json::to_value(&typed).expect("serialize")
}

#[test]
fn addressbook_roundtrip() {
    let value = fixture("contacts/addressbook.json");
    assert_eq!(roundtrip::<AddressBook>(&value), value);

    let address_book: AddressBook = serde_json::from_value(value).unwrap();
    assert_eq!(address_book.name, "Personal");
    assert_eq!(address_book.is_default, Some(true));
}

#[test]
fn contact_card_roundtrip() {
    let value = fixture("contacts/contact_card.json");
    assert_eq!(roundtrip::<ContactCard>(&value), value);

    let card: ContactCard = serde_json::from_value(value).unwrap();
    assert_eq!(card.card_type.as_deref(), Some("Card"));
    assert_eq!(
        card.name.as_ref().unwrap().full.as_deref(),
        Some("Vera Oldenburg")
    );
    let name = card.name.as_ref().unwrap();
    let components = name.components.as_ref().unwrap();
    assert_eq!(components[0].kind, "given");
    assert_eq!(components[0].value, "Vera");
    // How a name is pronounced has no field in the vCard `N` value and no
    // parameter beside it, and the save path writes the component list back
    // whole — so, exactly as for an address component, it has to stay visible
    // to the save rather than being deserialized away. The system that
    // spelling is written in sits on the name itself and rides along there.
    assert!(components[0].extra.contains_key("phonetic"));
    assert!(name.extra.contains_key("phoneticSystem"));
    assert_eq!(
        card.emails.as_ref().unwrap()["work"].address,
        "vera@example.com"
    );
    let organization = &card.organizations.as_ref().unwrap()["o1"];
    assert_eq!(organization.name.as_deref(), Some("Example GmbH"));
    assert_eq!(organization.units.as_ref().unwrap()[0].name, "Research");
    // Members of an organisation the vCard mapping has no room for stay
    // visible to the save path rather than being deserialized away.
    assert!(organization.extra.contains_key("sortAs"));
    assert!(
        organization.units.as_ref().unwrap()[0]
            .extra
            .contains_key("sortAs")
    );
    let titles = card.titles.as_ref().unwrap();
    assert_eq!(titles["t1"].name, "Research Scientist");
    assert_eq!(
        titles["t1"].kind, None,
        "`title` is RFC 9553 §2.2.4's default kind, and the card does not say it"
    );
    assert_eq!(titles["t2"].kind.as_deref(), Some("role"));
    // Which organisation a title is held at has no room on a TITLE line, so
    // it too stays visible to the save path.
    assert!(titles["t2"].extra.contains_key("organizationId"));
    let address = &card.addresses.as_ref().unwrap()["a1"];
    let components = address.components.as_ref().unwrap();
    assert_eq!(components[0].kind, "name");
    assert_eq!(components[0].value, "Hauptstraße");
    // A component member the `ADR` value has no field for, and the address
    // members it has no room for at all: both stay visible to the save path,
    // which writes the component list back whole.
    assert!(components[0].extra.contains_key("phonetic"));
    assert!(address.extra.contains_key("countryCode"));
    // The address written out for an envelope is modeled rather than carried:
    // vCard states it on a `LABEL` line of its own.
    assert_eq!(
        address.full.as_deref(),
        Some("Hauptstraße 1\n10115 Berlin\nGermany")
    );
    let notes = card.notes.as_ref().unwrap();
    assert_eq!(notes["n1"].note, "met at FOSDEM");
    // When a note was written and who wrote it have no room on a `NOTE`
    // line, so they too stay visible to the save path.
    assert!(notes["n1"].extra.contains_key("created"));
    assert!(notes["n1"].extra.contains_key("author"));
    let nicknames = card.nicknames.as_ref().unwrap();
    assert_eq!(nicknames["k1"].name, "Vee");
    // The context a nickname is used in and how strongly it is preferred have
    // no parameter on a `NICKNAME` line, so they too stay visible to the save
    // path.
    assert!(nicknames["k1"].extra.contains_key("contexts"));
    assert!(nicknames["k1"].extra.contains_key("pref"));
    let links = card.links.as_ref().unwrap();
    assert_eq!(links["l1"].uri, "https://vera.example/");
    assert_eq!(
        links["l1"].kind, None,
        "RFC 9553 §2.6.3 gives a Link no default kind, and this one names none"
    );
    assert_eq!(links["l2"].kind.as_deref(), Some("contact"));
    // What a link points at and how strongly it is preferred have no
    // parameter on a `URL` line, so they too stay visible to the save path.
    assert!(links["l1"].extra.contains_key("mediaType"));
    assert!(links["l1"].extra.contains_key("pref"));
    let services = card.online_services.as_ref().expect("onlineServices");
    assert_eq!(services["s1"].service.as_deref(), Some("Jabber"));
    assert_eq!(services["s1"].user.as_deref(), Some("vera@jabber.example"));
    assert_eq!(services["s1"].uri, None);
    // RFC 9553 §2.3.2 asks for the `uri` or the `user`, and only the `user` is
    // a handle: this entry states the other one, and the mapping has to be able
    // to see which.
    assert_eq!(services["s2"].user, None);
    assert_eq!(
        services["s2"].uri.as_deref(),
        Some("https://social.example/@vera")
    );
    // Where a service is used and how strongly it is preferred are not stated
    // by the `X-` line's `TYPE` — that parameter is the slot EDS files the
    // handle in — so they stay visible to the save path.
    assert!(services["s1"].extra.contains_key("contexts"));
    assert!(services["s1"].extra.contains_key("pref"));
    // `keywords` is an RFC 9553 §1.4.3 Set — the keys are the tags and every
    // value is `true`. vCard states the whole set on one `CATEGORIES` line.
    let keywords = card.keywords.as_ref().expect("keywords");
    assert_eq!(keywords.keys().collect::<Vec<_>>(), ["hiking"]);
    assert!(keywords.values().all(|set| set == &Value::Bool(true)));
    // Unmodeled JSContact properties (preferredLanguages) survive via `extra`.
    assert!(card.extra.contains_key("preferredLanguages"));
}
