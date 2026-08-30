// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Round-trip tests for the RFC 9610 contact types (JSContact, RFC 9553).

#![cfg(feature = "contacts")]

use jmap_proto::Id;
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
fn addressbook_my_rights_and_share_with_roundtrip() {
    let value = fixture("contacts/addressbook_with_rights.json");
    assert_eq!(roundtrip::<AddressBook>(&value), value);

    let address_book: AddressBook = serde_json::from_value(value).unwrap();
    let rights = address_book.my_rights.expect("myRights");
    assert_eq!(rights.may_read, Some(true));
    assert_eq!(rights.may_write, Some(false));
    assert!(!rights.is_writable());

    let share_with = address_book.share_with.expect("shareWith");
    let shared_rights = &share_with[&Id::new("P1")];
    assert_eq!(shared_rights.may_write, Some(true));
    assert!(shared_rights.is_writable());
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
    assert!(organization.sort_as.is_some() || organization.extra.contains_key("sortAs"));
    assert!(
        organization.units.as_ref().unwrap()[0].sort_as.is_some()
            || organization.units.as_ref().unwrap()[0]
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
    assert!(
        titles["t2"].organization_id.is_some() || titles["t2"].extra.contains_key("organizationId")
    );
    let address = &card.addresses.as_ref().unwrap()["a1"];
    let components = address.components.as_ref().unwrap();
    assert_eq!(components[0].kind, "name");
    assert_eq!(components[0].value, "Hauptstraße");
    assert!(components[0].extra.contains_key("phonetic"));
    assert!(address.country_code.is_some() || address.extra.contains_key("countryCode"));
    assert_eq!(
        address.full.as_deref(),
        Some("Hauptstraße 1\n10115 Berlin\nGermany")
    );
    let notes = card.notes.as_ref().unwrap();
    assert_eq!(notes["n1"].note, "met at FOSDEM");
    assert!(notes["n1"].created.is_some() || notes["n1"].extra.contains_key("created"));
    assert!(notes["n1"].author.is_some() || notes["n1"].extra.contains_key("author"));
    let nicknames = card.nicknames.as_ref().unwrap();
    assert_eq!(nicknames["k1"].name, "Vee");
    assert!(nicknames["k1"].contexts.is_some() || nicknames["k1"].extra.contains_key("contexts"));
    assert!(nicknames["k1"].pref.is_some() || nicknames["k1"].extra.contains_key("pref"));
    let links = card.links.as_ref().unwrap();
    assert_eq!(links["l1"].uri, "https://vera.example/");
    assert_eq!(
        links["l1"].kind, None,
        "RFC 9553 §2.6.3 gives a Link no default kind, and this one names none"
    );
    assert_eq!(links["l2"].kind.as_deref(), Some("contact"));
    assert!(links["l1"].media_type.is_some() || links["l1"].extra.contains_key("mediaType"));
    assert!(links["l1"].pref.is_some() || links["l1"].extra.contains_key("pref"));
    let calendars = card.calendars.as_ref().expect("calendars");
    assert_eq!(calendars["c1"].uri, "https://vera.example/cal/vera.ics");
    assert_eq!(calendars["c1"].kind.as_deref(), Some("calendar"));
    assert_eq!(
        calendars["c2"].kind.as_deref(),
        Some("freeBusy"),
        "the kind is the mapping's filter: it says which of the two lines the URI goes on"
    );
    assert!(
        calendars["c1"].media_type.is_some() || calendars["c1"].extra.contains_key("mediaType")
    );
    assert!(calendars["c1"].pref.is_some() || calendars["c1"].extra.contains_key("pref"));
    let media = card.media.as_ref().expect("media");
    assert_eq!(media["m1"].kind.as_deref(), Some("photo"));
    assert_eq!(media["m1"].uri, "data:image/jpeg;base64,aGVsbG8tcGhvdG8=");
    assert_eq!(media["m1"].media_type.as_deref(), Some("image/jpeg"));
    assert_eq!(
        media["m2"].kind.as_deref(),
        Some("logo"),
        "the kind is the mapping's filter: only a photo is a `PHOTO` line"
    );
    assert!(media["m1"].pref.is_some() || media["m1"].extra.contains_key("pref"));
    let services = card.online_services.as_ref().expect("onlineServices");
    assert_eq!(services["s1"].service.as_deref(), Some("Jabber"));
    assert_eq!(services["s1"].user.as_deref(), Some("vera@jabber.example"));
    assert_eq!(services["s1"].uri, None);
    assert_eq!(services["s2"].user, None);
    assert_eq!(
        services["s2"].uri.as_deref(),
        Some("https://social.example/@vera")
    );
    assert!(services["s1"].contexts.is_some() || services["s1"].extra.contains_key("contexts"));
    assert!(services["s1"].pref.is_some() || services["s1"].extra.contains_key("pref"));
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
    // Modeled JSContact properties (preferredLanguages) are parsed into typed fields.
    assert!(card.preferred_languages.is_some());
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
            "mayRead": true,
            "mayWrite": true,
            "mayShare": false,
            "mayDelete": false
        }
    }))
    .unwrap();

    assert_eq!(book.name, "Shared Book");
    let rights = book.my_rights.as_ref().unwrap();
    assert_eq!(rights.may_read, Some(true));
    assert_eq!(rights.may_write, Some(true));
    assert_eq!(rights.may_share, Some(false));
    assert_eq!(rights.may_delete, Some(false));
    assert!(rights.is_writable());
}

