// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Round-trip tests for the RFC 9610 contact types (JSContact, RFC 9553).

#![cfg(feature = "contacts")]

use jmap_proto::contacts::{AddressBook, ContactCard, ContactCardQueryFilter};
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
    let calendars = card.calendars.as_ref().expect("calendars");
    assert_eq!(calendars["c1"].uri, "https://vera.example/cal/vera.ics");
    assert_eq!(calendars["c1"].kind.as_deref(), Some("calendar"));
    assert_eq!(
        calendars["c2"].kind.as_deref(),
        Some("freeBusy"),
        "the kind is the mapping's filter: it says which of the two lines the URI goes on"
    );
    // What the resource is and how strongly it is preferred have no parameter
    // on a `CALURI` line, so they too stay visible to the save path.
    assert!(calendars["c1"].extra.contains_key("mediaType"));
    assert!(calendars["c1"].extra.contains_key("pref"));
    let media = card.media.as_ref().expect("media");
    assert_eq!(media["m1"].kind.as_deref(), Some("photo"));
    assert_eq!(media["m1"].uri, "data:image/jpeg;base64,aGVsbG8tcGhvdG8=");
    assert_eq!(media["m1"].media_type.as_deref(), Some("image/jpeg"));
    assert_eq!(
        media["m2"].kind.as_deref(),
        Some("logo"),
        "the kind is the mapping's filter: only a photo is a `PHOTO` line"
    );
    // How strongly a picture is preferred, and what to call it, have no
    // parameter on a `PHOTO` line, so they stay visible to the save path.
    assert!(media["m1"].extra.contains_key("pref"));
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
    // `relatedTo` is the one map keyed by the related entity rather than by an
    // id of whoever wrote the entry: RFC 9553 §2.1.8 makes the key the related
    // Card's `uid`, and RFC 9555 §2.9.5 puts free text there where the vCard
    // stated `RELATED;VALUE=text` — which is the only case holding a name a
    // user could read.
    let related = card.related_to.as_ref().expect("relatedTo");
    let spouse = &related["Jean Paul Oldenburg"];
    let relation = spouse.relation.as_ref().expect("relation");
    assert_eq!(relation.keys().collect::<Vec<_>>(), ["kin", "spouse"]);
    assert!(relation.values().all(|set| set == &Value::Bool(true)));
    // An entity related some other way, named the way §2.1.8 asks for: nothing
    // on the card says who they are, so no line can show them.
    assert_eq!(
        related["urn:uuid:e1f0a1c2-0f6b-4d2e-9c3a-2b1f9d0e7c44"]
            .relation
            .as_ref()
            .expect("relation")
            .keys()
            .collect::<Vec<_>>(),
        ["colleague"]
    );
    // Unmodeled JSContact properties (preferredLanguages) survive via `extra`.
    assert!(card.extra.contains_key("preferredLanguages"));
}

#[test]
fn contact_card_simple_sets_every_field() {
    let card = ContactCard::simple("AB1", "Alice Example", "alice@example.com");
    let address_book_ids: Vec<_> = card
        .address_book_ids
        .as_ref()
        .unwrap()
        .iter()
        .map(|(id, included)| (id.as_str(), *included))
        .collect();
    assert_eq!(address_book_ids, [("AB1", true)]);
    assert_eq!(card.card_type.as_deref(), Some("Card"));
    assert_eq!(card.version.as_deref(), Some("1.0"));
    assert_eq!(
        card.name.as_ref().unwrap().full.as_deref(),
        Some("Alice Example")
    );
    let emails = card.emails.as_ref().unwrap();
    assert_eq!(emails["e0"].address, "alice@example.com");
}

#[test]
fn contact_card_query_filter_in_address_book_sets_only_that_field() {
    let filter = ContactCardQueryFilter::in_address_book("AB1");
    assert_eq!(filter.in_address_book.as_ref().unwrap().as_str(), "AB1");
    assert_eq!(filter.text, None);
    assert_eq!(filter.name, None);
    assert_eq!(filter.email, None);
}

