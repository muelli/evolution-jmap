// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSContact `ContactCard` ↔ vCard 3.0, the minimal property set the
//! address book backend needs: UID, FN, N, EMAIL, TEL, ADR, ORG, TITLE,
//! ROLE, NOTE.

use jmap_proto::contacts::{
    Address, AddressComponent, ContactCard, ContactEmail, ContactPhone, Name, NameComponent, Note,
    OrgUnit, Organization, Title,
};
use jmap_vcard::{card_to_vcard, vcard_to_card};
use serde_json::json;

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
                NameComponent {
                    kind: "title".to_owned(),
                    value: "Dr.".to_owned(),
                },
                NameComponent {
                    kind: "given".to_owned(),
                    value: "Vera".to_owned(),
                },
                NameComponent {
                    kind: "given2".to_owned(),
                    value: "Marie".to_owned(),
                },
                NameComponent {
                    kind: "surname".to_owned(),
                    value: "Oldenburg".to_owned(),
                },
                NameComponent {
                    kind: "credential".to_owned(),
                    value: "MSc".to_owned(),
                },
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
                NameComponent {
                    kind: "surname".to_owned(),
                    value: "Oldenburg".to_owned(),
                },
                NameComponent {
                    kind: "given".to_owned(),
                    value: "Vera".to_owned(),
                },
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
    // RFC 9553 §2.5.1 has kinds — floor, room, landmark, and the street
    // `number` on its own — that vCard's seven fields cannot state. Putting
    // one in a field it does not belong to would misplace it, so it stays
    // off the line, and an address made of nothing else has no line at all.
    let card = one_address(
        "a1",
        Address {
            components: Some(vec![
                AddressComponent::new("name", "Hauptstraße"),
                AddressComponent::new("number", "1"),
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
    // RFC 9553 §2.8.1 keys the notes like every other JSContact map; RFC
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

#[test]
fn unmodeled_jscontact_properties_are_dropped_not_mangled() {
    // `nicknames` has no place in the minimal vCard set. Dropping it is safe
    // only because saving goes through a PatchObject that touches the mapped
    // properties alone — this test pins that expectation down.
    let vcard = card_to_vcard(&fixture_card());
    assert!(!vcard.contains("Vee"), "{vcard}");
    assert!(!vcard.contains("nicknames"), "{vcard}");
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