#[test]
fn jscontact_crypto_keys_directories_personal_info_and_groups_roundtrip() {
    use jmap_proto::contacts::{
        CardGroup, ContactCard, CryptoKey, Directory, PersonalInfo, crypto_key_kind,
        directory_kind, personal_info_kind,
    };
    use std::collections::BTreeMap;

    assert_eq!(crypto_key_kind::KEY, "key");
    assert_eq!(crypto_key_kind::CERT, "cert");

    assert_eq!(directory_kind::DIRECTORY, "directory");

    assert_eq!(personal_info_kind::GENDER, "gender");
    assert_eq!(personal_info_kind::EXPERTISE, "expertise");
    assert_eq!(personal_info_kind::HOBBY, "hobby");
    assert_eq!(personal_info_kind::INTEREST, "interest");

    let card = ContactCard {
        id: Some("C1".into()),
        crypto_keys: Some(BTreeMap::from([(
            "k1".to_owned(),
            CryptoKey {
                kind: Some(crypto_key_kind::KEY.to_owned()),
                uri: "https://example.com/pgp.asc".to_owned(),
                media_type: Some("application/pgp-keys".to_owned()),
                pref: Some(1),
                extra: BTreeMap::new(),
            },
        )])),
        directories: Some(BTreeMap::from([(
            "d1".to_owned(),
            Directory {
                kind: Some(directory_kind::DIRECTORY.to_owned()),
                uri: "ldap://ldap.example.com/ou=people".to_owned(),
                media_type: None,
                pref: None,
                extra: BTreeMap::new(),
            },
        )])),
        personal_info: Some(BTreeMap::from([(
            "pi1".to_owned(),
            PersonalInfo {
                kind: personal_info_kind::EXPERTISE.to_owned(),
                value: Some("Rust Programming".to_owned()),
                list_as: Some("Developer".to_owned()),
                extra: BTreeMap::new(),
            },
        )])),
        ..ContactCard::default()
    };

    let json = serde_json::to_value(&card).unwrap();
    assert_eq!(
        json["cryptoKeys"]["k1"]["uri"],
        "https://example.com/pgp.asc"
    );
    assert_eq!(
        json["directories"]["d1"]["uri"],
        "ldap://ldap.example.com/ou=people"
    );
    assert_eq!(json["personalInfo"]["pi1"]["value"], "Rust Programming");

    let round_tripped: ContactCard = serde_json::from_value(json).unwrap();
    assert_eq!(round_tripped, card);

    let group = CardGroup {
        id: Some("G1".into()),
        card_type: Some("Group".to_owned()),
        name: Some("Core Team".to_owned()),
        members: Some(BTreeMap::from([
            ("urn:uuid:c1".to_owned(), true),
            ("urn:uuid:c2".to_owned(), true),
        ])),
        extra: BTreeMap::new(),
    };
    let g_val = serde_json::to_value(&group).unwrap();
    assert_eq!(g_val["@type"], "Group");
    assert_eq!(g_val["name"], "Core Team");
    assert_eq!(g_val["members"]["urn:uuid:c1"], true);

    let g_round_tripped: CardGroup = serde_json::from_value(g_val).unwrap();
    assert_eq!(g_round_tripped, group);
}

