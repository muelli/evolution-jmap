// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Structure-aware fuzzing of the JSContact ↔ vCard 3.0 mapping using `proptest`.
//!
//! Asserts:
//! 1. `card_to_vcard` never panics on arbitrary `ContactCard` instances.
//! 2. `vcard_to_card` never panics on arbitrary strings or arbitrary vCard envelopes.
//! 3. Round-trip stability: Emitting a card, parsing it back, and re-emitting reaches a fixed point.

use std::collections::BTreeMap;

use jmap_proto::contacts::{
    Address, AddressComponent, Anniversary, Calendar, ContactCard, ContactEmail, ContactPhone,
    Link, Media, Name, NameComponent, Nickname, Note, OnlineService, OrgUnit, Organization,
    Relation, Title,
};
use jmap_vcard::{card_to_vcard, vcard_to_card};
use proptest::prelude::*;
use serde_json::json;

prop_compose! {
    fn arb_name_component()(
        kind in prop_oneof![
            Just("given".to_string()),
            Just("surname".to_string()),
            Just("middle".to_string()),
            Just("prefix".to_string()),
            Just("suffix".to_string()),
            Just("x-custom".to_string()),
            "[a-z]{1,8}",
        ],
        value in "\\PC*",
    ) -> NameComponent {
        NameComponent::new(&kind, &value)
    }
}

prop_compose! {
    fn arb_name()(
        components in prop::option::of(prop::collection::vec(arb_name_component(), 0..6)),
        full in prop::option::of("\\PC*"),
    ) -> Name {
        Name {
            components,
            full,
            extra: BTreeMap::new(),
        }
    }
}

prop_compose! {
    fn arb_nickname()(name in "\\PC*") -> Nickname {
        Nickname {
            name,
            extra: BTreeMap::new(),
        }
    }
}

prop_compose! {
    fn arb_email()(
        address in "\\PC*",
        contexts in prop::option::of(prop_oneof![
            Just(json!("work")),
            Just(json!("home")),
            Just(json!({"work": true})),
            Just(json!({"home": true})),
            Just(json!({"other": true})),
            Just(json!(123)),
        ]),
        pref in prop::option::of(0..100u32),
    ) -> ContactEmail {
        ContactEmail {
            address,
            contexts,
            pref,
            extra: BTreeMap::new(),
        }
    }
}

prop_compose! {
    fn arb_phone()(
        number in "\\PC*",
        features in prop::option::of(prop_oneof![
            Just(json!({"voice": true})),
            Just(json!({"fax": true})),
            Just(json!({"mobile": true})),
            Just(json!({"cell": true})),
            Just(json!({"pager": true})),
            Just(json!({"video": true})),
            Just(json!({"voice": true, "fax": true})),
            Just(json!("voice")),
            Just(json!(42)),
        ]),
        contexts in prop::option::of(prop_oneof![
            Just(json!("work")),
            Just(json!("home")),
            Just(json!({"work": true})),
            Just(json!({"home": true})),
        ]),
        pref in prop::option::of(0..100u32),
    ) -> ContactPhone {
        ContactPhone {
            number,
            features,
            contexts,
            pref,
            extra: BTreeMap::new(),
        }
    }
}

prop_compose! {
    fn arb_org_unit()(name in "\\PC*") -> OrgUnit {
        OrgUnit::new(&name)
    }
}

prop_compose! {
    fn arb_organization()(
        name in prop::option::of("\\PC*"),
        units in prop::option::of(prop::collection::vec(arb_org_unit(), 0..6)),
    ) -> Organization {
        Organization {
            name,
            units,
            extra: BTreeMap::new(),
        }
    }
}

prop_compose! {
    fn arb_title()(
        name in "\\PC*",
        kind in prop::option::of(prop_oneof![
            Just("title".to_string()),
            Just("role".to_string()),
            Just("x-honour".to_string()),
            "[a-z]{1,6}",
        ]),
    ) -> Title {
        Title {
            name,
            kind,
            extra: BTreeMap::new(),
        }
    }
}

prop_compose! {
    fn arb_address_component()(
        kind in prop_oneof![
            Just("name".to_string()),
            Just("locality".to_string()),
            Just("region".to_string()),
            Just("postcode".to_string()),
            Just("country".to_string()),
            Just("postOfficeBox".to_string()),
            Just("extendedAddress".to_string()),
            Just("floor".to_string()),
            Just("room".to_string()),
            "[a-zA-Z]{1,10}",
        ],
        value in "\\PC*",
    ) -> AddressComponent {
        AddressComponent::new(&kind, &value)
    }
}