#[test]
fn address_book_set_error_has_card_code() {
    assert_eq!(
        jmap_proto::contacts::address_book_set_error::HAS_CARD,
        "addressBookHasCard"
    );
}

#[test]
fn contact_card_query_filter_properties_cover_rfc9610() {
    let filter: ContactCardQueryFilter = serde_json::from_value(serde_json::json!({
        "uid": "urn:uuid:1234",
        "phone": "+1234567890",
        "onlineService": "matrix",
        "address": "Berlin",
        "kind": "individual"
    }))
    .unwrap();

    assert_eq!(filter.uid.as_deref(), Some("urn:uuid:1234"));
    assert_eq!(filter.phone.as_deref(), Some("+1234567890"));
    assert_eq!(filter.online_service.as_deref(), Some("matrix"));
    assert_eq!(filter.address.as_deref(), Some("Berlin"));
    assert_eq!(filter.kind.as_deref(), Some("individual"));
}

#[test]
fn contact_constants_cover_rfc9553_kinds() {
    use jmap_proto::contacts::*;
    assert_eq!(anniversary_kind::BIRTH, "birth");
    assert_eq!(anniversary_kind::DEATH, "death");
    assert_eq!(anniversary_kind::WEDDING, "wedding");

    assert_eq!(title_kind::TITLE, "title");
    assert_eq!(title_kind::ROLE, "role");

    assert_eq!(calendar_kind::CALENDAR, "calendar");
    assert_eq!(calendar_kind::FREE_BUSY, "freeBusy");

    assert_eq!(media_kind::PHOTO, "photo");
    assert_eq!(media_kind::SOUND, "sound");
    assert_eq!(media_kind::LOGO, "logo");

    assert_eq!(link_kind::CONTACT, "contact");

    assert_eq!(name_component_kind::PREFIX, "prefix");
    assert_eq!(name_component_kind::GIVEN, "given");
    assert_eq!(name_component_kind::MIDDLE, "middle");
    assert_eq!(name_component_kind::SURNAME, "surname");
    assert_eq!(name_component_kind::SUFFIX, "suffix");

    assert_eq!(address_component_kind::NAME, "name");
    assert_eq!(address_component_kind::UNIT, "unit");
    assert_eq!(address_component_kind::FLOOR, "floor");
    assert_eq!(address_component_kind::STREET, "street");
    assert_eq!(address_component_kind::APPARTMENT, "appartment");
    assert_eq!(address_component_kind::ROOM, "room");
    assert_eq!(address_component_kind::BUILDING, "building");
    assert_eq!(address_component_kind::LOCALITY, "locality");
    assert_eq!(address_component_kind::REGION, "region");
    assert_eq!(address_component_kind::POSTCODE, "postcode");
    assert_eq!(address_component_kind::COUNTRY, "country");
}

#[test]
fn address_book_rights_roundtrip_covers_rfc9610() {
    let book: AddressBook = serde_json::from_value(serde_json::json!({
        "name": "Shared Book",
        "myRights": {
            "mayReadItems": true,
            "mayAddItems": true,
            "mayModifyItems": false,
            "mayRemoveItems": false,
            "mayDelete": false,
            "mayRename": false,
            "mayAdmin": false
        }
    }))
    .unwrap();

    assert_eq!(book.name, "Shared Book");
    let rights_val = book.extra.get("myRights").unwrap();
    let rights: jmap_proto::contacts::AddressBookRights =
        serde_json::from_value(rights_val.clone()).unwrap();
    assert!(rights.may_read_items);
    assert!(rights.may_add_items);
    assert!(!rights.may_modify_items);
    assert!(!rights.may_remove_items);
    assert!(!rights.may_delete);
    assert!(!rights.may_rename);
    assert!(!rights.may_admin);
}
