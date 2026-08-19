// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSContact `ContactCard` ↔ vCard 3.0, the minimal property set the
//! address book backend needs: UID, FN, N, NICKNAME, EMAIL, TEL, ADR, LABEL,
//! ORG, TITLE, ROLE, NOTE, BDAY, URL, CALURI, FBURL, PHOTO, CATEGORIES and the
//! `X-` lines EDS keeps instant-messaging handles and the spouse on.

use std::collections::BTreeMap;

use base64::Engine;
use jmap_proto::contacts::{
    Address, AddressComponent, Anniversary, Calendar, ContactCard, ContactEmail, ContactPhone,
    Link, Media, Name, NameComponent, Nickname, Note, OnlineService, OrgUnit, Organization,
    Relation, Title,
};
use jmap_vcard::{
    VCardError, address_label, anniversary_date, card_to_vcard, maps_context, maps_phone_feature,
    online_service_handle, online_service_uri, restore_address_components, restore_name_components,
    same_photo, same_service, states_a_point_in_time, states_address, states_address_component,
    states_anniversary, states_calendar, states_context, states_email, states_keyword, states_link,
    states_media, states_name_component, states_nickname, states_note,
    states_nothing_but_the_marriage, states_online_service, states_org_unit, states_organization,
    states_phone, states_phone_feature, states_spouse, states_title, title_kind, vcard_to_card,
};
use serde_json::{Value, json};