/// ContactsCapability, SpeakToAs, LanguagePref, and card kinds cover RFC 9610 §1.3 and RFC 9553 §2.1.1, §2.2.5, §2.8.5.
#[test]
fn contacts_capabilities_speak_to_as_and_languages_roundtrip_covers_rfc9610_rfc9553() {
    use jmap_proto::contacts::{
        ContactCard, ContactsCapability, LanguagePref, SpeakToAs, card_kind, grammatical_gender,
    };
    use std::collections::BTreeMap;

    assert_eq!(grammatical_gender::ANIMATE, "animate");
    assert_eq!(grammatical_gender::INANIMATE, "inanimate");
    assert_eq!(grammatical_gender::FEMININE, "feminine");
    assert_eq!(grammatical_gender::MASCULINE, "masculine");
    assert_eq!(grammatical_gender::NEUTER, "neuter");
    assert_eq!(grammatical_gender::COMMON, "common");

    assert_eq!(card_kind::INDIVIDUAL, "individual");
    assert_eq!(card_kind::GROUP, "group");
    assert_eq!(card_kind::ORG, "org");
    assert_eq!(card_kind::LOCATION, "location");
    assert_eq!(card_kind::DEVICE, "device");
    assert_eq!(card_kind::APPLICATION, "application");

    let cap = ContactsCapability {
        max_size_attachments_per_card: 10_000_000,
        max_number_of_cards_in_set: 500,
        extra: BTreeMap::new(),
    };
    let cap_val = serde_json::to_value(&cap).unwrap();
    assert_eq!(cap_val["maxSizeAttachmentsPerCard"], 10_000_000);
    assert_eq!(cap_val["maxNumberOfCardsInSet"], 500);

    let round_cap: ContactsCapability = serde_json::from_value(cap_val).unwrap();
    assert_eq!(round_cap, cap);

    let speak = SpeakToAs {
        grammatical_gender: Some(grammatical_gender::NEUTER.to_owned()),
        pronouns: Some("they/them".to_owned()),
        extra: BTreeMap::new(),
    };
    let s_val = serde_json::to_value(&speak).unwrap();
    assert_eq!(s_val["grammaticalGender"], "neuter");
    assert_eq!(s_val["pronouns"], "they/them");
    let round_speak: SpeakToAs = serde_json::from_value(s_val).unwrap();
    assert_eq!(round_speak, speak);

    let lang = LanguagePref {
        language: "en-US".to_owned(),
        contexts: Some(serde_json::json!({"work": true})),
        pref: Some(1),
        extra: BTreeMap::new(),
    };
    let l_val = serde_json::to_value(&lang).unwrap();
    assert_eq!(l_val["language"], "en-US");
    assert_eq!(l_val["pref"], 1);
    let round_lang: LanguagePref = serde_json::from_value(l_val).unwrap();
    assert_eq!(round_lang, lang);

    let card = ContactCard {
        id: Some("C1".into()),
        card_type: Some("Card".to_owned()),
        kind: Some(card_kind::INDIVIDUAL.to_owned()),
        ..ContactCard::default()
    };
    let c_val = serde_json::to_value(&card).unwrap();
    assert_eq!(c_val["@type"], "Card");
    assert_eq!(c_val["kind"], "individual");

    let round_card: ContactCard = serde_json::from_value(c_val).unwrap();
    assert_eq!(round_card, card);
}