prop_compose! {
    fn arb_address()(
        components in prop::option::of(prop::collection::vec(arb_address_component(), 0..8)),
        full in prop::option::of("\\PC*"),
        contexts in prop::option::of(prop_oneof![
            Just(json!("work")),
            Just(json!("home")),
            Just(json!({"work": true})),
            Just(json!({"home": true})),
        ]),
    ) -> Address {
        Address {
            components,
            contexts,
            full,
            extra: BTreeMap::new(),
        }
    }
}

prop_compose! {
    fn arb_note()(note in "\\PC*") -> Note {
        Note {
            note,
            extra: BTreeMap::new(),
        }
    }
}

prop_compose! {
    fn arb_anniversary()(
        kind in prop_oneof![
            Just("birth".to_string()),
            Just("wedding".to_string()),
            Just("death".to_string()),
            "[a-z]{1,8}",
        ],
        date in prop::option::of(prop_oneof![
            Just(json!({"@type": "PartialDate", "year": 1990, "month": 5, "day": 12})),
            Just(json!({"@type": "PartialDate", "year": 2000})),
            Just(json!({"@type": "PartialDate", "month": 11, "day": 3})),
            Just(json!({"@type": "PartialDate", "year": 1984, "month": 6})),
            Just(json!({"@type": "PartialDate", "year": 1984, "calendarScale": "hebrew"})),
            Just(json!({"@type": "Timestamp", "utc": "2020-01-01T12:00:00Z"})),
            Just(json!("1990-05-12")),
            Just(json!("2026-08-19T00:00:00Z")),
            Just(json!(1990)),
            Just(json!(null)),
        ]),
    ) -> Anniversary {
        Anniversary {
            kind,
            date,
            extra: BTreeMap::new(),
        }
    }
}

prop_compose! {
    fn arb_link()(
        uri in "\\PC*",
        kind in prop::option::of(prop_oneof![
            Just("contact".to_string()),
            Just("website".to_string()),
            "[a-z]{1,8}",
        ]),
    ) -> Link {
        Link {
            uri,
            kind,
            extra: BTreeMap::new(),
        }
    }
}

prop_compose! {
    fn arb_calendar()(
        uri in "\\PC*",
        kind in prop::option::of(prop_oneof![
            Just("calendar".to_string()),
            Just("freeBusy".to_string()),
            "[a-z]{1,8}",
        ]),
    ) -> Calendar {
        Calendar {
            kind,
            uri,
            extra: BTreeMap::new(),
        }
    }
}

prop_compose! {
    fn arb_media()(
        uri in prop_oneof![
            Just("data:image/jpeg;base64,/9j/4AAQSkZJRg==".to_string()),
            Just("https://example.com/photo.jpg".to_string()),
            "https://[a-z]+\\.example\\.com/[a-z]+\\.png",
            "\\PC*",
        ],
        kind in prop::option::of(prop_oneof![
            Just("photo".to_string()),
            Just("logo".to_string()),
            Just("sound".to_string()),
            "[a-z]{1,6}",
        ]),
        media_type in prop::option::of(prop_oneof![
            Just("image/jpeg".to_string()),
            Just("image/png".to_string()),
            Just("image/gif".to_string()),
            "[a-z]+/[a-z]+",
        ]),
    ) -> Media {
        Media {
            kind,
            uri,
            media_type,
            extra: BTreeMap::new(),
        }
    }
}

prop_compose! {
    fn arb_online_service()(
        service in prop::option::of(prop_oneof![
            Just("Jabber".to_string()),
            Just("Matrix".to_string()),
            Just("Skype".to_string()),
            Just("Twitter".to_string()),
            Just("SIP".to_string()),
            Just("AIM".to_string()),
            Just("ICQ".to_string()),
            Just("MSN".to_string()),
            Just("Yahoo".to_string()),
            Just("GroupWise".to_string()),
            "[a-zA-Z]{1,10}",
        ]),
        user in prop::option::of("\\PC*"),
        uri in prop::option::of(prop_oneof![
            "xmpp:[a-z]+@example\\.com",
            "matrix:u/[a-z]+:example\\.com",
            "sip:[a-z]+@example\\.com",
            "aim:[a-z]+",
            "icq:[0-9]+",
            "ymsgr:[a-z]+",
            "msnim:[a-z]+@example\\.com",
            "https://twitter\\.com/[a-z]+",
            "\\PC*",
        ]),
    ) -> OnlineService {
        OnlineService {
            service,
            user,
            uri,
            extra: BTreeMap::new(),
        }
    }
}

