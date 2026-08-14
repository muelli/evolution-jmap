// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSContact `ContactCard` ↔ vCard 3.0, the minimal property set the
//! address book backend needs: UID, FN, N, NICKNAME, EMAIL, TEL, ADR, LABEL,
//! ORG, TITLE, ROLE, NOTE, BDAY, URL, CALURI, FBURL, PHOTO, CATEGORIES and the
//! `X-` lines EDS keeps instant-messaging handles and the spouse on.

use jmap_proto::contacts::{
    Address, AddressComponent, Anniversary, Calendar, ContactCard, ContactEmail, ContactPhone,
    Link, Media, Name, NameComponent, Nickname, Note, OnlineService, OrgUnit, Organization,
    Relation, Title,
};
use jmap_vcard::{card_to_vcard, states_keyword, states_media, vcard_to_card};
use serde_json::{Value, json};

fn fixture_card() -> ContactCard {
    let path = format!(
        "{}/tests/fixtures/contact_card.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn line<'a>(vcard: &'a str, prefix: &str) -> &'a str {
    vcard
        .split("\r\n")
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no line starting {prefix} in\n{vcard}"))
}

#[test]
fn emits_a_vcard_30_envelope() {
    let vcard = card_to_vcard(&fixture_card());
    assert!(
        vcard.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"),
        "{vcard}"
    );
    assert!(vcard.ends_with("END:VCARD\r\n"), "{vcard}");
}

#[test]
fn uid_is_the_jmap_id_and_the_jscontact_uid_is_kept_aside() {
    // EDS keys its cache on the vCard UID and hands it back to
    // load_contact_sync/remove_contact_sync, so it has to be the identifier
    // the JMAP methods take: the server-assigned id.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(line(&vcard, "UID:"), "UID:C1");
    assert_eq!(
        line(&vcard, "X-JMAP-UID:"),
        "X-JMAP-UID:urn:uuid:ab4310aa-fa43-11e9-8f0b-362b9e155667"
    );

    let card = vcard_to_card(&vcard).expect("parse");
    assert_eq!(card.id.as_ref().unwrap().as_str(), "C1");
    assert_eq!(
        card.uid.as_deref(),
        Some("urn:uuid:ab4310aa-fa43-11e9-8f0b-362b9e155667")
    );
}

#[test]
fn a_card_without_a_jscontact_uid_omits_the_extra_property() {
    let card = ContactCard {
        id: Some("C9".into()),
        ..ContactCard::default()
    };
    let vcard = card_to_vcard(&card);
    assert_eq!(line(&vcard, "UID:"), "UID:C9");
    assert!(!vcard.contains("X-JMAP-UID"), "{vcard}");

    // Reading it back, the JMAP id is the only identifier there is.
    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(back.id.as_ref().unwrap().as_str(), "C9");
    assert_eq!(back.uid, None);
}

#[test]
fn a_vcard_from_evolution_has_no_jmap_id_yet() {
    // Evolution invents a UID for a contact the user just typed in; it is
    // not a JMAP id, so the caller must be able to tell the two apart.
    let vcard = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:pas-id-6890AB\r\nFN:Vera\r\nEND:VCARD\r\n";
    let card = vcard_to_card(vcard).expect("parse");
    assert_eq!(card.id.as_ref().unwrap().as_str(), "pas-id-6890AB");
    assert_eq!(card.name.unwrap().full.as_deref(), Some("Vera"));
}

#[test]
fn maps_name_components_onto_the_structured_n_property() {
    let card = ContactCard {
        name: Some(Name {
            full: Some("Dr. Vera Marie Oldenburg MSc".to_owned()),
            components: Some(vec![
                NameComponent::new("title", "Dr."),
                NameComponent::new("given", "Vera"),
                NameComponent::new("given2", "Marie"),
                NameComponent::new("surname", "Oldenburg"),
                NameComponent::new("credential", "MSc"),
            ]),
            ..Name::default()
        }),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert_eq!(line(&vcard, "N:"), "N:Oldenburg;Vera;Marie;Dr.;MSc");
    assert_eq!(line(&vcard, "FN:"), "FN:Dr. Vera Marie Oldenburg MSc");

    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(back.name.as_ref().unwrap(), card.name.as_ref().unwrap());
}

#[test]
fn derives_the_missing_half_of_the_name() {
    // No `full`: build the display name from the components, in reading
    // order, so Evolution has something to show.
    let components = ContactCard {
        name: Some(Name {
            components: Some(vec![
                NameComponent::new("surname", "Oldenburg"),
                NameComponent::new("given", "Vera"),
            ]),
            ..Name::default()
        }),
        ..ContactCard::default()
    };
    assert_eq!(
        line(&card_to_vcard(&components), "FN:"),
        "FN:Vera Oldenburg"
    );

    // No N in the vCard: `full` is all we know, and guessing at a split
    // would be worse than leaving the components unset.
    let card = vcard_to_card("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera Oldenburg\r\nEND:VCARD\r\n")
        .expect("parse");
    let name = card.name.unwrap();
    assert_eq!(name.full.as_deref(), Some("Vera Oldenburg"));
    assert_eq!(name.components, None);
}

#[test]
fn a_card_with_no_name_at_all_emits_no_fn_or_n() {
    let vcard = card_to_vcard(&ContactCard::default());
    assert!(!vcard.contains("\r\nFN"), "{vcard}");
    assert!(!vcard.contains("\r\nN:"), "{vcard}");
    assert_eq!(vcard_to_card(&vcard).expect("parse").name, None);
}

#[test]
fn maps_emails_with_their_contexts() {
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(
        line(&vcard, "EMAIL"),
        "EMAIL;X-JMAP-KEY=work;TYPE=WORK,PREF:vera@example.com"
    );

    let card = vcard_to_card(&vcard).expect("parse");
    let emails = card.emails.expect("emails");
    let email = &emails["work"];
    assert_eq!(email.address, "vera@example.com");
    assert_eq!(email.pref, Some(1));
    assert_eq!(email.contexts, Some(json!({"work": true})));
}

#[test]
fn maps_phones_with_their_features() {
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(
        line(&vcard, "TEL"),
        "TEL;X-JMAP-KEY=mobile;TYPE=HOME,CELL:+49 30 123456"
    );

    let card = vcard_to_card(&vcard).expect("parse");
    let phones = card.phones.expect("phones");
    let phone = &phones["mobile"];
    assert_eq!(phone.number, "+49 30 123456");
    assert_eq!(phone.contexts, Some(json!({"private": true})));
    assert_eq!(phone.features, Some(json!({"mobile": true})));
}

#[test]
fn keys_survive_the_round_trip_so_patches_address_the_right_entry() {
    // A JSContact patch names the map key ("emails/work/address"), so
    // losing the key would turn every edit into a remove-and-re-add.
    let card = fixture_card();
    let back = vcard_to_card(&card_to_vcard(&card)).expect("parse");
    assert_eq!(
        back.emails.unwrap().keys().collect::<Vec<_>>(),
        card.emails.unwrap().keys().collect::<Vec<_>>()
    );
}

#[test]
fn invents_keys_for_entries_that_have_none() {
    // A vCard straight from Evolution carries no X-JMAP-KEY.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "EMAIL;TYPE=HOME:vera@example.org\r\n",
        "EMAIL:vera@example.net\r\n",
        "TEL;TYPE=VOICE,FAX:+49 30 111\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let emails = card.emails.expect("emails");
    assert_eq!(emails.len(), 2);
    let addresses: Vec<&str> = emails.values().map(|e| e.address.as_str()).collect();
    assert!(addresses.contains(&"vera@example.org"));
    assert!(addresses.contains(&"vera@example.net"));
    assert!(
        emails
            .values()
            .any(|e| e.contexts == Some(json!({"private": true})))
    );
    // No TYPE at all means no contexts, not an empty object.
    assert!(emails.values().any(|e| e.contexts.is_none()));

    let phone = card.phones.expect("phones").into_values().next().unwrap();
    assert_eq!(phone.features, Some(json!({"voice": true, "fax": true})));
}

#[test]
fn an_email_without_an_address_is_skipped_in_both_directions() {
    let card = ContactCard {
        emails: Some(
            [
                ("e1".to_owned(), ContactEmail::default()),
                (
                    "e2".to_owned(),
                    ContactEmail {
                        address: "vera@example.com".to_owned(),
                        ..ContactEmail::default()
                    },
                ),
            ]
            .into(),
        ),
        phones: Some([("p1".to_owned(), ContactPhone::default())].into()),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert_eq!(vcard.matches("\r\nEMAIL").count(), 1, "{vcard}");
    assert!(!vcard.contains("\r\nTEL"), "{vcard}");

    let back =
        vcard_to_card("BEGIN:VCARD\r\nVERSION:3.0\r\nEMAIL:\r\nEND:VCARD\r\n").expect("parse");
    assert_eq!(back.emails, None);
}

#[test]
fn maps_organizations_onto_the_structured_org_property() {
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(line(&vcard, "ORG"), "ORG;X-JMAP-KEY=o1:Example GmbH");

    let card = vcard_to_card(&vcard).expect("parse");
    let organizations = card.organizations.expect("organizations");
    assert_eq!(organizations["o1"].name.as_deref(), Some("Example GmbH"));
    assert_eq!(organizations["o1"].units, None);
}

#[test]
fn maps_organization_units_onto_the_components_after_the_name() {
    // RFC 2426 §3.5.5: the ORG value is the organisation's name followed by
    // its units, outermost first — which is the order RFC 9553 §2.2.3 gives
    // `units` too.
    let card = ContactCard {
        organizations: Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some("Example GmbH".to_owned()),
                    units: Some(vec![OrgUnit::new("Research"), OrgUnit::new("Optics")]),
                    ..Organization::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert_eq!(
        line(&vcard, "ORG"),
        "ORG;X-JMAP-KEY=o1:Example GmbH;Research;Optics"
    );

    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(back.organizations, card.organizations);
}

#[test]
fn an_organization_with_units_but_no_name_keeps_the_field_it_leaves_empty() {
    // The name is the first component, so a unit cannot move up into its
    // place without changing which organisation is meant.
    let card = ContactCard {
        organizations: Some(
            [(
                "o1".to_owned(),
                Organization {
                    units: Some(vec![OrgUnit::new("Research")]),
                    ..Organization::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert_eq!(line(&vcard, "ORG"), "ORG;X-JMAP-KEY=o1:;Research");

    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(back.organizations, card.organizations);
}

#[test]
fn an_organization_with_nothing_in_it_is_skipped_in_both_directions() {
    let card = ContactCard {
        organizations: Some([("o1".to_owned(), Organization::default())].into()),
        ..ContactCard::default()
    };
    assert!(!card_to_vcard(&card).contains("\r\nORG"));

    let back =
        vcard_to_card("BEGIN:VCARD\r\nVERSION:3.0\r\nORG:;;\r\nEND:VCARD\r\n").expect("parse");
    assert_eq!(back.organizations, None);
}

#[test]
fn invents_a_key_for_an_organization_that_has_none() {
    // A vCard straight from Evolution carries no X-JMAP-KEY, and Evolution
    // writes the department in the second component.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "ORG:Example GmbH;Research\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let organizations = card.organizations.expect("organizations");
    assert_eq!(organizations.keys().collect::<Vec<_>>(), vec!["o1"]);
    assert_eq!(organizations["o1"].name.as_deref(), Some("Example GmbH"));
    assert_eq!(
        organizations["o1"].units,
        Some(vec![OrgUnit::new("Research")])
    );
}

#[test]
fn maps_titles_onto_title_and_role_by_their_kind() {
    // RFC 9553 §2.2.4 holds the job title and the role played in one map,
    // told apart by `kind`; vCard 3.0 has a property for each (RFC 2426
    // §§3.5.1–3.5.2).
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(
        line(&vcard, "TITLE"),
        "TITLE;X-JMAP-KEY=t1:Research Scientist"
    );
    assert_eq!(line(&vcard, "ROLE"), "ROLE;X-JMAP-KEY=t2:Project Lead");

    let card = vcard_to_card(&vcard).expect("parse");
    let titles = card.titles.expect("titles");
    assert_eq!(titles["t1"].name, "Research Scientist");
    assert_eq!(
        titles["t1"].kind, None,
        "`title` is the default kind, so the card leaves it unsaid"
    );
    assert_eq!(titles["t2"].name, "Project Lead");
    assert_eq!(titles["t2"].kind.as_deref(), Some("role"));
}

#[test]
fn a_title_of_a_kind_vcard_has_no_property_for_is_dropped() {
    // RFC 9553 §2.2.4 allows vendor kinds besides `title` and `role`, and
    // vCard 3.0 has nowhere to put one. Writing it as a TITLE would tell the
    // user it is their job title, which it is not.
    let card = ContactCard {
        titles: Some(
            [(
                "t1".to_owned(),
                Title {
                    name: "Knight of the Realm".to_owned(),
                    kind: Some("x-honour".to_owned()),
                    ..Title::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert!(!vcard.contains("Knight"), "{vcard}");
    assert!(!vcard.contains("\r\nTITLE"), "{vcard}");
    assert!(!vcard.contains("\r\nROLE"), "{vcard}");
}

#[test]
fn a_title_with_no_name_is_skipped_in_both_directions() {
    let card = ContactCard {
        titles: Some([("t1".to_owned(), Title::default())].into()),
        ..ContactCard::default()
    };
    assert!(!card_to_vcard(&card).contains("\r\nTITLE"));

    let back =
        vcard_to_card("BEGIN:VCARD\r\nVERSION:3.0\r\nTITLE:\r\nEND:VCARD\r\n").expect("parse");
    assert_eq!(back.titles, None);
}

#[test]
fn invents_a_key_for_a_title_that_has_none() {
    // A vCard straight from Evolution carries no X-JMAP-KEY, and its two
    // separate fields land in one JSContact map.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "TITLE:Research Scientist\r\n",
        "ROLE:Project Lead\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let titles = card.titles.expect("titles");
    assert_eq!(titles.keys().collect::<Vec<_>>(), vec!["t1", "t2"]);
    assert_eq!(titles["t1"].name, "Research Scientist");
    assert_eq!(titles["t1"].kind, None);
    assert_eq!(titles["t2"].name, "Project Lead");
    assert_eq!(titles["t2"].kind.as_deref(), Some("role"));
}

/// The kinds and values of an address's components, in the order it lists
/// them.
fn components_of(address: &Address) -> Vec<(&str, &str)> {
    address
        .components
        .iter()
        .flatten()
        .map(|component| (component.kind.as_str(), component.value.as_str()))
        .collect()
}

fn one_address(key: &str, address: Address) -> ContactCard {
    ContactCard {
        addresses: Some([(key.to_owned(), address)].into()),
        ..ContactCard::default()
    }
}

#[test]
fn maps_addresses_onto_the_structured_adr_property() {
    // RFC 2426 §3.2.1's seven fields, in order: post office box, extended
    // address, street, locality, region, postal code, country.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(
        line(&vcard, "ADR"),
        "ADR;X-JMAP-KEY=a1;TYPE=WORK:;;Hauptstraße 1;Berlin;;10115;Germany"
    );

    let card = vcard_to_card(&vcard).expect("parse");
    let addresses = card.addresses.expect("addresses");
    let address = &addresses["a1"];
    assert_eq!(address.contexts, Some(json!({"work": true})));
    assert_eq!(
        components_of(address),
        vec![
            ("name", "Hauptstraße 1"),
            ("locality", "Berlin"),
            ("postcode", "10115"),
            ("country", "Germany"),
        ]
    );
}

#[test]
fn maps_every_field_the_adr_value_has() {
    // A vCard straight from Evolution carries no X-JMAP-KEY, and fills in
    // fields the fixture leaves empty.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "ADR;TYPE=HOME:PO Box 12;Apt 4;Hauptstraße 1;Berlin;Brandenburg;10115;Germany\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let addresses = card.addresses.as_ref().expect("addresses");
    assert_eq!(addresses.keys().collect::<Vec<_>>(), vec!["a1"]);
    let address = &addresses["a1"];
    assert_eq!(address.contexts, Some(json!({"private": true})));
    assert_eq!(
        components_of(address),
        vec![
            ("postOfficeBox", "PO Box 12"),
            ("apartment", "Apt 4"),
            ("name", "Hauptstraße 1"),
            ("locality", "Berlin"),
            ("region", "Brandenburg"),
            ("postcode", "10115"),
            ("country", "Germany"),
        ]
    );

    let back = vcard_to_card(&card_to_vcard(&card)).expect("parse");
    assert_eq!(back.addresses, card.addresses);
}

#[test]
fn an_address_component_of_a_kind_the_adr_value_has_no_field_for_is_dropped() {
    // RFC 9553 §2.5.1 has kinds — floor, room, landmark — that vCard's seven
    // fields cannot state. Putting one in a field it does not belong to would
    // misplace it, so it stays off the line, and an address made of nothing
    // else has no line at all.
    let card = one_address(
        "a1",
        Address {
            components: Some(vec![
                AddressComponent::new("name", "Hauptstraße"),
                AddressComponent::new("floor", "3"),
            ]),
            ..Address::default()
        },
    );
    assert_eq!(
        line(&card_to_vcard(&card), "ADR"),
        "ADR;X-JMAP-KEY=a1:;;Hauptstraße;;;;"
    );

    let only_unmapped = one_address(
        "a1",
        Address {
            components: Some(vec![AddressComponent::new("floor", "3")]),
            ..Address::default()
        },
    );
    assert!(!card_to_vcard(&only_unmapped).contains("\r\nADR"));
}

#[test]
fn a_house_number_stated_apart_from_its_street_shares_the_street_field() {
    // RFC 9553 §2.5.1 lets a card name the street and the house number as two
    // components; RFC 2426 §3.2.1 gives the street address one field. Leaving
    // the number off would take the house out of the address the user reads,
    // so both go on the field, in the order the card lists them — which is
    // the only thing that says whether the number is read before the street
    // name or after it.
    let english = one_address(
        "a1",
        Address {
            components: Some(vec![
                AddressComponent::new("number", "1"),
                AddressComponent::new("name", "Main Street"),
            ]),
            ..Address::default()
        },
    );
    assert_eq!(
        line(&card_to_vcard(&english), "ADR"),
        "ADR;X-JMAP-KEY=a1:;;1 Main Street;;;;"
    );

    let german = one_address(
        "a1",
        Address {
            components: Some(vec![
                AddressComponent::new("name", "Hauptstraße"),
                AddressComponent::new("number", "1"),
            ]),
            ..Address::default()
        },
    );
    let vcard = card_to_vcard(&german);
    assert_eq!(line(&vcard, "ADR"), "ADR;X-JMAP-KEY=a1:;;Hauptstraße 1;;;;");

    // Read back, the field is one street name again: nothing in
    // "Hauptstraße 1" says where the number ends and the street begins, and
    // a guess would be wrong in half the world's addresses. Putting the parts
    // back together is the save path's job, and it does it by asking whether
    // the field still says what they said — see the book-sync save tests.
    let back = vcard_to_card(&vcard).expect("parse");
    let addresses = back.addresses.expect("addresses");
    assert_eq!(
        components_of(&addresses["a1"]),
        vec![("name", "Hauptstraße 1")]
    );
}

#[test]
fn an_address_that_states_only_a_house_number_is_on_the_line() {
    // The number is not decoration on a street name: an address holding
    // nothing else is still something to show, so it gets an ADR of its own
    // rather than being counted invisible and hidden from the save.
    let card = one_address(
        "a1",
        Address {
            components: Some(vec![AddressComponent::new("number", "1")]),
            ..Address::default()
        },
    );
    assert_eq!(
        line(&card_to_vcard(&card), "ADR"),
        "ADR;X-JMAP-KEY=a1:;;1;;;;"
    );
}

#[test]
fn two_address_components_of_one_kind_share_the_field_they_map_onto() {
    let card = one_address(
        "a1",
        Address {
            components: Some(vec![
                AddressComponent::new("name", "Hauptstraße 1"),
                AddressComponent::new("name", "Hinterhaus"),
            ]),
            ..Address::default()
        },
    );
    assert_eq!(
        line(&card_to_vcard(&card), "ADR"),
        "ADR;X-JMAP-KEY=a1:;;Hauptstraße 1 Hinterhaus;;;;"
    );
}

#[test]
fn an_address_with_nothing_in_it_is_skipped_in_both_directions() {
    let card = one_address("a1", Address::default());
    assert!(!card_to_vcard(&card).contains("\r\nADR"));

    let back =
        vcard_to_card("BEGIN:VCARD\r\nVERSION:3.0\r\nADR:;;;;;;\r\nEND:VCARD\r\n").expect("parse");
    assert_eq!(back.addresses, None);
}

#[test]
fn maps_the_written_out_address_onto_the_label_property() {
    // RFC 9553 §2.5.1's `full` is the address as it should be printed on an
    // envelope, which is exactly what RFC 2426 §3.2.2's `LABEL` states — and
    // what EDS keeps in E_CONTACT_ADDRESS_LABEL_WORK. The line breaks in it
    // are the point: they are what makes it a label rather than a street.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(
        line(&vcard, "LABEL"),
        "LABEL;X-JMAP-KEY=a1;TYPE=WORK:Hauptstraße 1\\n10115 Berlin\\nGermany"
    );

    let card = vcard_to_card(&vcard).expect("parse");
    let addresses = card.addresses.expect("addresses");
    assert_eq!(addresses.keys().collect::<Vec<_>>(), vec!["a1"]);
    assert_eq!(
        addresses["a1"].full.as_deref(),
        Some("Hauptstraße 1\n10115 Berlin\nGermany"),
        "the label did not join the address it was written for"
    );
}

#[test]
fn a_label_without_a_key_joins_the_address_of_the_same_type() {
    // The vCard EDS writes back. E_CONTACT_ADDRESS_LABEL_HOME is one of its
    // three synthetic fields, so the line is rebuilt from scratch and the
    // X-JMAP-KEY that named the address is gone. What survives is the TYPE,
    // which is how RFC 2426 §3.2.2 has a LABEL name the ADR it belongs to,
    // so that is what the two are paired on.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "ADR;TYPE=HOME:;;Hauptstraße 1;Berlin;;10115;Germany\r\n",
        "LABEL;TYPE=HOME:Hauptstraße 1\\n10115 Berlin\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let addresses = card.addresses.as_ref().expect("addresses");
    assert_eq!(
        addresses.keys().collect::<Vec<_>>(),
        vec!["a1"],
        "the label was read as an address of its own: {addresses:?}"
    );
    assert_eq!(
        addresses["a1"].full.as_deref(),
        Some("Hauptstraße 1\n10115 Berlin")
    );
    assert_eq!(
        components_of(&addresses["a1"])[0],
        ("name", "Hauptstraße 1")
    );

    let back = vcard_to_card(&card_to_vcard(&card)).expect("parse");
    assert_eq!(back.addresses, card.addresses);
}

#[test]
fn a_label_that_belongs_to_no_address_becomes_one_of_its_own() {
    // A card may hold a written-out address and no components at all — RFC
    // 9553 §2.5.1 allows exactly that, "even if the individual address
    // components are not known". It gets a LABEL line and no ADR, so the key
    // it is filed under crosses on the LABEL or not at all.
    let card = one_address(
        "a2",
        Address {
            full: Some("Postfach 42\n10115 Berlin".to_owned()),
            ..Address::default()
        },
    );
    let vcard = card_to_vcard(&card);
    assert!(!vcard.contains("\r\nADR"), "{vcard}");
    assert_eq!(
        line(&vcard, "LABEL"),
        "LABEL;X-JMAP-KEY=a2:Postfach 42\\n10115 Berlin"
    );

    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(back.addresses, card.addresses);

    // And a label whose TYPE matches no address on the card is not folded
    // into an unrelated one.
    let mixed = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "ADR;TYPE=HOME:;;Hauptstraße 1;Berlin;;10115;Germany\r\n",
        "LABEL;TYPE=WORK:Acme Ltd\\nBerlin\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");
    let addresses = mixed.addresses.expect("addresses");
    assert_eq!(addresses.keys().collect::<Vec<_>>(), vec!["a1", "a2"]);
    assert_eq!(addresses["a1"].full, None);
    assert_eq!(addresses["a2"].full.as_deref(), Some("Acme Ltd\nBerlin"));
    assert_eq!(addresses["a2"].contexts, Some(json!({"work": true})));
}

#[test]
fn an_address_written_out_as_nothing_gets_no_label_line() {
    let card = one_address(
        "a1",
        Address {
            components: Some(vec![AddressComponent::new("locality", "Berlin")]),
            full: Some(String::new()),
            ..Address::default()
        },
    );
    let vcard = card_to_vcard(&card);
    assert!(!vcard.contains("\r\nLABEL"), "{vcard}");

    let back =
        vcard_to_card("BEGIN:VCARD\r\nVERSION:3.0\r\nLABEL:\r\nEND:VCARD\r\n").expect("parse");
    assert_eq!(back.addresses, None);
}

fn one_note(key: &str, note: &str) -> ContactCard {
    ContactCard {
        notes: Some(
            [(
                key.to_owned(),
                Note {
                    note: note.to_owned(),
                    ..Note::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    }
}

#[test]
fn maps_notes_onto_note_lines() {
    // RFC 9553 §2.8.3 keys the notes like every other JSContact map; RFC
    // 2426 §3.6.2's NOTE is plain text, so the key rides in X-JMAP-KEY as it
    // does on an EMAIL.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(line(&vcard, "NOTE"), "NOTE;X-JMAP-KEY=n1:met at FOSDEM");

    let card = vcard_to_card(&vcard).expect("parse");
    let notes = card.notes.expect("notes");
    assert_eq!(notes.keys().collect::<Vec<_>>(), vec!["n1"]);
    assert_eq!(notes["n1"].note, "met at FOSDEM");
}

#[test]
fn a_note_keeps_the_line_breaks_and_separators_it_was_written_with() {
    // A note is the one mapped property that routinely holds prose, so the
    // characters vCard gives structural meaning to are exactly the ones a
    // user types into it.
    let card = one_note("n1", "met at FOSDEM;\nbuys the next round, apparently");
    let vcard = card_to_vcard(&card);
    assert_eq!(
        line(&vcard, "NOTE"),
        "NOTE;X-JMAP-KEY=n1:met at FOSDEM\\;\\nbuys the next round\\, apparently"
    );

    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(
        back.notes.expect("notes")["n1"].note,
        "met at FOSDEM;\nbuys the next round, apparently",
        "a note came back as something other than what was written"
    );
}

#[test]
fn a_note_with_no_text_is_skipped_in_both_directions() {
    let card = one_note("n1", "");
    assert!(!card_to_vcard(&card).contains("\r\nNOTE"));

    let back =
        vcard_to_card("BEGIN:VCARD\r\nVERSION:3.0\r\nNOTE:\r\nEND:VCARD\r\n").expect("parse");
    assert_eq!(back.notes, None);
}

#[test]
fn invents_a_key_for_a_note_that_has_none() {
    // Evolution has one Notes field per contact and writes no X-JMAP-KEY,
    // so a note it typed arrives needing a key of its own.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "NOTE:met at FOSDEM\r\n",
        "NOTE;X-JMAP-KEY=n1:owes me a beer\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let notes = card.notes.expect("notes");
    assert_eq!(notes.keys().collect::<Vec<_>>(), vec!["n1", "n2"]);
    assert_eq!(notes["n1"].note, "met at FOSDEM");
    assert_eq!(notes["n2"].note, "owes me a beer");
}

fn one_anniversary(kind: &str, date: serde_json::Value) -> ContactCard {
    ContactCard {
        anniversaries: Some(
            [(
                "y1".to_owned(),
                Anniversary {
                    kind: kind.to_owned(),
                    date: Some(date),
                    ..Anniversary::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    }
}

#[test]
fn maps_a_birthday_onto_the_bday_line() {
    // RFC 9553 §2.8.1 keeps every memorable date in one map, told apart by
    // `kind`; RFC 2426 §3.1.5's BDAY states the birthday and nothing else.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(line(&vcard, "BDAY"), "BDAY;X-JMAP-KEY=y1:1964-03-27");

    let card = vcard_to_card(&vcard).expect("parse");
    let anniversaries = card.anniversaries.expect("anniversaries");
    assert_eq!(anniversaries["y1"].kind, "birth");
    assert_eq!(
        anniversaries["y1"].date,
        Some(json!({"@type": "PartialDate", "year": 1964, "month": 3, "day": 27}))
    );
}

#[test]
fn maps_a_wedding_day_onto_the_line_evolution_reads_as_the_anniversary() {
    // vCard 3.0 has no property for it — RFC 6474's ANNIVERSARY is vCard 4.0
    // — and EDS reads E_CONTACT_ANNIVERSARY, the field Evolution's contact
    // editor labels "Anniversary", off X-EVOLUTION-ANNIVERSARY. Writing the
    // date anywhere else would keep it out of the only field that shows it.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(
        line(&vcard, "X-EVOLUTION-ANNIVERSARY"),
        "X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=y2:1996-08-03"
    );

    let card = vcard_to_card(&vcard).expect("parse");
    let anniversaries = card.anniversaries.expect("anniversaries");
    assert_eq!(anniversaries["y2"].kind, "wedding");
    assert_eq!(
        anniversaries["y2"].date,
        Some(json!({"@type": "PartialDate", "year": 1996, "month": 8, "day": 3}))
    );
}

#[test]
fn an_anniversary_of_a_kind_vcard_has_no_property_for_is_dropped() {
    // RFC 9553 §2.8.1 also has `death`, which vCard 3.0 states nowhere — RFC
    // 6474's DEATHDATE is vCard 4.0, and EDS has no field for it. Putting the
    // date on a BDAY would tell the user it is a birthday.
    let vcard = card_to_vcard(&one_anniversary(
        "death",
        json!({"year": 2019, "month": 10, "day": 15}),
    ));
    assert!(!vcard.contains("2019"), "{vcard}");
    assert!(!vcard.contains("\r\nBDAY"), "{vcard}");
    assert!(!vcard.contains("ANNIVERSARY"), "{vcard}");
}

#[test]
fn a_date_that_names_no_single_day_gets_no_line() {
    // RFC 9553 §2.8.1's PartialDate may state only a year, or a day in a
    // month with no year. EDS's e_contact_date_from_string reads anything
    // short of a whole date as no date at all and hands the contact editor
    // 1000-01-01 — so a partial date written onto a line would reach the user
    // as a wrong date, and be saved back as one.
    for date in [
        json!({"year": 1964}),
        json!({"month": 3, "day": 27}),
        json!({"year": 1964, "month": 3}),
    ] {
        let vcard = card_to_vcard(&one_anniversary("birth", date.clone()));
        assert!(!vcard.contains("\r\nBDAY"), "{date}: {vcard}");
    }
}

#[test]
fn an_anniversary_stated_as_a_point_in_time_crosses_as_the_day_it_names() {
    // The other shape RFC 9553 §2.8.1 allows. A vCard 3.0 date line states a
    // day, so the hour is left behind — and, being left behind rather than
    // mapped, must survive the save untouched.
    let vcard = card_to_vcard(&one_anniversary(
        "birth",
        json!({"@type": "Timestamp", "utc": "1953-10-15T23:10:00Z"}),
    ));
    assert_eq!(line(&vcard, "BDAY"), "BDAY;X-JMAP-KEY=y1:1953-10-15");
}

#[test]
fn a_date_line_written_without_separators_states_the_same_day() {
    // ISO 8601's basic form, which EDS parses as readily as the extended one,
    // so a vCard from elsewhere may well carry it.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "BDAY:19640327\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    assert_eq!(
        card.anniversaries.expect("anniversaries")["y1"].date,
        Some(json!({"@type": "PartialDate", "year": 1964, "month": 3, "day": 27}))
    );
}

#[test]
fn a_date_line_that_states_no_day_is_skipped_in_both_directions() {
    let card = one_anniversary("birth", json!({}));
    assert!(!card_to_vcard(&card).contains("\r\nBDAY"));

    // Nothing at all, prose, and a date that does not exist. None of them
    // says which day the user meant, and inventing one would write it to the
    // server on the next save.
    for value in ["", "sometime in March", "1964-13-45", "1964-03"] {
        let back = vcard_to_card(&format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nBDAY:{value}\r\nEND:VCARD\r\n"
        ))
        .expect("parse");
        assert_eq!(back.anniversaries, None, "BDAY:{value}");
    }
}

#[test]
fn invents_a_key_for_a_date_that_has_none() {
    // Evolution stores the birthday in a structured field and rebuilds the
    // line from it, dropping the X-JMAP-KEY this side wrote — so a date that
    // has just been edited arrives needing a key of its own.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "BDAY:1964-03-27\r\n",
        "X-EVOLUTION-ANNIVERSARY:1996-08-03\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let anniversaries = card.anniversaries.expect("anniversaries");
    assert_eq!(anniversaries.keys().collect::<Vec<_>>(), vec!["y1", "y2"]);
    assert_eq!(anniversaries["y1"].kind, "birth");
    assert_eq!(anniversaries["y2"].kind, "wedding");
}

#[test]
fn partial_dates_for_wedding_and_death_anniversaries_get_no_vcard_lines() {
    for date in [
        json!({"year": 1996}),
        json!({"month": 8, "day": 3}),
        json!({"year": 1996, "month": 8}),
    ] {
        let vcard = card_to_vcard(&one_anniversary("wedding", date.clone()));
        assert!(
            !vcard.contains("\r\nX-EVOLUTION-ANNIVERSARY"),
            "{date}: {vcard}"
        );
        assert!(!vcard.contains("ANNIVERSARY"), "{date}: {vcard}");
    }

    for date in [
        json!({"year": 2019}),
        json!({"month": 10, "day": 15}),
        json!({"year": 2019, "month": 10, "day": 15}),
    ] {
        let vcard = card_to_vcard(&one_anniversary("death", date.clone()));
        assert!(!vcard.contains("\r\nBDAY"), "{date}: {vcard}");
        assert!(
            !vcard.contains("\r\nX-EVOLUTION-ANNIVERSARY"),
            "{date}: {vcard}"
        );
        assert!(!vcard.contains("DEATHDATE"), "{date}: {vcard}");
    }
}

#[test]
fn multiple_anniversaries_with_unmodeled_death_and_partial_dates_emit_only_full_dates() {
    let card = ContactCard {
        anniversaries: Some(
            [
                (
                    "y1".to_owned(),
                    Anniversary {
                        kind: "birth".to_owned(),
                        date: Some(json!({"year": 1964})),
                        ..Anniversary::default()
                    },
                ),
                (
                    "y2".to_owned(),
                    Anniversary {
                        kind: "death".to_owned(),
                        date: Some(json!({"year": 2019, "month": 10, "day": 15})),
                        ..Anniversary::default()
                    },
                ),
                (
                    "y3".to_owned(),
                    Anniversary {
                        kind: "wedding".to_owned(),
                        date: Some(json!({"year": 1996, "month": 8, "day": 3})),
                        ..Anniversary::default()
                    },
                ),
            ]
            .into(),
        ),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert!(!vcard.contains("BDAY"), "{vcard}");
    assert!(!vcard.contains("death"), "{vcard}");
    assert!(!vcard.contains("DEATHDATE"), "{vcard}");
    assert_eq!(
        line(&vcard, "X-EVOLUTION-ANNIVERSARY"),
        "X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=y3:1996-08-03"
    );
}

fn one_nickname(key: &str, name: &str) -> ContactCard {
    ContactCard {
        nicknames: Some(
            [(
                key.to_owned(),
                Nickname {
                    name: name.to_owned(),
                    ..Nickname::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    }
}

#[test]
fn maps_nicknames_onto_nickname_lines() {
    // RFC 9553 §2.2.2 keys the nicknames like every other JSContact map; RFC
    // 2426 §3.1.3's NICKNAME is text, so the key rides in X-JMAP-KEY as it
    // does on an EMAIL — and unlike a date line, EDS rewrites the value in
    // place and leaves the parameter alone, so the key comes back.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(line(&vcard, "NICKNAME"), "NICKNAME;X-JMAP-KEY=k1:Vee");

    let card = vcard_to_card(&vcard).expect("parse");
    let nicknames = card.nicknames.expect("nicknames");
    assert_eq!(nicknames.keys().collect::<Vec<_>>(), vec!["k1"]);
    assert_eq!(nicknames["k1"].name, "Vee");
}

#[test]
fn each_nickname_gets_a_line_of_its_own() {
    // RFC 2426 §3.1.3 states the nicknames as one comma-separated list, which
    // would leave the entries with no key each and nowhere to carry one. EDS
    // does not read the value as a list anyway — verified against
    // libebook-contacts 3.52, which hands the whole value back as one string
    // and escapes a comma inside it — so a list on one line would reach the
    // user as a single nickname with commas in it.
    let mut card = one_nickname("k1", "Vee");
    card.nicknames.as_mut().expect("nicknames").insert(
        "k2".to_owned(),
        Nickname {
            name: "Vera the Elder".to_owned(),
            ..Nickname::default()
        },
    );
    let vcard = card_to_vcard(&card);

    assert_eq!(
        vcard.matches("\r\nNICKNAME").count(),
        2,
        "one line each, not one list: {vcard}"
    );
    let nicknames = vcard_to_card(&vcard).expect("parse").nicknames.unwrap();
    assert_eq!(nicknames["k1"].name, "Vee");
    assert_eq!(nicknames["k2"].name, "Vera the Elder");
}

#[test]
fn a_nickname_that_reads_as_a_list_is_still_one_nickname() {
    // The other side of the same decision. A comma in the value is escaped on
    // the way out, exactly as EDS escapes it, and read back as the text it
    // was — never split into two entries the server would then be told about.
    let vcard = card_to_vcard(&one_nickname("k1", "Jim, the tall one"));
    assert_eq!(
        line(&vcard, "NICKNAME"),
        "NICKNAME;X-JMAP-KEY=k1:Jim\\, the tall one"
    );

    let nicknames = vcard_to_card(&vcard).expect("parse").nicknames.unwrap();
    assert_eq!(nicknames.len(), 1);
    assert_eq!(nicknames["k1"].name, "Jim, the tall one");

    // And an *unescaped* comma, which is what a vCard from some other client
    // carries when it does mean RFC 2426 §3.1.3's list. It is read as the one
    // text it says rather than as two entries — which is not a shrug at the
    // RFC but agreement with EDS, whose own reader hands the whole value back
    // as one string and re-escapes the comma on the way out. Splitting here
    // would tell the server about a nickname EDS will never show.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "NICKNAME:Jim,Jimmie\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");
    let nicknames = card.nicknames.expect("nicknames");
    assert_eq!(nicknames.len(), 1);
    assert_eq!(nicknames["k1"].name, "Jim,Jimmie");
}

#[test]
fn a_nickname_that_names_nothing_is_skipped_in_both_directions() {
    // The same invisibility every other keyed map has: an entry with no name
    // says no more than an `EMAIL:` with no address, gets no line, and must
    // therefore be invisible to the save as well.
    assert!(!card_to_vcard(&one_nickname("k1", "")).contains("\r\nNICKNAME"));

    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "NICKNAME:\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");
    assert_eq!(card.nicknames, None);
}

#[test]
fn invents_a_key_for_a_nickname_that_has_none() {
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "NICKNAME:Vee\r\n",
        "NICKNAME;X-JMAP-KEY=k7:Vera the Elder\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let nicknames = card.nicknames.expect("nicknames");
    assert_eq!(nicknames["k1"].name, "Vee");
    assert_eq!(nicknames["k7"].name, "Vera the Elder");
}

fn one_link(key: &str, uri: &str) -> ContactCard {
    ContactCard {
        links: Some(
            [(
                key.to_owned(),
                Link {
                    uri: uri.to_owned(),
                    ..Link::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    }
}

#[test]
fn maps_links_onto_url_lines() {
    // RFC 2426 §3.6.8's `URL` is a bare URI with no room for anything else,
    // so the JSContact key rides in X-JMAP-KEY as it does on an EMAIL — and
    // EDS rewrites the value of that line in place, leaving the parameter
    // where it was, verified against libebook-contacts 3.52. Evolution shows
    // the first `URL` as the contact's home page.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(
        line(&vcard, "URL"),
        "URL;X-JMAP-KEY=l1:https://vera.example/"
    );

    let card = vcard_to_card(&vcard).expect("parse");
    let links = card.links.expect("links");
    assert_eq!(links.keys().collect::<Vec<_>>(), vec!["l1"]);
    assert_eq!(links["l1"].uri, "https://vera.example/");
    assert_eq!(
        links["l1"].kind, None,
        "a `URL` states no kind, and the reader must not invent one"
    );
}

#[test]
fn a_link_of_a_kind_no_vcard_30_property_states_gets_no_line() {
    // The fixture's `l2` is RFC 9553 §2.6.3's one defined kind, `contact`: a
    // URI for writing to the person rather than a page about them. RFC 9555
    // §2.6.3 states that on vCard 4.0's `CONTACT-URI`, which the 3.0 reader
    // EDS gives us has never heard of, and putting it on a `URL` would tell
    // the user it is the home page. So it gets no line — the same partial
    // visibility `titles` has — and must therefore be invisible to the save.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(vcard.matches("\r\nURL").count(), 1, "{vcard}");
    assert!(!vcard.contains("write-to-me"), "{vcard}");

    // And a kind this version has never heard of, which is the same case: the
    // mapping states the kinds it knows and leaves the rest to the server.
    let mut card = one_link("l1", "https://vera.example/");
    card.links.as_mut().expect("links").insert(
        "l2".to_owned(),
        Link {
            uri: "https://vera.example/rss".to_owned(),
            kind: Some("example.com:feed".to_owned()),
            ..Link::default()
        },
    );
    let vcard = card_to_vcard(&card);
    assert_eq!(vcard.matches("\r\nURL").count(), 1, "{vcard}");
}

#[test]
fn a_link_that_points_nowhere_is_skipped_in_both_directions() {
    // The same invisibility every other keyed map has: an entry with no URI
    // says no more than an `EMAIL:` with no address, gets no line, and must
    // therefore be invisible to the save as well.
    assert!(!card_to_vcard(&one_link("l1", "")).contains("\r\nURL"));

    // That empty line is not hypothetical: it is what EDS leaves behind when
    // the user clears Evolution's Home Page field, measured against
    // libebook-contacts 3.52, which rewrites the value and keeps the line.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "URL;X-JMAP-KEY=l1:\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");
    assert_eq!(card.links, None);
}

#[test]
fn a_uri_keeps_the_punctuation_a_vcard_value_gives_meaning_to() {
    // A URI may hold a comma or a semicolon — a query string listing tags,
    // say — and a vCard value gives both structural meaning. They are escaped
    // on the way out, exactly as EDS escapes them, and read back as the URI
    // that went in; splitting on either would send the server half a URI.
    let vcard = card_to_vcard(&one_link("l1", "https://vera.example/q?tags=a,b;c"));
    assert_eq!(
        line(&vcard, "URL"),
        "URL;X-JMAP-KEY=l1:https://vera.example/q?tags=a\\,b\\;c"
    );

    let links = vcard_to_card(&vcard).expect("parse").links.unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links["l1"].uri, "https://vera.example/q?tags=a,b;c");
}

#[test]
fn invents_a_key_for_a_link_that_has_none() {
    // Which is what a home page the user has just typed arrives as: EDS
    // writes a fresh `URL` line with no parameters on it at all.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "URL:https://vera.example/\r\n",
        "URL;X-JMAP-KEY=l7:https://vera.example/photos\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let links = card.links.expect("links");
    assert_eq!(links["l1"].uri, "https://vera.example/");
    assert_eq!(links["l7"].uri, "https://vera.example/photos");
}

fn one_calendar(key: &str, kind: Option<&str>, uri: &str) -> ContactCard {
    ContactCard {
        calendars: Some(
            [(
                key.to_owned(),
                Calendar {
                    kind: kind.map(str::to_owned),
                    uri: uri.to_owned(),
                    ..Calendar::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    }
}

#[test]
fn maps_calendars_onto_the_caluri_and_fburl_lines() {
    // RFC 9555 §2.13.2 and §2.13.3 pair a Calendar of kind `calendar` with
    // `CALURI` and one of kind `freeBusy` with `FBURL` — the two lines
    // libebook-contacts 3.52 gives a field of their own, which Evolution shows
    // as the contact's Calendar and Free/Busy addresses. Both are bare URIs
    // with no room for anything else, so the JSContact key rides in
    // X-JMAP-KEY as it does on a `URL`; measured against that version, a set
    // rewrites the value of the first line of each name in place and leaves
    // its parameters — the key included — where they were.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(
        line(&vcard, "CALURI"),
        "CALURI;X-JMAP-KEY=c1:https://vera.example/cal/vera.ics"
    );
    assert_eq!(
        line(&vcard, "FBURL"),
        "FBURL;X-JMAP-KEY=c2:https://vera.example/fb/vera.ifb"
    );

    let card = vcard_to_card(&vcard).expect("parse");
    let calendars = card.calendars.expect("calendars");
    assert_eq!(calendars.keys().collect::<Vec<_>>(), vec!["c1", "c2"]);
    assert_eq!(calendars["c1"].uri, "https://vera.example/cal/vera.ics");
    assert_eq!(calendars["c1"].kind.as_deref(), Some("calendar"));
    assert_eq!(calendars["c2"].uri, "https://vera.example/fb/vera.ifb");
    assert_eq!(
        calendars["c2"].kind.as_deref(),
        Some("freeBusy"),
        "the line the URI was read off is what says which kind it is"
    );
}

#[test]
fn a_calendar_of_a_kind_no_eds_field_holds_gets_no_line() {
    // The fixture's `c3` names no kind at all. RFC 9553 §2.4.1 makes `kind`
    // mandatory and gives it no default, so such an entry does not say whether
    // its URI is a calendar or the free/busy data drawn from one — and the two
    // are different fields. Guessing either would show the user the URI under a
    // heading the server never claimed for it, so it gets no line, the same
    // partial visibility `titles` has, and must be invisible to the save.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(vcard.matches("\r\nCALURI").count(), 1, "{vcard}");
    assert!(!vcard.contains("nameless"), "{vcard}");

    // And a kind this version has never heard of, which is the same case.
    let vcard = card_to_vcard(&one_calendar(
        "c1",
        Some("example.com:tasks"),
        "https://vera.example/tasks",
    ));
    assert!(!vcard.contains("tasks"), "{vcard}");
}

#[test]
fn a_calendar_that_points_nowhere_is_skipped_in_both_directions() {
    // The same invisibility every other keyed map has: an entry with no URI
    // says no more than an `EMAIL:` with no address, gets no line, and must
    // therefore be invisible to the save as well.
    let vcard = card_to_vcard(&one_calendar("c1", Some("calendar"), ""));
    assert!(!vcard.contains("\r\nCALURI"), "{vcard}");

    // That empty line is not hypothetical: it is what EDS leaves behind when
    // the user clears Evolution's Calendar field, measured against
    // libebook-contacts 3.52, which rewrites the value and keeps the line.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "CALURI;X-JMAP-KEY=c1:\r\n",
        "FBURL;X-JMAP-KEY=c2:\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");
    assert_eq!(card.calendars, None);
}

#[test]
fn invents_a_key_for_a_calendar_that_has_none() {
    // Which is what an address the user has just typed arrives as: EDS writes
    // a fresh `CALURI` line with no parameters on it at all, measured against
    // libebook-contacts 3.52. The two lines are one keyed map, so the keys the
    // reader invents have to be free of each other's as well as of the ones
    // the card already carries.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "CALURI:https://vera.example/cal/vera.ics\r\n",
        "FBURL:https://vera.example/fb/vera.ifb\r\n",
        "CALURI;X-JMAP-KEY=c7:https://vera.example/cal/team.ics\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let calendars = card.calendars.expect("calendars");
    assert_eq!(calendars["c1"].uri, "https://vera.example/cal/vera.ics");
    assert_eq!(calendars["c1"].kind.as_deref(), Some("calendar"));
    assert_eq!(calendars["c2"].uri, "https://vera.example/fb/vera.ifb");
    assert_eq!(calendars["c2"].kind.as_deref(), Some("freeBusy"));
    // The second `CALURI` line is read too, though EDS shows the user only the
    // first: a line nobody can edit is still one a save must not delete.
    assert_eq!(calendars["c7"].uri, "https://vera.example/cal/team.ics");
}

fn one_relation(key: &str, types: &[&str]) -> ContactCard {
    ContactCard {
        related_to: Some(
            [(
                key.to_owned(),
                Relation {
                    relation: Some(
                        types
                            .iter()
                            .map(|kind| ((*kind).to_owned(), json!(true)))
                            .collect(),
                    ),
                    ..Relation::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    }
}

fn spouse_of(vcard: &str) -> Option<String> {
    let card = vcard_to_card(vcard).expect("parse");
    let related = card.related_to?;
    let (key, _) = related.into_iter().next()?;
    Some(key)
}

#[test]
fn maps_a_spouse_onto_the_line_eds_keeps_the_spouse_on() {
    // RFC 9553 §2.1.8's `relatedTo` states who else an entity is related to,
    // and of the twenty relation types it lists, `spouse` is the one Evolution
    // has a field for: `E_CONTACT_SPOUSE`, which libebook-contacts 3.52 keeps
    // on `X-EVOLUTION-SPOUSE` and the contact editor labels "Spouse". vCard 3.0
    // has no `RELATED` at all — RFC 6350 §6.6.6 is 4.0 — so the `X-` line is not
    // a shortcut, it is the only line there is.
    //
    // And it carries no X-JMAP-KEY, which every other keyed map here needs:
    // §2.1.8 keys the map by *who the related entity is*, so the name on the
    // line is the key, and there is nothing left for a parameter to say.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(
        line(&vcard, "X-EVOLUTION-SPOUSE"),
        "X-EVOLUTION-SPOUSE:Jean Paul Oldenburg"
    );

    let card = vcard_to_card(&vcard).expect("parse");
    let related = card.related_to.expect("relatedTo");
    assert_eq!(
        related.keys().collect::<Vec<_>>(),
        vec!["Jean Paul Oldenburg"]
    );
    let relation = related["Jean Paul Oldenburg"]
        .relation
        .as_ref()
        .expect("relation");
    assert_eq!(
        relation.keys().collect::<Vec<_>>(),
        vec!["spouse"],
        "the line says the entity is a spouse and nothing else, so the reader \
         may not claim the `kin` the fixture also states — a save has to patch \
         into the set rather than replace it"
    );
    assert_eq!(relation["spouse"], Value::Bool(true));
}

#[test]
fn a_relation_no_eds_field_holds_gets_no_line() {
    // The fixture's `Nils Oldenburg` is a `child`. Nineteen of RFC 9553
    // §2.1.8's twenty relation types are like that: no vCard 3.0 property and
    // no EDS field states them, and putting a name on the spouse line would
    // tell the user something the card never said. Evolution's Manager and
    // Assistant fields are the near misses, and §2.1.8 has no type meaning
    // either — `agent` is whoever acts on the contact's behalf, which is wider
    // than an assistant, so it stays off the line too.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(vcard.matches("X-EVOLUTION-SPOUSE").count(), 1, "{vcard}");
    assert!(!vcard.contains("Nils"), "{vcard}");

    // Nor does a relation stating no type at all reach a line: RFC 9555 §2.9.5
    // reads a `RELATED` line carrying no `TYPE` into exactly that, and an
    // unspecified relation is not a marriage.
    let vcard = card_to_vcard(&one_relation("Jean Paul Oldenburg", &[]));
    assert!(!vcard.contains("SPOUSE"), "{vcard}");
    let vcard = card_to_vcard(&ContactCard {
        related_to: Some([("Jean Paul Oldenburg".to_owned(), Relation::default())].into()),
        ..ContactCard::default()
    });
    assert!(!vcard.contains("SPOUSE"), "{vcard}");
}

#[test]
fn a_spouse_the_card_names_by_uid_gets_no_line() {
    // The fixture's third entry is a spouse the way RFC 9553 §2.1.8 asks for
    // one: keyed by the related Card's `uid`. There is no name in a UID, so the
    // line would show the user a URN under the heading "Spouse" — and the next
    // save would write it back as the person's name. RFC 9555 §2.9.5 is what
    // says the other kind of key exists: a `RELATED;VALUE=text` becomes a key
    // holding free text, which is the case that holds a name.
    let vcard = card_to_vcard(&fixture_card());
    assert!(!vcard.contains("e1f0a1c2"), "{vcard}");

    // Any URI, not just a URN: what disqualifies the key is that it names an
    // identifier rather than a person.
    for key in [
        "urn:uuid:e1f0a1c2-0f6b-4d2e-9c3a-2b1f9d0e7c44",
        "mailto:jean@example.com",
        "https://vera.example/jean",
        "XMPP:jean@jabber.example",
    ] {
        let vcard = card_to_vcard(&one_relation(key, &["spouse"]));
        assert!(!vcard.contains("SPOUSE"), "{key}: {vcard}");
    }

    // A name is not a URI for holding a colon in it, only for holding one after
    // something that reads as an RFC 3986 §3.1 scheme.
    let vcard = card_to_vcard(&one_relation("Jean Paul: the second", &["spouse"]));
    assert!(vcard.contains("SPOUSE"), "{vcard}");
}

#[test]
fn a_spouse_whose_name_eds_would_rename_gets_no_line() {
    // The name is the key, so a name EDS gives back spelled differently is a
    // *different entry* to the save — it would delete the relation the server
    // holds and add one under the renamed key. Three spellings do that, all
    // measured or forced on this side: ends made of ASCII whitespace are
    // trimmed by EDS, as they are on an instant-messaging handle; a carriage
    // return is dropped by `syntax::write`; and the empty name says nothing at
    // all.
    for key in [" Jean Paul Oldenburg", "Jean Paul Oldenburg\t", ""] {
        let vcard = card_to_vcard(&one_relation(key, &["spouse"]));
        assert!(!vcard.contains("SPOUSE"), "[{key}]: {vcard}");
    }
    let vcard = card_to_vcard(&one_relation("Jean\rPaul", &["spouse"]));
    assert!(!vcard.contains("SPOUSE"), "{vcard}");

    // What does survive: a name whose middle holds the whitespace, which is
    // every name with a space in it.
    let vcard = card_to_vcard(&one_relation("Jean Paul Oldenburg", &["spouse"]));
    assert_eq!(spouse_of(&vcard).as_deref(), Some("Jean Paul Oldenburg"));
}

#[test]
fn reads_every_spouse_line_and_keys_each_by_its_own_name() {
    // EDS shows the user the first `X-EVOLUTION-SPOUSE` line and passes any
    // further one through untouched, measured against libebook-contacts 3.52 —
    // so a second line is one a save must not delete. And an X-JMAP-KEY on a
    // line is not the key here: the name is. A line carrying one — from another
    // client, or from a card this mapping wrote before the key stopped being
    // written — is read by its text like any other.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "X-EVOLUTION-SPOUSE:Jean Paul Oldenburg\r\n",
        "X-EVOLUTION-SPOUSE;X-JMAP-KEY=r7:Jeanne Oldenburg\r\n",
        "X-EVOLUTION-SPOUSE:\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let related = card.related_to.expect("relatedTo");
    assert_eq!(
        related.keys().collect::<Vec<_>>(),
        vec!["Jean Paul Oldenburg", "Jeanne Oldenburg"],
        "the empty line states no entity, and EDS leaves one behind when the \
         user clears the field"
    );
    for spouse in related.values() {
        assert_eq!(
            spouse
                .relation
                .as_ref()
                .expect("relation")
                .keys()
                .collect::<Vec<_>>(),
            vec!["spouse"]
        );
    }
}

fn one_photo(uri: &str, media_type: Option<&str>) -> ContactCard {
    photo_of_kind(Some("photo"), uri, media_type)
}

fn photo_of_kind(kind: Option<&str>, uri: &str, media_type: Option<&str>) -> ContactCard {
    ContactCard {
        media: Some(
            [(
                "m1".to_owned(),
                Media {
                    kind: kind.map(str::to_owned),
                    uri: uri.to_owned(),
                    media_type: media_type.map(str::to_owned),
                    ..Media::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    }
}

/// "hello-photo", which stands in for the JPEG a real card carries.
const PAYLOAD: &str = "aGVsbG8tcGhvdG8=";

#[test]
fn maps_a_photo_onto_the_line_evolution_shows_it_on() {
    // The fixture's `m1` is a picture the card carries rather than points at:
    // RFC 9553 §2.6.4 states it as a `data:` URI, and RFC 2426 §3.1.4's `PHOTO`
    // takes the bytes themselves under `ENCODING=b`. That is the only form EDS
    // reads a mime type off — measured against libebook-contacts 3.52,
    // `TYPE=JPEG` arrives as `image/JPEG` — and it is what EDS's own writer
    // emits for a photo the user has just chosen.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(
        line(&vcard, "PHOTO"),
        format!("PHOTO;X-JMAP-KEY=m1;TYPE=jpeg;ENCODING=b:{PAYLOAD}")
    );
}

#[test]
fn a_picture_the_card_only_points_at_crosses_as_a_uri_line() {
    // A `media` entry may name any URI, and one that is not a `data:` URI has
    // no bytes to inline: it goes on the line RFC 2426 §3.1.4 gives a reference,
    // with `VALUE=uri`. The parameter is not decoration — measured against
    // libebook-contacts 3.52, a `PHOTO` whose value is a URI and which does not
    // carry it reaches no field at all, so EDS shows the user no picture.
    let vcard = card_to_vcard(&one_photo("https://vera.example/me.png", None));
    assert_eq!(
        line(&vcard, "PHOTO"),
        "PHOTO;X-JMAP-KEY=m1;VALUE=uri:https://vera.example/me.png"
    );

    // And no `TYPE`: on a URI line EDS reads no mime type off it (measured),
    // and its own writer emits none, so the parameter would state something
    // nothing reads. `TYPE` here means the inlined bytes' media type, once.
    assert!(!vcard.contains("TYPE"), "{vcard}");
}

#[test]
fn the_type_parameter_states_the_subtype_and_nothing_else() {
    // Because EDS builds the mime type by putting `image/` in front of it:
    // measured against libebook-contacts 3.52, `TYPE=image/jpeg` arrives as
    // `image/image/jpeg`, which names no image format at all.
    let vcard = card_to_vcard(&one_photo(
        &format!("data:image/jpeg;base64,{PAYLOAD}"),
        Some("image/png"),
    ));
    assert_eq!(
        line(&vcard, "PHOTO"),
        format!("PHOTO;X-JMAP-KEY=m1;TYPE=png;ENCODING=b:{PAYLOAD}"),
        "the entry's own `mediaType` is what it says the bytes are"
    );

    // With none stated, the `data:` URI states its own (RFC 2397 §3) — and a
    // media type may carry parameters, which are no part of the subtype.
    let vcard = card_to_vcard(&one_photo(
        &format!("data:image/png;charset=binary;base64,{PAYLOAD}"),
        None,
    ));
    assert_eq!(
        line(&vcard, "PHOTO"),
        format!("PHOTO;X-JMAP-KEY=m1;TYPE=png;ENCODING=b:{PAYLOAD}")
    );
}

#[test]
fn bytes_that_are_not_an_image_are_stated_without_a_type() {
    // A `TYPE` reaches EDS as `image/<type>`, so writing one for a media type
    // outside `image/*` would tell the user's address book the bytes are an
    // image format that does not exist. The line is still written — the bytes
    // are what the card carries — and EDS hands them on with no mime type,
    // which its own reader accepts (measured against libebook-contacts 3.52).
    let vcard = card_to_vcard(&one_photo(
        &format!("data:application/pdf;base64,{PAYLOAD}"),
        None,
    ));
    assert_eq!(
        line(&vcard, "PHOTO"),
        format!("PHOTO;X-JMAP-KEY=m1;ENCODING=b:{PAYLOAD}")
    );

    // The same for a `data:` URI that states no media type at all, whose RFC
    // 2397 §2 default is `text/plain`.
    let vcard = card_to_vcard(&one_photo(&format!("data:;base64,{PAYLOAD}"), None));
    assert_eq!(
        line(&vcard, "PHOTO"),
        format!("PHOTO;X-JMAP-KEY=m1;ENCODING=b:{PAYLOAD}")
    );
}

#[test]
fn a_payload_spelled_loosely_is_written_out_as_canonical_base64() {
    // The bytes are decoded and re-encoded rather than copied across, so what
    // the line carries is the canonical spelling of what the URI meant. A
    // `data:` URI is written by hand as often as by a library, and EDS decodes
    // the line with glib's base64 reader rather than with the URI's.
    let vcard = card_to_vcard(&one_photo(
        &format!("data:image/jpeg;base64,{}", PAYLOAD.trim_end_matches('=')),
        None,
    ));
    assert_eq!(
        line(&vcard, "PHOTO"),
        format!("PHOTO;X-JMAP-KEY=m1;TYPE=jpeg;ENCODING=b:{PAYLOAD}")
    );
}

#[test]
fn a_picture_whose_bytes_the_line_cannot_state_gets_no_line() {
    // RFC 2397 §3 lets a `data:` URI spell its data as percent-encoded octets
    // instead of base64, and `ENCODING=b` is the only encoding an EDS-bound
    // vCard 3.0 `PHOTO` carries. Rather than hand EDS a value it would decode
    // into a broken image, such an entry gets no line — the same invisibility
    // every other keyed map has — and must therefore be invisible to the save.
    for uri in [
        "data:image/jpeg,%89PNG%0D%0A",
        "data:image/jpeg;base64,not base64 at all",
        "data:image/jpeg",
        "data:",
        "",
    ] {
        let card = one_photo(uri, None);
        assert!(
            !card_to_vcard(&card).contains("\r\nPHOTO"),
            "{uri} should state no PHOTO line"
        );
        assert!(
            !states_media(&card.media.unwrap()["m1"]),
            "{uri} states no line, and the save has to know it"
        );
    }
}

#[test]
fn a_media_entry_of_a_kind_no_photo_line_states_gets_no_line() {
    // The fixture's `m2` is a `logo`: RFC 9553 §2.6.4 keeps the three kinds of
    // media in one map, and a `PHOTO` line is the picture *of the contact*.
    // Putting a logo — or a sound, or a vendor kind — on it would show the user
    // the wrong image, so only a photo crosses, exactly as only some `titles`
    // and only some `links` do.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(vcard.matches("\r\nPHOTO").count(), 1, "{vcard}");
    assert!(!vcard.contains("logo.png"), "{vcard}");

    for kind in [Some("logo"), Some("sound"), Some("example.com:scan"), None] {
        let card = photo_of_kind(kind, &format!("data:image/jpeg;base64,{PAYLOAD}"), None);
        assert!(
            !card_to_vcard(&card).contains("\r\nPHOTO"),
            "{kind:?} should state no PHOTO line"
        );
        assert!(!states_media(&card.media.unwrap()["m1"]), "{kind:?}");
    }
}

#[test]
fn a_card_with_no_media_states_no_photo() {
    let vcard = card_to_vcard(&ContactCard::default());
    assert!(!vcard.contains("PHOTO"), "{vcard}");
}

/// The one media entry of a card read back from `vcard`.
fn read_photo(vcard: &str) -> Media {
    let card = vcard_to_card(vcard).unwrap_or_else(|e| panic!("{vcard}: {e}"));
    let media = card
        .media
        .unwrap_or_else(|| panic!("no media read from {vcard}"));
    assert_eq!(media.len(), 1, "{media:?}");
    media.into_values().next().expect("one entry")
}

fn photo_line(line: &str) -> Media {
    read_photo(&format!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\n{line}\r\nEND:VCARD\r\n"
    ))
}

#[test]
fn reads_the_inline_photo_line_eds_writes_back_as_a_data_uri() {
    // The picture the *user* chose in Evolution arrives on the line EDS's own
    // writer emits — measured against libebook-contacts 3.52, a photo set with
    // mime type `image/jpeg` is written `PHOTO;TYPE=jpeg;ENCODING=b:…`. It comes
    // back as RFC 9553 §2.6.4 states a picture a card carries: a `data:` URI
    // (RFC 2397), with the media type the `TYPE` named.
    let media = photo_line(&format!("PHOTO;TYPE=jpeg;ENCODING=b:{PAYLOAD}"));
    assert_eq!(media.kind.as_deref(), Some("photo"));
    assert_eq!(media.uri, format!("data:image/jpeg;base64,{PAYLOAD}"));
    assert_eq!(media.media_type.as_deref(), Some("image/jpeg"));

    // `ENCODING=BASE64` is the same encoding spelled the way older exporters
    // spell it, and EDS reads it (measured), so this side does too.
    let media = photo_line(&format!("PHOTO;ENCODING=BASE64;TYPE=PNG:{PAYLOAD}"));
    assert_eq!(media.uri, format!("data:image/PNG;base64,{PAYLOAD}"));
    assert_eq!(media.media_type.as_deref(), Some("image/PNG"));
}

#[test]
fn a_photo_eds_holds_no_media_type_for_reads_back_without_one() {
    // Measured against libebook-contacts 3.52: a photo whose `mime_type` is
    // NULL is written `TYPE="X-EVOLUTION-UNKNOWN"`, which names no image format
    // — reading it as `image/X-EVOLUTION-UNKNOWN` would tell the server the
    // bytes are a format that does not exist.
    let media = photo_line(&format!(
        "PHOTO;TYPE=\"X-EVOLUTION-UNKNOWN\";ENCODING=b:{PAYLOAD}"
    ));
    assert_eq!(media.media_type, None);
    assert_eq!(media.uri, format!("data:;base64,{PAYLOAD}"));

    // And a line carrying no `TYPE` at all, which is what this mapping writes
    // for bytes that are not an image.
    let media = photo_line(&format!("PHOTO;ENCODING=b:{PAYLOAD}"));
    assert_eq!(media.media_type, None);
    assert_eq!(media.uri, format!("data:;base64,{PAYLOAD}"));
}

#[test]
fn reads_a_reference_photo_line_back_as_the_uri_it_names() {
    let media = photo_line("PHOTO;VALUE=uri:https://vera.example/me.png");
    assert_eq!(media.kind.as_deref(), Some("photo"));
    assert_eq!(media.uri, "https://vera.example/me.png");
    // EDS writes no `TYPE` on a URI line and reads none off one (measured), so
    // there is nothing here that says what the resource is.
    assert_eq!(media.media_type, None);
}

#[test]
fn a_photo_whose_bytes_are_not_text_reads_back_byte_for_byte() {
    // The real case: a PNG signature is not valid UTF-8, so the line's value is
    // binary rather than text and the reader has to take the decoded bytes. A
    // picture that *is* text — an SVG — goes through the other path, which the
    // round-trip tests exercise with their own payload.
    // `iVBORw0KGgr//g==` is a PNG signature followed by 0xFF 0xFE.
    let media = photo_line("PHOTO;TYPE=png;ENCODING=b:iVBORw0KGgr//g==");
    assert_eq!(media.uri, "data:image/png;base64,iVBORw0KGgr//g==");

    // Driven the whole way round, so that what comes back is compared against
    // the bytes that went in rather than against a payload written out by hand.
    let uri = "data:image/png;base64,iVBORw0KGgr//g==";
    let vcard = card_to_vcard(&one_photo(uri, None));
    assert_eq!(read_photo(&vcard).uri, uri, "{vcard}");
}

#[test]
fn a_photo_line_states_the_key_its_entry_is_filed_under() {
    // As every other keyed map's line does, so that an untouched picture is
    // recognised as the entry it came from. The key only survives a line EDS
    // merely re-renders — a `set()` rebuilds it and drops the parameters
    // (measured) — which is why the save path also pairs a keyless photo with
    // the entry it replaced; see `jmap-book-sync`'s patch module.
    let card = vcard_to_card(&format!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\n\
         PHOTO;TYPE=jpeg;ENCODING=b:{PAYLOAD}\r\n\
         PHOTO;X-JMAP-KEY=m9;VALUE=uri:https://vera.example/other.png\r\n\
         END:VCARD\r\n"
    ))
    .expect("parse");

    let media = card.media.expect("media");
    assert_eq!(media.len(), 2, "{media:?}");
    assert_eq!(media["m9"].uri, "https://vera.example/other.png");
    assert_eq!(
        media["m1"].uri,
        format!("data:image/jpeg;base64,{PAYLOAD}"),
        "a keyless line is filed under an invented key: {media:?}"
    );
}

#[test]
fn a_photo_that_says_nothing_reads_back_as_no_entry() {
    // An empty value names neither bytes nor a resource, which says no more
    // than an `EMAIL:` with no address — and `photo()` gives such an entry no
    // line either, so a card holding one would state it back on every save.
    for line in ["PHOTO;ENCODING=b:", "PHOTO;VALUE=uri:"] {
        let vcard = format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\n{line}\r\nEND:VCARD\r\n");
        let card = vcard_to_card(&vcard).unwrap_or_else(|e| panic!("{line}: {e}"));
        assert_eq!(card.media, None, "{line}");
    }
}

#[test]
fn a_photo_survives_the_round_trip_it_is_stated_on() {
    // Emitted and read back, the fixture's picture is the same picture: the
    // `data:` URI, the media type and the key all come back. Anything else
    // would make every save an edit of a photo nobody touched.
    let card = vcard_to_card(&card_to_vcard(&fixture_card())).expect("parse");
    let media = card.media.expect("media");
    let original = fixture_card().media.expect("media");
    assert_eq!(media["m1"].kind, original["m1"].kind);
    assert_eq!(media["m1"].uri, original["m1"].uri);
    assert_eq!(media["m1"].media_type, original["m1"].media_type);
    // The fixture's `m2` is a logo, which gets no line and so cannot come back.
    assert_eq!(media.len(), 1, "{media:?}");
}

fn tagged(tags: &[&str]) -> ContactCard {
    ContactCard {
        keywords: Some(
            tags.iter()
                .map(|tag| ((*tag).to_owned(), json!(true)))
                .collect(),
        ),
        ..ContactCard::default()
    }
}

#[test]
fn maps_keywords_onto_one_categories_line() {
    // RFC 9553 §2.8.2's `keywords` is a Set — the tags are the keys — and RFC
    // 2426 §3.7.1's `CATEGORIES` is a list of them on one line, which EDS reads
    // as `E_CONTACT_CATEGORY_LIST`: the Categories field of Evolution's contact
    // editor. There is no key to carry, so no `X-JMAP-KEY`; the tag is its own
    // identity.
    //
    // One line rather than one per tag, unlike `NICKNAME` and `NOTE`, because
    // that is all EDS reads: measured against libebook-contacts 3.52, a second
    // `CATEGORIES` line is left standing in the vCard but contributes nothing to
    // the field the user sees.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(line(&vcard, "CATEGORIES"), "CATEGORIES:hiking");
    assert_eq!(vcard.matches("\r\nCATEGORIES").count(), 1, "{vcard}");

    let keywords = vcard_to_card(&vcard)
        .expect("parse")
        .keywords
        .expect("tags");
    assert_eq!(keywords.keys().collect::<Vec<_>>(), vec!["hiking"]);
    assert!(keywords.values().all(|set| set == &json!(true)));
}

#[test]
fn every_tag_goes_on_the_one_line_in_sorted_order() {
    // A set has no order of its own, so the line states the tags in the order
    // the map holds them — sorted — which makes the vCard stable across
    // renderings. The save reads an edit off a difference from what was shown,
    // and a reordering would look like one.
    let vcard = card_to_vcard(&tagged(&["Work", "Friends", "hiking"]));
    assert_eq!(line(&vcard, "CATEGORIES"), "CATEGORIES:Friends,Work,hiking");
}

#[test]
fn a_tag_holding_the_separators_is_escaped_and_comes_back_whole() {
    // A JMAP keyword is any string, and both the comma and the semicolon are
    // separators in this value — the comma to RFC 2426 and this reader, the
    // semicolon to EDS as well, measured against libebook-contacts 3.52, which
    // splits a raw one and honours the escape. Unescaped, one tag would arrive
    // at the server as two.
    let vcard = card_to_vcard(&tagged(&["back, in Berlin", "a;b"]));
    assert_eq!(
        line(&vcard, "CATEGORIES"),
        "CATEGORIES:a\\;b,back\\, in Berlin"
    );

    let keywords = vcard_to_card(&vcard)
        .expect("parse")
        .keywords
        .expect("tags");
    assert_eq!(
        keywords.keys().collect::<Vec<_>>(),
        vec!["a;b", "back, in Berlin"]
    );
}

#[test]
fn a_tag_no_categories_line_can_carry_gets_no_line() {
    // The same partial visibility every keyed map has, one type over: the set
    // is drawn *whole*, so a tag the line cannot state is a tag the next save
    // would delete — which is why `states_keyword` exists and why these tags are
    // left off rather than mangled onto the line.
    //
    // The empty tag, because an empty item reads back as no tag at all. A tag
    // holding a carriage return, because `syntax::write` drops it — a security
    // property, not tidiness — so the tag would come back spelled differently.
    // And a tag whose ends are ASCII whitespace, because EDS trims them:
    // measured against libebook-contacts 3.52, a leading space, tab, form feed
    // or newline is gone by the time the user sees the tag, so the next save
    // would rename it on the server. The vertical tab is in this list without
    // having been measured to need it — EDS keeps that one — because refusing to
    // draw a tag costs nothing but the sight of it, while drawing one that comes
    // back renamed costs the tag.
    for tag in [
        "",
        "two\rlines",
        " leading",
        "trailing ",
        "\ttabbed",
        "\u{c}fed",
        "\u{b}vertical",
    ] {
        let vcard = card_to_vcard(&tagged(&[tag]));
        assert!(
            !vcard.contains("\r\nCATEGORIES"),
            "the tag {tag:?} was drawn: {vcard}"
        );
    }

    // A newline *inside* a tag is not that case: it has an escape, EDS reads it
    // back, and the tag survives the trip.
    let vcard = card_to_vcard(&tagged(&["two\nlines"]));
    assert_eq!(line(&vcard, "CATEGORIES"), "CATEGORIES:two\\nlines");
    let keywords = vcard_to_card(&vcard)
        .expect("parse")
        .keywords
        .expect("tags");
    assert_eq!(keywords.keys().collect::<Vec<_>>(), vec!["two\nlines"]);
}

#[test]
fn a_keyword_set_to_anything_but_true_gets_no_line() {
    // RFC 9553 §1.4.3 has every value of a Set be `true`. Drawing a tag whose
    // value is anything else would tell the user it is set where the server
    // said it is not.
    let card = ContactCard {
        keywords: Some([("hiking".to_owned(), json!(false))].into()),
        ..ContactCard::default()
    };
    assert!(!card_to_vcard(&card).contains("\r\nCATEGORIES"));
}

#[test]
fn states_keyword_answers_for_the_tag_the_line_left_off() {
    // The save needs the refusal *per tag*, not for the set: a tag the line
    // could not carry is one the user never saw and therefore never asked to
    // lose, so the save writes it back rather than dropping the whole edit. The
    // predicate is the emitter's own, so what the save calls invisible is what
    // the emitter actually left off.
    for tag in ["", "two\rlines", " leading", "trailing ", "\u{b}vertical"] {
        assert!(
            !states_keyword(tag, &json!(true)),
            "the tag {tag:?} was called visible"
        );
        assert!(
            !card_to_vcard(&tagged(&[tag])).contains("\r\nCATEGORIES"),
            "the tag {tag:?} was drawn"
        );
    }

    assert!(states_keyword("hiking", &json!(true)));
    assert!(states_keyword("two\nlines", &json!(true)));
    // A value RFC 9553 §1.4.3 does not admit is the one refusal that is about
    // the value rather than the spelling.
    assert!(!states_keyword("hiking", &json!(false)));
}

#[test]
fn a_card_with_no_tags_states_none() {
    // `None` rather than an empty set, for the reason the keyed maps have one:
    // the save reads an edit off a difference from what was shown, and an empty
    // set would claim the contact is untagged where the card made no claim.
    assert!(!card_to_vcard(&ContactCard::default()).contains("\r\nCATEGORIES"));
    let card =
        vcard_to_card("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\nEND:VCARD\r\n").expect("parse");
    assert_eq!(card.keywords, None);
}

#[test]
fn reads_the_tags_of_every_categories_line() {
    // EDS shows the user the first line only, and rewrites that one when the
    // Categories field is edited — a second line is left exactly as it was,
    // measured against libebook-contacts 3.52. Reading both is what keeps the
    // tags on it from being deleted by a save: they were never shown, so the
    // user never asked for them to go.
    //
    // A tag named twice is one member either way, because a set is what both
    // sides mean, and an empty item is dropped rather than carried as a tag
    // whose name is nothing.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "CATEGORIES:Friends,Work,\r\n",
        "CATEGORIES:Work,hiking\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let keywords = card.keywords.expect("tags");
    assert_eq!(
        keywords.keys().collect::<Vec<_>>(),
        vec!["Friends", "Work", "hiking"]
    );
}

fn on_service(service: Option<&str>, user: Option<&str>) -> ContactCard {
    ContactCard {
        online_services: Some(
            [(
                "s1".to_owned(),
                OnlineService {
                    service: service.map(str::to_owned),
                    user: user.map(str::to_owned),
                    ..OnlineService::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    }
}

#[test]
fn maps_an_online_service_onto_the_x_line_eds_keeps_it_on() {
    // RFC 9553 §2.3.2's `onlineServices` names the contact as one service knows
    // them. vCard 3.0 has no property for that at all — RFC 4770's `IMPP` is
    // 4.0, which is not the format `e_contact_new_from_vcard()` is handed — so
    // the line is the `X-` one EDS itself keeps a handle on: measured against
    // libebook-contacts 3.52, `X-JABBER` is what `E_CONTACT_IM_JABBER` and the
    // per-slot fields Evolution's contact editor shows are read out of.
    //
    // The `TYPE` is not optional decoration: a line without one reaches none of
    // the `E_CONTACT_IM_JABBER_HOME_1`…`_WORK_3` fields, so the handle would be
    // in the vCard and nowhere the user can see it.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(
        line(&vcard, "X-JABBER"),
        "X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera@jabber.example"
    );

    let services = vcard_to_card(&vcard)
        .expect("parse")
        .online_services
        .expect("online services");
    assert_eq!(services.keys().collect::<Vec<_>>(), vec!["s1"]);
    assert_eq!(services["s1"].service.as_deref(), Some("Jabber"));
    assert_eq!(services["s1"].user.as_deref(), Some("vera@jabber.example"));
    // Read back as a handle, never as a URI: the line states the one EDS field
    // that holds free text, so there is nothing here to call an RFC 3986 URI.
    assert_eq!(services["s1"].uri, None);
}

#[test]
fn every_service_eds_has_a_field_for_gets_its_own_line() {
    // The ten services libebook-contacts 3.52 gives HOME/WORK slots to. A
    // service name is matched case-insensitively as RFC 9553 §2.3.2 requires,
    // and a little wider than that — the punctuation inside it is ignored too —
    // because `Gadu-Gadu`, `GaduGadu` and `gadu gadu` are one service under
    // three spellings and only a normalising match keeps a save from renaming
    // whichever one the server chose.
    for (service, property) in [
        ("AIM", "X-AIM"),
        ("aim", "X-AIM"),
        ("Gadu-Gadu", "X-GADUGADU"),
        ("GaduGadu", "X-GADUGADU"),
        ("Google Talk", "X-GOOGLE-TALK"),
        ("google-talk", "X-GOOGLE-TALK"),
        ("GroupWise", "X-GROUPWISE"),
        ("ICQ", "X-ICQ"),
        ("Jabber", "X-JABBER"),
        ("Matrix", "X-MATRIX"),
        ("MSN", "X-MSN"),
        ("Skype", "X-SKYPE"),
        ("Yahoo", "X-YAHOO"),
    ] {
        let vcard = card_to_vcard(&on_service(Some(service), Some("handle")));
        assert_eq!(
            line(&vcard, property),
            format!("{property};X-JMAP-KEY=s1;TYPE=HOME:handle"),
            "{service}"
        );
    }
}

#[test]
fn a_service_eds_has_no_field_for_gets_no_line() {
    // An `X-SIGNAL` line would reach no field of EDS's and no field of
    // Evolution's, so it would say nothing to the user while making the save
    // believe the entry had been shown. `Twitter` is the same case for a less
    // obvious reason: libebook-contacts 3.52 knows `X-TWITTER` as a
    // multi-valued field but gives it no HOME/WORK slot, so a handle on it is
    // not one the contact editor can put anywhere.
    //
    // An entry naming no service at all cannot be drawn either: the property is
    // what states the service, so there would be no line to choose.
    for service in [Some("Signal"), Some("Twitter"), Some("SIP"), Some(""), None] {
        let vcard = card_to_vcard(&on_service(service, Some("handle")));
        assert!(
            !vcard.contains("handle"),
            "a handle at {service:?} was drawn: {vcard}"
        );
    }
}

#[test]
fn unmapped_or_unslotted_im_and_sip_lines_in_vcard_are_ignored_by_reader() {
    // libebook-contacts 3.52 knows X-TWITTER and X-SIP as EContactAttrList fields
    // with no slotted fields (_HOME_1..3, _WORK_1..3). When parsing a vCard with
    // X-TWITTER, X-SIP, or unknown X-SIGNAL, vcard_to_card safely ignores them rather
    // than creating unmodeled online_services entries.
    let vcard = "BEGIN:VCARD\r\n\
                 VERSION:3.0\r\n\
                 FN:Vera\r\n\
                 N:;Vera;;;\r\n\
                 X-SIP:sip:vera@example.com\r\n\
                 X-TWITTER:vera\r\n\
                 X-SIGNAL:vera\r\n\
                 END:VCARD\r\n";
    let card = vcard_to_card(vcard).expect("parse");
    assert_eq!(card.online_services, None);
}

#[test]
fn mapped_service_is_retained_while_unmapped_sip_and_twitter_are_ignored() {
    let vcard = "BEGIN:VCARD\r\n\
                 VERSION:3.0\r\n\
                 FN:Vera\r\n\
                 N:;Vera;;;\r\n\
                 X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera@jabber.example\r\n\
                 X-SIP:sip:vera@example.com\r\n\
                 X-TWITTER:vera\r\n\
                 END:VCARD\r\n";
    let card = vcard_to_card(vcard).expect("parse");
    let services = card.online_services.expect("online services");
    assert_eq!(services.len(), 1);
    assert_eq!(services["s1"].user.as_deref(), Some("vera@jabber.example"));
    assert_eq!(services["s1"].service.as_deref(), Some("Jabber"));
}

fn at_uri(service: Option<&str>, uri: &str) -> ContactCard {
    ContactCard {
        online_services: Some(
            [(
                "s1".to_owned(),
                OnlineService {
                    service: service.map(str::to_owned),
                    uri: Some(uri.to_owned()),
                    ..OnlineService::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    }
}

#[test]
fn an_entry_stated_only_as_a_uri_is_drawn_from_the_uri() {
    // RFC 9553 §2.3.2 asks for the `uri` or the `user`, and Evolution's
    // instant-messaging field holds only the second: a handle. Reading one out
    // of a URI means knowing the service's scheme — which for the services whose
    // scheme states the handle and nothing else is a fact, not a guess, so the
    // entry is drawn rather than left invisible.
    let vcard = card_to_vcard(&at_uri(Some("Jabber"), "xmpp:vera@jabber.example"));
    assert_eq!(
        line(&vcard, "X-JABBER"),
        "X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera@jabber.example"
    );

    // Read back as a handle with no URI, as every line is: the reader states
    // what the line says, and the save path is what knows the entry it belongs
    // to was a URI.
    let services = vcard_to_card(&vcard)
        .expect("parse")
        .online_services
        .expect("online services");
    assert_eq!(services["s1"].user.as_deref(), Some("vera@jabber.example"));
    assert_eq!(services["s1"].uri, None);

    // The scheme is compared case-insensitively, as RFC 3986 §3.1 requires.
    assert!(
        card_to_vcard(&at_uri(Some("Jabber"), "XMPP:vera@jabber.example"))
            .contains(":vera@jabber.example")
    );
    // And it is the service's scheme, not any scheme: an `https` URI under a
    // service whose handles are JIDs states no handle this mapping can read.
    assert!(
        !card_to_vcard(&at_uri(Some("Jabber"), "https://jabber.example/vera")).contains("vera")
    );
}

#[test]
fn a_uri_at_a_service_with_no_known_scheme_gets_no_line() {
    // The table holds the services whose scheme names the handle literally.
    // Everything else stays invisible exactly as before: `matrix:` states the
    // identifier as `u/vera:matrix.example`, which is not the `@vera:...` the
    // field holds, and GroupWise has no scheme at all. Inventing one for either
    // would put a fabricated handle in front of the user and then write the
    // user's correction of it back to the server.
    for (service, uri) in [
        (Some("Matrix"), "matrix:u/vera:matrix.example"),
        (Some("GroupWise"), "vera.oldenburg"),
        (Some("Signal"), "sgnl://signal.me/vera"),
        (None, "xmpp:vera@jabber.example"),
    ] {
        let vcard = card_to_vcard(&at_uri(service, uri));
        assert!(!vcard.contains("vera"), "{service:?} was drawn: {vcard}");
    }

    // The fixture's Mastodon entry is the same case twice over — a service EDS
    // has no field for, stated as a URI.
    let vcard = card_to_vcard(&fixture_card());
    assert!(!vcard.contains("social.example"), "{vcard}");
}

#[test]
fn a_uri_that_says_more_than_a_handle_gets_no_line() {
    // Only the plain `scheme:handle` shape states a handle and nothing else. A
    // path, a query, a fragment or a percent-encoding means the URI carries
    // something the field cannot hold — so drawing it would show the user a
    // handle the service does not know them by, and a save would write that
    // back. Whitespace is refused for the same reason it is in a `user`: what
    // the user sees and what the line says would differ.
    for uri in [
        "xmpp:",
        "xmpp:vera@jabber.example/work",
        "xmpp:vera@jabber.example?message",
        "xmpp:vera@jabber.example#anchor",
        "xmpp:vera%40jabber.example",
        "xmpp: vera@jabber.example",
        "xmpp:vera\t@jabber.example",
        "vera@jabber.example",
    ] {
        let vcard = card_to_vcard(&at_uri(Some("Jabber"), uri));
        assert!(!vcard.contains("X-JABBER"), "{uri} was drawn: {vcard}");
    }
}

#[test]
fn an_entry_stating_both_is_drawn_from_its_handle() {
    // The `user` is what the service calls the contact; the `uri` is a second
    // way of saying it. Where both are there the handle wins, so an entry whose
    // URI disagrees with its handle shows the handle — the member the field is
    // for.
    let mut card = at_uri(Some("Jabber"), "xmpp:old@jabber.example");
    let services = card.online_services.as_mut().expect("online services");
    services.get_mut("s1").expect("the entry").user = Some("vera@jabber.example".to_owned());
    assert_eq!(
        line(&card_to_vcard(&card), "X-JABBER"),
        "X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera@jabber.example"
    );
}

#[test]
fn a_handle_no_x_line_can_carry_gets_no_line() {
    // The same measured refusals the `CATEGORIES` line makes, for the same
    // reason: a handle that comes back from EDS spelled differently is a handle
    // the next save renames on the server.
    //
    // The empty handle says nothing. A carriage return is dropped by
    // `syntax::write` — a security property, not tidiness. And ends made of
    // ASCII whitespace are trimmed by EDS: measured against libebook-contacts
    // 3.52, `X-JABBER: vera@a ` reaches the user as `vera@a`, so the line keeps
    // what the server said while every field the user can see disagrees with it.
    for handle in [
        Some(""),
        None,
        Some("two\rlines"),
        Some(" leading"),
        Some("trailing "),
        Some("\ttabbed"),
    ] {
        let vcard = card_to_vcard(&on_service(Some("Jabber"), handle));
        assert!(
            !vcard.contains("X-JABBER"),
            "the handle {handle:?} was drawn: {vcard}"
        );
    }
}

#[test]
fn a_handle_holding_the_separators_is_escaped_and_comes_back_whole() {
    // A JSContact `user` is free text, and EDS reads a raw semicolon in this
    // value as the end of it: measured against libebook-contacts 3.52,
    // `X-JABBER:a;b@c` hands the user `a`, while `a\;b@c` comes back whole and
    // is re-escaped on the way out. The comma is the same case one step
    // earlier — it separates the items of a `text-list` — so both are escaped.
    let vcard = card_to_vcard(&on_service(Some("Jabber"), Some("a;b,c@d")));
    assert_eq!(
        line(&vcard, "X-JABBER"),
        "X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:a\\;b\\,c@d"
    );

    let services = vcard_to_card(&vcard)
        .expect("parse")
        .online_services
        .expect("online services");
    assert_eq!(services["s1"].user.as_deref(), Some("a;b,c@d"));
}

#[test]
fn the_work_context_files_the_handle_in_evolutions_work_slot() {
    // The `TYPE` is the slot rather than the entry's `contexts`, but it is
    // chosen from them where they say something: a work handle belongs in the
    // field Evolution labels Work. Exactly one slot per line — a handle wearing
    // both `TYPE`s shows up in two fields the user can edit independently, and
    // nothing would say which edit wins.
    let slotted = |contexts: Value| {
        let mut card = on_service(Some("Jabber"), Some("handle"));
        let services = card.online_services.as_mut().expect("online services");
        let service = services.get_mut("s1").expect("the entry");
        service.extra.insert("contexts".to_owned(), contexts);
        let vcard = card_to_vcard(&card);
        line(&vcard, "X-JABBER").to_owned()
    };

    assert!(slotted(json!({"work": true})).ends_with(";TYPE=WORK:handle"));
    assert!(slotted(json!({"private": true})).ends_with(";TYPE=HOME:handle"));
    // A handle used in both, and one used in a context vCard cannot spell, go
    // in the Home slot: it is where EDS puts a handle of its own accord, so an
    // entry the mapping cannot place lands where the user looks first.
    assert!(slotted(json!({"work": true, "private": true})).ends_with(";TYPE=HOME:handle"));
    assert!(slotted(json!({"school": true})).ends_with(";TYPE=HOME:handle"));
    assert!(
        card_to_vcard(&on_service(Some("Jabber"), Some("handle"))).contains(";TYPE=HOME:handle")
    );
}

#[test]
fn the_slot_a_handle_sits_in_is_not_read_back_as_a_context() {
    // The other direction of the same decision. Every line carries a `TYPE`
    // because a line without one is invisible, so reading the parameter back as
    // RFC 9553 §1.5.1 contexts would put a context on every entry that had
    // none — and the next save would write that invention to the server.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\n",
        "X-JABBER;TYPE=WORK:vera@work.example\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let services = card.online_services.expect("online services");
    assert_eq!(services["s1"].user.as_deref(), Some("vera@work.example"));
    assert!(
        services["s1"].extra.is_empty(),
        "the slot was read back as a member: {:?}",
        services["s1"]
    );
}

#[test]
fn reads_a_handle_on_a_line_that_never_carried_a_key() {
    // A handle the user has just typed reaches this side on a line EDS built
    // from its own field, without an `X-JMAP-KEY` — measured against
    // libebook-contacts 3.52, that is what setting one of the slots writes. It
    // is keyed by counting, as every other property's addition is, and its
    // service is the line's own.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\n",
        "X-MATRIX;TYPE=HOME:@vera:matrix.example\r\n",
        "X-SKYPE;TYPE=WORK:vera.oldenburg\r\n",
        "X-SIGNAL;TYPE=HOME:+49301234\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let services = card.online_services.expect("online services");
    // Two, not three: the `X-SIGNAL` line is not a property this mapping
    // states, so reading it would invent an entry the server never had — and
    // the line stays in EDS's copy either way.
    assert_eq!(services.keys().collect::<Vec<_>>(), vec!["s1", "s2"]);
    assert_eq!(services["s1"].service.as_deref(), Some("Matrix"));
    assert_eq!(services["s1"].user.as_deref(), Some("@vera:matrix.example"));
    assert_eq!(services["s2"].service.as_deref(), Some("Skype"));
    assert_eq!(services["s2"].user.as_deref(), Some("vera.oldenburg"));
}

#[test]
fn a_card_with_no_online_services_states_none() {
    // `None` rather than an empty map, for the reason every other keyed map has
    // one: the save reads an edit off a difference from what was shown, and an
    // empty map would claim the contact is on no service where the vCard made
    // no claim at all.
    assert!(!card_to_vcard(&ContactCard::default()).contains("X-JABBER"));
    let card =
        vcard_to_card("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\nEND:VCARD\r\n").expect("parse");
    assert_eq!(card.online_services, None);
}

#[test]
fn unmodeled_jscontact_properties_are_dropped_not_mangled() {
    // `preferredLanguages` has no place in the minimal vCard set. Dropping it
    // is safe only because saving goes through a PatchObject that touches the
    // mapped properties alone — this test pins that expectation down.
    let vcard = card_to_vcard(&fixture_card());
    assert!(!vcard.contains("de-DE"), "{vcard}");
    assert!(!vcard.contains("preferredLanguages"), "{vcard}");
    assert!(vcard_to_card(&vcard).expect("parse").extra.is_empty());
}

#[test]
fn address_book_membership_is_not_a_vcard_concept() {
    // It is decided by which EDS source the backend is serving, so the
    // mapping must not invent one on the way back.
    let card = vcard_to_card(&card_to_vcard(&fixture_card())).expect("parse");
    assert_eq!(card.address_book_ids, None);
}

#[test]
fn rejects_input_that_is_not_a_vcard() {
    assert!(vcard_to_card("").is_err());
    assert!(vcard_to_card("{\"id\": \"C1\"}").is_err());
}

#[test]
fn a_foreign_handle_holding_a_raw_separator_is_read_as_the_line_states_it() {
    // A card written by somebody else may carry an unescaped separator, and
    // neither of them separates anything in this value: calcard hands an `X-`
    // line's value back whole, so the handle is what the line says.
    //
    // Which agrees with EDS about the comma and not about the semicolon —
    // measured against libebook-contacts 3.52, `X-JABBER:a;b@c` reaches the user
    // as `a` while `X-JABBER:a,b@c` reaches them whole. The cost of that
    // disagreement is bounded: EDS re-emits such a line untouched, so the text
    // round-trips and the only difference is how much of the handle Evolution
    // shows for a card no client of ours wrote.
    for (line, handle) in [("X-JABBER:a,b@c", "a,b@c"), ("X-JABBER:a;b@c", "a;b@c")] {
        let card = vcard_to_card(&format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\n{line}\r\nEND:VCARD\r\n"
        ))
        .expect("parse");
        let services = card.online_services.expect("online services");
        assert_eq!(services["s1"].user.as_deref(), Some(handle), "{line}");
    }
}

#[test]
fn a_uri_only_gadu_gadu_entry_is_drawn_as_x_gadugadu() {
    // `gg` is the provisional IANA scheme (RFC 7595 template `gg:<userid>`) for
    // Gadu-Gadu, where the handle is the numerical user identifier (UIN).
    let vcard = card_to_vcard(&at_uri(Some("Gadu-Gadu"), "gg:12345678"));
    assert_eq!(
        line(&vcard, "X-GADUGADU"),
        "X-GADUGADU;X-JMAP-KEY=s1;TYPE=HOME:12345678"
    );

    let services = vcard_to_card(&vcard)
        .expect("parse")
        .online_services
        .expect("online services");
    assert_eq!(services["s1"].user.as_deref(), Some("12345678"));
    assert_eq!(services["s1"].uri, None);
}

#[test]
fn a_gadu_gadu_uri_that_says_more_than_a_handle_gets_no_line() {
    for uri in [
        "gg:",
        "gg:1234/work",
        "gg:1234?chat",
        "gg:1234#anchor",
        "gg: 1234",
    ] {
        let vcard = card_to_vcard(&at_uri(Some("Gadu-Gadu"), uri));
        assert!(!vcard.contains("X-GADUGADU"), "{uri} was drawn: {vcard}");
    }
}

#[test]
fn conventional_action_query_or_unregistered_im_uris_get_no_vcard_line() {
    // AIM, MSN, and Yahoo conventionally employ query/action URI formats
    // (e.g. `aim:goim?screenname=...`, `msnim:chat?contact=...`, `ymsgr:sendim?...`)
    // or unreserved schemes which are rejected because they do not represent bare
    // handles, and ICQ lacks a registered IANA scheme. When given only a `uri`
    // (and no `user`), card_to_vcard safely omits them so EDS fields are not corrupted.
    for (service, uri, header) in [
        ("AIM", "aim:goim?screenname=alice", "X-AIM"),
        ("AIM", "aim:addbuddy?screenname=alice", "X-AIM"),
        ("AIM", "aim:alice", "X-AIM"),
        ("MSN", "msnim:chat?contact=bob@example.com", "X-MSN"),
        ("MSN", "msnim:add?contact=bob@example.com", "X-MSN"),
        ("MSN", "msnim:bob@example.com", "X-MSN"),
        ("Yahoo", "ymsgr:sendim?carol", "X-YAHOO"),
        ("Yahoo", "ymsgr:chat?carol", "X-YAHOO"),
        ("Yahoo", "ymsgr:carol", "X-YAHOO"),
        ("ICQ", "icq:message?uin=12345678", "X-ICQ"),
        ("ICQ", "icq:12345678", "X-ICQ"),
    ] {
        let vcard = card_to_vcard(&at_uri(Some(service), uri));
        assert!(
            !vcard.contains(header),
            "{service} uri '{uri}' was unexpectedly drawn on {header}: {vcard}"
        );
    }
}

#[test]
fn conventional_im_services_with_user_handles_are_drawn_and_roundtripped() {
    for (service, handle, header) in [
        ("AIM", "alice_aim", "X-AIM"),
        ("ICQ", "12345678", "X-ICQ"),
        ("MSN", "bob@example.com", "X-MSN"),
        ("Yahoo", "carol_yahoo", "X-YAHOO"),
    ] {
        let card = on_service(Some(service), Some(handle));
        let vcard = card_to_vcard(&card);
        assert_eq!(
            line(&vcard, header),
            format!("{header};X-JMAP-KEY=s1;TYPE=HOME:{handle}")
        );

        let parsed = vcard_to_card(&vcard).expect("parse");
        let services = parsed.online_services.expect("online services");
        assert_eq!(services["s1"].user.as_deref(), Some(handle));
        assert_eq!(services["s1"].uri, None);
        assert_eq!(services["s1"].service.as_deref(), Some(service));
    }
}