#[test]
fn address_book_sharing_rights_and_contact_card_extensions_roundtrip() {
    use jmap_proto::contacts::{
        AddressBook, AddressBookRights, ContactCard, ContactCardQueryFilter, LanguagePref,
        SpeakToAs, grammatical_gender,
    };
    use std::collections::BTreeMap;

    let rights = AddressBookRights {
        may_read: Some(true),
        may_write: Some(true),
        may_share: Some(true),
        may_delete: Some(false),
        extra: BTreeMap::new(),
    };

    let book = AddressBook {
        id: Some("ab1".into()),
        name: "Shared Team Contacts".to_owned(),
        share_with: Some(BTreeMap::from([("usr_alice".into(), rights.clone())])),
        my_rights: Some(rights.clone()),
        ..AddressBook::default()
    };

    let b_val = serde_json::to_value(&book).unwrap();
    assert_eq!(b_val["shareWith"]["usr_alice"]["mayRead"], true);
    assert_eq!(b_val["myRights"]["mayWrite"], true);

    let round_book: AddressBook = serde_json::from_value(b_val).unwrap();
    assert_eq!(round_book, book);

    let card = ContactCard {
        id: Some("card_ext".into()),
        speak_to_as: Some(SpeakToAs {
            grammatical_gender: Some(grammatical_gender::FEMININE.to_owned()),
            pronouns: Some("she/her".to_owned()),
            extra: BTreeMap::new(),
        }),
        preferred_languages: Some(BTreeMap::from([(
            "en".to_owned(),
            LanguagePref {
                language: "en".to_owned(),
                contexts: None,
                pref: Some(1),
                extra: BTreeMap::new(),
            },
        )])),
        localizations: Some(BTreeMap::from([(
            "de".to_owned(),
            serde_json::json!({"name/full": "Erika Mustermann"}),
        )])),
        ..ContactCard::default()
    };

    let c_val = serde_json::to_value(&card).unwrap();
    assert_eq!(c_val["speakToAs"]["grammaticalGender"], "feminine");
    assert_eq!(c_val["preferredLanguages"]["en"]["language"], "en");
    assert_eq!(
        c_val["localizations"]["de"]["name/full"],
        "Erika Mustermann"
    );

    let round_card: ContactCard = serde_json::from_value(c_val).unwrap();
    assert_eq!(round_card, card);

    let filter = ContactCardQueryFilter::in_address_book("ab1")
        .uid("urn:uuid:123")
        .name("Erika")
        .email("erika@example.com")
        .phone("+49123456")
        .text("Mustermann");

    assert_eq!(filter.in_address_book.as_ref().unwrap().as_str(), "ab1");
    assert_eq!(filter.uid.as_deref(), Some("urn:uuid:123"));
    assert_eq!(filter.name.as_deref(), Some("Erika"));
    assert_eq!(filter.email.as_deref(), Some("erika@example.com"));
    assert_eq!(filter.phone.as_deref(), Some("+49123456"));
    assert_eq!(filter.text.as_deref(), Some("Mustermann"));
}

