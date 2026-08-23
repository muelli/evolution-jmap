// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Structure-aware fuzzing of the JSContact ↔ vCard 3.0 mapping using `proptest`.
//!
//! Asserts:
//! 1. `card_to_vcard` never panics on arbitrary `ContactCard` instances.
//! 2. `vcard_to_card` never panics on arbitrary strings or arbitrary vCard envelopes.
//! 3. Round-trip stability: Emitting a card, parsing it back, and re-emitting reaches a fixed point.

use std::collections::BTreeMap;

use base64::Engine;
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
    fn arb_nickname()(
        name in prop_oneof![
            "\\PC*",
            "[A-Za-z0-9_-]{1,15}",
            "[A-Za-z]+, [A-Za-z]+",
            "[A-Za-z]+; [A-Za-z]+",
            "\"[A-Za-z ]+\"",
            Just("Bob, The Builder".to_string()),
            Just("Dr. Who, PhD".to_string()),
            Just("たなか (田中)".to_string()),
            Just("Саша".to_string()),
            Just("🌟 Star".to_string()),
        ],
    ) -> Nickname {
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
            Just(json!({"car": true})),
            Just(json!({"isdn": true})),
            Just(json!({"ttytdd": true})),
            Just(json!({"voice": true, "fax": true})),
            Just(json!({"voice": true, "mobile": true})),
            Just(json!({"cell": true, "video": true})),
            Just(json!("voice")),
            Just(json!(42)),
        ]),
        contexts in prop::option::of(prop_oneof![
            Just(json!("work")),
            Just(json!("home")),
            Just(json!({"work": true})),
            Just(json!({"home": true})),
            Just(json!({"other": true})),
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
        pref in prop::option::of(0..100u32),
    ) -> Address {
        let mut extra = BTreeMap::new();
        if let Some(p) = pref {
            extra.insert("pref".to_owned(), json!(p));
        }
        Address {
            components,
            contexts,
            full,
            extra,
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
        uri in prop_oneof![
            "\\PC*",
            "https://[a-z]{3,10}\\.example\\.(com|org)/[a-z0-9_-]{0,10}",
            "https://api\\.example\\.com/search\\?q=[a-z]+,[a-z]+;[a-z]+",
            "http://\\[2001:db8::1\\]:8080/path\\?ref=123;456",
            "mailto:[a-z]+@[a-z]+\\.example\\.com",
            Just("https://example.com/tags?a=1,2&b=3;4#sec".to_string()),
            Just("".to_string()),
        ],
        kind in prop::option::of(prop_oneof![
            Just("contact".to_string()),
            Just("website".to_string()),
            Just("feed".to_string()),
            Just("blog".to_string()),
            Just("video".to_string()),
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
            Just("Google Talk".to_string()),
            Just("Gadu-Gadu".to_string()),
            "[a-zA-Z]{1,10}",
        ]),
        user in prop::option::of("\\PC*"),
        uri in prop::option::of(prop_oneof![
            "xmpp:[a-z]+@example\\.com",
            "jabber:[a-z]+@example\\.com",
            "gtalk:[a-z]+@example\\.com",
            "matrix:u/[a-z]+:example\\.com",
            "matrix:@[a-z]+:example\\.com",
            "sip:[a-z]+@example\\.com",
            "aim:[a-z]+",
            "aol:[a-z]+",
            "icq:[0-9]+",
            "gg:[0-9]+",
            "gadugadu:[0-9]+",
            "gadu:[0-9]+",
            "groupwise:[a-z]+",
            "novell:[a-z]+",
            "yahoo:[a-z]+",
            "ymsgr:[a-z]+",
            "msn:[a-z]+@example\\.com",
            "msnim:[a-z]+@example\\.com",
            "skype:[a-z]+",
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
            Just([("manager".to_string(), json!(true))].into()),
            Just([("assistant".to_string(), json!(true))].into()),
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

fn arb_keyword_tag() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-zA-Z0-9_-]{1,10}",
        Just("Work, Urgent".to_string()),
        Just("Acme, Inc.".to_string()),
        Just("Project;Alpha".to_string()),
        Just("Dept\\Core".to_string()),
        Just("Line 1\nLine 2".to_string()),
        Just("Büro & Verwaltung".to_string()),
        Just("🚀 VIP".to_string()),
        Just(" leading".to_string()),
        Just("trailing ".to_string()),
        Just("with\rcr".to_string()),
        "\\PC{1,10}",
    ]
}

prop_compose! {
    fn arb_card_resources()(
        anniversaries in prop::option::of(prop::collection::btree_map(arb_key(), arb_anniversary(), 0..3)),
        links in prop::option::of(prop::collection::btree_map(arb_key(), arb_link(), 0..3)),
        calendars in prop::option::of(prop::collection::btree_map(arb_key(), arb_calendar(), 0..3)),
        media in prop::option::of(prop::collection::btree_map(arb_key(), arb_media(), 0..2)),
        keywords in prop::option::of(prop::collection::btree_map(
            arb_keyword_tag(),
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
            Just("GEO".to_string()),
            Just("TZ".to_string()),
            Just("MAILER".to_string()),
            Just("PRODID".to_string()),
            Just("REV".to_string()),
            Just("SORT-STRING".to_string()),
            Just("CLASS".to_string()),
            Just("SOUND".to_string()),
            Just("LOGO".to_string()),
            Just("KEY".to_string()),
            Just("X-AIM".to_string()),
            Just("X-GADUGADU".to_string()),
            Just("X-GOOGLE-TALK".to_string()),
            Just("X-GROUPWISE".to_string()),
            Just("X-ICQ".to_string()),
            Just("X-JABBER".to_string()),
            Just("X-MSN".to_string()),
            Just("X-MATRIX".to_string()),
            Just("X-SKYPE".to_string()),
            Just("X-YAHOO".to_string()),
            Just("X-TWITTER".to_string()),
            Just("X-SIP".to_string()),
            Just("X-EVOLUTION-ANNIVERSARY".to_string()),
            Just("X-EVOLUTION-SPOUSE".to_string()),
            Just("X-EVOLUTION-MANAGER".to_string()),
            Just("X-EVOLUTION-ASSISTANT".to_string()),
            Just("X-EVOLUTION-BLOG-URL".to_string()),
            Just("X-EVOLUTION-VIDEO-URL".to_string()),
            Just("X-EVOLUTION-FILE-AS".to_string()),
            Just("X-ABLabel".to_string()),
            Just("X-ABRELATEDNAMES".to_string()),
            Just("X-ABDATE".to_string()),
            Just("X-MOZILLA-HTML".to_string()),
            Just("X-PHONETIC-FIRST-NAME".to_string()),
            Just("X-ABShowAs".to_string()),
            Just("X-MS-CARDPICTURE".to_string()),
            Just("X-DISCORD".to_string()),
            Just("X-SIGNAL".to_string()),
            Just("X-TELEGRAM".to_string()),
            Just("X-CUSTOM".to_string()),
            "[A-Z0-9-]{1,12}",
        ],
        group in prop_oneof![
            Just("".to_string()),
            Just("item1.".to_string()),
            Just("item2.".to_string()),
            Just("itemA.".to_string()),
        ],
        params in prop::collection::vec(
            prop_oneof![
                Just(";TYPE=WORK".to_string()),
                Just(";TYPE=HOME".to_string()),
                Just(";TYPE=CELL".to_string()),
                Just(";TYPE=MOBILE".to_string()),
                Just(";TYPE=PAGER".to_string()),
                Just(";TYPE=VOICE".to_string()),
                Just(";TYPE=FAX".to_string()),
                Just(";TYPE=VIDEO".to_string()),
                Just(";TYPE=WORK,CELL".to_string()),
                Just(";TYPE=HOME,MOBILE".to_string()),
                Just(";TYPE=WORK,FAX".to_string()),
                Just(";TYPE=HOME,FAX".to_string()),
                Just(";TYPE=VOICE,FAX".to_string()),
                Just(";TYPE=CAR".to_string()),
                Just(";TYPE=ISDN".to_string()),
                Just(";TYPE=TTYTDD".to_string()),
                Just(";TYPE=WORK,PREF".to_string()),
                Just(";TYPE=HOME,PREF".to_string()),
                Just(";TYPE=X509".to_string()),
                Just(";TYPE=PGP".to_string()),
                Just(";TYPE=X509;ENCODING=b".to_string()),
                Just(";TYPE=PGP;ENCODING=b".to_string()),
                Just(";LABEL=\"Suite 100\\nCity\"".to_string()),
                Just(";LABEL=Office".to_string()),
                Just(";X-JMAP-KEY=k1".to_string()),
                Just(";VALUE=uri".to_string()),
                Just(";ENCODING=b".to_string()),
                Just(";ENCODING=BASE64".to_string()),
                Just(";ENCODING=8BIT".to_string()),
                Just(";ENCODING=7BIT".to_string()),
                Just(";ENCODING=QUOTED-PRINTABLE".to_string()),
                Just(";CHARSET=UTF-8".to_string()),
                Just(";CHARSET=utf-8".to_string()),
                Just(";CHARSET=ISO-8859-1".to_string()),
                Just(";CHARSET=WINDOWS-1252".to_string()),
                Just(";ENCODING=QUOTED-PRINTABLE;CHARSET=UTF-8".to_string()),
                Just(";ENCODING=QUOTED-PRINTABLE;CHARSET=ISO-8859-1".to_string()),
                Just(";ALTID=1".to_string()),
                Just(";LANGUAGE=en-US".to_string()),
                Just(";LANGUAGE=de".to_string()),
                Just(";ALTID=group1;LANGUAGE=ja".to_string()),
                Just(";X-CUSTOM-PARAM=val1".to_string()),
                Just(";X-VENDOR-STATUS=ACTIVE".to_string()),
                // vCard 2.1 bare parameter names
                Just(";WORK".to_string()),
                Just(";HOME".to_string()),
                Just(";CELL".to_string()),
                Just(";MOBILE".to_string()),
                Just(";VOICE".to_string()),
                Just(";FAX".to_string()),
                Just(";PAGER".to_string()),
                Just(";PREF".to_string()),
                Just(";INTERNET".to_string()),
                Just(";BASE64".to_string()),
                Just(";JPEG".to_string()),
                Just(";GIF".to_string()),
                Just(";PNG".to_string()),
                Just(";POSTAL".to_string()),
                Just(";PARCEL".to_string()),
                Just(";DOM".to_string()),
                ";[A-Z-]+=[A-Za-z0-9-]+",
            ],
            0..3,
        ),
        value in prop_oneof![
            "\\PC*",
            Just("_$!<Work>!$_".to_string()),
            Just("_$!<Home>!$_".to_string()),
            Just("_$!<Mobile>!$_".to_string()),
            Just("_$!<Spouse>!$_".to_string()),
            Just("_$!<Manager>!$_".to_string()),
            Just("_$!<Assistant>!$_".to_string()),
            Just("_$!<Anniversary>!$_".to_string()),
        ],
    ) -> String {
        let param_str = params.join("");
        format!("{group}{name}{param_str}:{value}")
    }
}

prop_compose! {
    fn arb_raw_vcard()(
        version in prop_oneof![Just("3.0"), Just("2.1"), Just("4.0")],
        lines in prop::collection::vec(arb_vcard_property_line(), 0..10),
        trailing in prop::option::of("\\PC*"),
    ) -> String {
        let mut out = format!("BEGIN:VCARD\r\nVERSION:{version}\r\n");
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

fn identify_oscillating_vcard_property(export2: &str, export3: &str) -> String {
    let lines2: Vec<&str> = export2.lines().collect();
    let lines3: Vec<&str> = export3.lines().collect();

    for (i, (l2, l3)) in lines2.iter().zip(lines3.iter()).enumerate() {
        if l2 != l3 {
            let prop_name = l2
                .split([';', ':'])
                .next()
                .unwrap_or("UNKNOWN")
                .trim_start_matches(' ');
            return format!(
                "Property '{prop_name}' oscillated at line {}:\n  Export₂: {l2}\n  Export₃: {l3}",
                i + 1
            );
        }
    }

    if lines2.len() != lines3.len() {
        if lines2.len() > lines3.len() {
            let extra = &lines2[lines3.len()..];
            let prop_name = extra[0]
                .split([';', ':'])
                .next()
                .unwrap_or("UNKNOWN")
                .trim_start_matches(' ');
            return format!(
                "Property '{prop_name}' oscillated (lines missing in Export₃):\n  {}",
                extra.join("\n  ")
            );
        } else {
            let extra = &lines3[lines2.len()..];
            let prop_name = extra[0]
                .split([';', ':'])
                .next()
                .unwrap_or("UNKNOWN")
                .trim_start_matches(' ');
            return format!(
                "Property '{prop_name}' oscillated (spurious lines in Export₃):\n  {}",
                extra.join("\n  ")
            );
        }
    }

    "Byte/content mismatch without line divergence".to_string()
}

fn identify_oscillating_card_field(card2: &ContactCard, card3: &ContactCard) -> String {
    if card2.name != card3.name {
        return format!(
            "Field 'name' oscillated:\n  Card₂: {:?}\n  Card₃: {:?}",
            card2.name, card3.name
        );
    }
    if card2.nicknames != card3.nicknames {
        return format!(
            "Field 'nicknames' oscillated:\n  Card₂: {:?}\n  Card₃: {:?}",
            card2.nicknames, card3.nicknames
        );
    }
    if card2.emails != card3.emails {
        return format!(
            "Field 'emails' oscillated:\n  Card₂: {:?}\n  Card₃: {:?}",
            card2.emails, card3.emails
        );
    }
    if card2.phones != card3.phones {
        return format!(
            "Field 'phones' oscillated:\n  Card₂: {:?}\n  Card₃: {:?}",
            card2.phones, card3.phones
        );
    }
    if card2.organizations != card3.organizations {
        return format!(
            "Field 'organizations' oscillated:\n  Card₂: {:?}\n  Card₃: {:?}",
            card2.organizations, card3.organizations
        );
    }
    if card2.titles != card3.titles {
        return format!(
            "Field 'titles' oscillated:\n  Card₂: {:?}\n  Card₃: {:?}",
            card2.titles, card3.titles
        );
    }
    if card2.addresses != card3.addresses {
        return format!(
            "Field 'addresses' oscillated:\n  Card₂: {:?}\n  Card₃: {:?}",
            card2.addresses, card3.addresses
        );
    }
    if card2.notes != card3.notes {
        return format!(
            "Field 'notes' oscillated:\n  Card₂: {:?}\n  Card₃: {:?}",
            card2.notes, card3.notes
        );
    }
    if card2.anniversaries != card3.anniversaries {
        return format!(
            "Field 'anniversaries' oscillated:\n  Card₂: {:?}\n  Card₃: {:?}",
            card2.anniversaries, card3.anniversaries
        );
    }
    if card2.links != card3.links {
        return format!(
            "Field 'links' oscillated:\n  Card₂: {:?}\n  Card₃: {:?}",
            card2.links, card3.links
        );
    }
    if card2.calendars != card3.calendars {
        return format!(
            "Field 'calendars' oscillated:\n  Card₂: {:?}\n  Card₃: {:?}",
            card2.calendars, card3.calendars
        );
    }
    if card2.media != card3.media {
        return format!(
            "Field 'media' oscillated:\n  Card₂: {:?}\n  Card₃: {:?}",
            card2.media, card3.media
        );
    }
    if card2.online_services != card3.online_services {
        return format!(
            "Field 'online_services' oscillated:\n  Card₂: {:?}\n  Card₃: {:?}",
            card2.online_services, card3.online_services
        );
    }
    if card2.keywords != card3.keywords {
        return format!(
            "Field 'keywords' oscillated:\n  Card₂: {:?}\n  Card₃: {:?}",
            card2.keywords, card3.keywords
        );
    }
    if card2.related_to != card3.related_to {
        return format!(
            "Field 'related_to' oscillated:\n  Card₂: {:?}\n  Card₃: {:?}",
            card2.related_to, card3.related_to
        );
    }
    format!("Unknown card field oscillated:\n  Card₂: {card2:?}\n  Card₃: {card3:?}")
}

fn assert_vcard_fixpoint(export2: &str, export3: &str) -> Result<(), TestCaseError> {
    if export2 != export3 {
        let explanation = identify_oscillating_vcard_property(export2, export3);
        return Err(TestCaseError::fail(format!(
            "vCard roundtrip failed to reach fixed point (Export₂ != Export₃)!\n{explanation}"
        )));
    }
    Ok(())
}

fn assert_card_fixpoint(card2: &ContactCard, card3: &ContactCard) -> Result<(), TestCaseError> {
    if card2 != card3 {
        let explanation = identify_oscillating_card_field(card2, card3);
        return Err(TestCaseError::fail(format!(
            "JSContact roundtrip failed to reach fixed point (Card₂ != Card₃)!\n{explanation}"
        )));
    }
    Ok(())
}

// Regression for a fixed-point failure `prop_card_roundtrip_reaches_fixed_point_stability`
// found on a random seed (docs/BACKLOG.md, "jmap-vcard round trip is not a fixed point for
// a value with trailing whitespace", second occurrence): a `Name` with a stated-but-empty
// `full` and only a `given` component round-tripped once (to `parsed1`) drops `full`
// (an empty FN is indistinguishable from an absent one on read), but round-tripping that
// result again (to `parsed2`) synthesizes a derived `full` from the components — so
// `parsed1 != parsed2` and the property never reaches the fixed point it asserts.
#[test]
fn a_name_with_an_empty_stated_full_and_only_a_given_component_reaches_fixed_point() {
    let card = ContactCard {
        name: Some(Name {
            components: Some(vec![NameComponent::new("given", "A")]),
            full: Some(String::new()),
            extra: BTreeMap::new(),
        }),
        ..Default::default()
    };

    let vcard1 = card_to_vcard(&card);
    let parsed1 = vcard_to_card(&vcard1).expect("first roundtrip must parse cleanly");
    let vcard2 = card_to_vcard(&parsed1);
    let parsed2 = vcard_to_card(&vcard2).expect("second roundtrip must parse cleanly");

    assert_eq!(
        parsed1.name, parsed2.name,
        "name field oscillated between the first and second roundtrip"
    );
}

// Regression for `prop_emitted_vcard_lines_target_75_octets_and_are_valid_utf8`
// found on a random seed (CI run 32663971912): calcard's writer emits a
// structured value's `;` separators *after* its fold check, and skips the fold
// altogether when the component coming next is empty text — so an `ADR` whose
// value folds to exactly 75 octets keeps all six empty trailing slots on the
// same physical line, at 81. The value here is byte-for-byte the minimal case
// proptest shrank to: 53 octets of multi-byte UTF-8 that land the 22-octet
// prefix `ADR;X-JMAP-KEY=-AaaAa:` plus the value at exactly the edge.
#[test]
fn an_adr_that_folds_to_exactly_the_limit_keeps_its_empty_slots_within_it() {
    let mut addresses = BTreeMap::new();
    addresses.insert(
        "-AaaAa".to_owned(),
        Address {
            components: Some(vec![AddressComponent::new(
                "postOfficeBox",
                "ொ\u{a980}ꧏ a¡ₐA A𐕼Σ𞴁AAವ⺛𫝀\u{fffc}\u{1a60}aA0প",
            )]),
            contexts: None,
            full: None,
            extra: BTreeMap::new(),
        },
    );
    let card = ContactCard {
        addresses: Some(addresses),
        ..Default::default()
    };

    for line in card_to_vcard(&card).split("\r\n") {
        assert!(
            line.len() <= 75,
            "physical line exceeds 75 octets (len = {}): {line:?}",
            line.len()
        );
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

            assert_vcard_fixpoint(&vcard2, &vcard3)?;
            assert_card_fixpoint(&parsed1, &parsed2)?;
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

            assert_vcard_fixpoint(&vcard2, &vcard3)?;
            assert_card_fixpoint(&parsed2, &parsed3)?;
        }
    }

    #[test]
    fn prop_emitted_vcard_lines_target_75_octets_and_are_valid_utf8(card in arb_contact_card()) {
        let vcard = card_to_vcard(&card);
        for line in vcard.split("\r\n") {
            // Exactly the RFC 2426 §2.6 width: calcard's own writer overshoots
            // when empty structured slots trail a value at the boundary, and
            // `card_to_vcard`'s refold pass exists to take that back to 75 —
            // a looser bound here is what let that overshoot go unseen.
            prop_assert!(
                line.len() <= 75,
                "Physical line exceeds maximum line length (len = {}): {:?}",
                line.len(),
                line
            );
            // Multi-byte UTF-8 code points must never be split across a fold
            prop_assert!(
                std::str::from_utf8(line.as_bytes()).is_ok(),
                "Invalid UTF-8 sequence in line slice: {:?}",
                line
            );
        }
    }

    #[test]
    fn prop_value_escaping_never_double_escapes_or_loses_characters(
        prefix in "[a-zA-Z0-9 ]{0,10}",
        escapes in prop::collection::vec(
            prop_oneof![
                Just("\n"),
                Just("\r\n"),
                Just(","),
                Just(";"),
                Just("\\"),
                Just("\\n"),
                Just("\\,"),
                Just("\\;"),
                Just("\\\\"),
            ],
            1..8,
        ),
        suffix in "[a-zA-Z0-9 ]{0,10}",
    ) {
        let text = format!("{prefix}{}{suffix}", escapes.join(""));
        let mut notes = BTreeMap::new();
        notes.insert(
            "n1".to_owned(),
            Note {
                note: text.clone(),
                extra: BTreeMap::new(),
            },
        );
        let card = ContactCard {
            id: Some("C-PROP-ESC".into()),
            notes: Some(notes),
            ..ContactCard::default()
        };

        let vcard1 = card_to_vcard(&card);
        let parsed1 = vcard_to_card(&vcard1).expect("parse emitted escaped vcard");
        let parsed_note = &parsed1.notes.as_ref().unwrap()["n1"].note;
        // Text must match original exactly (calcard preserves CRLF and escapes losslessly)
        prop_assert_eq!(parsed_note, &text);

        let vcard2 = card_to_vcard(&parsed1);
        let parsed2 = vcard_to_card(&vcard2).expect("parse second roundtrip vcard");
        let vcard3 = card_to_vcard(&parsed2);
        prop_assert_eq!(vcard2, vcard3, "Escaped value must reach fixed point");
    }

    #[test]
    fn prop_non_ascii_unicode_card_roundtrips_without_corruption(
        given in prop_oneof![
            Just("René".to_string()),
            Just("Jörg".to_string()),
            Just("María José".to_string()),
            Just("Лев".to_string()),
            Just("Σωκράτης".to_string()),
            Just("שלום".to_string()),
            Just("نجيب".to_string()),
            Just("李".to_string()),
            Just("駿".to_string()),
            Just("김".to_string()),
            Just("रवीन्द्रनाथ".to_string()),
            Just("🧑‍💻".to_string()),
            "\\PC{1,15}",
        ],
        surname in prop_oneof![
            Just("Müller".to_string()),
            Just("Weiß".to_string()),
            Just("Carreño".to_string()),
            Just("Толстой".to_string()),
            Just("محفوظ".to_string()),
            Just("白".to_string()),
            Just("宮崎".to_string()),
            Just("연아".to_string()),
            Just("ठाकुर".to_string()),
            Just("🚀".to_string()),
            "\\PC{1,15}",
        ],
        note_text in prop_oneof![
            Just("Café & Croissants in München.\n∀x ∈ ℝ: x² ≥ 0 🌟".to_string()),
            Just("Привет, мир! 🌍".to_string()),
            Just("こんにちは 世界 🌸".to_string()),
            Just("مرحبا بالعالم".to_string()),
            "\\PC{1,50}",
        ],
    ) {
        let full = format!("{given} {surname}");
        let card = ContactCard {
            id: Some("C-PROP-UNICODE".into()),
            name: Some(Name {
                full: Some(full.clone()),
                components: Some(vec![
                    NameComponent::new("given", &given),
                    NameComponent::new("surname", &surname),
                ]),
                extra: BTreeMap::new(),
            }),
            notes: Some([(
                "n1".to_owned(),
                Note {
                    note: note_text.clone(),
                    extra: BTreeMap::new(),
                },
            )].into()),
            ..ContactCard::default()
        };

        let vcard1 = card_to_vcard(&card);
        let parsed1 = vcard_to_card(&vcard1).expect("parse non-ascii unicode card");

        let p_name = parsed1.name.as_ref().expect("name present");
        prop_assert_eq!(p_name.full.as_deref(), Some(full.as_str()));

        let p_note = &parsed1.notes.as_ref().expect("notes present")["n1"].note;
        prop_assert_eq!(p_note, &note_text);

        let vcard2 = card_to_vcard(&parsed1);
        let parsed2 = vcard_to_card(&vcard2).expect("parse second roundtrip");
        let vcard3 = card_to_vcard(&parsed2);
        prop_assert_eq!(vcard2, vcard3, "Unicode card must reach fixed point");
    }

    #[test]
    fn prop_photo_inline_and_uri_roundtrip_stability(
        is_uri in any::<bool>(),
        subtype in prop_oneof![
            Just("jpeg".to_string()),
            Just("png".to_string()),
            Just("gif".to_string()),
            Just("webp".to_string()),
            Just("svg+xml".to_string()),
            Just("bmp".to_string()),
            Just("tiff".to_string()),
            Just("avif".to_string()),
            Just("heic".to_string()),
            Just("x-icon".to_string()),
        ],
        raw_bytes in prop::collection::vec(any::<u8>(), 1..256),
        uri_str in prop_oneof![
            Just("https://example.com/avatar.jpg".to_string()),
            Just("http://cdn.org/pic.png?w=100&h=100".to_string()),
            Just("file:///home/user/.face".to_string()),
            Just("https://photos.example.org/image.webp#top".to_string()),
        ],
    ) {
        let (uri, media_type) = if is_uri {
            (uri_str, None)
        } else {
            let encoded = base64::engine::general_purpose::STANDARD.encode(&raw_bytes);
            (
                format!("data:image/{subtype};base64,{encoded}"),
                Some(format!("image/{subtype}")),
            )
        };

        let card = ContactCard {
            id: Some("C-PROP-PHOTO".into()),
            media: Some([(
                "m1".to_owned(),
                Media {
                    kind: Some("photo".to_owned()),
                    uri: uri.clone(),
                    media_type: media_type.clone(),
                    extra: BTreeMap::new(),
                },
            )].into()),
            ..ContactCard::default()
        };

        let vcard1 = card_to_vcard(&card);
        prop_assert!(vcard1.contains("PHOTO;"));
        if is_uri {
            prop_assert!(vcard1.contains("VALUE=uri"));
            prop_assert!(!vcard1.contains("ENCODING="));
        } else {
            let expected_type = format!("TYPE={subtype}");
            prop_assert!(vcard1.contains("ENCODING=b"));
            prop_assert!(vcard1.contains(&expected_type));
        }

        let parsed1 = vcard_to_card(&vcard1).expect("parse emitted photo vcard");
        let entry1 = &parsed1.media.as_ref().expect("media present")["m1"];
        prop_assert_eq!(entry1.kind.as_deref(), Some("photo"));

        if is_uri {
            prop_assert_eq!(&entry1.uri, &uri);
            prop_assert_eq!(entry1.media_type.as_ref(), None);
        } else {
            prop_assert_eq!(entry1.media_type.as_deref(), media_type.as_deref());
            let prefix = format!("data:image/{subtype};base64,");
            let payload = entry1.uri.strip_prefix(&prefix).expect("data prefix");
            let decoded_bytes = base64::engine::general_purpose::STANDARD.decode(payload).expect("decode base64");
            prop_assert_eq!(decoded_bytes, raw_bytes);
        }

        let vcard2 = card_to_vcard(&parsed1);
        let parsed2 = vcard_to_card(&vcard2).expect("parse second roundtrip");
        let vcard3 = card_to_vcard(&parsed2);
        prop_assert_eq!(vcard2, vcard3, "Photo roundtrip must reach fixed point");
    }

    #[test]
    fn prop_fixpoint_telephony_domain(
        phones in prop::collection::btree_map(arb_key(), arb_phone(), 1..6)
    ) {
        let card = ContactCard {
            id: Some("C-TEL".into()),
            phones: Some(phones),
            ..ContactCard::default()
        };
        let vcard1 = card_to_vcard(&card);
        let parsed1 = vcard_to_card(&vcard1).expect("telephony vcard1 parse");
        let vcard2 = card_to_vcard(&parsed1);
        let parsed2 = vcard_to_card(&vcard2).expect("telephony vcard2 parse");
        let vcard3 = card_to_vcard(&parsed2);

        assert_vcard_fixpoint(&vcard2, &vcard3)?;
        assert_card_fixpoint(&parsed1, &parsed2)?;
    }

    #[test]
    fn prop_fixpoint_email_domain(
        emails in prop::collection::btree_map(arb_key(), arb_email(), 1..6)
    ) {
        let card = ContactCard {
            id: Some("C-EMAIL".into()),
            emails: Some(emails),
            ..ContactCard::default()
        };
        let vcard1 = card_to_vcard(&card);
        let parsed1 = vcard_to_card(&vcard1).expect("email vcard1 parse");
        let vcard2 = card_to_vcard(&parsed1);
        let parsed2 = vcard_to_card(&vcard2).expect("email vcard2 parse");
        let vcard3 = card_to_vcard(&parsed2);

        assert_vcard_fixpoint(&vcard2, &vcard3)?;
        assert_card_fixpoint(&parsed1, &parsed2)?;
    }

    #[test]
    fn prop_fixpoint_address_and_label_domain(
        addresses in prop::collection::btree_map(arb_key(), arb_address(), 1..4)
    ) {
        let card = ContactCard {
            id: Some("C-ADR".into()),
            addresses: Some(addresses),
            ..ContactCard::default()
        };
        let vcard1 = card_to_vcard(&card);
        let parsed1 = vcard_to_card(&vcard1).expect("address vcard1 parse");
        let vcard2 = card_to_vcard(&parsed1);
        let parsed2 = vcard_to_card(&vcard2).expect("address vcard2 parse");
        let vcard3 = card_to_vcard(&parsed2);

        assert_vcard_fixpoint(&vcard2, &vcard3)?;
        assert_card_fixpoint(&parsed1, &parsed2)?;
    }

    #[test]
    fn prop_fixpoint_organization_domain(
        organizations in prop::collection::btree_map(arb_key(), arb_organization(), 1..3)
    ) {
        let card = ContactCard {
            id: Some("C-ORG".into()),
            organizations: Some(organizations),
            ..ContactCard::default()
        };
        let vcard1 = card_to_vcard(&card);
        let parsed1 = vcard_to_card(&vcard1).expect("org vcard1 parse");
        let vcard2 = card_to_vcard(&parsed1);
        let parsed2 = vcard_to_card(&vcard2).expect("org vcard2 parse");
        let vcard3 = card_to_vcard(&parsed2);

        assert_vcard_fixpoint(&vcard2, &vcard3)?;
        assert_card_fixpoint(&parsed1, &parsed2)?;
    }

    #[test]
    fn prop_fixpoint_relation_domain(
        relations in prop::collection::btree_map(arb_key(), arb_relation(), 1..4)
    ) {
        let card = ContactCard {
            id: Some("C-REL".into()),
            related_to: Some(relations),
            ..ContactCard::default()
        };
        let vcard1 = card_to_vcard(&card);
        let parsed1 = vcard_to_card(&vcard1).expect("relation vcard1 parse");
        let vcard2 = card_to_vcard(&parsed1);
        let parsed2 = vcard_to_card(&vcard2).expect("relation vcard2 parse");
        let vcard3 = card_to_vcard(&parsed2);

        assert_vcard_fixpoint(&vcard2, &vcard3)?;
        assert_card_fixpoint(&parsed1, &parsed2)?;
    }

    #[test]
    fn prop_fixpoint_anniversary_domain(
        anniversaries in prop::collection::btree_map(arb_key(), arb_anniversary(), 1..4)
    ) {
        let card = ContactCard {
            id: Some("C-ANNIV".into()),
            anniversaries: Some(anniversaries),
            ..ContactCard::default()
        };
        let vcard1 = card_to_vcard(&card);
        let parsed1 = vcard_to_card(&vcard1).expect("anniversary vcard1 parse");
        let vcard2 = card_to_vcard(&parsed1);
        let parsed2 = vcard_to_card(&vcard2).expect("anniversary vcard2 parse");
        let vcard3 = card_to_vcard(&parsed2);

        assert_vcard_fixpoint(&vcard2, &vcard3)?;
        assert_card_fixpoint(&parsed1, &parsed2)?;
    }

    #[test]
    fn prop_fixpoint_categories_domain(
        keywords in prop::collection::btree_map(
            arb_keyword_tag(),
            prop_oneof![Just(json!(true)), Just(json!(false)), Just(json!("tag")), Just(json!(1))],
            1..6,
        )
    ) {
        let card = ContactCard {
            id: Some("C-CAT".into()),
            keywords: Some(keywords),
            ..ContactCard::default()
        };
        let vcard1 = card_to_vcard(&card);
        let parsed1 = vcard_to_card(&vcard1).expect("categories vcard1 parse");
        let vcard2 = card_to_vcard(&parsed1);
        let parsed2 = vcard_to_card(&vcard2).expect("categories vcard2 parse");
        let vcard3 = card_to_vcard(&parsed2);

        assert_vcard_fixpoint(&vcard2, &vcard3)?;
        assert_card_fixpoint(&parsed1, &parsed2)?;
    }

    #[test]
    fn prop_fixpoint_notes_escaping_domain(
        notes in prop::collection::btree_map(arb_key(), arb_note(), 1..3)
    ) {
        let card = ContactCard {
            id: Some("C-NOTE".into()),
            notes: Some(notes),
            ..ContactCard::default()
        };
        let vcard1 = card_to_vcard(&card);
        let parsed1 = vcard_to_card(&vcard1).expect("notes vcard1 parse");
        let vcard2 = card_to_vcard(&parsed1);
        let parsed2 = vcard_to_card(&vcard2).expect("notes vcard2 parse");
        let vcard3 = card_to_vcard(&parsed2);

        assert_vcard_fixpoint(&vcard2, &vcard3)?;
        assert_card_fixpoint(&parsed1, &parsed2)?;
    }

    #[test]
    fn prop_fixpoint_online_services_domain(
        online_services in prop::collection::btree_map(arb_key(), arb_online_service(), 1..4)
    ) {
        let card = ContactCard {
            id: Some("C-IM".into()),
            online_services: Some(online_services),
            ..ContactCard::default()
        };
        let vcard1 = card_to_vcard(&card);
        let parsed1 = vcard_to_card(&vcard1).expect("im vcard1 parse");
        let vcard2 = card_to_vcard(&parsed1);
        let parsed2 = vcard_to_card(&vcard2).expect("im vcard2 parse");
        let vcard3 = card_to_vcard(&parsed2);

        assert_vcard_fixpoint(&vcard2, &vcard3)?;
        assert_card_fixpoint(&parsed1, &parsed2)?;
    }
}
