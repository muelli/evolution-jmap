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
    let components = card.name.as_ref().unwrap().components.as_ref().unwrap();
    assert_eq!(components[0].kind, "given");
    assert_eq!(components[0].value, "Vera");
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
    assert!(address.extra.contains_key("full"));
    // Unmodeled JSContact properties (notes) survive via `extra`.
    assert!(card.extra.contains_key("notes"));
}