#[test]
fn jscontact_spec_properties_roundtrip() {
    use jmap_proto::contacts::{
        Address, AddressBook, Calendar, ContactCard, Link, Media, Name, Nickname, Note,
        OnlineService, OrgUnit, Organization, Title,
    };
    use jmap_proto::state::UtcDate;
    use std::collections::BTreeMap;

    let book = AddressBook {
        id: Some("ab_custom".into()),
        name: "Custom Book".to_owned(),
        may_delete: Some(true),
        ..AddressBook::default()
    };
    let b_json = serde_json::to_value(&book).unwrap();
    assert_eq!(b_json["mayDelete"], true);
    assert_eq!(serde_json::from_value::<AddressBook>(b_json).unwrap(), book);

    let created_date = UtcDate::new("2026-08-29T12:00:00Z");
    let updated_date = UtcDate::new("2026-08-29T12:30:00Z");

    let card = ContactCard {
        id: Some("c_full_spec".into()),
        created: Some(created_date.clone()),
        updated: Some(updated_date.clone()),
        name: Some(Name {
            full: Some("Dr. Ada Lovelace".to_owned()),
            is_ordered: Some(true),
            sort_as: Some(BTreeMap::from([(
                "en".to_owned(),
                "Lovelace, Ada".to_owned(),
            )])),
            ..Name::default()
        }),
        nicknames: Some(BTreeMap::from([(
            "k1".to_owned(),
            Nickname {
                name: "Enchantress of Numbers".to_owned(),
                contexts: Some(serde_json::json!({"private": true})),
                pref: Some(1),
                ..Nickname::default()
            },
        )])),
        organizations: Some(BTreeMap::from([(
            "o1".to_owned(),
            Organization {
                name: Some("Analytical Engine Corp".to_owned()),
                sort_as: Some("Analytical".to_owned()),
                contexts: Some(serde_json::json!({"work": true})),
                units: Some(vec![OrgUnit {
                    name: "Computing".to_owned(),
                    sort_as: Some("Comp".to_owned()),
                    ..OrgUnit::default()
                }]),
                ..Organization::default()
            },
        )])),
        titles: Some(BTreeMap::from([(
            "t1".to_owned(),
            Title {
                name: "Chief Mathematician".to_owned(),
                kind: Some("title".to_owned()),
                organization_id: Some("o1".to_owned()),
                ..Title::default()
            },
        )])),
        addresses: Some(BTreeMap::from([(
            "a1".to_owned(),
            Address {
                full: Some("12 St James's Square\nLondon\nUK".to_owned()),
                is_ordered: Some(true),
                country_code: Some("GB".to_owned()),
                coordinates: Some("geo:51.5074,-0.1364".to_owned()),
                time_zone: Some("Europe/London".to_owned()),
                pref: Some(1),
                ..Address::default()
            },
        )])),
        notes: Some(BTreeMap::from([(
            "n1".to_owned(),
            Note {
                note: "First computer programmer".to_owned(),
                created: Some(created_date.clone()),
                author: Some(serde_json::json!("Charles Babbage")),
                ..Note::default()
            },
        )])),
        links: Some(BTreeMap::from([(
            "l1".to_owned(),
            Link {
                uri: "https://adalovelace.example.org".to_owned(),
                kind: Some("website".to_owned()),
                contexts: Some(serde_json::json!({"work": true})),
                media_type: Some("text/html".to_owned()),
                pref: Some(1),
                label: Some("Personal Website".to_owned()),
                ..Link::default()
            },
        )])),
        calendars: Some(BTreeMap::from([(
            "c1".to_owned(),
            Calendar {
                uri: "https://cal.example.org/ada.ics".to_owned(),
                kind: Some("calendar".to_owned()),
                contexts: Some(serde_json::json!({"work": true})),
                media_type: Some("text/calendar".to_owned()),
                pref: Some(1),
                label: Some("Primary Schedule".to_owned()),
                ..Calendar::default()
            },
        )])),
        media: Some(BTreeMap::from([(
            "m1".to_owned(),
            Media {
                uri: "https://example.org/portrait.jpg".to_owned(),
                kind: Some("photo".to_owned()),
                media_type: Some("image/jpeg".to_owned()),
                contexts: Some(serde_json::json!({"work": true})),
                pref: Some(1),
                label: Some("1840 Portrait".to_owned()),
                ..Media::default()
            },
        )])),
        online_services: Some(BTreeMap::from([(
            "s1".to_owned(),
            OnlineService {
                service: Some("Matrix".to_owned()),
                user: Some("@ada:example.org".to_owned()),
                uri: Some("matrix:@ada:example.org".to_owned()),
                contexts: Some(serde_json::json!({"work": true})),
                pref: Some(1),
                label: Some("Matrix Handle".to_owned()),
                ..OnlineService::default()
            },
        )])),
        ..ContactCard::default()
    };

    let c_json = serde_json::to_value(&card).unwrap();
    assert_eq!(c_json["created"], "2026-08-29T12:00:00Z");
    assert_eq!(c_json["updated"], "2026-08-29T12:30:00Z");
    assert_eq!(c_json["name"]["isOrdered"], true);
    assert_eq!(c_json["name"]["sortAs"]["en"], "Lovelace, Ada");
    assert_eq!(c_json["nicknames"]["k1"]["pref"], 1);
    assert_eq!(c_json["organizations"]["o1"]["sortAs"], "Analytical");
    assert_eq!(c_json["organizations"]["o1"]["units"][0]["sortAs"], "Comp");
    assert_eq!(c_json["titles"]["t1"]["organizationId"], "o1");
    assert_eq!(c_json["addresses"]["a1"]["countryCode"], "GB");
    assert_eq!(c_json["addresses"]["a1"]["timeZone"], "Europe/London");
    assert_eq!(
        c_json["addresses"]["a1"]["coordinates"],
        "geo:51.5074,-0.1364"
    );
    assert_eq!(c_json["notes"]["n1"]["author"], "Charles Babbage");
    assert_eq!(c_json["links"]["l1"]["mediaType"], "text/html");
    assert_eq!(c_json["links"]["l1"]["label"], "Personal Website");
    assert_eq!(c_json["calendars"]["c1"]["mediaType"], "text/calendar");
    assert_eq!(c_json["calendars"]["c1"]["label"], "Primary Schedule");
    assert_eq!(c_json["media"]["m1"]["label"], "1840 Portrait");
    assert_eq!(c_json["onlineServices"]["s1"]["service"], "Matrix");
    assert_eq!(c_json["onlineServices"]["s1"]["label"], "Matrix Handle");

    let round_card: ContactCard = serde_json::from_value(c_json).unwrap();
    assert_eq!(round_card, card);
}