prop_compose! {
    fn arb_relation()(
        relation in prop::option::of(prop_oneof![
            Just([("spouse".to_string(), json!(true))].into()),
            Just([("child".to_string(), json!(true))].into()),
            Just([("colleague".to_string(), json!(true))].into()),
            Just([("spouse".to_string(), json!(1))].into()),
            Just(BTreeMap::new()),
        ]),
    ) -> Relation {
        Relation {
            relation,
            extra: BTreeMap::new(),
        }
    }
}

fn arb_key() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-zA-Z0-9_-]{1,8}",
        Just("k1".to_string()),
        Just("e1\r\nFN:Injected".to_string()),
        Just("p1\"quoted".to_string()),
        "\\PC{1,8}",
    ]
}

prop_compose! {
    fn arb_card_ids()(
        id in prop::option::of("[a-zA-Z0-9_-]{1,16}"),
        uid in prop::option::of("[a-zA-Z0-9_-]{1,16}"),
        name in prop::option::of(arb_name()),
        nicknames in prop::option::of(prop::collection::btree_map(arb_key(), arb_nickname(), 0..3)),
    ) -> (
        Option<String>,
        Option<String>,
        Option<Name>,
        Option<BTreeMap<String, Nickname>>,
    ) {
        (id, uid, name, nicknames)
    }
}

prop_compose! {
    fn arb_card_comm()(
        emails in prop::option::of(prop::collection::btree_map(arb_key(), arb_email(), 0..3)),
        phones in prop::option::of(prop::collection::btree_map(arb_key(), arb_phone(), 0..3)),
        online_services in prop::option::of(prop::collection::btree_map(arb_key(), arb_online_service(), 0..3)),
    ) -> (
        Option<BTreeMap<String, ContactEmail>>,
        Option<BTreeMap<String, ContactPhone>>,
        Option<BTreeMap<String, OnlineService>>,
    ) {
        (emails, phones, online_services)
    }
}

prop_compose! {
    fn arb_card_org()(
        organizations in prop::option::of(prop::collection::btree_map(arb_key(), arb_organization(), 0..3)),
        titles in prop::option::of(prop::collection::btree_map(arb_key(), arb_title(), 0..3)),
        addresses in prop::option::of(prop::collection::btree_map(arb_key(), arb_address(), 0..3)),
        notes in prop::option::of(prop::collection::btree_map(arb_key(), arb_note(), 0..3)),
    ) -> (
        Option<BTreeMap<String, Organization>>,
        Option<BTreeMap<String, Title>>,
        Option<BTreeMap<String, Address>>,
        Option<BTreeMap<String, Note>>,
    ) {
        (organizations, titles, addresses, notes)
    }
}

prop_compose! {
    fn arb_card_resources()(
        anniversaries in prop::option::of(prop::collection::btree_map(arb_key(), arb_anniversary(), 0..3)),
        links in prop::option::of(prop::collection::btree_map(arb_key(), arb_link(), 0..3)),
        calendars in prop::option::of(prop::collection::btree_map(arb_key(), arb_calendar(), 0..3)),
        media in prop::option::of(prop::collection::btree_map(arb_key(), arb_media(), 0..2)),
        keywords in prop::option::of(prop::collection::btree_map(
            "[a-zA-Z0-9_-]{1,10}",
            prop_oneof![Just(json!(true)), Just(json!(false)), Just(json!("tag")), Just(json!(1))],
            0..4,
        )),
        related_to in prop::option::of(prop::collection::btree_map(arb_key(), arb_relation(), 0..2)),
    ) -> (
        Option<BTreeMap<String, Anniversary>>,
        Option<BTreeMap<String, Link>>,
        Option<BTreeMap<String, Calendar>>,
        Option<BTreeMap<String, Media>>,
        Option<BTreeMap<String, serde_json::Value>>,
        Option<BTreeMap<String, Relation>>,
    ) {
        (
            anniversaries,
            links,
            calendars,
            media,
            keywords,
            related_to,
        )
    }
}

fn arb_contact_card() -> impl Strategy<Value = ContactCard> {
    (
        arb_card_ids(),
        arb_card_comm(),
        arb_card_org(),
        arb_card_resources(),
    )
        .prop_map(
            |(
                (id, uid, name, nicknames),
                (emails, phones, online_services),
                (organizations, titles, addresses, notes),
                (anniversaries, links, calendars, media, keywords, related_to),
            )| {
                ContactCard {
                    id: id.map(Into::into),
                    uid,
                    card_type: Some("Card".to_string()),
                    version: Some("1.0".to_string()),
                    name,
                    nicknames,
                    emails,
                    phones,
                    organizations,
                    titles,
                    addresses,
                    notes,
                    anniversaries,
                    links,
                    calendars,
                    media,
                    online_services,
                    keywords,
                    related_to,
                    ..ContactCard::default()
                }
            },
        )
}