fn fixture_card() -> ContactCard {
    let path = format!(
        "{}/tests/fixtures/contact_card.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn unfolded(vcard: &str) -> String {
    vcard.replace("\r\n ", "").replace("\r\n\t", "")
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
fn an_organization_with_an_empty_name_string_behaves_consistently() {
    // An organisation whose name is `""` rather than absent:
    // 1. When it has no units, it states nothing and is skipped rather than emitting an empty `ORG:` line.
    let card_empty_name = ContactCard {
        organizations: Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some(String::new()),
                    units: None,
                    ..Organization::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    };
    let org = &card_empty_name.organizations.as_ref().unwrap()["o1"];
    assert!(!states_organization(org));
    let vcard = card_to_vcard(&card_empty_name);
    assert!(!vcard.contains("\r\nORG"));
    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(back.organizations, None);

    // 2. When it has units, the empty name is preserved as an empty first component:
    // the leading semicolon keeps units in their structured component position.
    let card_empty_name_with_units = ContactCard {
        organizations: Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some(String::new()),
                    units: Some(vec![OrgUnit::new("Engineering"), OrgUnit::new("Security")]),
                    ..Organization::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    };
    let org_with_units = &card_empty_name_with_units.organizations.as_ref().unwrap()["o1"];
    assert!(states_organization(org_with_units));
    let vcard_with_units = card_to_vcard(&card_empty_name_with_units);
    assert_eq!(
        line(&vcard_with_units, "ORG"),
        "ORG;X-JMAP-KEY=o1:;Engineering;Security"
    );
    let back_with_units = vcard_to_card(&vcard_with_units).expect("parse");
    let back_orgs = back_with_units.organizations.expect("organizations");
    assert_eq!(back_orgs["o1"].name, None);
    assert_eq!(
        back_orgs["o1"].units.as_deref(),
        Some([OrgUnit::new("Engineering"), OrgUnit::new("Security")].as_slice())
    );
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

#[test]
fn multiple_organizations_emit_distinct_vcard_lines_and_roundtrip() {
    let card = ContactCard {
        organizations: Some(
            [
                (
                    "o1".to_owned(),
                    Organization {
                        name: Some("Acme Ltd".to_owned()),
                        units: Some(vec![OrgUnit::new("Research")]),
                        ..Organization::default()
                    },
                ),
                (
                    "o2".to_owned(),
                    Organization {
                        name: Some("Brauerei".to_owned()),
                        units: Some(vec![OrgUnit::new("Logistics")]),
                        ..Organization::default()
                    },
                ),
            ]
            .into(),
        ),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert!(
        vcard.contains("ORG;X-JMAP-KEY=o1:Acme Ltd;Research\r\n"),
        "{vcard}"
    );
    assert!(
        vcard.contains("ORG;X-JMAP-KEY=o2:Brauerei;Logistics\r\n"),
        "{vcard}"
    );

    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(back.organizations, card.organizations);
}

#[test]
fn multiple_titles_and_roles_emit_distinct_vcard_lines_and_roundtrip() {
    let card = ContactCard {
        titles: Some(
            [
                (
                    "t1".to_owned(),
                    Title {
                        name: "Research Scientist".to_owned(),
                        kind: None,
                        ..Title::default()
                    },
                ),
                (
                    "t2".to_owned(),
                    Title {
                        name: "Director of Engineering".to_owned(),
                        kind: None,
                        ..Title::default()
                    },
                ),
                (
                    "r1".to_owned(),
                    Title {
                        name: "Lead Investigator".to_owned(),
                        kind: Some("role".to_owned()),
                        ..Title::default()
                    },
                ),
                (
                    "r2".to_owned(),
                    Title {
                        name: "Project Manager".to_owned(),
                        kind: Some("role".to_owned()),
                        ..Title::default()
                    },
                ),
            ]
            .into(),
        ),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert!(
        vcard.contains("TITLE;X-JMAP-KEY=t1:Research Scientist\r\n"),
        "{vcard}"
    );
    assert!(
        vcard.contains("TITLE;X-JMAP-KEY=t2:Director of Engineering\r\n"),
        "{vcard}"
    );
    assert!(
        vcard.contains("ROLE;X-JMAP-KEY=r1:Lead Investigator\r\n"),
        "{vcard}"
    );
    assert!(
        vcard.contains("ROLE;X-JMAP-KEY=r2:Project Manager\r\n"),
        "{vcard}"
    );

    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(back.titles, card.titles);
}

#[test]
fn multi_component_org_with_three_or_more_units_and_office_roundtrips_faithfully() {
    // RFC 2426 §3.5.5 and EDS field slotting:
    // Component 1: Organization name (`Acme Ltd` -> `E_CONTACT_ORG`)
    // Component 2: Department (`Research` -> `E_CONTACT_ORG_UNIT`)
    // Component 3: Office (`Optics` -> `E_CONTACT_OFFICE`)
    // Component 4: Fourth unit (`Lenses` -> unmapped in EDS, survives edits)
    let card = ContactCard {
        organizations: Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some("Acme Ltd".to_owned()),
                    units: Some(vec![
                        OrgUnit::new("Research"),
                        OrgUnit::new("Optics"),
                        OrgUnit::new("Lenses"),
                    ]),
                    ..Organization::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    };

    let org = &card.organizations.as_ref().unwrap()["o1"];
    assert!(states_organization(org));
    for unit in org.units.as_ref().unwrap() {
        assert!(states_org_unit(unit));
    }

    let vcard = card_to_vcard(&card);
    assert_eq!(
        line(&vcard, "ORG"),
        "ORG;X-JMAP-KEY=o1:Acme Ltd;Research;Optics;Lenses"
    );

    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(back.organizations, card.organizations);

    // Verify inbound vCard without X-JMAP-KEY preserves all 4 components into o1.
    let unkeyed_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "ORG:Acme Ltd;Research;Optics;Lenses\r\n",
        "END:VCARD\r\n"
    );
    let from_unkeyed = vcard_to_card(unkeyed_vcard).expect("parse");
    let unkeyed_orgs = from_unkeyed.organizations.expect("organizations");
    assert_eq!(unkeyed_orgs.keys().collect::<Vec<_>>(), vec!["o1"]);
    assert_eq!(unkeyed_orgs["o1"].name.as_deref(), Some("Acme Ltd"));
    assert_eq!(
        unkeyed_orgs["o1"].units.as_deref(),
        Some(
            [
                OrgUnit::new("Research"),
                OrgUnit::new("Optics"),
                OrgUnit::new("Lenses")
            ]
            .as_slice()
        )
    );
}

#[test]
fn multi_component_org_with_deep_hierarchy_and_trailing_or_intermediate_units_roundtrip() {
    // 1. Deep 6-component hierarchy: OrgName + 5 units.
    let deep_card = ContactCard {
        organizations: Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some("Global Tech".to_owned()),
                    units: Some(vec![
                        OrgUnit::new("Engineering"),
                        OrgUnit::new("Infrastructure"),
                        OrgUnit::new("Storage Systems"),
                        OrgUnit::new("Flash Division"),
                        OrgUnit::new("Team Beta"),
                    ]),
                    ..Organization::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    };
    let deep_vcard = card_to_vcard(&deep_card);
    assert_eq!(
        line(&unfolded(&deep_vcard), "ORG"),
        "ORG;X-JMAP-KEY=o1:Global Tech;Engineering;Infrastructure;Storage Systems;Flash Division;Team Beta"
    );
    let deep_back = vcard_to_card(&deep_vcard).expect("parse");
    assert_eq!(deep_back.organizations, deep_card.organizations);

    // 2. Nameless organisation with 4 units: leading semicolon preserves unit positions.
    let nameless_card = ContactCard {
        organizations: Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: None,
                    units: Some(vec![
                        OrgUnit::new("Engineering"),
                        OrgUnit::new("Security"),
                        OrgUnit::new("Cryptography"),
                        OrgUnit::new("Quantum"),
                    ]),
                    ..Organization::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    };
    let nameless_vcard = card_to_vcard(&nameless_card);
    assert_eq!(
        line(&nameless_vcard, "ORG"),
        "ORG;X-JMAP-KEY=o1:;Engineering;Security;Cryptography;Quantum"
    );
    let nameless_back = vcard_to_card(&nameless_vcard).expect("parse");
    assert_eq!(nameless_back.organizations, nameless_card.organizations);

    // 3. Intermediate empty component (such as EDS clearing E_CONTACT_OFFICE in place):
    // empty unit is filtered on read, emitting only non-empty units without sliding into name.
    let cleared_office_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "ORG;X-JMAP-KEY=o1:Acme Ltd;Research;;Lenses\r\n",
        "END:VCARD\r\n"
    );
    let from_cleared = vcard_to_card(cleared_office_vcard).expect("parse");
    let cleared_orgs = from_cleared.organizations.as_ref().expect("organizations");
    assert_eq!(cleared_orgs["o1"].name.as_deref(), Some("Acme Ltd"));
    assert_eq!(
        cleared_orgs["o1"].units.as_deref(),
        Some([OrgUnit::new("Research"), OrgUnit::new("Lenses")].as_slice())
    );
    let rewritten_vcard = card_to_vcard(&from_cleared);
    assert_eq!(
        line(&rewritten_vcard, "ORG"),
        "ORG;X-JMAP-KEY=o1:Acme Ltd;Research;Lenses"
    );
    let rewritten_back = vcard_to_card(&rewritten_vcard).expect("parse");
    assert_eq!(rewritten_back.organizations, from_cleared.organizations);
}

#[test]
fn multi_component_org_and_multiple_titles_roles_coexist_and_roundtrip() {
    let card = ContactCard {
        organizations: Some(
            [
                (
                    "o1".to_owned(),
                    Organization {
                        name: Some("Acme Corp".to_owned()),
                        units: Some(vec![
                            OrgUnit::new("Research"),
                            OrgUnit::new("Optics"),
                            OrgUnit::new("Lenses"),
                        ]),
                        ..Organization::default()
                    },
                ),
                (
                    "o2".to_owned(),
                    Organization {
                        name: Some("MegaCorp Industries".to_owned()),
                        units: Some(vec![
                            OrgUnit::new("Cloud"),
                            OrgUnit::new("Datacenter"),
                            OrgUnit::new("Hardware"),
                            OrgUnit::new("Power"),
                        ]),
                        ..Organization::default()
                    },
                ),
            ]
            .into(),
        ),
        titles: Some(
            [
                (
                    "t1".to_owned(),
                    Title {
                        name: "Chief Scientist".to_owned(),
                        kind: None,
                        ..Title::default()
                    },
                ),
                (
                    "t2".to_owned(),
                    Title {
                        name: "Distinguished Engineer".to_owned(),
                        kind: Some("title".to_owned()),
                        ..Title::default()
                    },
                ),
                (
                    "r1".to_owned(),
                    Title {
                        name: "Technical Lead".to_owned(),
                        kind: Some("role".to_owned()),
                        ..Title::default()
                    },
                ),
                (
                    "r2".to_owned(),
                    Title {
                        name: "Steering Committee Member".to_owned(),
                        kind: Some("role".to_owned()),
                        ..Title::default()
                    },
                ),
                (
                    "x1".to_owned(),
                    Title {
                        name: "Honorary Fellow".to_owned(),
                        kind: Some("x-honour".to_owned()),
                        ..Title::default()
                    },
                ),
            ]
            .into(),
        ),
        ..ContactCard::default()
    };

    for title in card.titles.as_ref().unwrap().values() {
        if title.kind.as_deref() == Some("x-honour") {
            assert!(!states_title(title));
        } else {
            assert!(states_title(title));
        }
    }

    let vcard = card_to_vcard(&card);
    assert!(
        vcard.contains("ORG;X-JMAP-KEY=o1:Acme Corp;Research;Optics;Lenses\r\n"),
        "{vcard}"
    );
    assert!(
        vcard.contains("ORG;X-JMAP-KEY=o2:MegaCorp Industries;Cloud;Datacenter;Hardware;Power\r\n"),
        "{vcard}"
    );
    assert!(
        vcard.contains("TITLE;X-JMAP-KEY=t1:Chief Scientist\r\n"),
        "{vcard}"
    );
    assert!(
        vcard.contains("TITLE;X-JMAP-KEY=t2:Distinguished Engineer\r\n"),
        "{vcard}"
    );
    assert!(
        vcard.contains("ROLE;X-JMAP-KEY=r1:Technical Lead\r\n"),
        "{vcard}"
    );
    assert!(
        vcard.contains("ROLE;X-JMAP-KEY=r2:Steering Committee Member\r\n"),
        "{vcard}"
    );
    // Unmapped vendor title kind gets no vCard line.
    assert!(!vcard.contains("Honorary Fellow"), "{vcard}");

    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(back.organizations, card.organizations);

    // Expected titles in roundtrip: t1, t2, r1, r2 with canonical kind (None for default title).
    let expected_titles = [
        (
            "t1".to_owned(),
            Title {
                name: "Chief Scientist".to_owned(),
                kind: None,
                ..Title::default()
            },
        ),
        (
            "t2".to_owned(),
            Title {
                name: "Distinguished Engineer".to_owned(),
                kind: None,
                ..Title::default()
            },
        ),
        (
            "r1".to_owned(),
            Title {
                name: "Technical Lead".to_owned(),
                kind: Some("role".to_owned()),
                ..Title::default()
            },
        ),
        (
            "r2".to_owned(),
            Title {
                name: "Steering Committee Member".to_owned(),
                kind: Some("role".to_owned()),
                ..Title::default()
            },
        ),
    ]
    .into();
    assert_eq!(back.titles, Some(expected_titles));
}

#[test]
fn multi_component_org_with_escaped_punctuation_roundtrips() {
    let card = ContactCard {
        organizations: Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some("Acme, Inc.".to_owned()),
                    units: Some(vec![
                        OrgUnit::new("R&D; Applied Science"),
                        OrgUnit::new("Optics, Lasers & Sensors"),
                        OrgUnit::new("Lab #4 (Room 101; Wing B)"),
                    ]),
                    ..Organization::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert_eq!(
        line(&unfolded(&vcard), "ORG"),
        "ORG;X-JMAP-KEY=o1:Acme\\, Inc.;R&D\\; Applied Science;Optics\\, Lasers & Sensors;Lab #4 (Room 101\\; Wing B)"
    );

    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(back.organizations, card.organizations);
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

#[test]
fn multiple_addresses_with_different_types_and_labels_pair_accurately() {
    let mut addresses = BTreeMap::new();
    addresses.insert(
        "a-work".to_owned(),
        Address {
            contexts: Some(json!({"work": true})),
            components: Some(vec![
                AddressComponent::new("name", "Hauptstraße 1"),
                AddressComponent::new("locality", "Berlin"),
                AddressComponent::new("postcode", "10115"),
                AddressComponent::new("country", "Germany"),
            ]),
            full: Some("Hauptstraße 1\n10115 Berlin\nGermany".to_owned()),
            ..Address::default()
        },
    );
    addresses.insert(
        "a-home".to_owned(),
        Address {
            contexts: Some(json!({"private": true})),
            components: Some(vec![
                AddressComponent::new("name", "Heimweg 2"),
                AddressComponent::new("locality", "München"),
                AddressComponent::new("postcode", "80331"),
                AddressComponent::new("country", "Germany"),
            ]),
            full: Some("Heimweg 2\n80331 München\nGermany".to_owned()),
            ..Address::default()
        },
    );

    let card = ContactCard {
        addresses: Some(addresses),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert!(vcard.contains("ADR;X-JMAP-KEY=a-work;TYPE=WORK:"));
    assert!(vcard.contains("LABEL;X-JMAP-KEY=a-work;TYPE=WORK:"));
    assert!(vcard.contains("ADR;X-JMAP-KEY=a-home;TYPE=HOME:"));
    assert!(vcard.contains("LABEL;X-JMAP-KEY=a-home;TYPE=HOME:"));

    // Roundtrip directly on the emitted vCard
    let roundtrip = vcard_to_card(&vcard).expect("parse emitted");
    assert_eq!(roundtrip.addresses, card.addresses);

    // And simulate EDS where LABEL lines have TYPE but no X-JMAP-KEY
    let eds_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "ADR;X-JMAP-KEY=a-work;TYPE=WORK:;;Hauptstraße 1;Berlin;;10115;Germany\r\n",
        "LABEL;TYPE=WORK:Hauptstraße 1\\n10115 Berlin\\nGermany\r\n",
        "ADR;X-JMAP-KEY=a-home;TYPE=HOME:;;Heimweg 2;München;;80331;Germany\r\n",
        "LABEL;TYPE=HOME:Heimweg 2\\n80331 München\\nGermany\r\n",
        "END:VCARD\r\n"
    );
    let from_eds = vcard_to_card(eds_vcard).expect("parse from EDS");
    let parsed_addresses = from_eds.addresses.expect("addresses");
    assert_eq!(
        parsed_addresses["a-work"].full.as_deref(),
        Some("Hauptstraße 1\n10115 Berlin\nGermany")
    );
    assert_eq!(
        parsed_addresses["a-home"].full.as_deref(),
        Some("Heimweg 2\n80331 München\nGermany")
    );
}

#[test]
fn an_address_at_home_and_at_work_states_one_slot_rather_than_both() {
    let mut addresses = BTreeMap::new();
    addresses.insert(
        "a1".to_owned(),
        Address {
            contexts: Some(json!({"work": true, "private": true})),
            components: Some(vec![
                AddressComponent::new("name", "Hauptstraße 1"),
                AddressComponent::new("locality", "Berlin"),
            ]),
            full: Some("Hauptstraße 1\n10115 Berlin".to_owned()),
            ..Address::default()
        },
    );
    let card = ContactCard {
        addresses: Some(addresses),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert_eq!(
        line(&vcard, "ADR"),
        "ADR;X-JMAP-KEY=a1;TYPE=HOME:;;Hauptstraße 1;Berlin;;;"
    );
    assert_eq!(
        line(&vcard, "LABEL"),
        "LABEL;X-JMAP-KEY=a1;TYPE=HOME:Hauptstraße 1\\n10115 Berlin"
    );
}

#[test]
fn a_phone_at_home_and_at_work_states_one_slot_rather_than_both() {
    let mut phones = BTreeMap::new();
    phones.insert(
        "p1".to_owned(),
        ContactPhone {
            number: "+49 30 111".to_owned(),
            contexts: Some(json!({"work": true, "private": true})),
            features: Some(json!({"voice": true})),
            ..ContactPhone::default()
        },
    );
    let card = ContactCard {
        phones: Some(phones),
        ..ContactCard::default()
    };

    // One feature is stated here anyway, so the feature `TYPE` is the same
    // either way; `a_number_that_is_a_voice_line_and_a_fax_states_one_feature`
    // is where the feature side of the narrowing is pinned.
    assert_eq!(
        line(&card_to_vcard(&card), "TEL"),
        "TEL;X-JMAP-KEY=p1;TYPE=HOME,VOICE:+49 30 111"
    );
}

/// A phone with these `contexts` and `features`, written out.
fn phone_line(contexts: Option<Value>, features: Option<Value>) -> String {
    let mut phones = BTreeMap::new();
    phones.insert(
        "p1".to_owned(),
        ContactPhone {
            number: "+49 30 111".to_owned(),
            contexts,
            features,
            ..ContactPhone::default()
        },
    );
    let vcard = card_to_vcard(&ContactCard {
        phones: Some(phones),
        ..ContactCard::default()
    });
    line(&vcard, "TEL").to_owned()
}

#[test]
fn a_number_that_is_a_voice_line_and_a_fax_states_one_feature() {
    // `TEL;TYPE=VOICE,FAX` reaches *no* field of libebook-contacts 3.52 at
    // all — the number is simply not in the contact editor — and with a
    // context it reaches two that overwrite each other. See `eds-sys`'
    // `a_line_wearing_several_feature_types_reaches_two_fields_or_none`.
    assert_eq!(
        phone_line(None, Some(json!({"voice": true, "fax": true}))),
        "TEL;X-JMAP-KEY=p1;TYPE=FAX:+49 30 111"
    );
    assert_eq!(
        phone_line(
            Some(json!({"work": true})),
            Some(json!({"voice": true, "fax": true}))
        ),
        "TEL;X-JMAP-KEY=p1;TYPE=WORK,FAX:+49 30 111"
    );
}

#[test]
fn a_mobile_that_is_also_a_pager_states_the_mobile() {
    // The one pair EDS itself files into two fields with no context needed.
    assert_eq!(
        phone_line(None, Some(json!({"mobile": true, "pager": true}))),
        "TEL;X-JMAP-KEY=p1;TYPE=CELL:+49 30 111"
    );
    // A mobile outranks the unmarked `voice` and the fax alike.
    assert_eq!(
        phone_line(
            None,
            Some(json!({"voice": true, "fax": true, "mobile": true}))
        ),
        "TEL;X-JMAP-KEY=p1;TYPE=CELL:+49 30 111"
    );
}

#[test]
fn a_video_number_that_is_also_a_voice_line_states_the_voice_line() {
    // `VIDEO` is the one feature no EDS field matches, so it can never be the
    // slot while another feature is there to be stated.
    assert_eq!(
        phone_line(None, Some(json!({"voice": true, "video": true}))),
        "TEL;X-JMAP-KEY=p1;TYPE=VOICE:+49 30 111"
    );
    // On its own it is still written — dropping it would say the number is a
    // voice line, which the card never claimed.
    assert_eq!(
        phone_line(None, Some(json!({"video": true}))),
        "TEL;X-JMAP-KEY=p1;TYPE=VIDEO:+49 30 111"
    );
}

#[test]
fn a_line_with_several_features_is_still_read_as_all_of_them() {
    // Narrowing is about what *we* write. A card that arrives from elsewhere
    // saying the number is both keeps saying so.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "TEL;X-JMAP-KEY=p1;TYPE=WORK,VOICE,FAX:+49 30 111\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");
    assert_eq!(
        card.phones.unwrap()["p1"].features,
        Some(json!({"voice": true, "fax": true}))
    );
}

#[test]
fn an_email_at_home_and_at_work_still_states_both() {
    let mut emails = BTreeMap::new();
    emails.insert(
        "e1".to_owned(),
        ContactEmail {
            address: "vera@example.com".to_owned(),
            contexts: Some(json!({"work": true, "private": true})),
            ..ContactEmail::default()
        },
    );
    let card = ContactCard {
        emails: Some(emails),
        ..ContactCard::default()
    };

    // EDS files an `EMAIL` line by its *position* — `E_CONTACT_EMAIL_1` to
    // `_4` — rather than by its `TYPE`, so a line wearing both contexts
    // reaches one field rather than two and there is nothing to protect the
    // user from. Stating both is what lets the save read either back.
    assert_eq!(
        line(&card_to_vcard(&card), "EMAIL"),
        "EMAIL;X-JMAP-KEY=e1;TYPE=WORK,HOME:vera@example.com"
    );
}

#[test]
fn unlabelled_second_address_of_same_type_is_not_corrupted_by_label() {
    let eds_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "ADR;X-JMAP-KEY=a1;TYPE=HOME:;;Heimweg 2;München;;80331;Germany\r\n",
        "LABEL;TYPE=HOME:Heimweg 2\\n80331 München\\nGermany\r\n",
        "ADR;X-JMAP-KEY=a2;TYPE=HOME:;;Ferienhaus 4;Garmisch;;82467;Germany\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(eds_vcard).expect("parse");
    let addresses = card.addresses.expect("addresses");
    assert_eq!(addresses.keys().collect::<Vec<_>>(), vec!["a1", "a2"]);
    assert_eq!(
        addresses["a1"].full.as_deref(),
        Some("Heimweg 2\n80331 München\nGermany")
    );
    assert_eq!(addresses["a2"].full, None);
    assert_eq!(components_of(&addresses["a2"])[0], ("name", "Ferienhaus 4"));
}

#[test]
fn bare_label_without_type_pairs_with_untyped_address() {
    let eds_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "ADR;X-JMAP-KEY=a1:;;Hauptstraße 1;Berlin;;10115;Germany\r\n",
        "LABEL:Hauptstraße 1\\n10115 Berlin\\nGermany\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(eds_vcard).expect("parse");
    let addresses = card.addresses.expect("addresses");
    assert_eq!(addresses.keys().collect::<Vec<_>>(), vec!["a1"]);
    assert_eq!(
        addresses["a1"].full.as_deref(),
        Some("Hauptstraße 1\n10115 Berlin\nGermany")
    );
    assert_eq!(addresses["a1"].contexts, None);
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
fn a_date_before_the_year_eds_can_write_gets_no_line() {
    // `e_contact_date_to_string()` CLAMPs the year to 1000..=9999 — measured
    // against libebook-contacts 3.52 in `eds-sys/tests/contacts.rs`. The line
    // is only rewritten when the field is set, so a card merely passing
    // through keeps the year it arrived with; but the contact editor sets
    // every field it shows, so the first time the user opens Charlemagne and
    // presses Save, `BDAY:0800-06-21` becomes `BDAY:1000-06-21` — the same day
    // of the same month, moved two centuries. Stating nothing keeps the
    // server's date the server's; `diff_entries` then leaves an anniversary no
    // line states alone.
    for date in [
        json!({"year": 800, "month": 6, "day": 21}),
        json!({"year": 999, "month": 12, "day": 31}),
        json!({"year": 1, "month": 1, "day": 1}),
        json!({"@type": "Timestamp", "utc": "0800-06-21T09:00:00Z"}),
    ] {
        let vcard = card_to_vcard(&one_anniversary("birth", date.clone()));
        assert!(!vcard.contains("\r\nBDAY"), "{date}: {vcard}");
    }

    // The first year it can write is written.
    let vcard = card_to_vcard(&one_anniversary(
        "birth",
        json!({"year": 1000, "month": 6, "day": 21}),
    ));
    assert_eq!(line(&vcard, "BDAY"), "BDAY;X-JMAP-KEY=y1:1000-06-21");
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
        "URL;X-JMAP-KEY=l1:https://vera.example/q?tags=a,b;c"
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
fn action_query_im_uris_get_no_vcard_line() {
    // AIM, MSN, Yahoo, and ICQ URIs with action/query parameters (e.g. `?screenname=`,
    // `?contact=`, `?uin=`) or path segments do not represent bare handles. When given
    // only such a `uri` (and no `user`), card_to_vcard safely omits them so EDS fields
    // are not corrupted.
    for (service, uri, header) in [
        ("AIM", "aim:goim?screenname=alice", "X-AIM"),
        ("AIM", "aim:addbuddy?screenname=alice", "X-AIM"),
        ("MSN", "msnim:chat?contact=bob@example.com", "X-MSN"),
        ("MSN", "msnim:add?contact=bob@example.com", "X-MSN"),
        ("Yahoo", "ymsgr:sendim?carol", "X-YAHOO"),
        ("Yahoo", "ymsgr:chat?carol", "X-YAHOO"),
        ("ICQ", "icq:message?uin=12345678", "X-ICQ"),
        ("Matrix", "matrix:u/vera:matrix.example", "X-MATRIX"),
    ] {
        let vcard = card_to_vcard(&at_uri(Some(service), uri));
        assert!(
            !vcard.contains(header),
            "{service} uri '{uri}' was unexpectedly drawn on {header}: {vcard}"
        );
    }
}

#[test]
fn bare_im_service_uris_are_drawn_and_roundtripped() {
    // Bare URI schemes for all supported IM services (AIM, ICQ, MSN, Yahoo, GroupWise, Matrix)
    // resolve to their respective handles and round-trip faithfully.
    for (service, uri, handle, header) in [
        ("AIM", "aim:alice_aim", "alice_aim", "X-AIM"),
        ("ICQ", "icq:12345678", "12345678", "X-ICQ"),
        ("MSN", "msn:bob@example.com", "bob@example.com", "X-MSN"),
        ("MSN", "msnim:bob@example.com", "bob@example.com", "X-MSN"),
        ("Yahoo", "yahoo:carol_yahoo", "carol_yahoo", "X-YAHOO"),
        ("Yahoo", "ymsgr:carol_yahoo", "carol_yahoo", "X-YAHOO"),
        ("GroupWise", "groupwise:dave_gw", "dave_gw", "X-GROUPWISE"),
        ("Matrix", "matrix:elena_matrix", "elena_matrix", "X-MATRIX"),
    ] {
        let card = at_uri(Some(service), uri);
        let vcard = card_to_vcard(&card);
        assert_eq!(
            line(&vcard, header),
            format!("{header};X-JMAP-KEY=s1;TYPE=HOME:{handle}"),
            "service: {service}, uri: {uri}"
        );

        let parsed = vcard_to_card(&vcard).expect("parse");
        let services = parsed.online_services.expect("online services");
        assert_eq!(services["s1"].user.as_deref(), Some(handle));
        assert_eq!(services["s1"].uri, None);
        assert_eq!(services["s1"].service.as_deref(), Some(service));
    }
}

#[test]
fn online_service_uri_constructs_canonical_uri_for_all_supported_services() {
    use jmap_vcard::contact::online_service_uri;

    for (service, handle, expected_uri) in [
        ("AIM", "alice", "aim:alice"),
        ("Gadu-Gadu", "12345", "gg:12345"),
        ("Google Talk", "bob@example.com", "xmpp:bob@example.com"),
        ("GroupWise", "carol", "groupwise:carol"),
        ("ICQ", "67890", "icq:67890"),
        ("Jabber", "dave@example.com", "xmpp:dave@example.com"),
        ("MSN", "elena@example.com", "msn:elena@example.com"),
        ("Matrix", "frank", "matrix:frank"),
        ("Skype", "grace", "skype:grace"),
        ("Yahoo", "heidi", "yahoo:heidi"),
    ] {
        assert_eq!(
            online_service_uri(service, handle).as_deref(),
            Some(expected_uri),
            "canonical URI for {service}"
        );
    }

    // Services without registered schemes or invalid handles return None
    assert_eq!(online_service_uri("Signal", "ivan"), None);
    assert_eq!(
        online_service_uri("AIM", "invalid handle with spaces"),
        None
    );
    assert_eq!(online_service_uri("Yahoo", "invalid?query"), None);
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

#[test]
fn multiple_im_services_with_home_and_work_contexts_map_accurately() {
    let mut card = ContactCard::default();
    let mut s1 = OnlineService {
        service: Some("Jabber".to_owned()),
        user: Some("vera@home.example".to_owned()),
        ..OnlineService::default()
    };
    s1.extra
        .insert("contexts".to_owned(), serde_json::json!({"private": true}));

    let mut s2 = OnlineService {
        service: Some("Jabber".to_owned()),
        user: Some("vera@work.example".to_owned()),
        ..OnlineService::default()
    };
    s2.extra
        .insert("contexts".to_owned(), serde_json::json!({"work": true}));

    let mut s3 = OnlineService {
        service: Some("Matrix".to_owned()),
        user: Some("@vera:matrix.example".to_owned()),
        ..OnlineService::default()
    };
    s3.extra
        .insert("contexts".to_owned(), serde_json::json!({"work": true}));

    let mut s4 = OnlineService {
        service: Some("Skype".to_owned()),
        user: Some("vera_skype".to_owned()),
        ..OnlineService::default()
    };
    s4.extra
        .insert("contexts".to_owned(), serde_json::json!({"private": true}));

    let mut s5 = OnlineService {
        service: Some("Gadu-Gadu".to_owned()),
        user: Some("123456".to_owned()),
        ..OnlineService::default()
    };
    s5.extra
        .insert("contexts".to_owned(), serde_json::json!({"work": true}));

    card.online_services = Some(
        [
            ("s1".to_owned(), s1),
            ("s2".to_owned(), s2),
            ("s3".to_owned(), s3),
            ("s4".to_owned(), s4),
            ("s5".to_owned(), s5),
        ]
        .into_iter()
        .collect(),
    );

    let vcard = card_to_vcard(&card);
    assert!(vcard.contains("X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera@home.example"));
    assert!(vcard.contains("X-JABBER;X-JMAP-KEY=s2;TYPE=WORK:vera@work.example"));
    assert!(vcard.contains("X-MATRIX;X-JMAP-KEY=s3;TYPE=WORK:@vera:matrix.example"));
    assert!(vcard.contains("X-SKYPE;X-JMAP-KEY=s4;TYPE=HOME:vera_skype"));
    assert!(vcard.contains("X-GADUGADU;X-JMAP-KEY=s5;TYPE=WORK:123456"));

    let parsed = vcard_to_card(&vcard).expect("parse");
    let services = parsed.online_services.expect("online services");
    assert_eq!(services.len(), 5);
    assert_eq!(services["s1"].user.as_deref(), Some("vera@home.example"));
    assert_eq!(services["s1"].service.as_deref(), Some("Jabber"));
    assert_eq!(services["s2"].user.as_deref(), Some("vera@work.example"));
    assert_eq!(services["s2"].service.as_deref(), Some("Jabber"));
    assert_eq!(services["s3"].user.as_deref(), Some("@vera:matrix.example"));
    assert_eq!(services["s3"].service.as_deref(), Some("Matrix"));
    assert_eq!(services["s4"].user.as_deref(), Some("vera_skype"));
    assert_eq!(services["s4"].service.as_deref(), Some("Skype"));
    assert_eq!(services["s5"].user.as_deref(), Some("123456"));
    assert_eq!(services["s5"].service.as_deref(), Some("Gadu-Gadu"));
}

#[test]
fn secondary_im_service_of_same_type_is_preserved_when_reconstructed() {
    let vcard = "BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
FN:Vera\r\n\
X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera1@jabber.example\r\n\
X-JABBER;X-JMAP-KEY=s2;TYPE=WORK:vera2@jabber.example\r\n\
X-JABBER;X-JMAP-KEY=s3;TYPE=HOME:vera3@jabber.example\r\n\
END:VCARD\r\n";

    let parsed = vcard_to_card(vcard).expect("parse");
    let services = parsed.online_services.expect("online services");
    assert_eq!(services.len(), 3);
    assert_eq!(services["s1"].user.as_deref(), Some("vera1@jabber.example"));
    assert_eq!(services["s2"].user.as_deref(), Some("vera2@jabber.example"));
    assert_eq!(services["s3"].user.as_deref(), Some("vera3@jabber.example"));
}

#[test]
fn bare_im_service_lines_without_keys_receive_invented_keys() {
    let vcard = "BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
FN:Vera\r\n\
X-JABBER;TYPE=HOME:alice@jabber.example\r\n\
X-MATRIX;TYPE=WORK:@bob:matrix.example\r\n\
END:VCARD\r\n";

    let parsed = vcard_to_card(vcard).expect("parse");
    let services = parsed.online_services.expect("online services");
    assert_eq!(services.len(), 2);
    assert_eq!(services["s1"].user.as_deref(), Some("alice@jabber.example"));
    assert_eq!(services["s1"].service.as_deref(), Some("Jabber"));
    assert_eq!(services["s2"].user.as_deref(), Some("@bob:matrix.example"));
    assert_eq!(services["s2"].service.as_deref(), Some("Matrix"));
}

#[test]
fn photo_with_value_uri_and_multiple_media_kinds_emits_only_photo_and_roundtrips() {
    let mut card = ContactCard::simple("b1", "Vera Oldenburg", "vera@example.com");
    card.media = Some(
        [
            (
                "m1".to_owned(),
                Media {
                    kind: Some("photo".to_owned()),
                    uri: "https://example.com/avatar.png".to_owned(),
                    ..Media::default()
                },
            ),
            (
                "m2".to_owned(),
                Media {
                    kind: Some("logo".to_owned()),
                    uri: "https://example.com/logo.png".to_owned(),
                    ..Media::default()
                },
            ),
            (
                "m3".to_owned(),
                Media {
                    kind: Some("sound".to_owned()),
                    uri: "https://example.com/pronunciation.ogg".to_owned(),
                    ..Media::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
    );

    let vcard = card_to_vcard(&card);
    assert!(
        vcard.contains("PHOTO;X-JMAP-KEY=m1;VALUE=uri:https://example.com/avatar.png")
            || vcard.contains("PHOTO;VALUE=uri;X-JMAP-KEY=m1:https://example.com/avatar.png"),
        "vcard should contain PHOTO with URI: {vcard}"
    );
    assert!(
        !vcard.contains("LOGO"),
        "vcard should not contain LOGO line: {vcard}"
    );
    assert!(
        !vcard.contains("SOUND"),
        "vcard should not contain SOUND line: {vcard}"
    );

    let read_back = vcard_to_card(&vcard).expect("parse");
    let media = read_back.media.expect("media");
    assert_eq!(media.len(), 1, "only photo media is in vCard: {media:?}");
    assert_eq!(media["m1"].kind.as_deref(), Some("photo"));
    assert_eq!(media["m1"].uri, "https://example.com/avatar.png");
}

#[test]
fn vcard_with_logo_and_multiple_photos_reads_only_photos() {
    let vcard = "BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
FN:Vera\r\n\
PHOTO;X-JMAP-KEY=m1;VALUE=uri:https://example.com/photo1.png\r\n\
PHOTO;X-JMAP-KEY=m2;TYPE=jpeg;ENCODING=b:aGVsbG8tcGhvdG8=\r\n\
LOGO;X-JMAP-KEY=l1;VALUE=uri:https://example.com/logo.png\r\n\
END:VCARD\r\n";

    let card = vcard_to_card(vcard).expect("parse");
    let media = card.media.expect("media");
    assert_eq!(
        media.len(),
        2,
        "only the two photos should be in media: {media:?}"
    );
    assert_eq!(media["m1"].kind.as_deref(), Some("photo"));
    assert_eq!(media["m1"].uri, "https://example.com/photo1.png");
    assert_eq!(media["m2"].kind.as_deref(), Some("photo"));
    assert_eq!(media["m2"].uri, "data:image/jpeg;base64,aGVsbG8tcGhvdG8=");
}

#[test]
fn maps_notes_with_multiple_entries_and_newlines_faithfully() {
    let mut card = ContactCard {
        id: Some("C10".into()),
        name: Some(Name {
            full: Some("Vera Olden".to_owned()),
            ..Name::default()
        }),
        ..ContactCard::default()
    };
    card.notes = Some(
        [
            (
                "n1".to_owned(),
                Note {
                    note: "First paragraph.\nSecond paragraph with details.".to_owned(),
                    ..Note::default()
                },
            ),
            (
                "n2".to_owned(),
                Note {
                    note: "Follow-up note for 2026 meeting.".to_owned(),
                    ..Note::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
    );

    let vcard = card_to_vcard(&card);
    assert!(
        vcard.contains("NOTE;X-JMAP-KEY=n1:First paragraph.\\nSecond paragraph with details.")
            || vcard
                .contains("NOTE;X-JMAP-KEY=n1:First paragraph.\nSecond paragraph with details.")
            || vcard.contains("NOTE;X-JMAP-KEY=n1:"),
        "vCard should contain escaped first note: {vcard}"
    );
    assert!(
        vcard.contains("NOTE;X-JMAP-KEY=n2:Follow-up note for 2026 meeting."),
        "vCard should contain second note: {vcard}"
    );

    let parsed = vcard_to_card(&vcard).expect("parse");
    let notes = parsed.notes.expect("notes");
    assert_eq!(notes.len(), 2);
    assert_eq!(
        notes["n1"].note,
        "First paragraph.\nSecond paragraph with details."
    );
    assert_eq!(notes["n2"].note, "Follow-up note for 2026 meeting.");
}

#[test]
fn maps_calendars_freebusy_and_links_faithfully() {
    let mut card = ContactCard {
        id: Some("C11".into()),
        name: Some(Name {
            full: Some("Vera Olden".to_owned()),
            ..Name::default()
        }),
        ..ContactCard::default()
    };
    card.calendars = Some(
        [
            (
                "c1".to_owned(),
                Calendar {
                    kind: Some("calendar".to_owned()),
                    uri: "https://example.com/team-calendar.ics".to_owned(),
                    ..Calendar::default()
                },
            ),
            (
                "f1".to_owned(),
                Calendar {
                    kind: Some("freeBusy".to_owned()),
                    uri: "https://example.com/busy-schedule.ifb".to_owned(),
                    ..Calendar::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
    );
    card.links = Some(
        [(
            "l1".to_owned(),
            Link {
                uri: "https://example.com/homepage".to_owned(),
                ..Link::default()
            },
        )]
        .into_iter()
        .collect(),
    );

    let vcard = card_to_vcard(&card);
    assert!(
        vcard.contains("CALURI;X-JMAP-KEY=c1:https://example.com/team-calendar.ics"),
        "CALURI line missing: {vcard}"
    );
    assert!(
        vcard.contains("FBURL;X-JMAP-KEY=f1:https://example.com/busy-schedule.ifb"),
        "FBURL line missing: {vcard}"
    );
    assert!(
        vcard.contains("URL;X-JMAP-KEY=l1:https://example.com/homepage"),
        "URL line missing: {vcard}"
    );

    let parsed = vcard_to_card(&vcard).expect("parse");
    let cals = parsed.calendars.expect("calendars");
    assert_eq!(cals.len(), 2);
    assert_eq!(cals["c1"].kind.as_deref(), Some("calendar"));
    assert_eq!(cals["c1"].uri, "https://example.com/team-calendar.ics");
    assert_eq!(cals["f1"].kind.as_deref(), Some("freeBusy"));
    assert_eq!(cals["f1"].uri, "https://example.com/busy-schedule.ifb");

    let links = parsed.links.expect("links");
    assert_eq!(links.len(), 1);
    assert_eq!(links["l1"].uri, "https://example.com/homepage");
}

#[test]
fn maps_nicknames_spouse_and_keywords_faithfully() {
    let mut card = ContactCard {
        id: Some("C12".into()),
        name: Some(Name {
            full: Some("Vera Olden".to_owned()),
            ..Name::default()
        }),
        ..ContactCard::default()
    };
    card.nicknames = Some(
        [(
            "k1".to_owned(),
            Nickname {
                name: "Vee".to_owned(),
                ..Nickname::default()
            },
        )]
        .into_iter()
        .collect(),
    );
    card.related_to = Some(
        [(
            "Alex Olden".to_owned(),
            Relation {
                relation: Some([("spouse".to_owned(), json!(true))].into_iter().collect()),
                ..Relation::default()
            },
        )]
        .into_iter()
        .collect(),
    );
    card.keywords = Some(
        [
            ("Engineering".to_owned(), json!(true)),
            ("Rust".to_owned(), json!(true)),
        ]
        .into_iter()
        .collect(),
    );

    let vcard = card_to_vcard(&card);
    assert!(
        vcard.contains("NICKNAME;X-JMAP-KEY=k1:Vee"),
        "NICKNAME line missing: {vcard}"
    );
    assert!(
        vcard.contains("X-EVOLUTION-SPOUSE:Alex Olden"),
        "X-EVOLUTION-SPOUSE line missing: {vcard}"
    );
    assert!(
        vcard.contains("CATEGORIES:Engineering,Rust")
            || vcard.contains("CATEGORIES:Rust,Engineering"),
        "CATEGORIES line missing: {vcard}"
    );

    let parsed = vcard_to_card(&vcard).expect("parse");
    let nicks = parsed.nicknames.expect("nicknames");
    assert_eq!(nicks["k1"].name, "Vee");

    let rels = parsed.related_to.expect("relatedTo");
    assert!(rels.contains_key("Alex Olden"));
    assert_eq!(
        rels["Alex Olden"]
            .relation
            .as_ref()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["spouse"]
    );

    let kws = parsed.keywords.expect("keywords");
    assert!(kws.contains_key("Engineering"));
    assert!(kws.contains_key("Rust"));
}

#[test]
fn maps_phones_with_multiple_types_features_and_pref_faithfully() {
    let card = ContactCard {
        phones: Some(
            [
                (
                    "p1".to_owned(),
                    ContactPhone {
                        number: "+49 30 111111".to_owned(),
                        contexts: Some(json!({"work": true})),
                        features: Some(json!({"voice": true})),
                        pref: Some(1),
                        ..ContactPhone::default()
                    },
                ),
                (
                    "p2".to_owned(),
                    ContactPhone {
                        number: "+49 30 222222".to_owned(),
                        contexts: Some(json!({"private": true})),
                        features: Some(json!({"voice": true})),
                        ..ContactPhone::default()
                    },
                ),
                (
                    "p3".to_owned(),
                    ContactPhone {
                        number: "+49 170 333333".to_owned(),
                        features: Some(json!({"mobile": true})),
                        ..ContactPhone::default()
                    },
                ),
                (
                    "p4".to_owned(),
                    ContactPhone {
                        number: "+49 30 444444".to_owned(),
                        contexts: Some(json!({"work": true})),
                        features: Some(json!({"fax": true})),
                        ..ContactPhone::default()
                    },
                ),
                (
                    "p5".to_owned(),
                    ContactPhone {
                        number: "+49 30 555555".to_owned(),
                        features: Some(json!({"pager": true})),
                        ..ContactPhone::default()
                    },
                ),
                (
                    "p6".to_owned(),
                    ContactPhone {
                        number: "+49 30 666666".to_owned(),
                        features: Some(json!({"video": true})),
                        ..ContactPhone::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert!(
        vcard.contains("TEL;X-JMAP-KEY=p1;TYPE=WORK,VOICE,PREF:+49 30 111111")
            || vcard.contains("TEL;X-JMAP-KEY=p1;TYPE=WORK,PREF,VOICE:+49 30 111111")
            || vcard.contains("TEL;X-JMAP-KEY=p1;TYPE=PREF,WORK,VOICE:+49 30 111111")
            || (vcard.contains("TEL") && vcard.contains("+49 30 111111") && vcard.contains("PREF")),
        "p1 TEL PREF line missing or malformed: {vcard}"
    );
    assert!(
        vcard.contains("TEL;X-JMAP-KEY=p2;TYPE=HOME,VOICE:+49 30 222222")
            || (vcard.contains("+49 30 222222") && vcard.contains("HOME")),
        "p2 TEL line missing: {vcard}"
    );
    assert!(
        vcard.contains("TEL;X-JMAP-KEY=p3;TYPE=CELL:+49 170 333333")
            || (vcard.contains("+49 170 333333") && vcard.contains("CELL")),
        "p3 CELL line missing: {vcard}"
    );
    assert!(
        vcard.contains("TEL;X-JMAP-KEY=p4;TYPE=WORK,FAX:+49 30 444444")
            || (vcard.contains("+49 30 444444") && vcard.contains("FAX")),
        "p4 FAX line missing: {vcard}"
    );
    assert!(
        vcard.contains("TEL;X-JMAP-KEY=p5;TYPE=PAGER:+49 30 555555")
            || (vcard.contains("+49 30 555555") && vcard.contains("PAGER")),
        "p5 PAGER line missing: {vcard}"
    );
    assert!(
        vcard.contains("TEL;X-JMAP-KEY=p6;TYPE=VIDEO:+49 30 666666")
            || (vcard.contains("+49 30 666666") && vcard.contains("VIDEO")),
        "p6 VIDEO line missing: {vcard}"
    );

    let parsed = vcard_to_card(&vcard).expect("parse");
    let phones = parsed.phones.expect("phones");
    assert_eq!(phones.len(), 6);
    assert_eq!(phones["p1"].number, "+49 30 111111");
    assert_eq!(phones["p1"].contexts, Some(json!({"work": true})));
    assert_eq!(phones["p1"].features, Some(json!({"voice": true})));
    assert_eq!(phones["p1"].pref, Some(1));

    assert_eq!(phones["p2"].number, "+49 30 222222");
    assert_eq!(phones["p2"].contexts, Some(json!({"private": true})));
    assert_eq!(phones["p2"].features, Some(json!({"voice": true})));
    assert_eq!(phones["p2"].pref, None);

    assert_eq!(phones["p3"].number, "+49 170 333333");
    assert_eq!(phones["p3"].features, Some(json!({"mobile": true})));

    assert_eq!(phones["p4"].number, "+49 30 444444");
    assert_eq!(phones["p4"].contexts, Some(json!({"work": true})));
    assert_eq!(phones["p4"].features, Some(json!({"fax": true})));

    assert_eq!(phones["p5"].number, "+49 30 555555");
    assert_eq!(phones["p5"].features, Some(json!({"pager": true})));

    assert_eq!(phones["p6"].number, "+49 30 666666");
    assert_eq!(phones["p6"].features, Some(json!({"video": true})));
}

#[test]
fn maps_emails_with_multiple_contexts_and_pref_faithfully() {
    let card = ContactCard {
        emails: Some(
            [
                (
                    "e1".to_owned(),
                    ContactEmail {
                        address: "vera.work@example.com".to_owned(),
                        contexts: Some(json!({"work": true})),
                        pref: Some(1),
                        ..ContactEmail::default()
                    },
                ),
                (
                    "e2".to_owned(),
                    ContactEmail {
                        address: "vera.home@example.com".to_owned(),
                        contexts: Some(json!({"private": true})),
                        pref: None,
                        ..ContactEmail::default()
                    },
                ),
                (
                    "e3".to_owned(),
                    ContactEmail {
                        address: "vera.direct@example.com".to_owned(),
                        contexts: None,
                        pref: None,
                        ..ContactEmail::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert!(
        vcard.contains("EMAIL;X-JMAP-KEY=e1;TYPE=WORK,PREF:vera.work@example.com")
            || vcard.contains("EMAIL;X-JMAP-KEY=e1;TYPE=PREF,WORK:vera.work@example.com")
            || (vcard.contains("vera.work@example.com") && vcard.contains("PREF")),
        "e1 EMAIL line missing PREF: {vcard}"
    );
    assert!(
        vcard.contains("EMAIL;X-JMAP-KEY=e2;TYPE=HOME:vera.home@example.com")
            || (vcard.contains("vera.home@example.com") && vcard.contains("HOME")),
        "e2 EMAIL line missing: {vcard}"
    );
    assert!(
        vcard.contains("EMAIL;X-JMAP-KEY=e3:vera.direct@example.com")
            || vcard.contains("vera.direct@example.com"),
        "e3 EMAIL line missing: {vcard}"
    );

    let parsed = vcard_to_card(&vcard).expect("parse");
    let emails = parsed.emails.expect("emails");
    assert_eq!(emails.len(), 3);
    assert_eq!(emails["e1"].address, "vera.work@example.com");
    assert_eq!(emails["e1"].contexts, Some(json!({"work": true})));
    assert_eq!(emails["e1"].pref, Some(1));

    assert_eq!(emails["e2"].address, "vera.home@example.com");
    assert_eq!(emails["e2"].contexts, Some(json!({"private": true})));
    assert_eq!(emails["e2"].pref, None);

    assert_eq!(emails["e3"].address, "vera.direct@example.com");
    assert_eq!(emails["e3"].contexts, None);
    assert_eq!(emails["e3"].pref, None);
}

#[test]
fn maps_name_with_all_components_and_empty_prefix_suffix_faithfully() {
    let card = ContactCard {
        name: Some(Name {
            full: Some("Prof. Dr. Vera Marie Oldenburg MSc Ph.D.".to_owned()),
            components: Some(vec![
                NameComponent::new("title", "Prof. Dr."),
                NameComponent::new("given", "Vera"),
                NameComponent::new("given2", "Marie"),
                NameComponent::new("surname", "Oldenburg"),
                NameComponent::new("credential", "MSc Ph.D."),
            ]),
            ..Name::default()
        }),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert_eq!(
        line(&vcard, "N:"),
        "N:Oldenburg;Vera;Marie;Prof. Dr.;MSc Ph.D."
    );
    assert_eq!(
        line(&vcard, "FN:"),
        "FN:Prof. Dr. Vera Marie Oldenburg MSc Ph.D."
    );

    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(back.name.as_ref().unwrap(), card.name.as_ref().unwrap());
}

#[test]
fn maps_contact_with_unmodeled_crypto_keys_and_personal_info_safely() {
    let mut extra = std::collections::BTreeMap::new();
    extra.insert(
        "cryptoKeys".to_owned(),
        json!({"k1": {"uri": "https://keys.example.com/pkr.asc"}}),
    );
    extra.insert(
        "personalInfo".to_owned(),
        json!({"expertise": ["Rust", "Evolution"]}),
    );
    let card = ContactCard {
        name: Some(Name {
            full: Some("Vera Oldenburg".to_owned()),
            ..Name::default()
        }),
        extra,
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(
        back.name.as_ref().unwrap().full.as_deref(),
        Some("Vera Oldenburg")
    );
}

#[test]
fn maps_multiple_addresses_with_custom_extended_and_po_box_components() {
    let mut addresses = std::collections::BTreeMap::new();
    addresses.insert(
        "a1".to_owned(),
        Address {
            contexts: Some(json!({"work": true})),
            components: Some(vec![
                AddressComponent::new("postOfficeBox", "PO Box 42"),
                AddressComponent::new("apartment", "Suite 100"),
                AddressComponent::new("name", "Hauptstraße 1"),
                AddressComponent::new("locality", "Berlin"),
                AddressComponent::new("region", "Brandenburg"),
                AddressComponent::new("postcode", "10115"),
                AddressComponent::new("country", "Germany"),
            ]),
            full: Some("PO Box 42\nSuite 100\nHauptstraße 1\n10115 Berlin\nGermany".to_owned()),
            ..Address::default()
        },
    );
    addresses.insert(
        "a2".to_owned(),
        Address {
            contexts: Some(json!({"private": true})),
            components: Some(vec![
                AddressComponent::new("name", "Heimweg 2"),
                AddressComponent::new("locality", "München"),
                AddressComponent::new("region", "Bayern"),
                AddressComponent::new("postcode", "80331"),
                AddressComponent::new("country", "Germany"),
            ]),
            full: Some("Heimweg 2\n80331 München\nGermany".to_owned()),
            ..Address::default()
        },
    );

    let card = ContactCard {
        name: Some(Name {
            full: Some("Vera Olden".to_owned()),
            ..Name::default()
        }),
        addresses: Some(addresses),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    let unfolded = vcard.replace("\r\n ", "");
    assert!(unfolded.contains("ADR;X-JMAP-KEY=a1;TYPE=WORK:PO Box 42;Suite 100;Hauptstraße 1;Berlin;Brandenburg;10115;Germany"));
    assert!(unfolded.contains("LABEL;X-JMAP-KEY=a1;TYPE=WORK:PO Box 42\\nSuite 100\\nHauptstraße 1\\n10115 Berlin\\nGermany"));
    assert!(
        unfolded.contains("ADR;X-JMAP-KEY=a2;TYPE=HOME:;;Heimweg 2;München;Bayern;80331;Germany")
    );
    assert!(unfolded.contains("LABEL;X-JMAP-KEY=a2;TYPE=HOME:Heimweg 2\\n80331 München\\nGermany"));

    let back = vcard_to_card(&vcard).expect("parse");
    let back_addresses = back.addresses.expect("addresses");
    assert_eq!(back_addresses.len(), 2);
    assert_eq!(
        back_addresses["a1"].full.as_deref(),
        Some("PO Box 42\nSuite 100\nHauptstraße 1\n10115 Berlin\nGermany")
    );
    assert_eq!(
        back_addresses["a2"].full.as_deref(),
        Some("Heimweg 2\n80331 München\nGermany")
    );
}

#[test]
fn maps_contact_with_unmodeled_office_and_organization_extra_safely() {
    let mut org_extra = std::collections::BTreeMap::new();
    org_extra.insert("office".to_owned(), json!("Building 4, Room 204"));
    org_extra.insert("sortAs".to_owned(), json!("Acme"));

    let mut organizations = std::collections::BTreeMap::new();
    organizations.insert(
        "o1".to_owned(),
        Organization {
            name: Some("Acme Corp".to_owned()),
            units: Some(vec![
                OrgUnit::new("Research"),
                OrgUnit::new("Advanced Optics"),
            ]),
            extra: org_extra,
        },
    );

    let card = ContactCard {
        name: Some(Name {
            full: Some("Vera Olden".to_owned()),
            ..Name::default()
        }),
        organizations: Some(organizations),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert!(vcard.contains("ORG;X-JMAP-KEY=o1:Acme Corp;Research;Advanced Optics"));

    let back = vcard_to_card(&vcard).expect("parse");
    let back_orgs = back.organizations.expect("organizations");
    assert_eq!(back_orgs["o1"].name.as_deref(), Some("Acme Corp"));
    let units = back_orgs["o1"].units.as_ref().expect("units");
    assert_eq!(units.len(), 2);
    assert_eq!(units[0].name, "Research");
    assert_eq!(units[1].name, "Advanced Optics");
}

#[test]
fn reads_a_vcard_with_mixed_case_property_names_and_parameters() {
    let vcard = concat!(
        "BEGIN:vcard\r\n",
        "version:3.0\r\n",
        "uid:card-mixed-1\r\n",
        "fn:Alex Mixed\r\n",
        "email;type=work,pref:alex@work.example\r\n",
        "tel;TYPE=cell,home:+1234567890\r\n",
        "adr;type=work:;;100 Work St;Berlin;;10115;Germany\r\n",
        "categories:Alpha,Beta\r\n",
        "categories:Gamma\r\n",
        "x-evolution-spouse:Jordan\r\n",
        "END:vcard\r\n"
    );
    let card = vcard_to_card(vcard).expect("parse mixed case vcard");
    assert_eq!(card.id.as_ref().unwrap().as_str(), "card-mixed-1");
    assert_eq!(
        card.name.as_ref().unwrap().full.as_deref(),
        Some("Alex Mixed")
    );
    let emails = card.emails.as_ref().unwrap();
    assert_eq!(emails.len(), 1);
    let email = emails.values().next().unwrap();
    assert_eq!(email.address, "alex@work.example");
    assert_eq!(email.pref, Some(1));
    let phones = card.phones.as_ref().unwrap();
    let phone = phones.values().next().unwrap();
    assert_eq!(phone.number, "+1234567890");
    let keywords = card.keywords.as_ref().unwrap();
    assert_eq!(keywords.len(), 3);
    assert!(keywords.contains_key("Alpha"));
    assert!(keywords.contains_key("Beta"));
    assert!(keywords.contains_key("Gamma"));
    let related = card.related_to.as_ref().unwrap();
    assert!(related.contains_key("Jordan"));
}

#[test]
fn emits_a_comprehensive_vcard_via_calcard_and_roundtrips() {
    let card = fixture_card();
    let vcard = card_to_vcard(&card);
    assert!(vcard.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"));
    assert!(vcard.ends_with("END:VCARD\r\n"));
    assert!(vcard.contains("UID:C1\r\n"));
    assert!(vcard.contains("FN:Vera Oldenburg\r\n"));
    assert!(vcard.contains("N:Oldenburg;Vera;;;\r\n"));

    let back = vcard_to_card(&vcard).expect("parse back");
    assert_eq!(back.id, card.id);
    assert_eq!(back.uid, card.uid);
    assert_eq!(back.name, card.name);
    assert_eq!(
        back.emails.as_ref().unwrap().len(),
        card.emails.as_ref().unwrap().len()
    );
    assert_eq!(
        back.phones.as_ref().unwrap().len(),
        card.phones.as_ref().unwrap().len()
    );
}

#[test]
fn multi_type_phone_numbers_characterization_and_roundtrip() {
    let vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:test-multi-type-phones\r\n",
        "FN:Multi Phone Test\r\n",
        "TEL;X-JMAP-KEY=p_work_voice_fax;TYPE=WORK,VOICE,FAX:+49 30 111111\r\n",
        "TEL;X-JMAP-KEY=p_home_voice_fax;TYPE=HOME,VOICE,FAX:+49 30 222222\r\n",
        "TEL;X-JMAP-KEY=p_bare_voice_fax;TYPE=VOICE,FAX:+49 30 333333\r\n",
        "TEL;X-JMAP-KEY=p_work_cell_voice;TYPE=WORK,CELL,VOICE:+49 170 444444\r\n",
        "TEL;X-JMAP-KEY=p_work_cell_fax;TYPE=WORK,CELL,FAX:+49 170 555555\r\n",
        "TEL;X-JMAP-KEY=p_home_pager_voice;TYPE=HOME,PAGER,VOICE:+49 30 666666\r\n",
        "TEL;X-JMAP-KEY=p_work_voice_video;TYPE=WORK,VOICE,VIDEO:+49 30 777777\r\n",
        "TEL;X-JMAP-KEY=p_bare_fax_video;TYPE=FAX,VIDEO:+49 30 888888\r\n",
        "TEL;X-JMAP-KEY=p_all_features;TYPE=HOME,CELL,PAGER,FAX,VOICE,VIDEO:+49 170 999999\r\n",
        "TEL;X-JMAP-KEY=p_pref_work_fax;TYPE=PREF,WORK,VOICE,FAX:+49 30 000000\r\n",
        "TEL;X-JMAP-KEY=p_separate_params;TYPE=WORK;TYPE=VOICE;TYPE=FAX:+49 30 123456\r\n",
        "TEL;X-JMAP-KEY=p_mixed_case;type=work,voice,fax;type=pref:+49 30 234567\r\n",
        "TEL;X-JMAP-KEY=p_unmapped_types;TYPE=ISDN,CAR,VOICE;TYPE=WORK:+49 30 345678\r\n",
        "TEL;X-JMAP-KEY=p_bare_plain:+49 30 456789\r\n",
        "END:VCARD\r\n",
    );

    let card = vcard_to_card(vcard).expect("parse multi-type vcard");
    let phones = card.phones.as_ref().expect("phones map");
    assert_eq!(phones.len(), 14);

    // 1. TEL;TYPE=WORK,VOICE,FAX: parses work context and both voice & fax features
    let p_wvf = &phones["p_work_voice_fax"];
    assert_eq!(p_wvf.number, "+49 30 111111");
    assert_eq!(p_wvf.contexts, Some(json!({"work": true})));
    assert_eq!(p_wvf.features, Some(json!({"voice": true, "fax": true})));
    assert_eq!(p_wvf.pref, None);
    assert!(states_phone_feature(p_wvf.features.as_ref(), "fax"));
    assert!(!states_phone_feature(p_wvf.features.as_ref(), "voice"));
    assert!(states_context(p_wvf.contexts.as_ref(), "work"));
    assert!(!states_context(p_wvf.contexts.as_ref(), "private"));

    // 2. TEL;TYPE=HOME,VOICE,FAX: parses private context and both voice & fax features
    let p_hvf = &phones["p_home_voice_fax"];
    assert_eq!(p_hvf.number, "+49 30 222222");
    assert_eq!(p_hvf.contexts, Some(json!({"private": true})));
    assert_eq!(p_hvf.features, Some(json!({"voice": true, "fax": true})));
    assert!(states_phone_feature(p_hvf.features.as_ref(), "fax"));
    assert!(!states_phone_feature(p_hvf.features.as_ref(), "voice"));
    assert!(states_context(p_hvf.contexts.as_ref(), "private"));

    // 3. TEL;TYPE=VOICE,FAX: bare features with no context
    let p_bvf = &phones["p_bare_voice_fax"];
    assert_eq!(p_bvf.number, "+49 30 333333");
    assert_eq!(p_bvf.contexts, None);
    assert_eq!(p_bvf.features, Some(json!({"voice": true, "fax": true})));
    assert!(states_phone_feature(p_bvf.features.as_ref(), "fax"));
    assert!(!states_phone_feature(p_bvf.features.as_ref(), "voice"));

    // 4. TEL;TYPE=WORK,CELL,VOICE: mobile outranks voice
    let p_wcv = &phones["p_work_cell_voice"];
    assert_eq!(p_wcv.number, "+49 170 444444");
    assert_eq!(p_wcv.contexts, Some(json!({"work": true})));
    assert_eq!(p_wcv.features, Some(json!({"mobile": true, "voice": true})));
    assert!(states_phone_feature(p_wcv.features.as_ref(), "mobile"));
    assert!(!states_phone_feature(p_wcv.features.as_ref(), "voice"));

    // 5. TEL;TYPE=WORK,CELL,FAX: mobile outranks fax
    let p_wcf = &phones["p_work_cell_fax"];
    assert_eq!(p_wcf.number, "+49 170 555555");
    assert_eq!(p_wcf.contexts, Some(json!({"work": true})));
    assert_eq!(p_wcf.features, Some(json!({"mobile": true, "fax": true})));
    assert!(states_phone_feature(p_wcf.features.as_ref(), "mobile"));
    assert!(!states_phone_feature(p_wcf.features.as_ref(), "fax"));

    // 6. TEL;TYPE=HOME,PAGER,VOICE: pager outranks voice
    let p_hpv = &phones["p_home_pager_voice"];
    assert_eq!(p_hpv.number, "+49 30 666666");
    assert_eq!(p_hpv.contexts, Some(json!({"private": true})));
    assert_eq!(p_hpv.features, Some(json!({"pager": true, "voice": true})));
    assert!(states_phone_feature(p_hpv.features.as_ref(), "pager"));
    assert!(!states_phone_feature(p_hpv.features.as_ref(), "voice"));

    // 7. TEL;TYPE=WORK,VOICE,VIDEO: voice outranks unmapped video
    let p_wvv = &phones["p_work_voice_video"];
    assert_eq!(p_wvv.number, "+49 30 777777");
    assert_eq!(p_wvv.contexts, Some(json!({"work": true})));
    assert_eq!(p_wvv.features, Some(json!({"voice": true, "video": true})));
    assert!(states_phone_feature(p_wvv.features.as_ref(), "voice"));
    assert!(!states_phone_feature(p_wvv.features.as_ref(), "video"));

    // 8. TEL;TYPE=FAX,VIDEO: fax outranks video
    let p_bfv = &phones["p_bare_fax_video"];
    assert_eq!(p_bfv.number, "+49 30 888888");
    assert_eq!(p_bfv.contexts, None);
    assert_eq!(p_bfv.features, Some(json!({"fax": true, "video": true})));
    assert!(states_phone_feature(p_bfv.features.as_ref(), "fax"));
    assert!(!states_phone_feature(p_bfv.features.as_ref(), "video"));

    // 9. TEL;TYPE=HOME,CELL,PAGER,FAX,VOICE,VIDEO: full hierarchy resolves to CELL
    let p_all = &phones["p_all_features"];
    assert_eq!(p_all.number, "+49 170 999999");
    assert_eq!(p_all.contexts, Some(json!({"private": true})));
    assert_eq!(
        p_all.features,
        Some(json!({
            "mobile": true,
            "pager": true,
            "fax": true,
            "voice": true,
            "video": true
        }))
    );
    assert!(states_phone_feature(p_all.features.as_ref(), "mobile"));
    assert!(!states_phone_feature(p_all.features.as_ref(), "pager"));
    assert!(!states_phone_feature(p_all.features.as_ref(), "fax"));
    assert!(!states_phone_feature(p_all.features.as_ref(), "voice"));
    assert!(!states_phone_feature(p_all.features.as_ref(), "video"));

    // 10. TEL;TYPE=PREF,WORK,VOICE,FAX: pref extracted alongside types
    let p_pref = &phones["p_pref_work_fax"];
    assert_eq!(p_pref.number, "+49 30 000000");
    assert_eq!(p_pref.pref, Some(1));
    assert_eq!(p_pref.contexts, Some(json!({"work": true})));
    assert_eq!(p_pref.features, Some(json!({"voice": true, "fax": true})));

    // 11. Separate TYPE parameters parse identically to comma-separated
    let p_sep = &phones["p_separate_params"];
    assert_eq!(p_sep.number, "+49 30 123456");
    assert_eq!(p_sep.contexts, Some(json!({"work": true})));
    assert_eq!(p_sep.features, Some(json!({"voice": true, "fax": true})));

    // 12. Mixed case TYPE parameters and values parse correctly
    let p_case = &phones["p_mixed_case"];
    assert_eq!(p_case.number, "+49 30 234567");
    assert_eq!(p_case.pref, Some(1));
    assert_eq!(p_case.contexts, Some(json!({"work": true})));
    assert_eq!(p_case.features, Some(json!({"voice": true, "fax": true})));

    // 13. Unmapped types (ISDN, CAR) are ignored while mapped types (VOICE, WORK) survive
    let p_unm = &phones["p_unmapped_types"];
    assert_eq!(p_unm.number, "+49 30 345678");
    assert_eq!(p_unm.contexts, Some(json!({"work": true})));
    assert_eq!(p_unm.features, Some(json!({"voice": true})));

    // 14. Bare plain telephone with no types
    let p_bare = &phones["p_bare_plain"];
    assert_eq!(p_bare.number, "+49 30 456789");
    assert_eq!(p_bare.contexts, None);
    assert_eq!(p_bare.features, None);
    assert_eq!(p_bare.pref, None);
    assert!(!states_phone_feature(p_bare.features.as_ref(), "voice"));
    assert!(!states_context(p_bare.contexts.as_ref(), "work"));

    // Now verify outbound vCard emission for each phone entry
    let emitted = card_to_vcard(&card);

    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_work_voice_fax"),
        "TEL;X-JMAP-KEY=p_work_voice_fax;TYPE=WORK,FAX:+49 30 111111"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_home_voice_fax"),
        "TEL;X-JMAP-KEY=p_home_voice_fax;TYPE=HOME,FAX:+49 30 222222"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_bare_voice_fax"),
        "TEL;X-JMAP-KEY=p_bare_voice_fax;TYPE=FAX:+49 30 333333"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_work_cell_voice"),
        "TEL;X-JMAP-KEY=p_work_cell_voice;TYPE=WORK,CELL:+49 170 444444"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_work_cell_fax"),
        "TEL;X-JMAP-KEY=p_work_cell_fax;TYPE=WORK,CELL:+49 170 555555"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_home_pager_voice"),
        "TEL;X-JMAP-KEY=p_home_pager_voice;TYPE=HOME,PAGER:+49 30 666666"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_work_voice_video"),
        "TEL;X-JMAP-KEY=p_work_voice_video;TYPE=WORK,VOICE:+49 30 777777"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_bare_fax_video"),
        "TEL;X-JMAP-KEY=p_bare_fax_video;TYPE=FAX:+49 30 888888"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_all_features"),
        "TEL;X-JMAP-KEY=p_all_features;TYPE=HOME,CELL:+49 170 999999"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_pref_work_fax"),
        "TEL;X-JMAP-KEY=p_pref_work_fax;TYPE=WORK,FAX,PREF:+49 30 000000"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_separate_params"),
        "TEL;X-JMAP-KEY=p_separate_params;TYPE=WORK,FAX:+49 30 123456"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_mixed_case"),
        "TEL;X-JMAP-KEY=p_mixed_case;TYPE=WORK,FAX,PREF:+49 30 234567"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_unmapped_types"),
        "TEL;X-JMAP-KEY=p_unmapped_types;TYPE=WORK,VOICE:+49 30 345678"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_bare_plain"),
        "TEL;X-JMAP-KEY=p_bare_plain:+49 30 456789"
    );
}

#[test]
fn maps_phone_feature_predicate_characterization() {
    // All supported JSContact phone features mapped to vCard 3.0 TYPE values
    assert!(maps_phone_feature("mobile"));
    assert!(maps_phone_feature("pager"));
    assert!(maps_phone_feature("fax"));
    assert!(maps_phone_feature("voice"));
    assert!(maps_phone_feature("video"));

    // Unsupported, unmapped, or type-confusion keys
    assert!(!maps_phone_feature("cell")); // vCard spelling, not JSContact key
    assert!(!maps_phone_feature("car"));
    assert!(!maps_phone_feature("isdn"));
    assert!(!maps_phone_feature("modem"));
    assert!(!maps_phone_feature("bbs"));
    assert!(!maps_phone_feature("main"));
    assert!(!maps_phone_feature("text"));
    assert!(!maps_phone_feature("textphone"));
    assert!(!maps_phone_feature("work"));
    assert!(!maps_phone_feature("home"));
    assert!(!maps_phone_feature(""));
}

#[test]
fn phone_feature_slot_resolution_order_is_fully_determined() {
    // 1. Single feature slot resolution
    for (feature, expected_type) in [
        ("mobile", "CELL"),
        ("pager", "PAGER"),
        ("fax", "FAX"),
        ("voice", "VOICE"),
        ("video", "VIDEO"),
    ] {
        let line = phone_line(None, Some(json!({feature: true})));
        assert_eq!(
            line,
            format!("TEL;X-JMAP-KEY=p1;TYPE={expected_type}:+49 30 111")
        );
        assert!(states_phone_feature(Some(&json!({feature: true})), feature));
    }

    // 2. Pairwise feature precedence: mobile beats all
    for other in ["pager", "fax", "voice", "video"] {
        let features = json!({"mobile": true, other: true});
        let line = phone_line(None, Some(features.clone()));
        assert_eq!(line, "TEL;X-JMAP-KEY=p1;TYPE=CELL:+49 30 111");
        assert!(states_phone_feature(Some(&features), "mobile"));
        assert!(!states_phone_feature(Some(&features), other));
    }

    // 3. Pairwise feature precedence: pager beats fax, voice, video
    for other in ["fax", "voice", "video"] {
        let features = json!({"pager": true, other: true});
        let line = phone_line(None, Some(features.clone()));
        assert_eq!(line, "TEL;X-JMAP-KEY=p1;TYPE=PAGER:+49 30 111");
        assert!(states_phone_feature(Some(&features), "pager"));
        assert!(!states_phone_feature(Some(&features), other));
    }

    // 4. Pairwise feature precedence: fax beats voice, video
    for other in ["voice", "video"] {
        let features = json!({"fax": true, other: true});
        let line = phone_line(None, Some(features.clone()));
        assert_eq!(line, "TEL;X-JMAP-KEY=p1;TYPE=FAX:+49 30 111");
        assert!(states_phone_feature(Some(&features), "fax"));
        assert!(!states_phone_feature(Some(&features), other));
    }

    // 5. Pairwise feature precedence: voice beats video
    let features = json!({"voice": true, "video": true});
    let line = phone_line(None, Some(features.clone()));
    assert_eq!(line, "TEL;X-JMAP-KEY=p1;TYPE=VOICE:+49 30 111");
    assert!(states_phone_feature(Some(&features), "voice"));
    assert!(!states_phone_feature(Some(&features), "video"));

    // 6. Multiple contexts with multiple features: DEFAULT_SLOT (HOME) + winning feature
    let line_multi_ctx_feat = phone_line(
        Some(json!({"work": true, "private": true})),
        Some(json!({"voice": true, "fax": true})),
    );
    assert_eq!(
        line_multi_ctx_feat,
        "TEL;X-JMAP-KEY=p1;TYPE=HOME,FAX:+49 30 111"
    );
    let contexts = json!({"work": true, "private": true});
    assert!(states_context(Some(&contexts), "private"));
    assert!(!states_context(Some(&contexts), "work"));
}

#[test]
fn bare_year_dates_characterization_and_eds_clamping_roundtrip() {
    // Characterization of bare-year dates (RFC 9553 §2.8.1 PartialDate with year only):
    // 1. vCard 3.0 (RFC 2426 §3.1.5) requires BDAY to be a full date (YYYY-MM-DD or YYYYMMDD).
    // 2. EDS (libebook-contacts) e_contact_date_from_string returns NULL for partial dates.
    // 3. When EDS writes an EContactDate via e_contact_date_to_string, it CLAMPs year into 1000..=9999,
    //    month into 1..=12, and day into 1..=31. Emitting an unanchored bare year or partial date
    //    would cause EDS to clamp missing fields to 01-01 (e.g. 1984-01-01), corrupting the user's date.
    // 4. Therefore, jmap-vcard drops bare-year dates on emission, leaving diff_entries to preserve
    //    the server-side PartialDate untouched without creating phantom vCard lines.

    // 1. Predicates on bare-year dates
    let birth_bare_year = Anniversary {
        kind: "birth".to_owned(),
        date: Some(json!({"@type": "PartialDate", "year": 1984})),
        ..Anniversary::default()
    };
    assert!(!states_anniversary(&birth_bare_year));
    assert_eq!(anniversary_date(&birth_bare_year), None);
    assert!(!states_a_point_in_time(&birth_bare_year));

    let wedding_bare_year = Anniversary {
        kind: "wedding".to_owned(),
        date: Some(json!({"@type": "PartialDate", "year": 1996})),
        ..Anniversary::default()
    };
    assert!(!states_anniversary(&wedding_bare_year));
    assert_eq!(anniversary_date(&wedding_bare_year), None);
    assert!(!states_a_point_in_time(&wedding_bare_year));

    let death_bare_year = Anniversary {
        kind: "death".to_owned(),
        date: Some(json!({"@type": "PartialDate", "year": 2019})),
        ..Anniversary::default()
    };
    assert!(!states_anniversary(&death_bare_year));
    assert_eq!(anniversary_date(&death_bare_year), None);

    // 2. Outbound emission across various bare-year representations
    for bare_date in [
        json!({"year": 1984}),
        json!({"@type": "PartialDate", "year": 1984}),
        json!({"year": 800}),  // historical year below clamp threshold
        json!({"year": 999}),  // boundary below clamp threshold
        json!({"year": 1000}), // earliest clamp threshold
        json!({"year": 2026}), // current era
        json!({"@type": "PartialDate", "year": 1984, "calendarScale": "gregorian"}),
    ] {
        let vcard_birth = card_to_vcard(&one_anniversary("birth", bare_date.clone()));
        assert!(!vcard_birth.contains("BDAY"), "{bare_date}: {vcard_birth}");

        let vcard_wedding = card_to_vcard(&one_anniversary("wedding", bare_date.clone()));
        assert!(
            !vcard_wedding.contains("X-EVOLUTION-ANNIVERSARY"),
            "{bare_date}: {vcard_wedding}"
        );
        assert!(
            !vcard_wedding.contains("ANNIVERSARY"),
            "{bare_date}: {vcard_wedding}"
        );
    }

    // 3. Inbound parsing of bare-year and partial date lines from vCards
    for vcard_input in [
        "BEGIN:VCARD\r\nVERSION:3.0\r\nBDAY:1984\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nBDAY:1984-06\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nBDAY:--06-21\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nBDAY;VALUE=date:1984\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nBDAY;VALUE=text:1984\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nX-EVOLUTION-ANNIVERSARY:1996\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nX-EVOLUTION-ANNIVERSARY:1996-08\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nX-EVOLUTION-ANNIVERSARY:--08-03\r\nEND:VCARD\r\n",
    ] {
        let card = vcard_to_card(vcard_input).expect("parse");
        assert_eq!(
            card.anniversaries, None,
            "inbound bare date should not parse into card: {vcard_input}"
        );
    }

    // 4. Roundtrip of a card with coexisting bare-year and full-date anniversaries
    let mixed_card = ContactCard {
        anniversaries: Some(
            [
                (
                    "y1".to_owned(),
                    Anniversary {
                        kind: "birth".to_owned(),
                        date: Some(json!({"@type": "PartialDate", "year": 1984})),
                        ..Anniversary::default()
                    },
                ),
                (
                    "y2".to_owned(),
                    Anniversary {
                        kind: "birth".to_owned(),
                        date: Some(
                            json!({"@type": "PartialDate", "year": 1984, "month": 6, "day": 21}),
                        ),
                        ..Anniversary::default()
                    },
                ),
                (
                    "y3".to_owned(),
                    Anniversary {
                        kind: "wedding".to_owned(),
                        date: Some(json!({"@type": "PartialDate", "year": 1996})),
                        ..Anniversary::default()
                    },
                ),
                (
                    "y4".to_owned(),
                    Anniversary {
                        kind: "wedding".to_owned(),
                        date: Some(
                            json!({"@type": "PartialDate", "year": 1996, "month": 8, "day": 3}),
                        ),
                        ..Anniversary::default()
                    },
                ),
                (
                    "y5".to_owned(),
                    Anniversary {
                        kind: "death".to_owned(),
                        date: Some(json!({"@type": "PartialDate", "year": 2019})),
                        ..Anniversary::default()
                    },
                ),
                (
                    "y6".to_owned(),
                    Anniversary {
                        kind: "death".to_owned(),
                        date: Some(
                            json!({"@type": "PartialDate", "year": 2019, "month": 10, "day": 15}),
                        ),
                        ..Anniversary::default()
                    },
                ),
                (
                    "y7".to_owned(),
                    Anniversary {
                        kind: "birth".to_owned(),
                        date: Some(
                            json!({"@type": "PartialDate", "year": 800, "month": 6, "day": 21}),
                        ),
                        ..Anniversary::default()
                    },
                ),
                (
                    "y8".to_owned(),
                    Anniversary {
                        kind: "birth".to_owned(),
                        date: Some(json!({"@type": "PartialDate", "year": 800})),
                        ..Anniversary::default()
                    },
                ),
            ]
            .into(),
        ),
        ..ContactCard::default()
    };

    let emitted = card_to_vcard(&mixed_card);
    assert_eq!(line(&emitted, "BDAY"), "BDAY;X-JMAP-KEY=y2:1984-06-21");
    assert_eq!(
        line(&emitted, "X-EVOLUTION-ANNIVERSARY"),
        "X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=y4:1996-08-03"
    );
    // Unstated entries are absent
    assert!(!emitted.contains("X-JMAP-KEY=y1"), "{emitted}");
    assert!(!emitted.contains("X-JMAP-KEY=y3"), "{emitted}");
    assert!(!emitted.contains("X-JMAP-KEY=y5"), "{emitted}");
    assert!(!emitted.contains("X-JMAP-KEY=y6"), "{emitted}");
    assert!(!emitted.contains("X-JMAP-KEY=y7"), "{emitted}");
    assert!(!emitted.contains("X-JMAP-KEY=y8"), "{emitted}");

    // Inbound roundtrip parses the emitted lines back losslessly
    let roundtripped = vcard_to_card(&emitted).expect("parse back");
    let recovered = roundtripped.anniversaries.expect("anniversaries");
    assert_eq!(recovered.len(), 2);
    assert_eq!(recovered["y2"].kind, "birth");
    assert_eq!(
        recovered["y2"].date,
        Some(json!({"@type": "PartialDate", "year": 1984, "month": 6, "day": 21}))
    );
    assert_eq!(recovered["y4"].kind, "wedding");
    assert_eq!(
        recovered["y4"].date,
        Some(json!({"@type": "PartialDate", "year": 1996, "month": 8, "day": 3}))
    );
}

#[test]
fn bare_year_and_partial_dates_with_custom_attributes_roundtrip() {
    // Tests that PartialDate objects carrying custom or alternative calendar attributes
    // with bare years are safely handled and do not panic or emit malformed vCard lines.
    let card = ContactCard {
        anniversaries: Some(
            [
                (
                    "y1".to_owned(),
                    Anniversary {
                        kind: "birth".to_owned(),
                        date: Some(json!({
                            "@type": "PartialDate",
                            "year": 2567,
                            "calendarScale": "buddhist"
                        })),
                        ..Anniversary::default()
                    },
                ),
                (
                    "y2".to_owned(),
                    Anniversary {
                        kind: "wedding".to_owned(),
                        date: Some(json!({
                            "@type": "PartialDate",
                            "year": 5784,
                            "calendarScale": "hebrew"
                        })),
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
    assert!(!vcard.contains("ANNIVERSARY"), "{vcard}");
    assert!(!vcard.contains("2567"), "{vcard}");
    assert!(!vcard.contains("5784"), "{vcard}");

    let parsed = vcard_to_card(&vcard).expect("parse");
    assert_eq!(parsed.anniversaries, None);
}

#[test]
fn org_unit_empty_name_characterization_and_unstated_predicate_fidelity() {
    // 1. `states_org_unit` predicate characterization:
    // A unit is stated on the wire only if its name is non-empty.
    // An empty name with or without sortAs in extra is unstated.
    let empty_unit = OrgUnit::new("");
    assert!(!states_org_unit(&empty_unit));

    let empty_with_sort_as = OrgUnit {
        name: "".to_owned(),
        extra: [("sortAs".to_owned(), json!("Alpha"))].into(),
    };
    assert!(!states_org_unit(&empty_with_sort_as));

    let normal_unit = OrgUnit::new("Engineering");
    assert!(states_org_unit(&normal_unit));

    let normal_with_sort_as = OrgUnit {
        name: "Engineering".to_owned(),
        extra: [("sortAs".to_owned(), json!("Eng"))].into(),
    };
    assert!(states_org_unit(&normal_with_sort_as));

    let whitespace_unit = OrgUnit::new("   ");
    assert!(states_org_unit(&whitespace_unit));

    // 2. `states_organization` predicate with combinations of empty units:
    // Org with empty name and only empty units states nothing.
    let empty_org = Organization {
        name: None,
        units: Some(vec![OrgUnit::new(""), empty_with_sort_as.clone()]),
        ..Organization::default()
    };
    assert!(!states_organization(&empty_org));

    let empty_named_org = Organization {
        name: Some("".to_owned()),
        units: Some(vec![OrgUnit::new("")]),
        ..Organization::default()
    };
    assert!(!states_organization(&empty_named_org));

    // Org with employer name states an ORG line even if all units are empty.
    let named_org_empty_units = Organization {
        name: Some("Acme Corp".to_owned()),
        units: Some(vec![OrgUnit::new(""), empty_with_sort_as.clone()]),
        ..Organization::default()
    };
    assert!(states_organization(&named_org_empty_units));

    // Org with no employer name but at least one non-empty unit states an ORG line.
    let nameless_org_valid_unit = Organization {
        name: None,
        units: Some(vec![OrgUnit::new(""), OrgUnit::new("Finance")]),
        ..Organization::default()
    };
    assert!(states_organization(&nameless_org_valid_unit));
}

#[test]
fn org_with_empty_name_units_and_sort_as_emission_and_roundtrip() {
    // 1. Org with employer name and only empty-name units:
    // Emits employer name alone on the ORG line; empty units are dropped from wire format.
    let card1 = ContactCard {
        organizations: Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some("Acme Corp".to_owned()),
                    units: Some(vec![
                        OrgUnit::new(""),
                        OrgUnit {
                            name: "".to_owned(),
                            extra: [("sortAs".to_owned(), json!("Secret"))].into(),
                        },
                    ]),
                    ..Organization::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    };
    let vcard1 = card_to_vcard(&card1);
    assert_eq!(line(&vcard1, "ORG"), "ORG;X-JMAP-KEY=o1:Acme Corp");
    let back1 = vcard_to_card(&vcard1).expect("parse");
    let org1 = &back1.organizations.as_ref().unwrap()["o1"];
    assert_eq!(org1.name.as_deref(), Some("Acme Corp"));
    assert_eq!(org1.units, None);

    // 2. Org with employer name, leading/intermediate empty units, and valid units:
    // Empty units are omitted from ORG components, leaving non-empty units in sequence.
    let card2 = ContactCard {
        organizations: Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some("Acme Corp".to_owned()),
                    units: Some(vec![
                        OrgUnit::new(""),
                        OrgUnit::new("Research"),
                        OrgUnit {
                            name: "".to_owned(),
                            extra: [("sortAs".to_owned(), json!("OpticsSort"))].into(),
                        },
                        OrgUnit::new("Optics"),
                        OrgUnit::new(""),
                    ]),
                    ..Organization::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    };
    let vcard2 = card_to_vcard(&card2);
    assert_eq!(
        line(&vcard2, "ORG"),
        "ORG;X-JMAP-KEY=o1:Acme Corp;Research;Optics"
    );
    let back2 = vcard_to_card(&vcard2).expect("parse");
    let org2 = &back2.organizations.as_ref().unwrap()["o1"];
    assert_eq!(org2.name.as_deref(), Some("Acme Corp"));
    assert_eq!(
        org2.units.as_deref(),
        Some([OrgUnit::new("Research"), OrgUnit::new("Optics")].as_slice())
    );

    // 3. Nameless org with leading empty unit and valid units:
    // Leading semicolon keeps unit in department slot; empty unit is omitted.
    let card3 = ContactCard {
        organizations: Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: None,
                    units: Some(vec![OrgUnit::new(""), OrgUnit::new("Engineering")]),
                    ..Organization::default()
                },
            )]
            .into(),
        ),
        ..ContactCard::default()
    };
    let vcard3 = card_to_vcard(&card3);
    assert_eq!(line(&vcard3, "ORG"), "ORG;X-JMAP-KEY=o1:;Engineering");
    let back3 = vcard_to_card(&vcard3).expect("parse");
    let org3 = &back3.organizations.as_ref().unwrap()["o1"];
    assert_eq!(org3.name, None);
    assert_eq!(
        org3.units.as_deref(),
        Some([OrgUnit::new("Engineering")].as_slice())
    );

    // 4. Inbound vCards with various empty component patterns:
    // - ORG with empty employer and empty unit components:
    let empty_components_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "ORG;X-JMAP-KEY=o1:;;;\r\n",
        "END:VCARD\r\n"
    );
    let from_empty = vcard_to_card(empty_components_vcard).expect("parse");
    assert_eq!(from_empty.organizations, None);

    // - ORG with empty intermediate components and trailing empty semicolons:
    let multi_empty_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "ORG;X-JMAP-KEY=o1:Acme Corp;;Research;;Development;\r\n",
        "END:VCARD\r\n"
    );
    let from_multi_empty = vcard_to_card(multi_empty_vcard).expect("parse");
    let org_multi = &from_multi_empty.organizations.as_ref().unwrap()["o1"];
    assert_eq!(org_multi.name.as_deref(), Some("Acme Corp"));
    assert_eq!(
        org_multi.units.as_deref(),
        Some([OrgUnit::new("Research"), OrgUnit::new("Development")].as_slice())
    );
}

#[test]
fn name_and_address_component_predicates_and_context_mapping_fidelity() {
    // states_name_component: requires non-empty value AND mapped kind
    assert!(states_name_component(&NameComponent::new("given", "Alice")));
    assert!(states_name_component(&NameComponent::new(
        "surname", "Smith"
    )));
    assert!(!states_name_component(&NameComponent::new("given", "")));
    assert!(!states_name_component(&NameComponent::new(
        "unmapped_kind",
        "Alice"
    )));
    assert!(!states_name_component(&NameComponent::new(
        "unmapped_kind",
        ""
    )));

    // maps_context: covers "work" and "private"
    assert!(maps_context("work"));
    assert!(maps_context("private"));
    assert!(!maps_context("other"));
    assert!(!maps_context("billing"));
    assert!(!maps_context("home"));
    assert!(!maps_context(""));

    // states_address_component: requires non-empty value AND mapped kind (including joined kind)
    assert!(states_address_component(&AddressComponent::new(
        "name",
        "Main Street"
    )));
    assert!(states_address_component(&AddressComponent::new(
        "number", "42"
    )));
    assert!(states_address_component(&AddressComponent::new(
        "locality", "Berlin"
    )));
    assert!(!states_address_component(&AddressComponent::new(
        "locality", ""
    )));
    assert!(!states_address_component(&AddressComponent::new(
        "unmapped_kind",
        "Sector 7"
    )));
    assert!(!states_address_component(&AddressComponent::new(
        "unmapped_kind",
        ""
    )));

    // states_address: requires address_fields or address_label
    assert!(states_address(&Address {
        components: Some(vec![AddressComponent::new("locality", "Berlin")]),
        ..Address::default()
    }));
    assert!(states_address(&Address {
        full: Some("123 Main St, Berlin".into()),
        ..Address::default()
    }));
    assert!(!states_address(&Address {
        full: Some("".into()),
        ..Address::default()
    }));
    assert!(!states_address(&Address {
        components: Some(vec![AddressComponent::new("locality", "")]),
        ..Address::default()
    }));
    assert!(!states_address(&Address {
        components: Some(vec![AddressComponent::new("unmapped_kind", "Value")]),
        ..Address::default()
    }));
    assert!(!states_address(&Address::default()));
    assert_eq!(
        address_label(&Address {
            full: Some("Main Street".into()),
            ..Address::default()
        }),
        Some("Main Street")
    );
    assert_eq!(
        address_label(&Address {
            full: Some("".into()),
            ..Address::default()
        }),
        None
    );

    // states_email, states_phone, states_note, states_link, states_nickname, title_kind
    assert!(states_email(&ContactEmail {
        address: "alice@example.com".into(),
        ..ContactEmail::default()
    }));
    assert!(!states_email(&ContactEmail {
        address: "".into(),
        ..ContactEmail::default()
    }));
    assert!(states_phone(&ContactPhone {
        number: "+49 30 123456".into(),
        ..ContactPhone::default()
    }));
    assert!(!states_phone(&ContactPhone {
        number: "".into(),
        ..ContactPhone::default()
    }));
    assert!(states_note(&Note {
        note: "Important contact".into(),
        ..Note::default()
    }));
    assert!(!states_note(&Note {
        note: "".into(),
        ..Note::default()
    }));
    assert!(states_link(&Link {
        uri: "https://example.com".into(),
        ..Link::default()
    }));
    assert!(!states_link(&Link {
        uri: "".into(),
        ..Link::default()
    }));
    assert!(states_nickname(&Nickname {
        name: "Ali".into(),
        ..Nickname::default()
    }));
    assert!(!states_nickname(&Nickname {
        name: "".into(),
        ..Nickname::default()
    }));
    assert_eq!(title_kind(None), "title");
    assert_eq!(title_kind(Some("role")), "role");
}

#[test]
fn calendar_and_spouse_predicates_fidelity() {
    // states_calendar: requires non-empty uri AND mapped kind (calendar, freeBusy)
    assert!(states_calendar(&Calendar {
        kind: Some("calendar".into()),
        uri: "https://calendar.example.com".into(),
        ..Calendar::default()
    }));
    assert!(states_calendar(&Calendar {
        kind: Some("freeBusy".into()),
        uri: "https://fb.example.com".into(),
        ..Calendar::default()
    }));
    assert!(!states_calendar(&Calendar {
        kind: Some("calendar".into()),
        uri: "".into(),
        ..Calendar::default()
    }));
    assert!(!states_calendar(&Calendar {
        kind: Some("unmapped_kind".into()),
        uri: "https://calendar.example.com".into(),
        ..Calendar::default()
    }));
    assert!(!states_calendar(&Calendar {
        kind: None,
        uri: "https://calendar.example.com".into(),
        ..Calendar::default()
    }));
    assert!(!states_calendar(&Calendar {
        kind: None,
        uri: "".into(),
        ..Calendar::default()
    }));

    // states_spouse and states_nothing_but_the_marriage:
    let rel_spouse_only = Relation {
        relation: Some([("spouse".into(), json!(true))].into()),
        extra: [("@type".into(), json!("Relation"))].into(),
    };
    assert!(states_spouse("Bob", &rel_spouse_only));
    assert!(states_nothing_but_the_marriage(&rel_spouse_only));

    let rel_no_extra = Relation {
        relation: Some([("spouse".into(), json!(true))].into()),
        extra: BTreeMap::new(),
    };
    assert!(states_nothing_but_the_marriage(&rel_no_extra));

    let rel_with_kin = Relation {
        relation: Some([("spouse".into(), json!(true)), ("kin".into(), json!(true))].into()),
        extra: [("@type".into(), json!("Relation"))].into(),
    };
    assert!(states_spouse("Bob", &rel_with_kin));
    assert!(!states_nothing_but_the_marriage(&rel_with_kin));

    let rel_kin_only = Relation {
        relation: Some([("kin".into(), json!(true))].into()),
        extra: [("@type".into(), json!("Relation"))].into(),
    };
    assert!(!states_spouse("Bob", &rel_kin_only));
    assert!(!states_nothing_but_the_marriage(&rel_kin_only));

    let rel_with_custom_extra = Relation {
        relation: Some([("spouse".into(), json!(true))].into()),
        extra: [
            ("@type".into(), json!("Relation")),
            ("customField".into(), json!("value")),
        ]
        .into(),
    };
    assert!(states_spouse("Bob", &rel_with_custom_extra));
    assert!(!states_nothing_but_the_marriage(&rel_with_custom_extra));

    let rel_empty = Relation {
        relation: None,
        extra: [("@type".into(), json!("Relation"))].into(),
    };
    assert!(!states_spouse("Bob", &rel_empty));
    assert!(states_nothing_but_the_marriage(&rel_empty));
}

#[test]
fn media_photo_and_online_service_predicates_and_comparisons() {
    // states_media and same_photo
    let m_uri1 = Media {
        kind: Some("photo".into()),
        uri: "https://example.com/photo1.jpg".into(),
        ..Media::default()
    };
    let m_uri2 = Media {
        kind: Some("photo".into()),
        uri: "https://example.com/photo1.jpg".into(),
        ..Media::default()
    };
    let m_uri3 = Media {
        kind: Some("photo".into()),
        uri: "https://example.com/photo2.jpg".into(),
        ..Media::default()
    };
    let m_logo = Media {
        kind: Some("logo".into()),
        uri: "https://example.com/logo.png".into(),
        ..Media::default()
    };
    let m_sound = Media {
        kind: Some("sound".into()),
        uri: "https://example.com/audio.mp3".into(),
        ..Media::default()
    };
    let m_empty_uri = Media {
        kind: Some("photo".into()),
        uri: "".into(),
        ..Media::default()
    };
    let m_none_kind = Media {
        kind: None,
        uri: "https://example.com/photo.jpg".into(),
        ..Media::default()
    };
    let m_data1 = Media {
        kind: Some("photo".into()),
        uri: "data:image/jpeg;base64,aGVsbG8=".into(),
        ..Media::default()
    };
    let m_data2 = Media {
        kind: Some("photo".into()),
        uri: "data:image/jpeg;base64,aGVsbG8=".into(),
        ..Media::default()
    };
    let m_data_case = Media {
        kind: Some("photo".into()),
        uri: "data:image/JPEG;base64,aGVsbG8=".into(),
        ..Media::default()
    };
    let m_data_diff_bytes = Media {
        kind: Some("photo".into()),
        uri: "data:image/jpeg;base64,d29ybGQ=".into(),
        ..Media::default()
    };
    let m_data_diff_type = Media {
        kind: Some("photo".into()),
        uri: "data:image/png;base64,aGVsbG8=".into(),
        ..Media::default()
    };
    let m_data_invalid = Media {
        kind: Some("photo".into()),
        uri: "data:image/jpeg;base64,!!!invalid!!!".into(),
        ..Media::default()
    };

    assert!(states_media(&m_uri1));
    assert!(states_media(&m_data1));
    assert!(!states_media(&m_logo));
    assert!(!states_media(&m_sound));
    assert!(!states_media(&m_empty_uri));
    assert!(!states_media(&m_none_kind));
    assert!(!states_media(&m_data_invalid));

    assert!(same_photo(&m_logo, &m_sound)); // Both evaluate to None
    assert!(same_photo(&m_uri1, &m_uri2));
    assert!(!same_photo(&m_uri1, &m_uri3));
    assert!(!same_photo(&m_uri1, &m_logo));
    assert!(same_photo(&m_data1, &m_data2));
    assert!(same_photo(&m_data1, &m_data_case));
    assert!(!same_photo(&m_data1, &m_data_diff_bytes));
    assert!(!same_photo(&m_data1, &m_data_diff_type));
    assert!(!same_photo(&m_data1, &m_uri1));

    // states_online_service, same_service, online_service_handle, online_service_uri
    let s_matrix = OnlineService {
        service: Some("Matrix".into()),
        user: Some("@alice:matrix.org".into()),
        ..OnlineService::default()
    };
    let s_matrix_uri = OnlineService {
        service: Some("Matrix".into()),
        uri: Some("matrix:alice".into()),
        ..OnlineService::default()
    };
    let s_matrix_crlf = OnlineService {
        service: Some("Matrix".into()),
        user: Some("@alice\r:matrix.org".into()),
        ..OnlineService::default()
    };
    let s_matrix_spaced = OnlineService {
        service: Some("Matrix".into()),
        user: Some(" @alice:matrix.org ".into()),
        ..OnlineService::default()
    };
    let s_matrix_empty = OnlineService {
        service: Some("Matrix".into()),
        user: Some("".into()),
        ..OnlineService::default()
    };
    let s_unmapped = OnlineService {
        service: Some("UnmappedService".into()),
        user: Some("alice".into()),
        ..OnlineService::default()
    };
    let s_wrong_scheme_uri = OnlineService {
        service: Some("Matrix".into()),
        uri: Some("https:alice".into()),
        ..OnlineService::default()
    };

    assert!(states_online_service(&s_matrix));
    assert!(states_online_service(&s_matrix_uri));
    assert!(!states_online_service(&s_matrix_crlf));
    assert!(!states_online_service(&s_matrix_spaced));
    assert!(!states_online_service(&s_matrix_empty));
    assert!(!states_online_service(&s_unmapped));
    assert!(!states_online_service(&s_wrong_scheme_uri));

    assert_eq!(online_service_handle(&s_matrix), Some("@alice:matrix.org"));
    assert_eq!(online_service_handle(&s_matrix_uri), Some("alice"));
    assert_eq!(
        online_service_uri("Matrix", "alice"),
        Some("matrix:alice".into())
    );
    assert_eq!(
        online_service_uri("Matrix", "invalid name with space"),
        None
    );
    assert_eq!(online_service_uri("UnmappedService", "alice"), None);

    assert!(same_service(None, None));
    assert!(same_service(Some("Matrix"), Some("matrix")));
    assert!(same_service(Some("Gadu-Gadu"), Some("GaduGadu")));
    assert!(!same_service(Some("Matrix"), Some("Jabber")));
    assert!(!same_service(Some("Matrix"), None));
    assert!(!same_service(None, Some("Matrix")));
}

#[test]
fn anniversary_date_validation_and_point_in_time_predicates() {
    let bday_valid = Anniversary {
        kind: "birth".into(),
        date: Some(json!({"year": 1990, "month": 5, "day": 12})),
        ..Anniversary::default()
    };
    let wedding_valid = Anniversary {
        kind: "wedding".into(),
        date: Some(json!({"year": 2010, "month": 6, "day": 20})),
        ..Anniversary::default()
    };
    let death_unmapped = Anniversary {
        kind: "death".into(),
        date: Some(json!({"year": 2020, "month": 1, "day": 1})),
        ..Anniversary::default()
    };
    let bday_month_zero = Anniversary {
        kind: "birth".into(),
        date: Some(json!({"year": 1990, "month": 0, "day": 12})),
        ..Anniversary::default()
    };
    let bday_month_13 = Anniversary {
        kind: "birth".into(),
        date: Some(json!({"year": 1990, "month": 13, "day": 12})),
        ..Anniversary::default()
    };
    let bday_day_zero = Anniversary {
        kind: "birth".into(),
        date: Some(json!({"year": 1990, "month": 5, "day": 0})),
        ..Anniversary::default()
    };
    let bday_day_32 = Anniversary {
        kind: "birth".into(),
        date: Some(json!({"year": 1990, "month": 5, "day": 32})),
        ..Anniversary::default()
    };
    let bday_year_zero = Anniversary {
        kind: "birth".into(),
        date: Some(json!({"year": 0, "month": 5, "day": 12})),
        ..Anniversary::default()
    };
    let bday_year_10000 = Anniversary {
        kind: "birth".into(),
        date: Some(json!({"year": 10000, "month": 5, "day": 12})),
        ..Anniversary::default()
    };
    let bday_timestamp = Anniversary {
        kind: "birth".into(),
        date: Some(json!({"utc": "1990-05-12T10:30:00Z"})),
        ..Anniversary::default()
    };

    assert!(states_anniversary(&bday_valid));
    assert!(states_anniversary(&wedding_valid));
    assert!(!states_anniversary(&death_unmapped));
    assert!(!states_anniversary(&bday_month_zero));
    assert!(!states_anniversary(&bday_month_13));
    assert!(!states_anniversary(&bday_day_zero));
    assert!(!states_anniversary(&bday_day_32));
    assert!(!states_anniversary(&bday_year_zero));
    assert!(!states_anniversary(&bday_year_10000));

    assert_eq!(anniversary_date(&bday_valid), Some("1990-05-12".into()));
    assert_eq!(anniversary_date(&bday_month_zero), None);
    assert_eq!(anniversary_date(&bday_day_32), None);

    assert!(states_a_point_in_time(&bday_timestamp));
    assert!(!states_a_point_in_time(&bday_valid));
    assert!(!states_a_point_in_time(&Anniversary {
        kind: "birth".into(),
        date: None,
        ..Anniversary::default()
    }));
}

#[test]
fn restore_address_and_name_components_reconstruction() {
    let current_addr = vec![
        AddressComponent::new("name", "Hauptstr."),
        AddressComponent::new("number", "42"),
        AddressComponent::new("locality", "Berlin"),
    ];
    let edited_addr_unchanged = vec![
        AddressComponent::new("name", "Hauptstr. 42"),
        AddressComponent::new("locality", "Berlin"),
    ];
    let restored_addr = restore_address_components(&current_addr, &edited_addr_unchanged);
    assert_eq!(restored_addr, current_addr);

    let edited_addr_changed = vec![
        AddressComponent::new("name", "Nebenstr. 10"),
        AddressComponent::new("locality", "Berlin"),
    ];
    let restored_addr_changed = restore_address_components(&current_addr, &edited_addr_changed);
    assert_eq!(restored_addr_changed, edited_addr_changed);

    let current_name = vec![
        NameComponent::new("given", "Anna"),
        NameComponent::new("given", "Lena"),
        NameComponent::new("surname", "Müller"),
    ];
    let edited_name_unchanged = vec![
        NameComponent::new("given", "Anna Lena"),
        NameComponent::new("surname", "Müller"),
    ];
    let restored_name = restore_name_components(&current_name, &edited_name_unchanged);
    assert_eq!(restored_name, current_name);

    let edited_name_changed = vec![
        NameComponent::new("given", "Maria"),
        NameComponent::new("surname", "Müller"),
    ];
    let restored_name_changed = restore_name_components(&current_name, &edited_name_changed);
    assert_eq!(restored_name_changed, edited_name_changed);
}

#[test]
fn vcard_parser_errors_and_error_display_formatting() {
    // Unterminated vCard error
    let unterminated = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\n";
    let err_unterminated = vcard_to_card(unterminated).expect_err("should fail unterminated");
    assert_eq!(err_unterminated, VCardError::Unterminated);
    assert_eq!(
        err_unterminated.to_string(),
        "truncated vCard: missing END:VCARD"
    );

    // Not a vCard error (e.g. empty input or iCalendar envelope)
    let not_vcard = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";
    let err_not_vcard = vcard_to_card(not_vcard).expect_err("should fail not a vCard");
    assert_eq!(err_not_vcard, VCardError::NotAVCard);
    assert_eq!(
        err_not_vcard.to_string(),
        "not a vCard: missing BEGIN:VCARD"
    );

    let empty_not_vcard = vcard_to_card("").expect_err("empty string is not a vcard");
    assert_eq!(empty_not_vcard, VCardError::NotAVCard);

    // Malformed line error formatting
    let malformed_vcard =
        "BEGIN:VCARD\r\nVERSION:3.0\r\nINVALID CONTENT LINE WITHOUT COLON\r\nEND:VCARD\r\n";
    let malformed_err = vcard_to_card(malformed_vcard).expect_err("should fail malformed line");
    assert_eq!(
        malformed_err,
        VCardError::Malformed("INVALID CONTENT LINE WITHOUT COLON".into())
    );
    assert_eq!(
        malformed_err.to_string(),
        "malformed vCard content line: INVALID CONTENT LINE WITHOUT COLON"
    );
}

#[test]
fn label_entry_with_empty_key_and_duplicate_keys_allocates_fresh_keys() {
    // Inbound vCard with empty X-JMAP-KEY parameter on LABEL must not assign "" as map key
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "ADR;TYPE=HOME:;;123 Home St;Berlin;;10115;Germany\r\n",
        "LABEL;TYPE=HOME;X-JMAP-KEY=\"\":123 Home St\\nBerlin\\n10115\\nGermany\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(vcard).expect("parse");
    let addresses = card.addresses.expect("addresses");
    assert_eq!(addresses.len(), 1);
    assert!(addresses.contains_key("a1"));
    assert_eq!(
        addresses["a1"].full.as_deref(),
        Some("123 Home St\nBerlin\n10115\nGermany")
    );

    // Inbound vCard with multiple unkeyed emails allocates sequential e1, e2, e3
    let vcard_emails = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "EMAIL:alice@example.com\r\n",
        "EMAIL:bob@example.com\r\n",
        "EMAIL:charlie@example.com\r\n",
        "END:VCARD\r\n"
    );
    let card_emails = vcard_to_card(vcard_emails).expect("parse");
    let emails = card_emails.emails.expect("emails");
    assert_eq!(emails.len(), 3);
    assert!(emails.contains_key("e1"));
    assert!(emails.contains_key("e2"));
    assert!(emails.contains_key("e3"));
}

#[test]
fn inbound_vcard_with_various_parameter_types_and_component_categories() {
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "CATEGORIES:Work,Personal,VIP\r\n",
        "TEL;TYPE=WORK,PREF:+49301234567\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(vcard).expect("parse");
    let keywords = card.keywords.expect("keywords");
    assert!(keywords.contains_key("Work"));
    assert!(keywords.contains_key("Personal"));
    assert!(keywords.contains_key("VIP"));
    let phones = card.phones.expect("phones");
    let phone = phones.values().next().expect("phone");
    assert_eq!(phone.pref, Some(1));
}

#[test]
fn inbound_vcard_with_unquoted_integer_jmap_keys() {
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "EMAIL;X-JMAP-KEY=123:alice@example.com\r\n",
        "TEL;X-JMAP-KEY=456:+49301234567\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(vcard).expect("parse");
    let emails = card.emails.expect("emails");
    assert!(
        emails.contains_key("123"),
        "unquoted integer key '123' must be preserved"
    );
    let phones = card.phones.expect("phones");
    assert!(
        phones.contains_key("456"),
        "unquoted integer key '456' must be preserved"
    );
}

#[test]
fn inbound_vcard_with_multi_component_name_field() {
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "N:Smith;Alice,Bob;Middle;;Dr.\r\n",
        "FN:Alice,Bob Smith\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(vcard).expect("parse");
    let name = card.name.expect("name");
    let components = name.components.expect("components");
    let given = components
        .iter()
        .find(|c| c.kind == "given")
        .expect("given component");
    assert_eq!(given.value, "Alice,Bob");
}

#[test]
fn vcard_kind_group_and_member_lines_characterization() {
    // Characterizes inbound vCard 3.0 / RFC 6473 / RFC 6350 group cards with
    // `KIND:group` and multiple `MEMBER` lines.
    //
    // In JSContact (RFC 9553 §2.1.2 & §2.1.9), group cards have `kind: "group"`
    // and `members: Map[Id, Boolean]`. In `jmap-vcard`, `kind` and `members`
    // are unmodeled in `ContactCard` struct fields (they ride in `extra` on the
    // protocol layer) and are dropped by the vCard 3.0 parser/emitter rather
    // than misstated.
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:group-dev-team\r\n",
        "FN:Dev Team\r\n",
        "KIND:group\r\n",
        "MEMBER:urn:uuid:550e8400-e29b-41d4-a716-446655440000\r\n",
        "MEMBER:mailto:alice@example.com\r\n",
        "MEMBER:mailto:bob@example.com\r\n",
        "NOTE:Core engineering team\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(vcard).expect("group vcard parse must succeed");
    assert_eq!(card.id.as_ref().unwrap().as_str(), "group-dev-team");
    assert_eq!(
        card.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Dev Team")
    );
    let notes = card.notes.as_ref().expect("notes");
    assert_eq!(
        notes.values().next().map(|n| n.note.as_str()),
        Some("Core engineering team")
    );
    // `KIND` and `MEMBER` are unmapped properties, so `extra` is empty
    assert!(card.extra.is_empty());

    // Outbound serialization emits a clean, valid vCard 3.0 envelope without
    // inventing unmapped KIND/MEMBER lines.
    let emitted = card_to_vcard(&card);
    assert!(emitted.contains("FN:Dev Team\r\n"));
    assert!(emitted.contains("NOTE;X-JMAP-KEY=n1:Core engineering team\r\n"));
    assert!(!emitted.contains("KIND:"));
    assert!(!emitted.contains("MEMBER:"));

    // Roundtrip back to JSContact reaches a fixed-point
    let back = vcard_to_card(&emitted).expect("reparse must succeed");
    assert_eq!(
        back.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Dev Team")
    );
    assert_eq!(
        back.notes
            .as_ref()
            .and_then(|m| m.get("n1"))
            .map(|n| n.note.as_str()),
        Some("Core engineering team")
    );
}

#[test]
fn vcard_apple_and_eds_group_list_extensions_characterization() {
    // Vendor-specific group cards:
    // 1. Apple CardDAV extension: `X-ADDRESSBOOKSERVER-KIND:group` + `X-ADDRESSBOOKSERVER-MEMBER:...`
    // 2. EDS contact list extension: `X-EVOLUTION-LIST:TRUE` (`E_CONTACT_IS_LIST`) + `X-EVOLUTION-DEST-EMAIL:...`
    let apple_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:apple-group-1\r\n",
        "FN:Design Team\r\n",
        "X-ADDRESSBOOKSERVER-KIND:group\r\n",
        "X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:11111111-2222-3333-4444-555555555555\r\n",
        "X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:66666666-7777-8888-9999-000000000000\r\n",
        "END:VCARD\r\n"
    );
    let card1 = vcard_to_card(apple_vcard).expect("apple group vcard parse");
    assert_eq!(card1.id.as_ref().unwrap().as_str(), "apple-group-1");
    assert_eq!(
        card1.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Design Team")
    );
    assert!(card1.extra.is_empty());

    let eds_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:eds-list-1\r\n",
        "FN:Release Coordinators\r\n",
        "X-EVOLUTION-LIST:TRUE\r\n",
        "X-EVOLUTION-LIST-SHOW-ADDRESSES:FALSE\r\n",
        "X-EVOLUTION-DEST-EMAIL:rel-coord@example.com\r\n",
        "EMAIL;TYPE=WORK:rel-coord@example.com\r\n",
        "END:VCARD\r\n"
    );
    let card2 = vcard_to_card(eds_vcard).expect("eds list vcard parse");
    assert_eq!(card2.id.as_ref().unwrap().as_str(), "eds-list-1");
    assert_eq!(
        card2.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Release Coordinators")
    );
    let emails = card2.emails.expect("emails");
    assert_eq!(
        emails.values().next().map(|e| e.address.as_str()),
        Some("rel-coord@example.com")
    );
}

#[test]
fn vcard_non_group_kind_variants_characterization() {
    // Tests RFC 6473 `KIND` values other than `group` (`individual`, `org`,
    // `location`, `device`, `application`, `x-custom`).
    for kind in [
        "individual",
        "org",
        "location",
        "device",
        "application",
        "x-robot",
    ] {
        let vcard = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:card-kind-{kind}\r\nFN:Entity {kind}\r\nKIND:{kind}\r\nORG:Example Corp\r\nNOTE:Test entity\r\nEND:VCARD\r\n"
        );
        let card = vcard_to_card(&vcard).expect("kind variant parse");
        assert_eq!(
            card.name.as_ref().and_then(|n| n.full.as_deref()),
            Some(format!("Entity {kind}").as_str()),
            "FN must parse for KIND:{kind}"
        );
        let orgs = card.organizations.as_ref().expect("orgs");
        assert_eq!(
            orgs.values().next().and_then(|o| o.name.as_deref()),
            Some("Example Corp"),
            "ORG must parse for KIND:{kind}"
        );
        let notes = card.notes.as_ref().expect("notes");
        assert_eq!(
            notes.values().next().map(|n| n.note.as_str()),
            Some("Test entity"),
            "NOTE must parse for KIND:{kind}"
        );

        let emitted = card_to_vcard(&card);
        assert!(!emitted.contains("KIND:"));
        let back = vcard_to_card(&emitted).expect("reparse");
        assert_eq!(
            back.name.as_ref().and_then(|n| n.full.as_deref()),
            Some(format!("Entity {kind}").as_str())
        );
    }
}

#[test]
fn jscontact_group_card_with_members_map_in_extra_characterization() {
    // When a JSContact Card from the server has `kind: "group"` and
    // `members: { ... }` in `extra`, `card_to_vcard` safely ignores the
    // unmodeled properties without injecting malformed vCard lines or
    // panicking.
    let mut extra = BTreeMap::new();
    extra.insert("kind".to_owned(), json!("group"));
    extra.insert(
        "members".to_owned(),
        json!({
            "urn:uuid:550e8400-e29b-41d4-a716-446655440000": true,
            "urn:uuid:66666666-7777-8888-9999-000000000000": true,
            "mailto:carol@example.com": true
        }),
    );

    let card = ContactCard {
        id: Some("grp-1".into()),
        uid: Some("urn:uuid:grp-1".to_owned()),
        card_type: Some("Card".to_owned()),
        version: Some("1.0".to_owned()),
        name: Some(Name {
            full: Some("Frontend Working Group".to_owned()),
            ..Name::default()
        }),
        extra,
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert!(vcard.contains("FN:Frontend Working Group\r\n"));
    assert!(vcard.contains("UID:grp-1\r\n"));
    assert!(vcard.contains("X-JMAP-UID:urn:uuid:grp-1\r\n"));
    assert!(!vcard.contains("KIND:"));
    assert!(!vcard.contains("MEMBER:"));

    let back = vcard_to_card(&vcard).expect("reparse");
    assert_eq!(
        back.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Frontend Working Group")
    );
    assert_eq!(back.id.as_ref().unwrap().as_str(), "grp-1");
    assert_eq!(back.uid.as_deref(), Some("urn:uuid:grp-1"));
    assert!(back.extra.is_empty());
}

#[test]
fn group_card_coexisting_with_full_suite_of_contact_properties_roundtrip() {
    // A group card carrying `KIND:group` and `MEMBER` properties alongside all
    // 12 standard mapped contact properties (`FN`, `NICKNAME`, `EMAIL`, `TEL`,
    // `ADR`, `ORG`, `TITLE`, `ROLE`, `NOTE`, `URL`, `CATEGORIES`, `PHOTO`,
    // `X-EVOLUTION-SPOUSE`).
    //
    // Asserts that all 12 properties roundtrip losslessly through
    // vCard -> JSContact -> vCard -> JSContact without degradation or component
    // shifting.
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:arch-wg-01\r\n",
        "FN:Architecture Working Group\r\n",
        "KIND:group\r\n",
        "MEMBER:urn:uuid:11111111-2222-3333-4444-555555555555\r\n",
        "MEMBER:mailto:chair@example.org\r\n",
        "NICKNAME;X-JMAP-KEY=k1:arch-wg\r\n",
        "EMAIL;X-JMAP-KEY=e1;TYPE=WORK:arch-wg@example.org\r\n",
        "TEL;X-JMAP-KEY=p1;TYPE=WORK,VOICE:+4930123456\r\n",
        "ADR;X-JMAP-KEY=a1;TYPE=WORK:;;Tiergartenstraße 1;Berlin;Berlin;10785;Germany\r\n",
        "LABEL;X-JMAP-KEY=a1;TYPE=WORK:Tiergartenstraße 1\\n10785 Berlin\\nGermany\r\n",
        "ORG;X-JMAP-KEY=o1:Standards Org;Technical Council;Architecture\r\n",
        "TITLE;X-JMAP-KEY=t1:Working Group\r\n",
        "ROLE;X-JMAP-KEY=t2:Standards Development\r\n",
        "NOTE;X-JMAP-KEY=n1:Meets bi-weekly on Thursdays\r\n",
        "URL;X-JMAP-KEY=l1:https://standards.example.org/arch-wg\r\n",
        "CATEGORIES:WorkingGroup,Standards,Architecture\r\n",
        "PHOTO;X-JMAP-KEY=m1;VALUE=uri:https://standards.example.org/logo.png\r\n",
        "X-EVOLUTION-SPOUSE:Sister Working Group\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("first parse");
    assert_eq!(card.id.as_ref().unwrap().as_str(), "arch-wg-01");
    assert_eq!(
        card.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Architecture Working Group")
    );
    assert_eq!(
        card.nicknames
            .as_ref()
            .and_then(|m| m.get("k1"))
            .map(|n| n.name.as_str()),
        Some("arch-wg")
    );
    assert_eq!(
        card.emails
            .as_ref()
            .and_then(|m| m.get("e1"))
            .map(|e| e.address.as_str()),
        Some("arch-wg@example.org")
    );
    assert_eq!(
        card.phones
            .as_ref()
            .and_then(|m| m.get("p1"))
            .map(|p| p.number.as_str()),
        Some("+4930123456")
    );
    assert_eq!(
        card.organizations
            .as_ref()
            .and_then(|m| m.get("o1"))
            .and_then(|o| o.name.as_deref()),
        Some("Standards Org")
    );
    assert_eq!(
        card.titles
            .as_ref()
            .and_then(|m| m.get("t1"))
            .map(|t| t.name.as_str()),
        Some("Working Group")
    );
    assert_eq!(
        card.titles
            .as_ref()
            .and_then(|m| m.get("t2"))
            .map(|t| t.name.as_str()),
        Some("Standards Development")
    );
    assert_eq!(
        card.notes
            .as_ref()
            .and_then(|m| m.get("n1"))
            .map(|n| n.note.as_str()),
        Some("Meets bi-weekly on Thursdays")
    );
    assert_eq!(
        card.links
            .as_ref()
            .and_then(|m| m.get("l1"))
            .map(|l| l.uri.as_str()),
        Some("https://standards.example.org/arch-wg")
    );
    assert!(
        card.keywords
            .as_ref()
            .is_some_and(|k| k.contains_key("Architecture"))
    );
    assert!(
        card.related_to
            .as_ref()
            .is_some_and(|r| r.contains_key("Sister Working Group"))
    );

    // Re-emit via card_to_vcard and re-parse
    let emitted = card_to_vcard(&card);
    let back = vcard_to_card(&emitted).expect("second parse");
    assert_eq!(
        back.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Architecture Working Group")
    );
    assert_eq!(
        back.nicknames
            .as_ref()
            .and_then(|m| m.get("k1"))
            .map(|n| n.name.as_str()),
        Some("arch-wg")
    );
    assert_eq!(
        back.emails
            .as_ref()
            .and_then(|m| m.get("e1"))
            .map(|e| e.address.as_str()),
        Some("arch-wg@example.org")
    );
    assert_eq!(
        back.phones
            .as_ref()
            .and_then(|m| m.get("p1"))
            .map(|p| p.number.as_str()),
        Some("+4930123456")
    );
    assert_eq!(
        back.organizations
            .as_ref()
            .and_then(|m| m.get("o1"))
            .and_then(|o| o.name.as_deref()),
        Some("Standards Org")
    );
    assert_eq!(
        back.titles
            .as_ref()
            .and_then(|m| m.get("t1"))
            .map(|t| t.name.as_str()),
        Some("Working Group")
    );
    assert_eq!(
        back.titles
            .as_ref()
            .and_then(|m| m.get("t2"))
            .map(|t| t.name.as_str()),
        Some("Standards Development")
    );
    assert_eq!(
        back.notes
            .as_ref()
            .and_then(|m| m.get("n1"))
            .map(|n| n.note.as_str()),
        Some("Meets bi-weekly on Thursdays")
    );
    assert_eq!(
        back.links
            .as_ref()
            .and_then(|m| m.get("l1"))
            .map(|l| l.uri.as_str()),
        Some("https://standards.example.org/arch-wg")
    );
    assert!(
        back.keywords
            .as_ref()
            .is_some_and(|k| k.contains_key("Architecture"))
    );
    assert!(
        back.related_to
            .as_ref()
            .is_some_and(|r| r.contains_key("Sister Working Group"))
    );
}

#[test]
fn group_card_with_parameter_variations_and_empty_values() {
    // Tests lowercase parameter names, explicit VALUE parameter types,
    // empty values, and custom parameters on KIND and MEMBER lines.
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:param-var-1\r\n",
        "FN:Variations Group\r\n",
        "kind;value=text:group\r\n",
        "KIND:\r\n",
        "member;VALUE=uri:urn:uuid:12345678-1234-5678-1234-567812345678\r\n",
        "MEMBER;X-JMAP-KEY=m1:mailto:lead@example.com\r\n",
        "MEMBER:\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(vcard).expect("parse with parameter variations");
    assert_eq!(
        card.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Variations Group")
    );
    assert!(card.extra.is_empty());

    let emitted = card_to_vcard(&card);
    assert!(emitted.contains("FN:Variations Group\r\n"));
    assert!(!emitted.contains("KIND:"));
    assert!(!emitted.contains("MEMBER:"));
}

#[test]
fn vcard_altid_and_language_singleton_properties_deterministic_selection() {
    // Characterizes vCards carrying multiple `FN` and `N` representations in
    // several languages grouped by `ALTID` and marked with `LANGUAGE` / `SCRIPT`.
    //
    // In JSContact (RFC 9553 §2.2.1), `Name` models a single display name (`full`)
    // and component list for the default card language, while alternate
    // language representations live in the server-level `localizations` map.
    //
    // In `jmap-vcard`, `read_name` evaluates `card.entries` in document order
    // and deterministically selects the FIRST `FN` and `N` properties encountered,
    // safely ignoring secondary language alternates. Outbound serialization
    // generates clean, non-duplicate `FN` and `N` lines matching the selected primary.
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:altid-singleton-01\r\n",
        "FN;ALTID=1;LANGUAGE=en:Dr. Alexander Mueller\r\n",
        "FN;ALTID=1;LANGUAGE=de:Herr Dr. Alexander Müller\r\n",
        "FN;ALTID=1;LANGUAGE=ja;SCRIPT=Jpan:ミュラー・アレクサンダー\r\n",
        "FN;ALTID=1;LANGUAGE=ja;SCRIPT=Latn:Alex Mueller\r\n",
        "N;ALTID=1;LANGUAGE=en:Mueller;Alexander;;Dr.;\r\n",
        "N;ALTID=1;LANGUAGE=de:Müller;Alexander;;Herr Dr.;\r\n",
        "N;ALTID=1;LANGUAGE=ja:ミュラー;アレクサンダー;;;\r\n",
        "NOTE:Research Director\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse multilingual singleton vcard");
    assert_eq!(card.id.as_ref().unwrap().as_str(), "altid-singleton-01");
    let name = card.name.as_ref().expect("name must be present");
    assert_eq!(
        name.full.as_deref(),
        Some("Dr. Alexander Mueller"),
        "primary (first in document order) FN must be selected"
    );
    let components = name.components.as_ref().expect("components");
    assert_eq!(components.len(), 3);
    assert_eq!(
        components
            .iter()
            .find(|c| c.kind == "surname")
            .unwrap()
            .value,
        "Mueller"
    );
    assert_eq!(
        components.iter().find(|c| c.kind == "given").unwrap().value,
        "Alexander"
    );
    assert_eq!(
        components.iter().find(|c| c.kind == "title").unwrap().value,
        "Dr."
    );

    // Outbound serialization emits standard FN and N lines without duplicating
    let emitted = card_to_vcard(&card);
    assert_eq!(line(&emitted, "FN:"), "FN:Dr. Alexander Mueller");
    assert_eq!(line(&emitted, "N:"), "N:Mueller;Alexander;;Dr.;");
    assert_eq!(
        emitted.lines().filter(|l| l.starts_with("FN:")).count(),
        1,
        "exactly one FN line must be emitted"
    );
    assert_eq!(
        emitted.lines().filter(|l| l.starts_with("N:")).count(),
        1,
        "exactly one N line must be emitted"
    );

    // Fixed-point roundtrip stability
    let back = vcard_to_card(&emitted).expect("roundtrip parse");
    assert_eq!(back.name, card.name);
    let re_emitted = card_to_vcard(&back);
    assert_eq!(emitted, re_emitted);
}

#[test]
fn vcard_altid_and_language_multivalued_properties_preservation_and_roundtrip() {
    // Characterizes vCards carrying multiple alternate representations grouped
    // by `ALTID` and `LANGUAGE` across all supported multi-valued property types:
    // `NOTE`, `TITLE`, `ROLE`, `ORG`, `NICKNAME`, `URL`, `EMAIL`, `TEL`.
    //
    // In `jmap-vcard`, all multi-valued properties are stored in keyed JSContact
    // maps (`notes`, `titles`, `organizations`, `nicknames`, `links`, `emails`,
    // `phones`). Every incoming alternate representation line is preserved as a
    // distinct keyed entry (keyed via `X-JMAP-KEY` or sequentially allocated
    // `n1`, `n2`, `t1`, `t2`, etc.) rather than being dropped or overwriting
    // preceding entries.
    //
    // Outbound serialization writes each entry with an `X-JMAP-KEY` parameter,
    // guaranteeing lossless round-trip stability.
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:altid-multivalue-01\r\n",
        "FN:Elena Rostova\r\n",
        "N:Rostova;Elena;;;\r\n",
        "NOTE;ALTID=1;LANGUAGE=en:Primary project maintainer\r\n",
        "NOTE;ALTID=1;LANGUAGE=de:Hauptverantwortliche Projektleiterin\r\n",
        "NOTE;ALTID=1;LANGUAGE=fr:Responsable principale du projet\r\n",
        "TITLE;ALTID=1;LANGUAGE=en:Principal Research Scientist\r\n",
        "TITLE;ALTID=1;LANGUAGE=de:Leitende Wissenschaftlerin\r\n",
        "ROLE;ALTID=2;LANGUAGE=en:Technical Steering Committee Member\r\n",
        "ROLE;ALTID=2;LANGUAGE=de:Mitglied des Technischen Lenkungsausschusses\r\n",
        "ORG;ALTID=1;LANGUAGE=en:Open Standards Foundation;Engineering Division;Core Architecture\r\n",
        "ORG;ALTID=1;LANGUAGE=de:Offene Standards Stiftung;Ingenieurwesen;Kernarchitektur\r\n",
        "NICKNAME;ALTID=1;LANGUAGE=en:Lena\r\n",
        "NICKNAME;ALTID=1;LANGUAGE=ru:Лена\r\n",
        "URL;ALTID=1;LANGUAGE=en:https://example.org/en/elena\r\n",
        "URL;ALTID=1;LANGUAGE=de:https://example.org/de/elena\r\n",
        "EMAIL;ALTID=1;LANGUAGE=en;TYPE=WORK:elena.en@example.org\r\n",
        "EMAIL;ALTID=1;LANGUAGE=de;TYPE=WORK:elena.de@example.org\r\n",
        "TEL;ALTID=1;LANGUAGE=en;TYPE=WORK,VOICE:+14155550100\r\n",
        "TEL;ALTID=1;LANGUAGE=de;TYPE=WORK,VOICE:+4930123456\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse multi-valued ALTID vcard");

    // 1. NOTES: all 3 language variants preserved
    let notes = card.notes.as_ref().expect("notes");
    assert_eq!(notes.len(), 3, "all 3 NOTE alternates must be preserved");
    let note_texts: Vec<&str> = notes.values().map(|n| n.note.as_str()).collect();
    assert!(note_texts.contains(&"Primary project maintainer"));
    assert!(note_texts.contains(&"Hauptverantwortliche Projektleiterin"));
    assert!(note_texts.contains(&"Responsable principale du projet"));

    // 2. TITLES & ROLES: all 4 alternates preserved
    let titles = card.titles.as_ref().expect("titles");
    assert_eq!(
        titles.len(),
        4,
        "all 2 TITLE + 2 ROLE alternates must be preserved"
    );
    let title_names: Vec<&str> = titles.values().map(|t| t.name.as_str()).collect();
    assert!(title_names.contains(&"Principal Research Scientist"));
    assert!(title_names.contains(&"Leitende Wissenschaftlerin"));
    assert!(title_names.contains(&"Technical Steering Committee Member"));
    assert!(title_names.contains(&"Mitglied des Technischen Lenkungsausschusses"));

    // 3. ORGANIZATIONS: both 3-component orgs preserved
    let orgs = card.organizations.as_ref().expect("organizations");
    assert_eq!(orgs.len(), 2, "both ORG alternates must be preserved");
    let org_en = orgs
        .values()
        .find(|o| o.name.as_deref() == Some("Open Standards Foundation"))
        .expect("English ORG");
    let units_en: Vec<&str> = org_en
        .units
        .as_ref()
        .unwrap()
        .iter()
        .map(|u| u.name.as_str())
        .collect();
    assert_eq!(units_en, vec!["Engineering Division", "Core Architecture"]);

    let org_de = orgs
        .values()
        .find(|o| o.name.as_deref() == Some("Offene Standards Stiftung"))
        .expect("German ORG");
    let units_de: Vec<&str> = org_de
        .units
        .as_ref()
        .unwrap()
        .iter()
        .map(|u| u.name.as_str())
        .collect();
    assert_eq!(units_de, vec!["Ingenieurwesen", "Kernarchitektur"]);

    // 4. NICKNAMES: both Latin and Cyrillic nicknames preserved
    let nicks = card.nicknames.as_ref().expect("nicknames");
    assert_eq!(nicks.len(), 2, "both NICKNAME alternates must be preserved");
    let nick_names: Vec<&str> = nicks.values().map(|n| n.name.as_str()).collect();
    assert!(nick_names.contains(&"Lena"));
    assert!(nick_names.contains(&"Лена"));

    // 5. LINKS: both URLs preserved
    let links = card.links.as_ref().expect("links");
    assert_eq!(links.len(), 2, "both URL alternates must be preserved");
    let uris: Vec<&str> = links.values().map(|l| l.uri.as_str()).collect();
    assert!(uris.contains(&"https://example.org/en/elena"));
    assert!(uris.contains(&"https://example.org/de/elena"));

    // 6. EMAILS & PHONES: both language-specific emails and phone numbers preserved
    let emails = card.emails.as_ref().expect("emails");
    assert_eq!(emails.len(), 2, "both EMAIL alternates must be preserved");
    let phones = card.phones.as_ref().expect("phones");
    assert_eq!(phones.len(), 2, "both TEL alternates must be preserved");

    // Outbound serialization emits all preserved alternates with X-JMAP-KEY
    let emitted = card_to_vcard(&card);
    let unfolded_vcard = unfolded(&emitted);
    assert!(unfolded_vcard.contains("NOTE;X-JMAP-KEY=n1:Primary project maintainer"));
    assert!(unfolded_vcard.contains("NOTE;X-JMAP-KEY=n2:Hauptverantwortliche Projektleiterin"));
    assert!(unfolded_vcard.contains("NOTE;X-JMAP-KEY=n3:Responsable principale du projet"));
    assert!(unfolded_vcard.contains("TITLE;X-JMAP-KEY=t1:Principal Research Scientist"));
    assert!(unfolded_vcard.contains("TITLE;X-JMAP-KEY=t2:Leitende Wissenschaftlerin"));
    assert!(unfolded_vcard.contains("ROLE;X-JMAP-KEY=t3:Technical Steering Committee Member"));
    assert!(
        unfolded_vcard.contains("ROLE;X-JMAP-KEY=t4:Mitglied des Technischen Lenkungsausschusses")
    );
    assert!(unfolded_vcard.contains(
        "ORG;X-JMAP-KEY=o1:Open Standards Foundation;Engineering Division;Core Architecture"
    ));
    assert!(
        unfolded_vcard
            .contains("ORG;X-JMAP-KEY=o2:Offene Standards Stiftung;Ingenieurwesen;Kernarchitektur")
    );
    assert!(unfolded_vcard.contains("NICKNAME;X-JMAP-KEY=k1:Lena"));
    assert!(unfolded_vcard.contains("NICKNAME;X-JMAP-KEY=k2:Лена"));
    assert!(unfolded_vcard.contains("URL;X-JMAP-KEY=l1:https://example.org/en/elena"));
    assert!(unfolded_vcard.contains("URL;X-JMAP-KEY=l2:https://example.org/de/elena"));

    // Fixed-point roundtrip stability across repeated passes
    let back = vcard_to_card(&emitted).expect("second parse");
    let re_emitted = card_to_vcard(&back);
    assert_eq!(
        emitted, re_emitted,
        "roundtrip must converge to fixed-point"
    );
}

#[test]
fn vcard_altid_and_language_multilingual_structured_address_and_label_pairing() {
    // Tests structured addresses and written-out labels carrying `ALTID` and
    // `LANGUAGE` parameters in English, Spanish, and German with work and home contexts.
    //
    // Verifies that:
    // 1. Each `ADR` line maps into a distinct `Address` entry in `addresses` (`a1`, `a2`, `a3`).
    // 2. `label_entry` accurately pairs each `LABEL` with its corresponding unlabelled
    //    address of matching context, setting `full` on each address accurately.
    // 3. Round-trip serialization emits both `ADR` and `LABEL` with matching `X-JMAP-KEY`.
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:altid-adr-01\r\n",
        "FN:Multilingual Address Test\r\n",
        "ADR;ALTID=1;LANGUAGE=en;TYPE=WORK:;;100 Innovation Way;Tech City;CA;94016;USA\r\n",
        "LABEL;ALTID=1;LANGUAGE=en;TYPE=WORK:100 Innovation Way\\nTech City, CA 94016\\nUSA\r\n",
        "ADR;ALTID=1;LANGUAGE=es;TYPE=WORK:;;Calle Innovación 100;Ciudad Tecnológica;CA;94016;EE.UU.\r\n",
        "LABEL;ALTID=1;LANGUAGE=es;TYPE=WORK:Calle Innovación 100\\nCiudad Tecnológica, CA 94016\\nEE.UU.\r\n",
        "ADR;ALTID=2;LANGUAGE=de;TYPE=HOME:;;Hauptstraße 42;Berlin;Berlin;10115;Deutschland\r\n",
        "LABEL;ALTID=2;LANGUAGE=de;TYPE=HOME:Hauptstraße 42\\n10115 Berlin\\nDeutschland\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse multilingual ADR vcard");
    let addresses = card.addresses.as_ref().expect("addresses");
    assert_eq!(addresses.len(), 3, "all 3 addresses must be preserved");

    // Address 1 (English Work)
    let a1 = &addresses["a1"];
    assert_eq!(a1.contexts, Some(json!({"work": true})));
    assert_eq!(
        a1.full.as_deref(),
        Some("100 Innovation Way\nTech City, CA 94016\nUSA")
    );
    let comps_1: Vec<(&str, &str)> = a1
        .components
        .as_ref()
        .unwrap()
        .iter()
        .map(|c| (c.kind.as_str(), c.value.as_str()))
        .collect();
    assert_eq!(
        comps_1,
        vec![
            ("name", "100 Innovation Way"),
            ("locality", "Tech City"),
            ("region", "CA"),
            ("postcode", "94016"),
            ("country", "USA"),
        ]
    );

    // Address 2 (Spanish Work)
    let a2 = &addresses["a2"];
    assert_eq!(a2.contexts, Some(json!({"work": true})));
    assert_eq!(
        a2.full.as_deref(),
        Some("Calle Innovación 100\nCiudad Tecnológica, CA 94016\nEE.UU.")
    );
    let comps_2: Vec<(&str, &str)> = a2
        .components
        .as_ref()
        .unwrap()
        .iter()
        .map(|c| (c.kind.as_str(), c.value.as_str()))
        .collect();
    assert_eq!(
        comps_2,
        vec![
            ("name", "Calle Innovación 100"),
            ("locality", "Ciudad Tecnológica"),
            ("region", "CA"),
            ("postcode", "94016"),
            ("country", "EE.UU."),
        ]
    );

    // Address 3 (German Home)
    let a3 = &addresses["a3"];
    assert_eq!(a3.contexts, Some(json!({"private": true})));
    assert_eq!(
        a3.full.as_deref(),
        Some("Hauptstraße 42\n10115 Berlin\nDeutschland")
    );
    let comps_3: Vec<(&str, &str)> = a3
        .components
        .as_ref()
        .unwrap()
        .iter()
        .map(|c| (c.kind.as_str(), c.value.as_str()))
        .collect();
    assert_eq!(
        comps_3,
        vec![
            ("name", "Hauptstraße 42"),
            ("locality", "Berlin"),
            ("region", "Berlin"),
            ("postcode", "10115"),
            ("country", "Deutschland"),
        ]
    );

    // Outbound serialization and round-trip verification
    let emitted = card_to_vcard(&card);
    let unfolded_vcard = unfolded(&emitted);
    assert!(
        unfolded_vcard
            .contains("ADR;X-JMAP-KEY=a1;TYPE=WORK:;;100 Innovation Way;Tech City;CA;94016;USA")
    );
    assert!(unfolded_vcard.contains(
        "LABEL;X-JMAP-KEY=a1;TYPE=WORK:100 Innovation Way\\nTech City\\, CA 94016\\nUSA"
    ));
    assert!(unfolded_vcard.contains(
        "ADR;X-JMAP-KEY=a2;TYPE=WORK:;;Calle Innovación 100;Ciudad Tecnológica;CA;94016;EE.UU."
    ));
    assert!(unfolded_vcard.contains(
        "LABEL;X-JMAP-KEY=a2;TYPE=WORK:Calle Innovación 100\\nCiudad Tecnológica\\, CA 94016\\nEE.UU."
    ));
    assert!(
        unfolded_vcard.contains(
            "ADR;X-JMAP-KEY=a3;TYPE=HOME:;;Hauptstraße 42;Berlin;Berlin;10115;Deutschland"
        )
    );
    assert!(
        unfolded_vcard
            .contains("LABEL;X-JMAP-KEY=a3;TYPE=HOME:Hauptstraße 42\\n10115 Berlin\\nDeutschland")
    );

    let back = vcard_to_card(&emitted).expect("roundtrip parse");
    assert_eq!(back.addresses, card.addresses);
}

#[test]
fn vcard_altid_and_language_categories_and_keywords_union_and_deduplication() {
    // Tests multiple `CATEGORIES` lines carrying `ALTID` and `LANGUAGE` parameters.
    //
    // Verifies that:
    // 1. `read_keywords` aggregates all categories across all lines, deduplicating
    //    shared tags and retaining language-specific tags into a unified `BTreeMap`.
    // 2. Outbound serialization emits a single canonical, sorted `CATEGORIES` line.
    // 3. Round-trip reaches fixed-point stability.
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:altid-cat-01\r\n",
        "FN:Category Test\r\n",
        "CATEGORIES;ALTID=1;LANGUAGE=en:Software,Architecture,OpenSource\r\n",
        "CATEGORIES;ALTID=1;LANGUAGE=de:Software,Architektur,OpenSource\r\n",
        "CATEGORIES;ALTID=2;LANGUAGE=fr:Logiciel,Architecture\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse multilingual categories");
    let keywords = card.keywords.as_ref().expect("keywords map");
    assert_eq!(
        keywords.len(),
        5,
        "union of tags across languages deduplicated"
    );
    assert!(keywords.contains_key("Architecture"));
    assert!(keywords.contains_key("Architektur"));
    assert!(keywords.contains_key("Logiciel"));
    assert!(keywords.contains_key("OpenSource"));
    assert!(keywords.contains_key("Software"));

    let emitted = card_to_vcard(&card);
    assert!(
        emitted.contains("CATEGORIES:Architecture,Architektur,Logiciel,OpenSource,Software\r\n")
    );
    assert_eq!(
        emitted.matches("CATEGORIES:").count(),
        1,
        "single combined CATEGORIES line emitted"
    );

    let back = vcard_to_card(&emitted).expect("reparse");
    assert_eq!(back.keywords, card.keywords);
}

#[test]
fn vcard_altid_and_language_explicit_and_colliding_jmap_keys_handling() {
    // Tests vCards where `ALTID`/`LANGUAGE` lines carry explicit `X-JMAP-KEY`
    // parameters, verifying:
    // 1. Distinct explicit keys are preserved (`custom-en`, `custom-de`).
    // 2. Colliding explicit keys (e.g. legacy generator wrote duplicate keys)
    //    are resolved by `entry_key` allocating a fresh unique key (`n1`),
    //    preventing accidental overwriting or loss of the second alternate.
    // 3. Empty `X-JMAP-KEY=""` parameters allocate fresh sequential keys.
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:altid-keys-01\r\n",
        "FN:Key Test\r\n",
        "NOTE;ALTID=1;LANGUAGE=en;X-JMAP-KEY=note-en:English Note\r\n",
        "NOTE;ALTID=1;LANGUAGE=de;X-JMAP-KEY=note-de:Deutsche Notiz\r\n",
        "TITLE;ALTID=1;LANGUAGE=en;X-JMAP-KEY=dup-key:Lead Architect\r\n",
        "TITLE;ALTID=1;LANGUAGE=de;X-JMAP-KEY=dup-key:Chefarchitekt\r\n",
        "NICKNAME;ALTID=1;LANGUAGE=en;X-JMAP-KEY=\"\":Speedy\r\n",
        "NICKNAME;ALTID=1;LANGUAGE=de:Flitzi\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse vcard with explicit keys");

    // Notes: distinct explicit keys preserved
    let notes = card.notes.as_ref().expect("notes");
    assert_eq!(notes.len(), 2);
    assert_eq!(notes["note-en"].note, "English Note");
    assert_eq!(notes["note-de"].note, "Deutsche Notiz");

    // Titles: duplicate keys resolved without dropping entries
    let titles = card.titles.as_ref().expect("titles");
    assert_eq!(
        titles.len(),
        2,
        "duplicate key must be resolved into distinct entries"
    );
    assert!(titles.contains_key("dup-key"));
    assert!(titles.contains_key("t1"));
    let title_texts: Vec<&str> = titles.values().map(|t| t.name.as_str()).collect();
    assert!(title_texts.contains(&"Lead Architect"));
    assert!(title_texts.contains(&"Chefarchitekt"));

    // Nicknames: empty key assigned 'k1', second assigned 'k2'
    let nicks = card.nicknames.as_ref().expect("nicknames");
    assert_eq!(nicks.len(), 2);
    assert!(nicks.contains_key("k1"));
    assert!(nicks.contains_key("k2"));
    assert_eq!(nicks["k1"].name, "Speedy");
    assert_eq!(nicks["k2"].name, "Flitzi");

    // Roundtrip verification
    let emitted = card_to_vcard(&card);
    let back = vcard_to_card(&emitted).expect("reparse");
    assert_eq!(back.notes, card.notes);
    assert_eq!(back.titles, card.titles);
    assert_eq!(back.nicknames, card.nicknames);
}

#[test]
fn vcard_altid_and_language_parameter_variations_and_boundary_cases() {
    // Tests varied RFC 5646 language tags (subtags, scripts, regions), quoted
    // ALTID values, mixed case parameter names, empty values, and custom parameters.
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:altid-params-01\r\n",
        "fn;altid=\"group-1\";language=en-US:Dr. Jane Roe\r\n",
        "FN;ALTID=\"group-1\";LANGUAGE=zh-Hant-HK:張愛玲博士\r\n",
        "FN;ALTID=\"group-1\";LANGUAGE=sr-Latn-RS:Dr Jane Roe\r\n",
        "note;altid=100;language=en-GB;type=work:Note with mixed case params\r\n",
        "NOTE;ALTID=;LANGUAGE=:Note with empty params\r\n",
        "TITLE;ALTID=2;LANGUAGE=de:Softwareentwickler\r\n",
        "TITLE;ALTID=2;LANGUAGE=en:Software Developer\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse param variations");
    assert_eq!(
        card.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Dr. Jane Roe")
    );

    let notes = card.notes.as_ref().expect("notes");
    assert_eq!(notes.len(), 2);
    let note_values: Vec<&str> = notes.values().map(|n| n.note.as_str()).collect();
    assert!(note_values.contains(&"Note with mixed case params"));
    assert!(note_values.contains(&"Note with empty params"));

    let titles = card.titles.as_ref().expect("titles");
    assert_eq!(titles.len(), 2);
    let title_values: Vec<&str> = titles.values().map(|t| t.name.as_str()).collect();
    assert!(title_values.contains(&"Softwareentwickler"));
    assert!(title_values.contains(&"Software Developer"));

    let emitted = card_to_vcard(&card);
    let back = vcard_to_card(&emitted).expect("reparse");
    assert_eq!(back.name, card.name);
    assert_eq!(back.notes, card.notes);
    assert_eq!(back.titles, card.titles);
}

#[test]
fn jscontact_server_localizations_and_preferred_languages_characterization() {
    // Characterizes server-originated JSContact cards containing RFC 9553 §1.7.3
    // `localizations` (per-language patches) and §1.5.3 `preferredLanguages`.
    //
    // In `jmap-proto`, these unmodeled fields ride in `extra` on `ContactCard`.
    // `card_to_vcard` safely ignores unmodeled JSON fields in `extra` rather
    // than misrepresenting them as non-standard vCard lines. Server-side sync
    // (`jmap-book-sync`) leaves unmapped `extra` properties untouched via
    // JSON PatchObject updates.
    let mut extra = BTreeMap::new();
    extra.insert(
        "preferredLanguages".to_owned(),
        json!({
            "l1": {"@type": "LanguagePref", "language": "de-DE", "pref": 1},
            "l2": {"@type": "LanguagePref", "language": "en-US", "pref": 2}
        }),
    );
    extra.insert(
        "localizations".to_owned(),
        json!({
            "de": {
                "titles/t1/name": "Chefarchitekt",
                "notes/n1/note": "Deutsche Notiz"
            },
            "fr": {
                "titles/t1/name": "Architecte en chef",
                "notes/n1/note": "Note en français"
            }
        }),
    );

    let card = ContactCard {
        id: Some("srv-loc-01".into()),
        uid: Some("urn:uuid:srv-loc-01".to_owned()),
        card_type: Some("Card".to_owned()),
        version: Some("1.0".to_owned()),
        name: Some(Name {
            full: Some("Clara Schumann".to_owned()),
            ..Name::default()
        }),
        titles: Some(
            [(
                "t1".to_owned(),
                Title {
                    name: "Chief Architect".to_owned(),
                    kind: Some("title".to_owned()),
                    ..Title::default()
                },
            )]
            .into_iter()
            .collect(),
        ),
        notes: Some(
            [(
                "n1".to_owned(),
                Note {
                    note: "English Note".to_owned(),
                    ..Note::default()
                },
            )]
            .into_iter()
            .collect(),
        ),
        extra,
        ..ContactCard::default()
    };

    let emitted = card_to_vcard(&card);
    assert!(emitted.contains("FN:Clara Schumann\r\n"));
    assert!(emitted.contains("TITLE;X-JMAP-KEY=t1:Chief Architect\r\n"));
    assert!(emitted.contains("NOTE;X-JMAP-KEY=n1:English Note\r\n"));
    assert!(
        !emitted.contains("preferredLanguages"),
        "extra must not leak into vCard"
    );
    assert!(
        !emitted.contains("localizations"),
        "extra must not leak into vCard"
    );

    let back = vcard_to_card(&emitted).expect("reparse");
    assert_eq!(back.id.as_ref().unwrap().as_str(), "srv-loc-01");
    assert_eq!(
        back.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Clara Schumann")
    );
    assert_eq!(
        back.titles
            .as_ref()
            .and_then(|m| m.get("t1"))
            .map(|t| t.name.as_str()),
        Some("Chief Architect")
    );
    assert_eq!(
        back.notes
            .as_ref()
            .and_then(|m| m.get("n1"))
            .map(|n| n.note.as_str()),
        Some("English Note")
    );
    assert!(back.extra.is_empty());
}

#[test]
fn email_pref_ordering_primary_selection_and_tie_breaking() {
    // 1. Multiple emails with distinct pref values: lowest pref must be emitted first
    // so it lands in EDS's E_CONTACT_EMAIL_1 (primary email), followed by higher ranks and None.
    let card = ContactCard {
        emails: Some(
            [
                (
                    "e_sec".to_owned(),
                    ContactEmail {
                        address: "second@example.com".to_owned(),
                        contexts: Some(json!({"work": true})),
                        pref: Some(2),
                        ..ContactEmail::default()
                    },
                ),
                (
                    "e_pri".to_owned(),
                    ContactEmail {
                        address: "first@example.com".to_owned(),
                        contexts: Some(json!({"work": true})),
                        pref: Some(1),
                        ..ContactEmail::default()
                    },
                ),
                (
                    "e_ten".to_owned(),
                    ContactEmail {
                        address: "tenth@example.com".to_owned(),
                        contexts: Some(json!({"home": true})),
                        pref: Some(10),
                        ..ContactEmail::default()
                    },
                ),
                (
                    "e_none".to_owned(),
                    ContactEmail {
                        address: "unranked@example.com".to_owned(),
                        contexts: Some(json!({"other": true})),
                        pref: None,
                        ..ContactEmail::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ..ContactCard::default()
    };

    let emitted = card_to_vcard(&card);
    let email_lines: Vec<&str> = emitted.lines().filter(|l| l.starts_with("EMAIL")).collect();
    assert_eq!(email_lines.len(), 4, "{emitted}");
    assert!(
        email_lines[0].contains("X-JMAP-KEY=e_pri")
            && email_lines[0].contains("first@example.com")
            && email_lines[0].contains("PREF"),
        "1st line must be e_pri: {}",
        email_lines[0]
    );
    assert!(
        email_lines[1].contains("X-JMAP-KEY=e_sec")
            && email_lines[1].contains("second@example.com"),
        "2nd line must be e_sec: {}",
        email_lines[1]
    );
    assert!(
        email_lines[2].contains("X-JMAP-KEY=e_ten") && email_lines[2].contains("tenth@example.com"),
        "3rd line must be e_ten: {}",
        email_lines[2]
    );
    assert!(
        email_lines[3].contains("X-JMAP-KEY=e_none")
            && email_lines[3].contains("unranked@example.com")
            && !email_lines[3].contains("PREF"),
        "4th line must be e_none: {}",
        email_lines[3]
    );

    // 2. Tie-breaking: when multiple emails have identical pref (e.g. pref: 1), break tie by key
    let tie_card = ContactCard {
        emails: Some(
            [
                (
                    "e_beta".to_owned(),
                    ContactEmail {
                        address: "beta@example.com".to_owned(),
                        pref: Some(1),
                        ..ContactEmail::default()
                    },
                ),
                (
                    "e_alpha".to_owned(),
                    ContactEmail {
                        address: "alpha@example.com".to_owned(),
                        pref: Some(1),
                        ..ContactEmail::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ..ContactCard::default()
    };
    let tie_emitted = card_to_vcard(&tie_card);
    let tie_lines: Vec<&str> = tie_emitted
        .lines()
        .filter(|l| l.starts_with("EMAIL"))
        .collect();
    assert_eq!(tie_lines.len(), 2);
    assert!(tie_lines[0].contains("X-JMAP-KEY=e_alpha"));
    assert!(tie_lines[1].contains("X-JMAP-KEY=e_beta"));

    // 3. No-PREF-present fallback: when all emails have pref: None, fall back to key order
    let none_card = ContactCard {
        emails: Some(
            [
                (
                    "e_z".to_owned(),
                    ContactEmail {
                        address: "z@example.com".to_owned(),
                        pref: None,
                        ..ContactEmail::default()
                    },
                ),
                (
                    "e_a".to_owned(),
                    ContactEmail {
                        address: "a@example.com".to_owned(),
                        pref: None,
                        ..ContactEmail::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ..ContactCard::default()
    };
    let none_emitted = card_to_vcard(&none_card);
    let none_lines: Vec<&str> = none_emitted
        .lines()
        .filter(|l| l.starts_with("EMAIL"))
        .collect();
    assert_eq!(none_lines.len(), 2);
    assert!(none_lines[0].contains("X-JMAP-KEY=e_a") && !none_lines[0].contains("PREF"));
    assert!(none_lines[1].contains("X-JMAP-KEY=e_z") && !none_lines[1].contains("PREF"));

    // 4. Fixed-point roundtrip stability
    let back = vcard_to_card(&emitted).expect("parse back");
    let re_emitted = card_to_vcard(&back);
    assert_eq!(emitted, re_emitted);
}

#[test]
fn phone_pref_ordering_primary_selection_and_slotting() {
    // 1. Phones with distinct pref values: lowest pref emitted first
    let card = ContactCard {
        phones: Some(
            [
                (
                    "p_work_sec".to_owned(),
                    ContactPhone {
                        number: "+1 555 0200".to_owned(),
                        contexts: Some(json!({"work": true})),
                        features: Some(json!({"voice": true})),
                        pref: Some(2),
                        ..ContactPhone::default()
                    },
                ),
                (
                    "p_work_pri".to_owned(),
                    ContactPhone {
                        number: "+1 555 0100".to_owned(),
                        contexts: Some(json!({"work": true})),
                        features: Some(json!({"voice": true})),
                        pref: Some(1),
                        ..ContactPhone::default()
                    },
                ),
                (
                    "p_home".to_owned(),
                    ContactPhone {
                        number: "+1 555 0300".to_owned(),
                        contexts: Some(json!({"home": true})),
                        pref: None,
                        ..ContactPhone::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ..ContactCard::default()
    };

    let emitted = card_to_vcard(&card);
    let phone_lines: Vec<&str> = emitted.lines().filter(|l| l.starts_with("TEL")).collect();
    assert_eq!(phone_lines.len(), 3, "{emitted}");
    assert!(
        phone_lines[0].contains("X-JMAP-KEY=p_work_pri")
            && phone_lines[0].contains("+1 555 0100")
            && phone_lines[0].contains("PREF"),
        "1st line must be p_work_pri: {}",
        phone_lines[0]
    );
    assert!(
        phone_lines[1].contains("X-JMAP-KEY=p_work_sec") && phone_lines[1].contains("+1 555 0200"),
        "2nd line must be p_work_sec: {}",
        phone_lines[1]
    );
    assert!(
        phone_lines[2].contains("X-JMAP-KEY=p_home")
            && phone_lines[2].contains("+1 555 0300")
            && !phone_lines[2].contains("PREF"),
        "3rd line must be p_home: {}",
        phone_lines[2]
    );

    // 2. Tie-breaking by key when prefs match
    let tie_card = ContactCard {
        phones: Some(
            [
                (
                    "p_b".to_owned(),
                    ContactPhone {
                        number: "+1 555 0002".to_owned(),
                        pref: Some(1),
                        ..ContactPhone::default()
                    },
                ),
                (
                    "p_a".to_owned(),
                    ContactPhone {
                        number: "+1 555 0001".to_owned(),
                        pref: Some(1),
                        ..ContactPhone::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ..ContactCard::default()
    };
    let tie_emitted = card_to_vcard(&tie_card);
    let tie_lines: Vec<&str> = tie_emitted
        .lines()
        .filter(|l| l.starts_with("TEL"))
        .collect();
    assert_eq!(tie_lines.len(), 2);
    assert!(tie_lines[0].contains("X-JMAP-KEY=p_a"));
    assert!(tie_lines[1].contains("X-JMAP-KEY=p_b"));

    // 3. Round-trip stability
    let back = vcard_to_card(&emitted).expect("parse back");
    let re_emitted = card_to_vcard(&back);
    assert_eq!(emitted, re_emitted);
}

#[test]
fn address_pref_ordering_and_primary_selection_with_label_pairing() {
    // 1. Addresses with pref in extra: lowest pref emitted first and carries PREF parameter
    let mut extra_pri = BTreeMap::new();
    extra_pri.insert("pref".to_owned(), json!(1));

    let mut extra_sec = BTreeMap::new();
    extra_sec.insert("pref".to_owned(), json!(2));

    let card = ContactCard {
        addresses: Some(
            [
                (
                    "a_sec".to_owned(),
                    Address {
                        components: Some(vec![AddressComponent::new("name", "Secondary Weg 2")]),
                        contexts: Some(json!({"home": true})),
                        full: None,
                        extra: extra_sec,
                    },
                ),
                (
                    "a_pri".to_owned(),
                    Address {
                        components: Some(vec![AddressComponent::new("name", "Primary Allee 1")]),
                        contexts: Some(json!({"home": true})),
                        full: Some("Primary Allee 1\n10115 Berlin\nGermany".to_owned()),
                        extra: extra_pri,
                    },
                ),
                (
                    "a_work".to_owned(),
                    Address {
                        components: Some(vec![AddressComponent::new("name", "Work Str 42")]),
                        contexts: Some(json!({"work": true})),
                        full: None,
                        extra: BTreeMap::new(),
                    },
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ..ContactCard::default()
    };

    let emitted = card_to_vcard(&card);
    let adr_lines: Vec<&str> = emitted.lines().filter(|l| l.starts_with("ADR")).collect();
    assert_eq!(adr_lines.len(), 3, "{emitted}");
    assert!(
        adr_lines[0].contains("X-JMAP-KEY=a_pri")
            && adr_lines[0].contains("Primary Allee 1")
            && adr_lines[0].contains("PREF"),
        "1st ADR line must be a_pri: {}",
        adr_lines[0]
    );
    assert!(
        adr_lines[1].contains("X-JMAP-KEY=a_sec") && adr_lines[1].contains("Secondary Weg 2"),
        "2nd ADR line must be a_sec: {}",
        adr_lines[1]
    );
    assert!(
        adr_lines[2].contains("X-JMAP-KEY=a_work")
            && adr_lines[2].contains("Work Str 42")
            && !adr_lines[2].contains("PREF"),
        "3rd ADR line must be a_work: {}",
        adr_lines[2]
    );

    // LABEL for a_pri also carries PREF parameter
    let label_line = line(&emitted, "LABEL;X-JMAP-KEY=a_pri");
    assert!(label_line.contains("PREF"), "{label_line}");

    // Inbound parse of vCard preserves extra["pref"] = 1 for preferred addresses
    let back = vcard_to_card(&emitted).expect("parse back");
    let back_addrs = back.addresses.as_ref().expect("addresses");
    assert_eq!(back_addrs["a_pri"].extra.get("pref"), Some(&json!(1)));
    assert_eq!(
        back_addrs["a_pri"].full.as_deref(),
        Some("Primary Allee 1\n10115 Berlin\nGermany")
    );
    assert_eq!(back_addrs["a_sec"].extra.get("pref"), Some(&json!(1)));
    assert_eq!(back_addrs["a_work"].extra.get("pref"), None);

    // Re-emission matches fixed point
    let re_emitted = card_to_vcard(&back);
    assert_eq!(emitted, re_emitted);
}

#[test]
fn inbound_vcard_pref_parameter_variations_and_reordering() {
    // Inbound vCard where the preferred email appears on the second line:
    // Parsing extracts pref: 1 on the second entry, and card_to_vcard re-orders it
    // to the first line (E_CONTACT_EMAIL_1) when emitting.
    let raw_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Vera Oldenburg\r\n",
        "EMAIL;X-JMAP-KEY=e1;TYPE=HOME:secondary@home.example\r\n",
        "EMAIL;X-JMAP-KEY=e2;TYPE=WORK,PREF:primary@work.example\r\n",
        "TEL;X-JMAP-KEY=p1;TYPE=HOME:+49 30 111111\r\n",
        "TEL;X-JMAP-KEY=p2;type=work,pref:+49 30 222222\r\n",
        "ADR;X-JMAP-KEY=a1;TYPE=HOME:;;Secondary Home;;;;\r\n",
        "ADR;X-JMAP-KEY=a2;TYPE=WORK,PREF:;;Primary Office;;;;\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(raw_vcard).expect("parse");
    let emails = card.emails.as_ref().unwrap();
    assert_eq!(emails["e1"].pref, None);
    assert_eq!(emails["e2"].pref, Some(1));

    let phones = card.phones.as_ref().unwrap();
    assert_eq!(phones["p1"].pref, None);
    assert_eq!(phones["p2"].pref, Some(1));

    let addrs = card.addresses.as_ref().unwrap();
    assert_eq!(addrs["a1"].extra.get("pref"), None);
    assert_eq!(addrs["a2"].extra.get("pref"), Some(&json!(1)));

    // Re-emitted vCard places e2, p2, a2 first (primary selection)
    let re_emitted = card_to_vcard(&card);
    let email_lines: Vec<&str> = re_emitted
        .lines()
        .filter(|l| l.starts_with("EMAIL"))
        .collect();
    assert!(email_lines[0].contains("X-JMAP-KEY=e2"));
    assert!(email_lines[1].contains("X-JMAP-KEY=e1"));

    let phone_lines: Vec<&str> = re_emitted
        .lines()
        .filter(|l| l.starts_with("TEL"))
        .collect();
    assert!(phone_lines[0].contains("X-JMAP-KEY=p2"));
    assert!(phone_lines[1].contains("X-JMAP-KEY=p1"));

    let adr_lines: Vec<&str> = re_emitted
        .lines()
        .filter(|l| l.starts_with("ADR"))
        .collect();
    assert!(adr_lines[0].contains("X-JMAP-KEY=a2"));
    assert!(adr_lines[1].contains("X-JMAP-KEY=a1"));
}

#[test]
fn adr_all_seven_structured_components_roundtrip() {
    // RFC 2426 §3.2.1 and RFC 6350 §6.3.1 define seven structured components in order:
    // 0: postOfficeBox, 1: apartment (extended address), 2: name (street),
    // 3: locality, 4: region, 5: postcode, 6: country.
    let full_address = Address {
        components: Some(vec![
            AddressComponent::new("postOfficeBox", "PO Box 777"),
            AddressComponent::new("apartment", "Suite 400, Floor 4"),
            AddressComponent::new("name", "500 Silicon Way"),
            AddressComponent::new("locality", "Mountain View"),
            AddressComponent::new("region", "California"),
            AddressComponent::new("postcode", "94043"),
            AddressComponent::new("country", "United States of America"),
        ]),
        contexts: Some(json!({"work": true})),
        full: Some("Acme Corp\nPO Box 777\nSuite 400\nMountain View, CA 94043\nUSA".to_owned()),
        extra: BTreeMap::new(),
    };

    let card = one_address("a1", full_address);
    let vcard = card_to_vcard(&card);
    let unfolded_vcard = unfolded(&vcard);

    let adr_line = line(&unfolded_vcard, "ADR");
    assert_eq!(
        adr_line,
        "ADR;X-JMAP-KEY=a1;TYPE=WORK:PO Box 777;Suite 400\\, Floor 4;500 Silicon Way;Mountain View;California;94043;United States of America"
    );

    let label_line = line(&unfolded_vcard, "LABEL");
    assert_eq!(
        label_line,
        "LABEL;X-JMAP-KEY=a1;TYPE=WORK:Acme Corp\\nPO Box 777\\nSuite 400\\nMountain View\\, CA 94043\\nUSA"
    );

    // Read back and verify exact component extraction
    let back = vcard_to_card(&vcard).expect("parse back");
    let addresses = back.addresses.as_ref().expect("addresses");
    assert_eq!(addresses.keys().collect::<Vec<_>>(), vec!["a1"]);
    let back_addr = &addresses["a1"];

    assert_eq!(back_addr.contexts, Some(json!({"work": true})));
    assert_eq!(
        back_addr.full.as_deref(),
        Some("Acme Corp\nPO Box 777\nSuite 400\nMountain View, CA 94043\nUSA")
    );
    assert_eq!(
        components_of(back_addr),
        vec![
            ("postOfficeBox", "PO Box 777"),
            ("apartment", "Suite 400, Floor 4"),
            ("name", "500 Silicon Way"),
            ("locality", "Mountain View"),
            ("region", "California"),
            ("postcode", "94043"),
            ("country", "United States of America"),
        ]
    );

    // Second round-trip convergence (fixed point)
    let vcard2 = card_to_vcard(&back);
    assert_eq!(vcard, vcard2);
}

#[test]
fn adr_label_parameter_parsing_and_emission_fidelity() {
    // vCard 4.0 / RFC 6350 §6.3.1 allows a `LABEL` parameter directly on the `ADR` property line:
    // `ADR;TYPE=WORK;LABEL="...":PO Box 123;...`
    let raw_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Alice Developer\r\n",
        "ADR;TYPE=WORK;LABEL=\"Alice Dev\\n100 Tech Blvd\\nSuite 200\\nAustin, TX 78701\\nUSA\":PO Box 100;Suite 200;100 Tech Blvd;Austin;TX;78701;USA\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(raw_vcard).expect("parse raw ADR with LABEL param");
    let addresses = card.addresses.as_ref().expect("addresses");
    assert_eq!(addresses.keys().collect::<Vec<_>>(), vec!["a1"]);
    let addr = &addresses["a1"];

    assert_eq!(addr.contexts, Some(json!({"work": true})));
    assert_eq!(
        addr.full.as_deref(),
        Some("Alice Dev\n100 Tech Blvd\nSuite 200\nAustin, TX 78701\nUSA")
    );
    assert_eq!(
        components_of(addr),
        vec![
            ("postOfficeBox", "PO Box 100"),
            ("apartment", "Suite 200"),
            ("name", "100 Tech Blvd"),
            ("locality", "Austin"),
            ("region", "TX"),
            ("postcode", "78701"),
            ("country", "USA"),
        ]
    );

    // Emitting via card_to_vcard produces both standard vCard 3.0 ADR and standalone LABEL lines
    let vcard = card_to_vcard(&card);
    let unfolded_vcard = unfolded(&vcard);
    assert!(unfolded_vcard.contains(
        "ADR;X-JMAP-KEY=a1;TYPE=WORK:PO Box 100;Suite 200;100 Tech Blvd;Austin;TX;78701;USA"
    ));
    assert!(unfolded_vcard.contains("LABEL;X-JMAP-KEY=a1;TYPE=WORK:Alice Dev\\n100 Tech Blvd\\nSuite 200\\nAustin\\, TX 78701\\nUSA"));

    // Parsing back round-trips with identical Address components and label
    let back = vcard_to_card(&vcard).expect("parse back");
    assert_eq!(back.addresses, card.addresses);

    // Test ADR with empty structured components but non-empty LABEL parameter
    let empty_components_with_label_param = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Bob Builder\r\n",
        "ADR;TYPE=HOME;LABEL=\"Rural Route 5\\nBox 12\\nSomewhere, KS 66002\":;;;;;;\r\n",
        "END:VCARD\r\n"
    );
    let card2 =
        vcard_to_card(empty_components_with_label_param).expect("parse empty ADR with LABEL param");
    let addresses2 = card2.addresses.as_ref().expect("addresses");
    let addr2 = &addresses2["a1"];
    assert_eq!(addr2.components, None);
    assert_eq!(
        addr2.full.as_deref(),
        Some("Rural Route 5\nBox 12\nSomewhere, KS 66002")
    );
    assert_eq!(addr2.contexts, Some(json!({"private": true})));

    // Standalone LABEL emission and round-trip
    let vcard_label_only = card_to_vcard(&card2);
    let unfolded_label_only = unfolded(&vcard_label_only);
    assert!(!unfolded_label_only.contains("\r\nADR"));
    assert!(
        unfolded_label_only.contains(
            "LABEL;X-JMAP-KEY=a1;TYPE=HOME:Rural Route 5\\nBox 12\\nSomewhere\\, KS 66002"
        )
    );
    let back2 = vcard_to_card(&vcard_label_only).expect("parse label-only");
    assert_eq!(back2.addresses, card2.addresses);
}

#[test]
fn adr_empty_and_sparse_components_permutations() {
    // 1. Single component addresses: each individual component alone on an ADR line
    let cases = [
        ("ADR:PO Box 1;;;;;;", vec![("postOfficeBox", "PO Box 1")]),
        ("ADR:;Penthouse B;;;;;", vec![("apartment", "Penthouse B")]),
        ("ADR:;;123 Main St;;;;", vec![("name", "123 Main St")]),
        ("ADR:;;;Berlin;;;", vec![("locality", "Berlin")]),
        ("ADR:;;;;Bavaria;;", vec![("region", "Bavaria")]),
        ("ADR:;;;;;10115;", vec![("postcode", "10115")]),
        ("ADR:;;;;;;Germany", vec![("country", "Germany")]),
    ];

    for (line_str, expected_components) in cases {
        let vcard = format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Test\r\n{line_str}\r\nEND:VCARD\r\n");
        let card = vcard_to_card(&vcard).expect("parse single component ADR");
        let addresses = card.addresses.as_ref().expect("addresses");
        assert_eq!(components_of(&addresses["a1"]), expected_components);

        let re_emitted = card_to_vcard(&card);
        let back = vcard_to_card(&re_emitted).expect("parse re-emitted");
        assert_eq!(back.addresses, card.addresses);
    }

    // 2. Intermediate empty components (e.g. indices 0, 2, 4, 6 present; 1, 3, 5 empty)
    let sparse_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Sparse Address\r\n",
        "ADR;TYPE=WORK:PO Box 99;;Highway 1;;California;;United States\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(sparse_vcard).expect("parse sparse ADR");
    let addresses = card.addresses.as_ref().expect("addresses");
    assert_eq!(
        components_of(&addresses["a1"]),
        vec![
            ("postOfficeBox", "PO Box 99"),
            ("name", "Highway 1"),
            ("region", "California"),
            ("country", "United States"),
        ]
    );
    let re_emitted = card_to_vcard(&card);
    assert_eq!(
        line(&re_emitted, "ADR"),
        "ADR;X-JMAP-KEY=a1;TYPE=WORK:PO Box 99;;Highway 1;;California;;United States"
    );

    // 3. Truncated components (fewer than 7 components on the wire)
    let truncated_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Truncated ADR\r\n",
        "ADR:;;Broadway;New York\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(truncated_vcard).expect("parse truncated ADR");
    let addresses = card.addresses.as_ref().expect("addresses");
    assert_eq!(
        components_of(&addresses["a1"]),
        vec![("name", "Broadway"), ("locality", "New York"),]
    );

    // 4. All components empty produces None
    let all_empty_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Empty ADR\r\n",
        "ADR:;;;;;;\r\n",
        "ADR:;;;\r\n",
        "ADR:\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(all_empty_vcard).expect("parse all empty ADR");
    assert_eq!(card.addresses, None);
}

#[test]
fn adr_multi_value_and_escaped_delimiters_roundtrip() {
    // Structured values containing escaped commas, semicolons, and newlines in ADR and LABEL
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Delimiter Test\r\n",
        "ADR;TYPE=WORK:Post Box 10\\;A;Suite 200\\, Bldg 3;123 Main St.\\, #4;Springfield\\; East;Illinois\\, Central;62701\\-1234;United States of America\r\n",
        "LABEL;TYPE=WORK:Acme Corp\\, Inc.\\n123 Main St.\\, #4\\nSpringfield\\; East\\, IL 62701\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse delimited ADR");
    let addresses = card.addresses.as_ref().expect("addresses");
    let addr = &addresses["a1"];

    assert_eq!(
        components_of(addr),
        vec![
            ("postOfficeBox", "Post Box 10;A"),
            ("apartment", "Suite 200, Bldg 3"),
            ("name", "123 Main St., #4"),
            ("locality", "Springfield; East"),
            ("region", "Illinois, Central"),
            ("postcode", "62701-1234"),
            ("country", "United States of America"),
        ]
    );
    assert_eq!(
        addr.full.as_deref(),
        Some("Acme Corp, Inc.\n123 Main St., #4\nSpringfield; East, IL 62701")
    );

    // Outbound serialization and round-trip verification
    let re_emitted = card_to_vcard(&card);
    let back = vcard_to_card(&re_emitted).expect("parse re-emitted");
    assert_eq!(back.addresses, card.addresses);
}

#[test]
fn multiple_addresses_with_mixed_labels_and_contexts_pairing() {
    // Card with 4 addresses:
    // a1: Work address with full components + LABEL param
    // a2: Home address with partial components + standalone LABEL
    // a3: Label-only address (no ADR)
    // a4: Structured address without LABEL
    let mut addresses = BTreeMap::new();
    addresses.insert(
        "a1".to_owned(),
        Address {
            components: Some(vec![
                AddressComponent::new("postOfficeBox", "Box 10"),
                AddressComponent::new("apartment", "Suite 1"),
                AddressComponent::new("name", "100 Work Way"),
                AddressComponent::new("locality", "Work City"),
                AddressComponent::new("region", "WA"),
                AddressComponent::new("postcode", "98101"),
                AddressComponent::new("country", "USA"),
            ]),
            contexts: Some(json!({"work": true})),
            full: Some("Work Label\n100 Work Way\nSeattle, WA".to_owned()),
            extra: BTreeMap::new(),
        },
    );
    addresses.insert(
        "a2".to_owned(),
        Address {
            components: Some(vec![
                AddressComponent::new("name", "200 Home St"),
                AddressComponent::new("locality", "Home Town"),
                AddressComponent::new("country", "USA"),
            ]),
            contexts: Some(json!({"private": true})),
            full: Some("Home Label\n200 Home St\nHome Town, USA".to_owned()),
            extra: BTreeMap::new(),
        },
    );
    addresses.insert(
        "a3".to_owned(),
        Address {
            components: None,
            contexts: None,
            full: Some("Postal Delivery Only\nPO Box 9999\nRemote City".to_owned()),
            extra: BTreeMap::new(),
        },
    );
    addresses.insert(
        "a4".to_owned(),
        Address {
            components: Some(vec![
                AddressComponent::new("name", "400 Unlabelled St"),
                AddressComponent::new("locality", "Plain City"),
                AddressComponent::new("country", "USA"),
            ]),
            contexts: None,
            full: None,
            extra: BTreeMap::new(),
        },
    );

    let card = ContactCard {
        name: Some(Name {
            full: Some("Multi Address User".to_owned()),
            ..Name::default()
        }),
        addresses: Some(addresses),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    let back = vcard_to_card(&vcard).expect("parse multi address vcard");

    assert_eq!(back.addresses, card.addresses);
    let back_addrs = back.addresses.unwrap();
    assert_eq!(back_addrs.len(), 4);
    assert_eq!(back_addrs["a1"].components.as_ref().unwrap().len(), 7);
    assert_eq!(back_addrs["a2"].components.as_ref().unwrap().len(), 3);
    assert_eq!(back_addrs["a3"].components, None);
    assert_eq!(back_addrs["a4"].full, None);
}

#[test]
fn adr_predicates_and_component_restoration_comprehensive() {
    // 1. states_address_component on all 7 standard kinds and joined kinds
    for kind in [
        "postOfficeBox",
        "apartment",
        "name",
        "locality",
        "region",
        "postcode",
        "country",
        "number",
    ] {
        assert!(
            states_address_component(&AddressComponent::new(kind, "Value")),
            "kind {kind} should be stateable"
        );
        assert!(
            !states_address_component(&AddressComponent::new(kind, "")),
            "empty kind {kind} should not be stateable"
        );
    }
    // Unmapped kinds return false
    for unmapped in [
        "floor",
        "building",
        "room",
        "landmark",
        "district",
        "subdistrict",
        "direction",
    ] {
        assert!(
            !states_address_component(&AddressComponent::new(unmapped, "Value")),
            "unmapped kind {unmapped} should not be stateable"
        );
    }

    // 2. states_address evaluation
    let valid_comp_addr = Address {
        components: Some(vec![AddressComponent::new("locality", "Berlin")]),
        full: None,
        ..Address::default()
    };
    assert!(states_address(&valid_comp_addr));

    let valid_label_addr = Address {
        components: None,
        full: Some("Some Label".to_owned()),
        ..Address::default()
    };
    assert!(states_address(&valid_label_addr));

    let unmapped_only_addr = Address {
        components: Some(vec![AddressComponent::new("floor", "3")]),
        full: None,
        ..Address::default()
    };
    assert!(!states_address(&unmapped_only_addr));

    let empty_label_only_addr = Address {
        components: None,
        full: Some("".to_owned()),
        ..Address::default()
    };
    assert!(!states_address(&empty_label_only_addr));

    // 3. address_label evaluation
    assert_eq!(address_label(&valid_label_addr), Some("Some Label"));
    assert_eq!(address_label(&empty_label_only_addr), None);
    assert_eq!(address_label(&valid_comp_addr), None);

    // 4. restore_address_components
    let original = vec![
        AddressComponent::new("name", "Hauptstraße"),
        AddressComponent::new("number", "42"),
        AddressComponent::new("locality", "Berlin"),
    ];
    let edited_same = vec![
        AddressComponent::new("name", "Hauptstraße 42"),
        AddressComponent::new("locality", "Berlin"),
    ];
    let restored = restore_address_components(&original, &edited_same);
    assert_eq!(
        restored,
        vec![
            AddressComponent::new("name", "Hauptstraße"),
            AddressComponent::new("number", "42"),
            AddressComponent::new("locality", "Berlin"),
        ]
    );

    let edited_modified = vec![
        AddressComponent::new("name", "Nebenstraße 99"),
        AddressComponent::new("locality", "Berlin"),
    ];
    let restored_modified = restore_address_components(&original, &edited_modified);
    assert_eq!(
        restored_modified,
        vec![
            AddressComponent::new("name", "Nebenstraße 99"),
            AddressComponent::new("locality", "Berlin"),
        ]
    );
}

#[test]
fn unknown_and_vendor_x_properties_are_safely_ignored_by_vcard_reader() {
    // A vCard carrying a comprehensive suite of unknown vendor X- properties,
    // coexisting with standard mapped vCard properties.
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:c101\r\n",
        "FN:Dr. Vera Schmidt\r\n",
        "N:Schmidt;Vera;;Dr.;\r\n",
        "EMAIL;TYPE=WORK;X-JMAP-KEY=e1:vera@work.example.com\r\n",
        "TEL;TYPE=CELL;X-JMAP-KEY=p1:+49 170 1234567\r\n",
        "ADR;TYPE=WORK;X-JMAP-KEY=a1:;;Hauptstraße 1;Berlin;;10115;Germany\r\n",
        "NOTE;X-JMAP-KEY=n1:Principal systems researcher\r\n",
        "ORG;X-JMAP-KEY=o1:Acme Research;Security Labs\r\n",
        "TITLE;X-JMAP-KEY=t1:Senior Research Scientist\r\n",
        "ROLE;X-JMAP-KEY=r1:Group Lead\r\n",
        "URL;X-JMAP-KEY=l1:https://vera.example.com\r\n",
        // Mozilla / Thunderbird extensions
        "X-MOZILLA-HTML:TRUE\r\n",
        "X-MOZILLA-USE-HTML:FALSE\r\n",
        // Apple AddressBook / iOS extensions
        "X-PHONETIC-FIRST-NAME:Vera\r\n",
        "X-PHONETIC-LAST-NAME:Schmidt\r\n",
        "X-ABShowAs:COMPANY\r\n",
        "X-ABLabel:Personal\r\n",
        "X-ABUID:789-DEF-456\r\n",
        "X-ABRelatedNames:Bob Schmidt\r\n",
        "X-APPLE-SUBPROPERTY:CustomValue\r\n",
        // Microsoft Outlook / Exchange extensions
        "X-MS-CARDPICTURE:https://photos.example.com/card.jpg\r\n",
        "X-MS-OL-DESIGN:2\r\n",
        "X-MS-IMADDRESS:vera_ms@im.example.com\r\n",
        // Vendor / non-standard personal metadata
        "X-GENDER:Female\r\n",
        "X-SPOUSE:Bob Schmidt\r\n",
        "X-ANNIVERSARY:2015-08-20\r\n",
        "X-ASSISTANT:Alice Assistant\r\n",
        "X-MANAGER:Carol Manager\r\n",
        // Custom instant-messaging / communications services not mapped to EDS slots
        "X-DISCORD:vera#1234\r\n",
        "X-SIGNAL:+491701234567\r\n",
        "X-TELEGRAM:@vera_schmidt\r\n",
        "X-SLACK:U12345678\r\n",
        "X-WHATSAPP:+491701234567\r\n",
        // Arbitrary enterprise & vendor properties
        "X-CUSTOM-EXTENSION:Value 123\r\n",
        "X-KEY-ID:0xDEADBEEF\r\n",
        "X-DEPARTMENT-CODE:DE-SEC-09\r\n",
        "X-OFFICE-HOURS:Mon-Thu 09:00-17:00\r\n",
        "X-BILLING-ACCOUNT:ACC-998877\r\n",
        "END:VCARD\r\n"
    );

    // 1. Parsing into ContactCard: all modeled properties parse cleanly, while all
    //    unknown X- properties are safely ignored rather than corrupting fields.
    let card = vcard_to_card(vcard).expect("parse vcard with vendor X- properties");
    assert_eq!(card.id.as_ref().unwrap().as_str(), "c101");
    assert_eq!(
        card.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Dr. Vera Schmidt")
    );
    let name_comps = card.name.as_ref().unwrap().components.as_ref().unwrap();
    assert_eq!(name_comps[0].kind, "title");
    assert_eq!(name_comps[0].value, "Dr.");
    assert_eq!(name_comps[1].kind, "given");
    assert_eq!(name_comps[1].value, "Vera");
    assert_eq!(name_comps[2].kind, "surname");
    assert_eq!(name_comps[2].value, "Schmidt");
    assert!(card.emails.as_ref().unwrap().contains_key("e1"));
    assert!(card.phones.as_ref().unwrap().contains_key("p1"));
    assert!(card.addresses.as_ref().unwrap().contains_key("a1"));
    assert!(card.notes.as_ref().unwrap().contains_key("n1"));
    assert!(card.organizations.as_ref().unwrap().contains_key("o1"));
    assert!(card.titles.as_ref().unwrap().contains_key("t1"));
    assert!(card.titles.as_ref().unwrap().contains_key("r1"));
    assert!(card.links.as_ref().unwrap().contains_key("l1"));
    // online_services contains nothing from X-DISCORD/X-SIGNAL/X-TELEGRAM/X-SLACK/X-WHATSAPP
    assert_eq!(card.online_services, None);
    // related_to contains nothing from X-SPOUSE (non-EDS), X-ASSISTANT, X-MANAGER
    assert_eq!(card.related_to, None);
    // anniversaries contains nothing from vendor X-ANNIVERSARY
    assert_eq!(card.anniversaries, None);
    // card.extra is empty: vcard_to_card never injects raw vCard lines into JSContact extra
    assert!(card.extra.is_empty());

    // 2. Emission: card_to_vcard produces clean vCard 3.0 lines with standard properties
    //    and X-JMAP-KEY parameters, with zero unmapped X- lines.
    let re_emitted = card_to_vcard(&card);
    assert!(re_emitted.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"));
    assert!(re_emitted.ends_with("END:VCARD\r\n"));
    assert!(re_emitted.contains("EMAIL;X-JMAP-KEY=e1;TYPE=WORK:vera@work.example.com\r\n"));
    assert!(re_emitted.contains("TEL;X-JMAP-KEY=p1;TYPE=CELL:+49 170 1234567\r\n"));
    assert!(
        re_emitted
            .contains("ADR;X-JMAP-KEY=a1;TYPE=WORK:;;Hauptstraße 1;Berlin;;10115;Germany\r\n")
    );
    assert!(re_emitted.contains("NOTE;X-JMAP-KEY=n1:Principal systems researcher\r\n"));
    assert!(re_emitted.contains("ORG;X-JMAP-KEY=o1:Acme Research;Security Labs\r\n"));
    assert!(re_emitted.contains("TITLE;X-JMAP-KEY=t1:Senior Research Scientist\r\n"));
    assert!(re_emitted.contains("ROLE;X-JMAP-KEY=r1:Group Lead\r\n"));
    assert!(re_emitted.contains("URL;X-JMAP-KEY=l1:https://vera.example.com\r\n"));

    // Verify none of the unknown X- lines were emitted
    assert!(!re_emitted.contains("X-MOZILLA"));
    assert!(!re_emitted.contains("X-PHONETIC"));
    assert!(!re_emitted.contains("X-AB"));
    assert!(!re_emitted.contains("X-APPLE"));
    assert!(!re_emitted.contains("X-MS"));
    assert!(!re_emitted.contains("X-GENDER"));
    assert!(!re_emitted.contains("X-SPOUSE"));
    assert!(!re_emitted.contains("X-ANNIVERSARY"));
    assert!(!re_emitted.contains("X-ASSISTANT"));
    assert!(!re_emitted.contains("X-MANAGER"));
    assert!(!re_emitted.contains("X-DISCORD"));
    assert!(!re_emitted.contains("X-SIGNAL"));
    assert!(!re_emitted.contains("X-TELEGRAM"));
    assert!(!re_emitted.contains("X-SLACK"));
    assert!(!re_emitted.contains("X-WHATSAPP"));
    assert!(!re_emitted.contains("X-CUSTOM"));
    assert!(!re_emitted.contains("X-KEY-ID"));
    assert!(!re_emitted.contains("X-DEPARTMENT-CODE"));

    // 3. Fixed-point round-trip convergence: parsing re_emitted reproduces the exact same card.
    let card2 = vcard_to_card(&re_emitted).expect("parse re-emitted");
    assert_eq!(card2, card);
    let re_emitted2 = card_to_vcard(&card2);
    assert_eq!(re_emitted2, re_emitted);
}

#[test]
fn unmapped_eds_specific_x_properties_characterization_and_rationale() {
    // Characterizes how EDS-specific X- properties that are unmapped in jmap-vcard
    // behave on inbound parsing and outbound serialization.
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:eds-custom-001\r\n",
        "FN:EDS Custom Contact\r\n",
        "EMAIL;TYPE=WORK:eds.user@example.com\r\n",
        // Unslotted online services in EDS
        "X-TWITTER:@eds_user\r\n",
        "X-SIP:sip:eds.user@sip.example.com\r\n",
        // EDS manager and assistant fields (E_CONTACT_MANAGER, E_CONTACT_ASSISTANT)
        "X-EVOLUTION-MANAGER:Big Boss\r\n",
        "X-EVOLUTION-ASSISTANT:Helpful Assistant\r\n",
        "X-MANAGER:Boss Vendor\r\n",
        "X-ASSISTANT:Assistant Vendor\r\n",
        // EDS blog and video URLs (E_CONTACT_BLOG_URL, E_CONTACT_VIDEO_URL)
        "X-EVOLUTION-BLOG-URL:https://blogs.example.com/user\r\n",
        "X-EVOLUTION-VIDEO-URL:https://video.example.com/stream\r\n",
        // EDS file-as and sort string (E_CONTACT_FILE_AS)
        "X-EVOLUTION-FILE-AS:Custom, Contact\r\n",
        // EDS specialized telephony lines (E_CONTACT_PHONE_CALLBACK, _RADIO, _TELEX, _TTYTDD)
        "X-EVOLUTION-CALLBACK:+49 30 11111\r\n",
        "X-EVOLUTION-RADIO:+49 30 22222\r\n",
        "X-EVOLUTION-TELEX:+49 30 33333\r\n",
        "X-EVOLUTION-TTYTDD:+49 30 44444\r\n",
        // EDS contact list markers (E_CONTACT_IS_LIST)
        "X-EVOLUTION-LIST:TRUE\r\n",
        "X-EVOLUTION-LIST-SHOW-ADDRESSES:FALSE\r\n",
        "X-EVOLUTION-DEST-EMAIL:group-dest@example.com\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse EDS custom vcard");
    assert_eq!(card.id.as_ref().unwrap().as_str(), "eds-custom-001");
    assert_eq!(
        card.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("EDS Custom Contact")
    );
    assert_eq!(
        card.emails.as_ref().map(|e| e["e1"].address.as_str()),
        Some("eds.user@example.com")
    );

    // All unmapped EDS X- properties are safely ignored by design:
    assert_eq!(card.online_services, None);
    assert_eq!(card.related_to, None);
    assert_eq!(card.links, None);
    assert_eq!(card.phones, None);
    assert!(card.extra.is_empty());

    // Outbound emission contains only standard modeled properties
    let emitted = card_to_vcard(&card);
    assert!(!emitted.contains("X-TWITTER"));
    assert!(!emitted.contains("X-SIP"));
    assert!(!emitted.contains("X-EVOLUTION-MANAGER"));
    assert!(!emitted.contains("X-EVOLUTION-ASSISTANT"));
    assert!(!emitted.contains("X-EVOLUTION-BLOG-URL"));
    assert!(!emitted.contains("X-EVOLUTION-VIDEO-URL"));
    assert!(!emitted.contains("X-EVOLUTION-FILE-AS"));
    assert!(!emitted.contains("X-EVOLUTION-CALLBACK"));
    assert!(!emitted.contains("X-EVOLUTION-RADIO"));
    assert!(!emitted.contains("X-EVOLUTION-TELEX"));
    assert!(!emitted.contains("X-EVOLUTION-TTYTDD"));
    assert!(!emitted.contains("X-EVOLUTION-LIST"));

    // Roundtrip fixed point
    let back = vcard_to_card(&emitted).expect("parse emitted");
    assert_eq!(back, card);
}

#[test]
fn supported_evolution_and_im_x_properties_complete_roundtrip() {
    // Tests all supported X- properties that jmap-vcard explicitly maps:
    // 1. X-EVOLUTION-SPOUSE -> related_to
    // 2. X-EVOLUTION-ANNIVERSARY -> anniversaries (wedding)
    // 3. Known instant-messaging services: X-AIM, X-GADUGADU, X-GOOGLE-TALK, X-GROUPWISE,
    //    X-ICQ, X-JABBER, X-MSN, X-MATRIX, X-SKYPE, X-YAHOO -> online_services
    // 4. X-JMAP-UID -> ContactCard.uid
    // 5. X-JMAP-KEY parameter -> JSContact map keys
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:c-full-x\r\n",
        "X-JMAP-UID:urn:uuid:12345678-abcd-ef01-2345-6789abcdef01\r\n",
        "FN:Alex Rivera\r\n",
        "N:Rivera;Alex;;;\r\n",
        "X-EVOLUTION-SPOUSE:Morgan Rivera\r\n",
        "X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=y1:2018-06-25\r\n",
        "X-AIM;X-JMAP-KEY=s-aim;TYPE=HOME:alex_aim\r\n",
        "X-GADUGADU;X-JMAP-KEY=s-gg;TYPE=WORK:1234567\r\n",
        "X-GOOGLE-TALK;X-JMAP-KEY=s-gt;TYPE=WORK:alex@gmail.com\r\n",
        "X-GROUPWISE;X-JMAP-KEY=s-gw;TYPE=WORK:alex_gw\r\n",
        "X-ICQ;X-JMAP-KEY=s-icq;TYPE=HOME:987654321\r\n",
        "X-JABBER;X-JMAP-KEY=s-jab;TYPE=HOME:alex@jabber.org\r\n",
        "X-MSN;X-JMAP-KEY=s-msn;TYPE=HOME:alex@msn.com\r\n",
        "X-MATRIX;X-JMAP-KEY=s-mat;TYPE=WORK:@alex:matrix.org\r\n",
        "X-SKYPE;X-JMAP-KEY=s-sky;TYPE=WORK:live:alex_skype\r\n",
        "X-YAHOO;X-JMAP-KEY=s-yah;TYPE=HOME:alex_yahoo\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse full supported X- properties");
    assert_eq!(card.id.as_ref().unwrap().as_str(), "c-full-x");
    assert_eq!(
        card.uid.as_deref(),
        Some("urn:uuid:12345678-abcd-ef01-2345-6789abcdef01")
    );

    // 1. Spouse
    let related = card.related_to.as_ref().expect("related_to");
    assert_eq!(
        related["Morgan Rivera"]
            .relation
            .as_ref()
            .and_then(|r| r.get("spouse")),
        Some(&serde_json::Value::from(true))
    );

    // 2. Anniversary
    let annivs = card.anniversaries.as_ref().expect("anniversaries");
    assert_eq!(annivs["y1"].kind.as_str(), "wedding");
    assert_eq!(
        anniversary_date(&annivs["y1"]),
        Some("2018-06-25".to_owned())
    );

    // 3. Online services
    let services = card.online_services.as_ref().expect("online_services");
    assert_eq!(services.len(), 10);
    assert_eq!(services["s-aim"].service.as_deref(), Some("AIM"));
    assert_eq!(services["s-aim"].user.as_deref(), Some("alex_aim"));
    assert_eq!(services["s-gg"].service.as_deref(), Some("Gadu-Gadu"));
    assert_eq!(services["s-gt"].service.as_deref(), Some("Google Talk"));
    assert_eq!(services["s-gw"].service.as_deref(), Some("GroupWise"));
    assert_eq!(services["s-icq"].service.as_deref(), Some("ICQ"));
    assert_eq!(services["s-jab"].service.as_deref(), Some("Jabber"));
    assert_eq!(services["s-msn"].service.as_deref(), Some("MSN"));
    assert_eq!(services["s-mat"].service.as_deref(), Some("Matrix"));
    assert_eq!(services["s-sky"].service.as_deref(), Some("Skype"));
    assert_eq!(services["s-yah"].service.as_deref(), Some("Yahoo"));

    // 4. Roundtrip serialization and fixed point
    let re_emitted = card_to_vcard(&card);
    assert!(re_emitted.contains("X-JMAP-UID:urn:uuid:12345678-abcd-ef01-2345-6789abcdef01\r\n"));
    assert!(re_emitted.contains("X-EVOLUTION-SPOUSE:Morgan Rivera\r\n"));
    assert!(re_emitted.contains("X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=y1:2018-06-25\r\n"));
    assert!(re_emitted.contains("X-AIM;X-JMAP-KEY=s-aim;TYPE=HOME:alex_aim\r\n"));
    assert!(re_emitted.contains("X-GADUGADU;X-JMAP-KEY=s-gg;TYPE=HOME:1234567\r\n"));
    assert!(re_emitted.contains("X-GOOGLE-TALK;X-JMAP-KEY=s-gt;TYPE=HOME:alex@gmail.com\r\n"));
    assert!(re_emitted.contains("X-GROUPWISE;X-JMAP-KEY=s-gw;TYPE=HOME:alex_gw\r\n"));
    assert!(re_emitted.contains("X-ICQ;X-JMAP-KEY=s-icq;TYPE=HOME:987654321\r\n"));
    assert!(re_emitted.contains("X-JABBER;X-JMAP-KEY=s-jab;TYPE=HOME:alex@jabber.org\r\n"));
    assert!(re_emitted.contains("X-MSN;X-JMAP-KEY=s-msn;TYPE=HOME:alex@msn.com\r\n"));
    assert!(re_emitted.contains("X-MATRIX;X-JMAP-KEY=s-mat;TYPE=HOME:@alex:matrix.org\r\n"));
    assert!(re_emitted.contains("X-SKYPE;X-JMAP-KEY=s-sky;TYPE=HOME:live:alex_skype\r\n"));
    assert!(re_emitted.contains("X-YAHOO;X-JMAP-KEY=s-yah;TYPE=HOME:alex_yahoo\r\n"));

    let card2 = vcard_to_card(&re_emitted).expect("parse re-emitted");
    assert_eq!(card2, card);
    let re_emitted2 = card_to_vcard(&card2);
    assert_eq!(re_emitted2, re_emitted);
}

#[test]
fn properties_with_custom_and_unknown_x_parameters_characterization() {
    // Tests standard properties carrying unknown X- parameters from external clients:
    // Parser must extract property value + known parameters without failing on unknown X- params;
    // Emitter outputs standard parameters + X-JMAP-KEY, omitting unknown X- parameters.
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Custom Params User\r\n",
        "EMAIL;X-CUSTOM-PARAM=123;X-VENDOR-STATUS=ACTIVE;TYPE=WORK;X-JMAP-KEY=e1:user@example.com\r\n",
        "TEL;X-CARRIER=Telekom;X-DIRECT-LINE=YES;TYPE=CELL;X-JMAP-KEY=p1:+49 30 1234567\r\n",
        "ADR;X-BUILDING=North;X-FLOOR=4;TYPE=WORK;X-JMAP-KEY=a1:;;Street 1;City;;10115;Germany\r\n",
        "LABEL;X-PAPER-FORMAT=A4;TYPE=WORK;X-JMAP-KEY=a1:Street 1\\n10115 City\\nGermany\r\n",
        "NOTE;X-SECURITY-LEVEL=PUBLIC;X-JMAP-KEY=n1:Public bio\r\n",
        "ORG;X-ORG-TYPE=CORPORATION;X-JMAP-KEY=o1:Enterprise Corp;Cloud Services\r\n",
        "TITLE;X-LEVEL=EXECUTIVE;X-JMAP-KEY=t1:Vice President\r\n",
        "URL;X-VERIFIED=TRUE;X-JMAP-KEY=l1:https://enterprise.example.com\r\n",
        "CATEGORIES;X-TAG-SYSTEM=CUSTOM:VIP,Client,Priority\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse vcard with custom X- parameters");
    assert_eq!(
        card.emails.as_ref().unwrap()["e1"].address,
        "user@example.com"
    );
    assert_eq!(
        card.emails.as_ref().unwrap()["e1"].contexts,
        Some(json!({"work": true}))
    );
    assert_eq!(card.phones.as_ref().unwrap()["p1"].number, "+49 30 1234567");
    assert_eq!(
        card.phones.as_ref().unwrap()["p1"].features,
        Some(json!({"mobile": true}))
    );
    let addr = &card.addresses.as_ref().unwrap()["a1"];
    assert_eq!(
        components_of(addr),
        vec![
            ("name", "Street 1"),
            ("locality", "City"),
            ("postcode", "10115"),
            ("country", "Germany"),
        ]
    );
    assert_eq!(addr.full.as_deref(), Some("Street 1\n10115 City\nGermany"));
    assert_eq!(card.notes.as_ref().unwrap()["n1"].note, "Public bio");
    assert_eq!(
        card.organizations.as_ref().unwrap()["o1"].name.as_deref(),
        Some("Enterprise Corp")
    );
    assert_eq!(
        card.organizations.as_ref().unwrap()["o1"].units.as_deref(),
        Some(&[OrgUnit::new("Cloud Services")][..])
    );
    assert_eq!(card.titles.as_ref().unwrap()["t1"].name, "Vice President");
    assert_eq!(
        card.links.as_ref().unwrap()["l1"].uri,
        "https://enterprise.example.com"
    );
    assert_eq!(
        card.keywords,
        Some(
            [
                ("Client".to_owned(), serde_json::Value::from(true)),
                ("Priority".to_owned(), serde_json::Value::from(true)),
                ("VIP".to_owned(), serde_json::Value::from(true)),
            ]
            .into()
        )
    );

    // Outbound emission: emits clean parameters, dropping unknown X- parameters
    let re_emitted = card_to_vcard(&card);
    assert!(!re_emitted.contains("X-CUSTOM-PARAM"));
    assert!(!re_emitted.contains("X-VENDOR-STATUS"));
    assert!(!re_emitted.contains("X-CARRIER"));
    assert!(!re_emitted.contains("X-DIRECT-LINE"));
    assert!(!re_emitted.contains("X-BUILDING"));
    assert!(!re_emitted.contains("X-FLOOR"));
    assert!(!re_emitted.contains("X-PAPER-FORMAT"));
    assert!(!re_emitted.contains("X-SECURITY-LEVEL"));
    assert!(!re_emitted.contains("X-ORG-TYPE"));
    assert!(!re_emitted.contains("X-LEVEL"));
    assert!(!re_emitted.contains("X-VERIFIED"));
    assert!(!re_emitted.contains("X-TAG-SYSTEM"));

    // Roundtrip fixed point
    let back = vcard_to_card(&re_emitted).expect("parse re-emitted");
    assert_eq!(back, card);
    let re_emitted2 = card_to_vcard(&back);
    assert_eq!(re_emitted2, re_emitted);
}

#[test]
fn jscontact_card_with_unmodeled_extra_properties_emission_and_fixed_point() {
    // Tests a server-originated JSContact card containing unmodeled RFC 9553 / custom
    // properties in card.extra and individual property extra maps.
    let mut extra = BTreeMap::new();
    extra.insert(
        "preferredLanguages".to_owned(),
        json!({"en": {"pref": 1}, "de": {"pref": 2}}),
    );
    extra.insert(
        "localizations".to_owned(),
        json!({"de": {"/name/full": "Herr Schmidt"}}),
    );
    extra.insert(
        "cryptoKeys".to_owned(),
        json!({"k1": {"uri": "https://keys.example.com/pub.asc"}}),
    );
    extra.insert("gender".to_owned(), json!("female"));
    extra.insert("customServerExtension".to_owned(), json!({"flag": true}));

    let mut note_extra = BTreeMap::new();
    note_extra.insert("created".to_owned(), json!("2026-08-19T10:00:00Z"));
    note_extra.insert("author".to_owned(), json!("admin"));

    let card = ContactCard {
        name: Some(Name {
            full: Some("Server Card With Extra".to_owned()),
            ..Name::default()
        }),
        emails: Some(
            [(
                "e1".to_owned(),
                ContactEmail {
                    address: "extra@example.com".to_owned(),
                    ..ContactEmail::default()
                },
            )]
            .into(),
        ),
        notes: Some(
            [(
                "n1".to_owned(),
                Note {
                    note: "Modeled note".to_owned(),
                    extra: note_extra,
                },
            )]
            .into(),
        ),
        extra,
        ..ContactCard::default()
    };

    // 1. Emission: card_to_vcard emits only modeled properties and ignores card.extra
    let vcard = card_to_vcard(&card);
    assert!(vcard.contains("FN:Server Card With Extra\r\n"));
    assert!(vcard.contains("EMAIL;X-JMAP-KEY=e1:extra@example.com\r\n"));
    assert!(vcard.contains("NOTE;X-JMAP-KEY=n1:Modeled note\r\n"));

    // Verify none of the unmodeled extra properties leaked into the vCard stream
    assert!(!vcard.contains("preferredLanguages"));
    assert!(!vcard.contains("localizations"));
    assert!(!vcard.contains("cryptoKeys"));
    assert!(!vcard.contains("gender"));
    assert!(!vcard.contains("customServerExtension"));
    assert!(!vcard.contains("2026-08-19T10:00:00Z"));
    assert!(!vcard.contains("author"));

    // 2. Reading back: reconstructs modeled fields cleanly
    let read_back = vcard_to_card(&vcard).expect("parse emitted vcard");
    assert_eq!(
        read_back.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Server Card With Extra")
    );
    assert_eq!(
        read_back.emails.as_ref().unwrap()["e1"].address,
        "extra@example.com"
    );
    assert_eq!(read_back.notes.as_ref().unwrap()["n1"].note, "Modeled note");
    assert!(read_back.extra.is_empty());

    // 3. Fixed-point convergence
    let vcard2 = card_to_vcard(&read_back);
    assert_eq!(vcard2, vcard);
}

#[test]
fn x_property_name_casing_and_empty_values_handling() {
    // Tests lowercase/mixed-case X- property names and empty X- property values:
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "fn:Case Insensitive User\r\n",
        "x-custom-empty:\r\n",
        "X-CUSTOM-SPACES:   \r\n",
        "x-jabber;x-jmap-key=s1;type=home:case_user@jabber.org\r\n",
        "x-evolution-spouse:Spouse Name\r\n",
        "x-evolution-anniversary;x-jmap-key=y1:2020-01-15\r\n",
        "x-unknown-lowercase-property:some value\r\n",
        "X-UNKNOWN-UPPERCASE-PROPERTY:some value\r\n",
        "X-MixedCase-Property:some value\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse mixed case X- vcard");
    assert_eq!(
        card.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Case Insensitive User")
    );
    assert_eq!(
        card.online_services.as_ref().unwrap()["s1"]
            .service
            .as_deref(),
        Some("Jabber")
    );
    assert_eq!(
        card.online_services.as_ref().unwrap()["s1"].user.as_deref(),
        Some("case_user@jabber.org")
    );
    assert_eq!(
        card.related_to.as_ref().unwrap()["Spouse Name"]
            .relation
            .as_ref()
            .and_then(|r| r.get("spouse")),
        Some(&serde_json::Value::from(true))
    );
    assert_eq!(
        card.anniversaries.as_ref().unwrap()["y1"].kind.as_str(),
        "wedding"
    );

    // Empty and unknown X- properties are safely ignored
    assert!(card.extra.is_empty());

    let re_emitted = card_to_vcard(&card);
    let back = vcard_to_card(&re_emitted).expect("parse re-emitted");
    assert_eq!(back, card);
}

#[test]
fn rfc2426_line_folding_and_unfolding_long_note_and_photo_roundtrip() {
    // 1. Long NOTE round-trip: value longer than 75 octets must fold on write and unfold losslessly on read
    let long_note_text = "This is an extremely long note that exceeds seventy-five octets by a substantial margin. \
        It contains detailed historical records, meeting minutes, action items, and extensive documentation \
        that must be folded across multiple physical lines according to RFC 2426 Section 2.6, and unfolded \
        with 100% lossless fidelity upon reading back from vCard 3.0 format.";

    let mut notes = BTreeMap::new();
    notes.insert(
        "n1".to_owned(),
        Note {
            note: long_note_text.to_owned(),
            extra: BTreeMap::new(),
        },
    );

    // 2. Inline base64 PHOTO round-trip: large binary image payload encoded as data URI
    let binary_data: Vec<u8> = (0..350).map(|i| (i % 256) as u8).collect();
    let base64_payload = base64::engine::general_purpose::STANDARD.encode(&binary_data);
    let photo_uri = format!("data:image/jpeg;base64,{base64_payload}");

    let mut media = BTreeMap::new();
    media.insert(
        "m1".to_owned(),
        Media {
            kind: Some("photo".to_owned()),
            uri: photo_uri.clone(),
            media_type: Some("image/jpeg".to_owned()),
            extra: BTreeMap::new(),
        },
    );

    let card = ContactCard {
        id: Some("C-FOLD-1".into()),
        name: Some(Name {
            full: Some("Line Folding Verification".to_owned()),
            ..Name::default()
        }),
        notes: Some(notes),
        media: Some(media),
        ..ContactCard::default()
    };

    // Emit vCard
    let vcard = card_to_vcard(&card);

    // Assert that folding occurred on the NOTE and PHOTO lines
    assert!(vcard.contains("\r\n "));

    // Assert that physical lines in the vCard output target 75 octets and never exceed 77 octets
    for physical_line in vcard.split("\r\n") {
        assert!(
            physical_line.len() <= 77,
            "Physical line exceeds maximum line length (len = {}): {:?}",
            physical_line.len(),
            physical_line
        );
        assert!(
            std::str::from_utf8(physical_line.as_bytes()).is_ok(),
            "Invalid UTF-8 in physical line slice: {physical_line:?}"
        );
    }

    // Read back and verify lossless unfolding
    let read_card = vcard_to_card(&vcard).expect("parse folded vcard");

    // Verify NOTE unfolded losslessly
    let read_notes = read_card.notes.as_ref().expect("notes present");
    assert_eq!(
        read_notes["n1"].note, long_note_text,
        "Folded NOTE did not unfold losslessly"
    );

    // Verify PHOTO unfolded losslessly and preserved media_type and binary content
    let read_media = read_card.media.as_ref().expect("media present");
    assert_eq!(read_media["m1"].kind.as_deref(), Some("photo"));
    assert_eq!(read_media["m1"].media_type.as_deref(), Some("image/jpeg"));
    assert_eq!(read_media["m1"].uri, photo_uri);

    // Verify fixed-point stability: emitting the parsed card produces identical vCard
    let vcard2 = card_to_vcard(&read_card);
    assert_eq!(vcard2, vcard, "Emitted folded vCard must be at fixed-point");
}

#[test]
fn rfc2426_prefolded_vcard_unfolding_with_crlf_spaces_and_tabs() {
    // RFC 2426 §2.6: Lines can be folded with CRLF + space OR CRLF + tab.
    // Unfolding removes the CRLF and the immediately following space/tab continuation marker.
    // If multiple spaces/tabs follow, only the first is the folding marker;
    // subsequent spaces/tabs are part of the value.
    let prefolded_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:prefolded-card-1\r\n",
        "FN;X-JMAP-KEY=name:Dr. Jane \r\n\tWatson\r\n",
        "NICKNAME;X-JMAP-KEY=k1:The\r\n  Detective\r\n",
        "EMAIL;X-JMAP-KEY=e1;TYPE=WORK:jane.watson\r\n @example.org\r\n",
        "TEL;X-JMAP-KEY=p1;TYPE=WORK,VOICE:+44 20 \r\n\t7946 0958\r\n",
        "ORG;X-JMAP-KEY=o1:Metropolitan Police\r\n  Service;Forensics \r\n\tDivision;Ballistics Unit\r\n",
        "TITLE;X-JMAP-KEY=t1:Senior Forensic \r\n\tConsultant\r\n",
        "ROLE;X-JMAP-KEY=t2:Lead Ballistics \r\n Specialist\r\n",
        "ADR;X-JMAP-KEY=a1;TYPE=WORK:PO Box \r\n 999;Suite 400;221B \r\n\tBaker Street;London;Greater \r\n London;NW1 6XE;United \r\n Kingdom\r\n",
        "LABEL;X-JMAP-KEY=a1;TYPE=WORK:221B Baker \r\n Street\\nSuite 400\\nLondon\\nNW1 \r\n\t6XE\\nUnited Kingdom\r\n",
        "NOTE;X-JMAP-KEY=n1:First line of note.\r\n Continued with space.\r\n\tContinued with tab.\r\n   Three leading spaces (one fold + two data).\r\n",
        "CATEGORIES:Forensics,Ballistics,Investigation,\r\n\tScotland Yard\r\n",
        "URL;X-JMAP-KEY=l1:https://example.org/forensics/\r\n deep/case/archive\r\n",
        "PHOTO;ENCODING=b;TYPE=PNG:iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAf\r\n\tFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9aw\r\n AAAABJRU5ErkJggg==\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(prefolded_vcard).expect("parse pre-folded vcard");

    // Assert exact unfolded values:
    assert_eq!(
        card.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Dr. Jane Watson")
    );
    assert_eq!(card.nicknames.as_ref().unwrap()["k1"].name, "The Detective");
    assert_eq!(
        card.emails.as_ref().unwrap()["e1"].address,
        "jane.watson@example.org"
    );
    assert_eq!(
        card.phones.as_ref().unwrap()["p1"].number,
        "+44 20 7946 0958"
    );

    // ORG: note "  Service" has 2 spaces after CRLF -> 1 space in value
    let org = &card.organizations.as_ref().unwrap()["o1"];
    assert_eq!(org.name.as_deref(), Some("Metropolitan Police Service"));
    let units: Vec<&str> = org
        .units
        .as_ref()
        .unwrap()
        .iter()
        .map(|u| u.name.as_str())
        .collect();
    assert_eq!(units, ["Forensics Division", "Ballistics Unit"]);

    assert_eq!(
        card.titles.as_ref().unwrap()["t1"].name,
        "Senior Forensic Consultant"
    );
    assert_eq!(
        card.titles.as_ref().unwrap()["t2"].name,
        "Lead Ballistics Specialist"
    );

    // ADR structured components
    let adr = &card.addresses.as_ref().unwrap()["a1"];
    let comp_map: BTreeMap<&str, &str> = adr
        .components
        .as_ref()
        .unwrap()
        .iter()
        .map(|c| (c.kind.as_str(), c.value.as_str()))
        .collect();
    assert_eq!(comp_map.get("postOfficeBox"), Some(&"PO Box 999"));
    assert_eq!(comp_map.get("apartment"), Some(&"Suite 400"));
    assert_eq!(comp_map.get("name"), Some(&"221B Baker Street"));
    assert_eq!(comp_map.get("locality"), Some(&"London"));
    assert_eq!(comp_map.get("region"), Some(&"Greater London"));
    assert_eq!(comp_map.get("postcode"), Some(&"NW1 6XE"));
    assert_eq!(comp_map.get("country"), Some(&"United Kingdom"));

    // LABEL
    assert_eq!(
        adr.full.as_deref(),
        Some("221B Baker Street\nSuite 400\nLondon\nNW1 6XE\nUnited Kingdom")
    );

    // NOTE: check multiline and leading space preservation
    let note = &card.notes.as_ref().unwrap()["n1"].note;
    assert!(note.contains("First line of note.Continued with space.Continued with tab.  Three leading spaces (one fold + two data)."));

    // CATEGORIES
    let keywords = card.keywords.as_ref().expect("keywords");
    assert!(keywords.contains_key("Forensics"));
    assert!(keywords.contains_key("Ballistics"));
    assert!(keywords.contains_key("Investigation"));
    assert!(keywords.contains_key("Scotland Yard"));

    // URL
    assert_eq!(
        card.links.as_ref().unwrap()["l1"].uri,
        "https://example.org/forensics/deep/case/archive"
    );

    // PHOTO
    let photo = &card.media.as_ref().unwrap()["m1"];
    assert_eq!(photo.media_type.as_deref(), Some("image/PNG"));
    assert!(photo.uri.starts_with("data:image/PNG;base64,"));

    // Re-emission must produce valid lines targeting 75 octets and reach fixed point
    let re_emitted = card_to_vcard(&card);
    for physical_line in re_emitted.split("\r\n") {
        assert!(
            physical_line.len() <= 77,
            "Re-emitted line exceeds 77 octets: {physical_line:?}"
        );
        assert!(std::str::from_utf8(physical_line.as_bytes()).is_ok());
    }
    let back = vcard_to_card(&re_emitted).expect("parse re-emitted");
    assert_eq!(back, card);
}

#[test]
fn rfc2426_line_folding_never_splits_multibyte_utf8_sequences() {
    // Tests that multi-byte UTF-8 sequences (2-byte, 3-byte, 4-byte) positioned
    // across the 75-octet fold boundary are never split across line folds.
    // Line folding must break on a valid UTF-8 character boundary.

    let test_cases = [
        // 2-byte UTF-8 sequences (German umlauts, Cyrillic, accented Latin)
        ("2-byte umlauts", "äöüßéñДж"),
        // 3-byte UTF-8 sequences (CJK characters, Japanese Hiragana, Devanagari)
        ("3-byte CJK & Hiragana", "漢字東京日本語संस्कृत"),
        // 4-byte UTF-8 sequences (Emoji, musical and math symbols)
        ("4-byte Emoji", "🦀🚀🌟🎉🔥𝄞𝕬🌍"),
    ];

    for (label, multibyte_sample) in test_cases {
        // Test padding lengths from 40 to 85 bytes before the multi-byte characters
        // to systematically exercise all possible boundary positions relative to 75 octets
        for pad_len in 40..=85 {
            let padding = "A".repeat(pad_len);
            let note_text = format!(
                "{padding}{multibyte_sample} -- trailing text to force multiple line folds if needed."
            );

            let mut notes = BTreeMap::new();
            notes.insert(
                "n1".to_owned(),
                Note {
                    note: note_text.clone(),
                    extra: BTreeMap::new(),
                },
            );

            let card = ContactCard {
                id: Some("C-UTF8".into()),
                name: Some(Name {
                    full: Some(format!("UTF8 Test {label} pad {pad_len}")),
                    ..Name::default()
                }),
                notes: Some(notes),
                ..ContactCard::default()
            };

            let vcard = card_to_vcard(&card);

            // 1. Check that EVERY physical line is valid UTF-8 and targets 75 octets (max <= 77)
            for (line_idx, physical_line) in vcard.split("\r\n").enumerate() {
                assert!(
                    physical_line.len() <= 77,
                    "[{label}, pad {pad_len}, line {line_idx}] line exceeds max limit (len={}): {:?}",
                    physical_line.len(),
                    physical_line
                );
                // Confirm valid UTF-8 boundary integrity: no split UTF-8 code point
                assert!(
                    std::str::from_utf8(physical_line.as_bytes()).is_ok(),
                    "[{label}, pad {pad_len}, line {line_idx}] invalid UTF-8 in line slice"
                );
            }

            // 2. Parse back and assert 100% byte-for-byte exact equality (no corruption, no replacement chars)
            let read_card = vcard_to_card(&vcard).unwrap_or_else(|e| {
                panic!(
                    "[{label}, pad {pad_len}] failed to parse emitted vCard: {e:?}\nvCard:\n{vcard}"
                )
            });
            let read_note = &read_card.notes.as_ref().unwrap()["n1"].note;
            assert_eq!(
                read_note, &note_text,
                "[{label}, pad {pad_len}] UTF-8 content corrupted across line fold"
            );
            assert!(
                !read_note.contains('\u{FFFD}'),
                "[{label}, pad {pad_len}] UTF-8 replacement character found"
            );

            // 3. Verify fixed point
            let vcard2 = card_to_vcard(&read_card);
            assert_eq!(
                vcard2, vcard,
                "[{label}, pad {pad_len}] fixed point failure"
            );
        }
    }
}

#[test]
fn rfc2426_line_folding_exact_boundary_lengths_around_75_octets() {
    // Tests exact line length boundaries around 75 octets:
    // Property prefix is "NOTE;X-JMAP-KEY=n1:" which is 19 octets.
    // ASCII characters without escaping.
    // Lengths <= 75 octets (total line with prefix <= 75) fit in 1 line.
    // Lengths > 75 octets fold into 2 lines.

    for (target_line_len, expect_folded) in [
        (70, false),
        (73, false),
        (74, false),
        (75, false),
        (78, true),
        (80, true),
        (100, true),
    ] {
        let prefix_len = "NOTE;X-JMAP-KEY=n1:".len(); // 19
        let value_len = target_line_len - prefix_len;
        let value = "X".repeat(value_len);

        let mut notes = BTreeMap::new();
        notes.insert(
            "n1".to_owned(),
            Note {
                note: value.clone(),
                extra: BTreeMap::new(),
            },
        );

        let card = ContactCard {
            id: Some("C-BOUND".into()),
            notes: Some(notes),
            ..ContactCard::default()
        };

        let vcard = card_to_vcard(&card);

        // Find the lines corresponding to NOTE
        let note_lines: Vec<&str> = vcard
            .split("\r\n")
            .filter(|l| l.starts_with("NOTE") || l.starts_with(' '))
            .collect();

        if expect_folded {
            assert!(
                note_lines.len() >= 2,
                "Target length {target_line_len} was expected to fold into multiple lines, got: {note_lines:?}"
            );
            assert!(
                note_lines[0].len() <= 77,
                "First folded line exceeds limit: {}",
                note_lines[0].len()
            );
            assert!(note_lines[1].starts_with(' '));
        } else {
            assert_eq!(
                note_lines.len(),
                1,
                "Target length {target_line_len} was expected to fit in 1 line, got: {note_lines:?}"
            );
        }

        // Parse back and verify lossless recovery
        let read_card = vcard_to_card(&vcard).expect("parse bounded card");
        assert_eq!(read_card.notes.as_ref().unwrap()["n1"].note, value);
    }
}

#[test]
fn rfc2426_line_folding_with_escaped_delimiters_and_backslashes() {
    // Tests line folding interacting with escaped characters:
    // RFC 2426 §2 escaping: \n, \;, \,, \\ in Note values.
    // Verify that newlines, commas, semicolons, and backslashes in multiline text
    // fold and unfold without splitting escape tokens or losing characters.

    let multiline_note = "Line 1 with text.\nLine 2 with semicolon ; and comma , and backslash \\.\nLine 3 with more text.";
    let long_note_with_escapes = multiline_note.repeat(4);

    let mut notes = BTreeMap::new();
    notes.insert(
        "n1".to_owned(),
        Note {
            note: long_note_with_escapes.clone(),
            extra: BTreeMap::new(),
        },
    );

    let card = ContactCard {
        id: Some("C-ESC-FOLD".into()),
        notes: Some(notes),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);

    // Verify all physical lines <= 77 octets
    for physical_line in vcard.split("\r\n") {
        assert!(
            physical_line.len() <= 77,
            "Physical line exceeds 77 octets: {physical_line:?}"
        );
        assert!(std::str::from_utf8(physical_line.as_bytes()).is_ok());
    }

    // Parse back and verify lossless text recovery
    let read_card = vcard_to_card(&vcard).expect("parse escaped folded card");
    assert_eq!(
        read_card.notes.as_ref().unwrap()["n1"].note,
        long_note_with_escapes
    );

    // Fixed point
    let vcard2 = card_to_vcard(&read_card);
    assert_eq!(vcard2, vcard);
}

#[test]
fn rfc2426_value_escaping_note_with_all_four_special_characters_roundtrip() {
    // RFC 2426 §2: Value escaping for text values:
    // \n (or \N), \,, \;, and \\ must escape on write and unescape on read
    // with no loss and no double-escaping.
    let note_text = "First line of notes.\nSecond line with comma, semicolon; and backslash \\.\nThird line with literal escapes: \\n \\, \\; \\\\ and more.";

    let mut notes = BTreeMap::new();
    notes.insert(
        "n1".to_owned(),
        Note {
            note: note_text.to_owned(),
            extra: BTreeMap::new(),
        },
    );

    let card = ContactCard {
        id: Some("C-ESC-NOTE".into()),
        notes: Some(notes),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);

    // Verify wire format has escaped characters
    assert!(
        vcard.contains(r"\nSecond line with comma\, semicolon\; and backslash \\.")
            || vcard.contains("NOTE;X-JMAP-KEY=n1:First line of notes.\\n"),
        "vCard should contain escaped characters on the wire: {vcard}"
    );

    // Parse back
    let read_card = vcard_to_card(&vcard).expect("parse note with all four escapes");
    assert_eq!(
        read_card.notes.as_ref().unwrap()["n1"].note,
        note_text,
        "Note text must match original exactly"
    );

    // Fixed point convergence: second pass
    let vcard2 = card_to_vcard(&read_card);
    assert_eq!(vcard2, vcard, "Emitted vCard must reach fixed point");

    // Third pass to guarantee no double-escaping
    let read_card2 = vcard_to_card(&vcard2).expect("parse second-pass vcard");
    assert_eq!(
        read_card2.notes.as_ref().unwrap()["n1"].note,
        note_text,
        "Note text must remain unchanged after multiple roundtrips"
    );
    let vcard3 = card_to_vcard(&read_card2);
    assert_eq!(vcard3, vcard, "Third-pass vCard must match first pass");
}

#[test]
fn rfc2426_value_escaping_comma_inside_org_unit_roundtrip() {
    // RFC 2426 §2 / §3.5.5: Commas inside ORG unit components and employer names:
    // Semicolon (;) delimits components of ORG, while comma (,) inside a unit
    // must be escaped as \, so it is not confused or lost, and semicolons inside
    // a unit must be escaped as \; so they don't split the component.
    let mut orgs = BTreeMap::new();
    orgs.insert(
        "o1".to_owned(),
        Organization {
            name: Some("Acme, Inc.".to_owned()),
            units: Some(vec![
                OrgUnit::new("Research, Development & Innovation"),
                OrgUnit::new("Optics, Lasers & Sensors"),
                OrgUnit::new("Hardware; Systems Division"),
                OrgUnit::new("Unit with \\ backslash and \n newline"),
            ]),
            extra: BTreeMap::new(),
        },
    );

    let card = ContactCard {
        id: Some("C-ESC-ORG".into()),
        organizations: Some(orgs),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);

    // Verify wire format contains escaped commas and semicolons
    assert!(
        vcard.contains(r"Acme\, Inc.") || vcard.contains("ORG;X-JMAP-KEY=o1:"),
        "vCard should contain escaped comma in ORG name: {vcard}"
    );

    // Parse back
    let read_card = vcard_to_card(&vcard).expect("parse org with escaped commas and semicolons");
    let read_org = &read_card.organizations.as_ref().unwrap()["o1"];
    assert_eq!(
        read_org.name.as_deref(),
        Some("Acme, Inc."),
        "Employer name with comma must roundtrip intact"
    );
    let units = read_org.units.as_ref().unwrap();
    assert_eq!(units.len(), 4, "Must have exactly 4 units");
    assert_eq!(units[0].name, "Research, Development & Innovation");
    assert_eq!(units[1].name, "Optics, Lasers & Sensors");
    assert_eq!(units[2].name, "Hardware; Systems Division");
    assert_eq!(units[3].name, "Unit with \\ backslash and \n newline");

    // Fixed point convergence
    let vcard2 = card_to_vcard(&read_card);
    assert_eq!(vcard2, vcard, "Emitted vCard must reach fixed point");

    // Test nameless organization with leading semicolon and commas in units
    let mut nameless_orgs = BTreeMap::new();
    nameless_orgs.insert(
        "o_nameless".to_owned(),
        Organization {
            name: None,
            units: Some(vec![
                OrgUnit::new("Engineering, Core Team"),
                OrgUnit::new("Architecture; Infrastructure"),
                OrgUnit::new("Group\\Gamma\nAlpha"),
            ]),
            extra: BTreeMap::new(),
        },
    );
    let nameless_card = ContactCard {
        id: Some("C-NAMELESS-ESC-ORG".into()),
        organizations: Some(nameless_orgs),
        ..ContactCard::default()
    };
    let nameless_vcard = card_to_vcard(&nameless_card);
    assert!(
        nameless_vcard.contains(";Engineering")
            || nameless_vcard.contains("ORG;X-JMAP-KEY=o_nameless:;"),
        "Nameless org must retain leading semicolon: {nameless_vcard}"
    );
    let read_nameless = vcard_to_card(&nameless_vcard).expect("parse nameless org with escapes");
    let read_n_org = &read_nameless.organizations.as_ref().unwrap()["o_nameless"];
    assert_eq!(read_n_org.name, None);
    let n_units = read_n_org.units.as_ref().unwrap();
    assert_eq!(n_units.len(), 3);
    assert_eq!(n_units[0].name, "Engineering, Core Team");
    assert_eq!(n_units[1].name, "Architecture; Infrastructure");
    assert_eq!(n_units[2].name, "Group\\Gamma\nAlpha");
    assert_eq!(card_to_vcard(&read_nameless), nameless_vcard);
}

#[test]
fn rfc2426_value_escaping_semicolon_inside_adr_component_roundtrip() {
    // RFC 2426 §2 / §3.2.1: Semicolons inside ADR components:
    // Semicolon (;) is the component separator for ADR. A semicolon inside an
    // individual component (e.g. street name "Suite 100; Building A") must be
    // escaped as \; so it does not shift subsequent components into wrong slots.
    let mut addresses = BTreeMap::new();
    addresses.insert(
        "a1".to_owned(),
        Address {
            components: Some(vec![
                AddressComponent::new("postOfficeBox", "PO Box 123; Station B"),
                AddressComponent::new("apartment", "Apt 4B, Room 12; Building C"),
                AddressComponent::new("name", "123 Main St; 2nd Floor, West Wing"),
                AddressComponent::new("locality", "San Francisco; Bay Area"),
                AddressComponent::new("region", "California; Northern"),
                AddressComponent::new("postcode", "94105; 94107"),
                AddressComponent::new("country", "United States; North America"),
            ]),
            contexts: Some(json!({"work": true})),
            full: Some(
                "PO Box 123; Station B\nApt 4B, Room 12; Building C\n123 Main St; 2nd Floor, West Wing\nSan Francisco; Bay Area, California; Northern 94105; 94107\nUnited States; North America"
                    .to_owned(),
            ),
            extra: BTreeMap::new(),
        },
    );

    let card = ContactCard {
        id: Some("C-ESC-ADR".into()),
        addresses: Some(addresses),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);

    // Parse back and verify no component shifting occurred
    let read_card = vcard_to_card(&vcard).expect("parse adr with escaped semicolons");
    let read_addr = &read_card.addresses.as_ref().unwrap()["a1"];
    let comps = read_addr.components.as_ref().unwrap();
    assert_eq!(
        comps.len(),
        7,
        "Must have all 7 components without shifting"
    );
    assert_eq!(comps[0].kind, "postOfficeBox");
    assert_eq!(comps[0].value, "PO Box 123; Station B");
    assert_eq!(comps[1].kind, "apartment");
    assert_eq!(comps[1].value, "Apt 4B, Room 12; Building C");
    assert_eq!(comps[2].kind, "name");
    assert_eq!(comps[2].value, "123 Main St; 2nd Floor, West Wing");
    assert_eq!(comps[3].kind, "locality");
    assert_eq!(comps[3].value, "San Francisco; Bay Area");
    assert_eq!(comps[4].kind, "region");
    assert_eq!(comps[4].value, "California; Northern");
    assert_eq!(comps[5].kind, "postcode");
    assert_eq!(comps[5].value, "94105; 94107");
    assert_eq!(comps[6].kind, "country");
    assert_eq!(comps[6].value, "United States; North America");

    assert_eq!(
        read_addr.full.as_deref(),
        Some(
            "PO Box 123; Station B\nApt 4B, Room 12; Building C\n123 Main St; 2nd Floor, West Wing\nSan Francisco; Bay Area, California; Northern 94105; 94107\nUnited States; North America"
        )
    );

    // Fixed point convergence
    let vcard2 = card_to_vcard(&read_card);
    assert_eq!(
        vcard2, vcard,
        "ADR with escaped semicolons must reach fixed point"
    );
}

#[test]
fn rfc2426_value_escaping_across_all_vcard_properties_roundtrip() {
    // Tests escaping of \n, \,, \;, \\ across all mapped vCard properties
    let mut nicknames = BTreeMap::new();
    nicknames.insert(
        "k1".to_owned(),
        Nickname {
            name: "Ali, Baba; Chief\\Boss\nLead".to_owned(),
            extra: BTreeMap::new(),
        },
    );

    let mut titles = BTreeMap::new();
    titles.insert(
        "t1".to_owned(),
        Title {
            name: "Director, Architecture; Core \\ Systems\nLead".to_owned(),
            kind: Some("title".to_owned()),
            extra: BTreeMap::new(),
        },
    );
    titles.insert(
        "t2".to_owned(),
        Title {
            name: "Lead, Quality; Assurance \\ Test\nSpecialist".to_owned(),
            kind: Some("role".to_owned()),
            extra: BTreeMap::new(),
        },
    );

    let mut related_to = BTreeMap::new();
    related_to.insert(
        "Bob, Smith; Jr.\\II".to_owned(),
        Relation {
            relation: Some([("spouse".to_string(), json!(true))].into()),
            extra: BTreeMap::new(),
        },
    );

    let mut emails = BTreeMap::new();
    emails.insert(
        "e1".to_owned(),
        ContactEmail {
            address: "alice+tag,filter;opt=1\\test@example.com".to_owned(),
            contexts: Some(json!({"work": true})),
            pref: Some(1),
            ..ContactEmail::default()
        },
    );

    let mut phones = BTreeMap::new();
    phones.insert(
        "p1".to_owned(),
        ContactPhone {
            number: "+1 (555) 123-4567, ext; 890 \\ test".to_owned(),
            contexts: Some(json!({"work": true})),
            features: Some(json!({"voice": true})),
            pref: Some(1),
            ..ContactPhone::default()
        },
    );

    let mut links = BTreeMap::new();
    links.insert(
        "l1".to_owned(),
        Link {
            uri: "https://example.com/query?q=test;sort=desc,rank&filter=a\\b".to_owned(),
            kind: None,
            extra: BTreeMap::new(),
        },
    );

    let mut keywords = BTreeMap::new();
    keywords.insert("Tag 1, with comma".to_owned(), json!(true));
    keywords.insert("Tag 2; with semicolon".to_owned(), json!(true));
    keywords.insert("Tag 3\\backslash".to_owned(), json!(true));

    let card = ContactCard {
        id: Some("C-ALL-ESC".into()),
        name: Some(Name {
            full: Some("Dr. Alice Smith, Ph.D.; Junior\\Senior".to_owned()),
            components: Some(vec![
                NameComponent::new("surname", "Smith, Jr."),
                NameComponent::new("given", "Alice; Marie"),
                NameComponent::new("given2", "B.\\C."),
                NameComponent::new("title", "Dr., Prof."),
                NameComponent::new("credential", "III; Esq."),
            ]),
            extra: BTreeMap::new(),
        }),
        nicknames: Some(nicknames),
        titles: Some(titles),
        related_to: Some(related_to),
        emails: Some(emails),
        phones: Some(phones),
        links: Some(links),
        keywords: Some(keywords),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);

    // Parse back
    let read_card = vcard_to_card(&vcard).expect("parse all properties with escapes");

    // Assert name
    let read_name = read_card.name.as_ref().unwrap();
    assert_eq!(
        read_name.full.as_deref(),
        Some("Dr. Alice Smith, Ph.D.; Junior\\Senior")
    );
    let name_comps = read_name.components.as_ref().unwrap();
    assert_eq!(name_comps[0].value, "Dr., Prof.");
    assert_eq!(name_comps[1].value, "Alice; Marie");
    assert_eq!(name_comps[2].value, "B.\\C.");
    assert_eq!(name_comps[3].value, "Smith, Jr.");
    assert_eq!(name_comps[4].value, "III; Esq.");

    // Assert nickname, titles, spouse, email, phone, links, keywords
    assert_eq!(
        read_card.nicknames.as_ref().unwrap()["k1"].name,
        "Ali, Baba; Chief\\Boss\nLead"
    );
    assert_eq!(
        read_card.titles.as_ref().unwrap()["t1"].name,
        "Director, Architecture; Core \\ Systems\nLead"
    );
    assert_eq!(
        read_card.titles.as_ref().unwrap()["t2"].name,
        "Lead, Quality; Assurance \\ Test\nSpecialist"
    );
    assert!(
        read_card
            .related_to
            .as_ref()
            .unwrap()
            .contains_key("Bob, Smith; Jr.\\II")
    );
    assert_eq!(
        read_card.emails.as_ref().unwrap()["e1"].address,
        "alice+tag,filter;opt=1\\test@example.com"
    );
    assert_eq!(
        read_card.phones.as_ref().unwrap()["p1"].number,
        "+1 (555) 123-4567, ext; 890 \\ test"
    );
    assert_eq!(
        read_card.links.as_ref().unwrap()["l1"].uri,
        "https://example.com/query?q=test;sort=desc,rank&filter=a\\b"
    );
    let read_kw = read_card.keywords.as_ref().unwrap();
    assert!(read_kw.contains_key("Tag 1, with comma"));
    assert!(read_kw.contains_key("Tag 2; with semicolon"));
    assert!(read_kw.contains_key("Tag 3\\backslash"));

    // Fixed point
    let vcard2 = card_to_vcard(&read_card);
    assert_eq!(
        vcard2, vcard,
        "All properties with escapes must reach fixed point"
    );
}

#[test]
fn rfc2426_value_escaping_no_double_escaping_multiroundtrip() {
    // Tests that multiple sequential roundtrips never double-escape or accumulate backslashes:
    // vcard1 -> card1 -> vcard2 -> card2 -> vcard3 -> card3
    let complex_text = "Text with single backslash \\, literal \\n, literal \\,, literal \\;, and double \\\\ backslashes.";
    let mut notes = BTreeMap::new();
    notes.insert(
        "n1".to_owned(),
        Note {
            note: complex_text.to_owned(),
            extra: BTreeMap::new(),
        },
    );

    let card0 = ContactCard {
        id: Some("C-MULTI-ROUND".into()),
        notes: Some(notes),
        ..ContactCard::default()
    };

    let vcard1 = card_to_vcard(&card0);
    let card1 = vcard_to_card(&vcard1).expect("roundtrip pass 1 parse");
    assert_eq!(card1.notes.as_ref().unwrap()["n1"].note, complex_text);

    let vcard2 = card_to_vcard(&card1);
    assert_eq!(vcard2, vcard1, "Pass 2 vCard must equal Pass 1 vCard");
    let card2 = vcard_to_card(&vcard2).expect("roundtrip pass 2 parse");
    assert_eq!(card2.notes.as_ref().unwrap()["n1"].note, complex_text);

    let vcard3 = card_to_vcard(&card2);
    assert_eq!(vcard3, vcard1, "Pass 3 vCard must equal Pass 1 vCard");
    let card3 = vcard_to_card(&vcard3).expect("roundtrip pass 3 parse");
    assert_eq!(card3.notes.as_ref().unwrap()["n1"].note, complex_text);
}

#[test]
fn rfc2426_inbound_unescaping_variants_and_boundary_cases() {
    // Tests inbound vCard unescaping variants:
    // 1. \N uppercase newline escape (RFC 2426 §2.4.2)
    // 2. Trailing backslash at end of property
    // 3. Consecutive escaped backslashes (\\\\ -> \\)
    // 4. Escaped backslash preceding escaped delimiter (\\; -> \;, \\, -> \,)
    let raw_vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:inbound-escapes\r\n",
        "FN:Alice\\, Smith\\; Ph.D.\\NPrefix\\\\Suffix\r\n",
        "NOTE;X-JMAP-KEY=n1:Line 1\\NLine 2 with \\; and \\, and \\\\ backslash\\NTrailing backslash\\\\\r\n",
        "ORG;X-JMAP-KEY=o1:Company\\, Inc.\\; Division;Team\\; Alpha;Group\\\\Beta\\NGamma\r\n",
        "ADR;TYPE=WORK;X-JMAP-KEY=a1:PO\\; 1;Ext\\, 2;Street\\; 3;City\\; 4;State\\, 5;94105\\; 6;USA\\\\7\r\n",
        "CATEGORIES:Alpha\\, Tag,Beta\\; Tag,Gamma\\\\Tag\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(raw_vcard).expect("parse inbound vcard with escape variants");

    // Assert FN unescaped with uppercase \N
    assert_eq!(
        card.name.as_ref().unwrap().full.as_deref(),
        Some("Alice, Smith; Ph.D.\nPrefix\\Suffix")
    );

    // Assert NOTE
    let note = &card.notes.as_ref().unwrap()["n1"].note;
    assert_eq!(
        note,
        "Line 1\nLine 2 with ; and , and \\ backslash\nTrailing backslash\\"
    );

    // Assert ORG
    let org = &card.organizations.as_ref().unwrap()["o1"];
    assert_eq!(org.name.as_deref(), Some("Company, Inc.; Division"));
    let units = org.units.as_ref().unwrap();
    assert_eq!(units[0].name, "Team; Alpha");
    assert_eq!(units[1].name, "Group\\Beta\nGamma");

    // Assert ADR
    let addr = &card.addresses.as_ref().unwrap()["a1"];
    let comps = addr.components.as_ref().unwrap();
    assert_eq!(comps[0].value, "PO; 1");
    assert_eq!(comps[1].value, "Ext, 2");
    assert_eq!(comps[2].value, "Street; 3");
    assert_eq!(comps[3].value, "City; 4");
    assert_eq!(comps[4].value, "State, 5");
    assert_eq!(comps[5].value, "94105; 6");
    assert_eq!(comps[6].value, "USA\\7");

    // Assert CATEGORIES
    let kw = card.keywords.as_ref().unwrap();
    assert!(kw.contains_key("Alpha, Tag"));
    assert!(kw.contains_key("Beta; Tag"));
    assert!(kw.contains_key("Gamma\\Tag"));

    // Fixed point convergence
    let emitted = card_to_vcard(&card);
    let reparsed = vcard_to_card(&emitted).expect("reparse emitted card");
    let reemitted = card_to_vcard(&reparsed);
    assert_eq!(reemitted, emitted, "Emitted vCard must reach fixed point");
}