#[test]
fn contact_card_parse_and_set_error_roundtrip_covers_rfc9610() {
    use jmap_proto::contacts::{
        ContactCard, ContactCardParseRequest, ContactCardParseResponse, contact_card_set_error,
    };
    use std::collections::BTreeMap;

    assert_eq!(contact_card_set_error::BLOB_NOT_FOUND, "blobNotFound");

    let parse_req =
        ContactCardParseRequest::new("A1", ["blob1", "blob2"]).properties(["id", "name", "emails"]);
    let req_json = serde_json::to_value(&parse_req).unwrap();
    assert_eq!(req_json["accountId"], "A1");
    assert_eq!(req_json["blobIds"], serde_json::json!(["blob1", "blob2"]));
    assert_eq!(
        req_json["properties"],
        serde_json::json!(["id", "name", "emails"])
    );
    assert_eq!(
        serde_json::from_value::<ContactCardParseRequest>(req_json).unwrap(),
        parse_req
    );

    let parse_resp = ContactCardParseResponse {
        account_id: "A1".into(),
        parsed: Some(BTreeMap::from([(
            "blob1".into(),
            ContactCard {
                id: Some("C1".into()),
                ..ContactCard::default()
            },
        )])),
        not_parsable: Some(vec!["blob2".into()]),
        not_found: Some(vec!["blob3".into()]),
    };
    let resp_json = serde_json::to_value(&parse_resp).unwrap();
    assert_eq!(resp_json["accountId"], "A1");
    assert_eq!(resp_json["parsed"]["blob1"]["id"], "C1");
    assert_eq!(resp_json["notParsable"], serde_json::json!(["blob2"]));
    assert_eq!(resp_json["notFound"], serde_json::json!(["blob3"]));
    assert_eq!(
        serde_json::from_value::<ContactCardParseResponse>(resp_json).unwrap(),
        parse_resp
    );
}

#[test]
fn address_book_builders_roundtrip() {
    use jmap_proto::contacts::AddressBook;

    let ab = AddressBook::new("Work Contacts")
        .with_description("Primary work colleagues and partners")
        .with_sort_order(5)
        .is_default(true)
        .is_subscribed(true);

    assert_eq!(ab.name, "Work Contacts");
    assert_eq!(
        ab.description.as_deref(),
        Some("Primary work colleagues and partners")
    );
    assert_eq!(ab.sort_order, Some(5));
    assert_eq!(ab.is_default, Some(true));
    assert_eq!(ab.is_subscribed, Some(true));

    let ab_val = serde_json::to_value(&ab).unwrap();
    assert_eq!(ab_val["name"], "Work Contacts");
    assert_eq!(
        ab_val["description"],
        "Primary work colleagues and partners"
    );
    assert_eq!(ab_val["sortOrder"], 5);
    assert_eq!(ab_val["isDefault"], true);
    assert_eq!(ab_val["isSubscribed"], true);
    assert_eq!(serde_json::from_value::<AddressBook>(ab_val).unwrap(), ab);
}