prop_compose! {
    fn arb_vcard_property_line()(
        name in prop_oneof![
            Just("FN".to_string()),
            Just("N".to_string()),
            Just("NICKNAME".to_string()),
            Just("EMAIL".to_string()),
            Just("TEL".to_string()),
            Just("ADR".to_string()),
            Just("LABEL".to_string()),
            Just("ORG".to_string()),
            Just("TITLE".to_string()),
            Just("ROLE".to_string()),
            Just("NOTE".to_string()),
            Just("BDAY".to_string()),
            Just("URL".to_string()),
            Just("CALURI".to_string()),
            Just("FBURL".to_string()),
            Just("PHOTO".to_string()),
            Just("CATEGORIES".to_string()),
            Just("X-JABBER".to_string()),
            Just("X-MATRIX".to_string()),
            Just("X-SKYPE".to_string()),
            Just("X-TWITTER".to_string()),
            Just("X-SIP".to_string()),
            Just("X-EVOLUTION-ANNIVERSARY".to_string()),
            Just("X-EVOLUTION-SPOUSE".to_string()),
            Just("X-CUSTOM".to_string()),
            "[A-Z0-9-]{1,12}",
        ],
        params in prop::collection::vec(
            prop_oneof![
                Just(";TYPE=WORK".to_string()),
                Just(";TYPE=HOME".to_string()),
                Just(";TYPE=VOICE,FAX".to_string()),
                Just(";X-JMAP-KEY=k1".to_string()),
                Just(";VALUE=uri".to_string()),
                Just(";ENCODING=b".to_string()),
                Just(";ALTID=1".to_string()),
                Just(";LANGUAGE=en-US".to_string()),
                Just(";LANGUAGE=de".to_string()),
                Just(";ALTID=group1;LANGUAGE=ja".to_string()),
                ";[A-Z-]+=[A-Za-z0-9-]+",
            ],
            0..3,
        ),
        value in "\\PC*",
    ) -> String {
        let param_str = params.join("");
        format!("{name}{param_str}:{value}")
    }
}

prop_compose! {
    fn arb_raw_vcard()(
        lines in prop::collection::vec(arb_vcard_property_line(), 0..10),
        trailing in prop::option::of("\\PC*"),
    ) -> String {
        let mut out = String::from("BEGIN:VCARD\r\nVERSION:3.0\r\n");
        for line in lines {
            out.push_str(&line);
            out.push_str("\r\n");
        }
        out.push_str("END:VCARD\r\n");
        if let Some(t) = trailing {
            out.push_str(&t);
        }
        out
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn prop_card_to_vcard_never_panics(card in arb_contact_card()) {
        let vcard = card_to_vcard(&card);
        prop_assert!(!vcard.is_empty());
        prop_assert!(vcard.starts_with("BEGIN:VCARD\r\n"));
        prop_assert!(vcard.ends_with("END:VCARD\r\n"));
    }

    #[test]
    fn prop_vcard_to_card_never_panics_on_raw_vcard(vcard_text in arb_raw_vcard()) {
        let _ = vcard_to_card(&vcard_text);
    }

    #[test]
    fn prop_vcard_to_card_never_panics_on_arbitrary_string(text in ".*") {
        let _ = vcard_to_card(&text);
    }

    #[test]
    fn prop_card_roundtrip_reaches_fixed_point_stability(card in arb_contact_card()) {
        let vcard1 = card_to_vcard(&card);
        if let Ok(parsed1) = vcard_to_card(&vcard1) {
            let vcard2 = card_to_vcard(&parsed1);
            let parsed2 = vcard_to_card(&vcard2).expect("second roundtrip must parse cleanly");
            let vcard3 = card_to_vcard(&parsed2);
            prop_assert_eq!(vcard2, vcard3, "vCard emission must reach a fixed-point");
        }
    }

    #[test]
    fn prop_vcard_roundtrip_reaches_fixed_point_stability(vcard_text in arb_raw_vcard()) {
        if let Ok(parsed1) = vcard_to_card(&vcard_text) {
            let vcard1 = card_to_vcard(&parsed1);
            let parsed2 = vcard_to_card(&vcard1).expect("re-parsing emitted vCard must succeed");
            let vcard2 = card_to_vcard(&parsed2);
            let parsed3 = vcard_to_card(&vcard2).expect("third roundtrip must parse cleanly");
            let vcard3 = card_to_vcard(&parsed3);
            prop_assert_eq!(vcard2, vcard3, "re-emitted vCard must reach a fixed-point");
        }
    }
}