#[test]
fn contact_card_parse_response_and_domain_builders() {
    use jmap_proto::UtcDate;
    use jmap_proto::contacts::{
        Address, AddressBookRights, AddressComponent, Anniversary, Calendar, CardGroup,
        ContactCard, ContactCardParseResponse, ContactEmail, ContactPhone, CryptoKey, Directory,
        LanguagePref, Link, Media, Name, NameComponent, Note, OnlineService, OrgUnit, Organization,
        PersonalInfo, Relation, SpeakToAs, Title,
    };
    use std::collections::BTreeMap;

    let parse_resp = ContactCardParseResponse::new("acc1")
        .with_parsed(BTreeMap::from([(
            "b1".into(),
            ContactCard::simple("ab1", "Alice", "alice@example.com"),
        )]))
        .with_not_parsable(["b2"])
        .with_not_found(["b3"]);

    assert_eq!(parse_resp.account_id.as_str(), "acc1");
    assert_eq!(parse_resp.parsed.as_ref().unwrap().len(), 1);
    assert_eq!(parse_resp.not_parsable.as_ref().unwrap().len(), 1);
    assert_eq!(parse_resp.not_found.as_ref().unwrap().len(), 1);

    let rights_all = AddressBookRights::all();
    assert_eq!(rights_all.may_read, Some(true));
    assert_eq!(rights_all.may_write, Some(true));
    assert_eq!(rights_all.may_share, Some(true));
    assert_eq!(rights_all.may_delete, Some(true));
    assert!(rights_all.is_writable());

    let rights_ro = AddressBookRights::read_only();
    assert_eq!(rights_ro.may_read, Some(true));
    assert_eq!(rights_ro.may_write, None);
    assert!(!rights_ro.is_writable());

    let name = Name::new("Alice Smith")
        .with_components([
            NameComponent::new("given", "Alice"),
            NameComponent::new("surname", "Smith"),
        ])
        .is_ordered(true)
        .with_sort_as(BTreeMap::from([(
            "surname".to_string(),
            "Smith".to_string(),
        )]));
    assert_eq!(name.full.as_deref(), Some("Alice Smith"));
    assert_eq!(name.components.as_ref().unwrap().len(), 2);
    assert_eq!(name.is_ordered, Some(true));

    let addr = Address::new()
        .with_full("123 Main St")
        .with_components([AddressComponent::new("street", "123 Main St")])
        .with_country_code("US")
        .with_time_zone("America/New_York")
        .with_coordinates("geo:40.7128,-74.0060")
        .with_pref(1)
        .with_contexts(serde_json::json!({"work": true}));
    assert_eq!(addr.full.as_deref(), Some("123 Main St"));
    assert_eq!(addr.country_code.as_deref(), Some("US"));
    assert_eq!(addr.pref, Some(1));

    let email = ContactEmail::new("alice@example.com")
        .with_contexts(serde_json::json!({"work": true}))
        .with_pref(1)
        .with_label("Direct");
    assert_eq!(email.address, "alice@example.com");
    assert_eq!(email.pref, Some(1));
    assert_eq!(
        email.extra.get("label").and_then(|v| v.as_str()),
        Some("Direct")
    );

    let phone = ContactPhone::new("+123456789")
        .with_contexts(serde_json::json!({"work": true}))
        .with_features(serde_json::json!({"voice": true}))
        .with_pref(1)
        .with_label("Mobile");
    assert_eq!(phone.number, "+123456789");
    assert_eq!(phone.pref, Some(1));
    assert_eq!(
        phone.extra.get("label").and_then(|v| v.as_str()),
        Some("Mobile")
    );

    let org = Organization::new("Acme Corp")
        .with_units([OrgUnit::new("Engineering").with_sort_as("Eng")])
        .with_sort_as("Acme");
    assert_eq!(org.name.as_deref(), Some("Acme Corp"));
    assert_eq!(org.units.as_ref().unwrap().len(), 1);

    let title = Title::new("Software Engineer")
        .with_kind("title")
        .with_organization_id("org1");
    assert_eq!(title.name, "Software Engineer");
    assert_eq!(title.kind.as_deref(), Some("title"));

    let note = Note::new("Met at conference").with_created(UtcDate::new("2026-08-01T10:00:00Z"));
    assert_eq!(note.note, "Met at conference");
    assert_eq!(
        note.created.as_ref().unwrap().as_str(),
        "2026-08-01T10:00:00Z"
    );

    let ann = Anniversary::new("birth")
        .with_date(serde_json::json!({"year": 1990, "month": 5, "day": 12}))
        .with_place(Address::new().with_full("Boston, MA"));
    assert_eq!(ann.kind, "birth");
    assert_eq!(ann.extra.get("place").unwrap()["full"], "Boston, MA");

    let link = Link::new("https://example.com")
        .with_kind("profile")
        .with_pref(1)
        .with_label("Homepage");
    assert_eq!(link.uri, "https://example.com");
    assert_eq!(link.pref, Some(1));

    let cal = Calendar::new("https://cal.example.com")
        .with_kind("calendar")
        .with_pref(1)
        .with_label("Calendar");
    assert_eq!(cal.uri, "https://cal.example.com");

    let media = Media::new("https://img.example.com/avatar.jpg")
        .with_kind("photo")
        .with_media_type("image/jpeg")
        .with_pref(1)
        .with_label("Avatar");
    assert_eq!(media.uri, "https://img.example.com/avatar.jpg");

    let online = OnlineService::new()
        .with_service("matrix")
        .with_uri("matrix:u/alice:example.com")
        .with_user("@alice:example.com")
        .with_pref(1)
        .with_label("Chat");
    assert_eq!(online.service.as_deref(), Some("matrix"));

    let rel = Relation::new().with_relation(BTreeMap::from([("spouse".to_string(), true)]));
    assert!(rel.relation.as_ref().unwrap().contains_key("spouse"));

    let crypto = CryptoKey::new("key", "https://example.com/key.asc")
        .with_media_type("application/pgp-keys")
        .with_pref(1);
    assert_eq!(crypto.kind.as_deref(), Some("key"));

    let dir = Directory::new("directory", "https://ldap.example.com")
        .with_media_type("text/directory")
        .with_pref(1);
    assert_eq!(dir.kind.as_deref(), Some("directory"));

    let info = PersonalInfo::new("hobby", "Photography").with_list_as("Photo");
    assert_eq!(info.kind, "hobby");

    let group =
        CardGroup::new("Team Alpha").with_members(BTreeMap::from([("c1".to_string(), true)]));
    assert_eq!(group.name.as_deref(), Some("Team Alpha"));

    let speak = SpeakToAs::new()
        .with_grammatical_gender("feminine")
        .with_pronouns("she/her");
    assert_eq!(speak.grammatical_gender.as_deref(), Some("feminine"));

    let lang = LanguagePref::new("en-US")
        .with_contexts(serde_json::json!({"work": true}))
        .with_pref(1);
    assert_eq!(lang.language, "en-US");
}

#[test]
fn contact_card_and_capability_builders() {
    use jmap_proto::contacts::{
        Address, ContactCard, ContactEmail, ContactPhone, ContactsCapability, Name, Note,
        Organization, SpeakToAs, Title,
    };
    use jmap_proto::{Id, UtcDate};

    let cap = ContactsCapability::new()
        .with_max_size_attachments_per_card(25_000_000)
        .with_max_number_of_cards_in_set(500);
    assert_eq!(cap.max_size_attachments_per_card, 25_000_000);
    assert_eq!(cap.max_number_of_cards_in_set, 500);

    let card = ContactCard::default()
        .with_id("card_100")
        .with_address_book_id("ab_main")
        .with_uid("urn:uuid:1234-5678")
        .with_name(Name::new("Alice Smith"))
        .with_kind("individual")
        .with_email("e1", ContactEmail::new("alice@example.com"))
        .with_phone("p1", ContactPhone::new("+15551234567"))
        .with_address("a1", Address::new().with_full("123 Main St, Springfield"))
        .with_organization("o1", Organization::new("Acme Corp"))
        .with_title("t1", Title::new("Lead Engineer"))
        .with_note("n1", Note::new("Met at conference"))
        .with_created(UtcDate::new("2026-08-20T09:00:00Z"))
        .with_updated(UtcDate::new("2026-08-29T11:00:00Z"))
        .with_speak_to_as(
            SpeakToAs::new()
                .with_grammatical_gender("feminine")
                .with_pronouns("she/her"),
        );

    assert_eq!(card.id.as_ref().unwrap().as_str(), "card_100");
    assert!(
        card.address_book_ids
            .as_ref()
            .unwrap()
            .contains_key(&Id::new("ab_main"))
    );
    assert_eq!(card.uid.as_deref(), Some("urn:uuid:1234-5678"));
    assert_eq!(
        card.name.as_ref().unwrap().full.as_deref(),
        Some("Alice Smith")
    );
    assert_eq!(card.kind.as_deref(), Some("individual"));
    assert_eq!(card.emails.as_ref().unwrap().len(), 1);
    assert_eq!(card.phones.as_ref().unwrap().len(), 1);
    assert_eq!(card.addresses.as_ref().unwrap().len(), 1);
    assert_eq!(card.organizations.as_ref().unwrap().len(), 1);
    assert_eq!(card.titles.as_ref().unwrap().len(), 1);
    assert_eq!(card.notes.as_ref().unwrap().len(), 1);
    assert_eq!(
        card.created.as_ref().unwrap().as_str(),
        "2026-08-20T09:00:00Z"
    );
    assert_eq!(
        card.updated.as_ref().unwrap().as_str(),
        "2026-08-29T11:00:00Z"
    );
    assert_eq!(
        card.speak_to_as.as_ref().unwrap().pronouns.as_deref(),
        Some("she/her")
    );
}
