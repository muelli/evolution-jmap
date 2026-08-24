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
    states_anniversary, states_assistant, states_calendar, states_context, states_email,
    states_file_as, states_keyword, states_link, states_manager, states_media,
    states_name_component, states_nickname, states_note, states_nothing_but_the_marriage,
    states_online_service, states_org_unit, states_organization, states_phone,
    states_phone_feature, states_spouse, states_title, title_kind, vcard_to_card,
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
    // Characterizes how EDS-specific and vendor X- properties that are unmapped in jmap-vcard
    // behave on inbound parsing and outbound serialization.
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:eds-custom-001\r\n",
        "FN:EDS Custom Contact\r\n",
        "EMAIL;TYPE=WORK:eds.user@example.com\r\n",
        // Unslotted online services in EDS
        "X-TWITTER:@eds_user\r\n",
        "X-SIP:sip:eds.user@sip.example.com\r\n",
        // Vendor manager and assistant fields without X-EVOLUTION-
        "X-MANAGER:Boss Vendor\r\n",
        "X-ASSISTANT:Assistant Vendor\r\n",
        // Vendor blog and video URLs without X-EVOLUTION-
        "X-BLOG-URL:https://blogs.vendor.com/user\r\n",
        "X-VIDEO-URL:https://video.vendor.com/stream\r\n",
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
        card.name.as_ref().unwrap().extra.get("fileAs"),
        Some(&json!("Custom, Contact"))
    );
    assert_eq!(
        card.emails.as_ref().map(|e| e["e1"].address.as_str()),
        Some("eds.user@example.com")
    );

    // All unmapped EDS and vendor X- properties are safely ignored by design:
    assert_eq!(card.online_services, None);
    assert_eq!(card.related_to, None);
    assert_eq!(card.links, None);
    assert_eq!(card.phones, None);
    assert!(card.extra.is_empty());

    // Outbound emission contains standard modeled properties and mapped X-EVOLUTION-FILE-AS
    let emitted = card_to_vcard(&card);
    assert!(emitted.contains("X-EVOLUTION-FILE-AS:Custom\\, Contact"));
    assert!(!emitted.contains("X-TWITTER"));
    assert!(!emitted.contains("X-SIP"));
    assert!(!emitted.contains("X-MANAGER"));
    assert!(!emitted.contains("X-ASSISTANT"));
    assert!(!emitted.contains("X-BLOG-URL"));
    assert!(!emitted.contains("X-VIDEO-URL"));
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

#[test]
fn categories_empty_absent_and_refused_permutations_roundtrip() {
    // Tests empty, absent, and refused keyword combinations:
    // 1. keywords: None -> No CATEGORIES line, roundtrips to keywords: None.
    // 2. keywords: Some(BTreeMap::new()) -> No CATEGORIES line, roundtrips to keywords: None.
    // 3. Inbound empty CATEGORIES: -> parses to keywords: None.
    // 4. Inbound CATEGORIES:,,, -> parses to keywords: None.
    // 5. Inbound CATEGORIES with only whitespace items -> states_keyword refuses them, re-emits no line.

    // 1. None keywords
    let card_none = ContactCard {
        name: Some(Name {
            full: Some("Alice Smith".to_owned()),
            ..Name::default()
        }),
        keywords: None,
        ..ContactCard::default()
    };
    let vcard_none = card_to_vcard(&card_none);
    assert!(!vcard_none.contains("\r\nCATEGORIES:"));
    let parsed_none = vcard_to_card(&vcard_none).expect("parse none card");
    assert_eq!(parsed_none.keywords, None);

    // 2. Empty map keywords
    let card_empty_map = ContactCard {
        name: Some(Name {
            full: Some("Alice Smith".to_owned()),
            ..Name::default()
        }),
        keywords: Some(BTreeMap::new()),
        ..ContactCard::default()
    };
    let vcard_empty_map = card_to_vcard(&card_empty_map);
    assert!(!vcard_empty_map.contains("\r\nCATEGORIES:"));
    let parsed_empty_map = vcard_to_card(&vcard_empty_map).expect("parse empty map card");
    assert_eq!(parsed_empty_map.keywords, None);

    // 3. Inbound CATEGORIES: (empty string)
    let inbound_empty =
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice Smith\r\nCATEGORIES:\r\nEND:VCARD\r\n";
    let parsed_inbound_empty = vcard_to_card(inbound_empty).expect("parse inbound empty");
    assert_eq!(parsed_inbound_empty.keywords, None);
    let reemitted_empty = card_to_vcard(&parsed_inbound_empty);
    assert!(!reemitted_empty.contains("\r\nCATEGORIES:"));

    // 4. Inbound CATEGORIES:,,, (consecutive empty items)
    let inbound_commas =
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice Smith\r\nCATEGORIES:,,,\r\nEND:VCARD\r\n";
    let parsed_inbound_commas = vcard_to_card(inbound_commas).expect("parse inbound commas");
    assert_eq!(parsed_inbound_commas.keywords, None);
    let reemitted_commas = card_to_vcard(&parsed_inbound_commas);
    assert!(!reemitted_commas.contains("\r\nCATEGORIES:"));

    // 5. Keywords map with only refused tags (empty, leading/trailing whitespace, carriage return, non-bool)
    let card_refused = ContactCard {
        name: Some(Name {
            full: Some("Alice Smith".to_owned()),
            ..Name::default()
        }),
        keywords: Some(
            [
                ("".to_owned(), json!(true)),
                (" leading".to_owned(), json!(true)),
                ("trailing ".to_owned(), json!(true)),
                ("\ttabbed".to_owned(), json!(true)),
                ("with\rreturn".to_owned(), json!(true)),
                ("not_bool".to_owned(), json!(false)),
                ("string_val".to_owned(), json!("tag")),
            ]
            .into(),
        ),
        ..ContactCard::default()
    };
    let vcard_refused = card_to_vcard(&card_refused);
    assert!(!vcard_refused.contains("\r\nCATEGORIES:"));
    let parsed_refused = vcard_to_card(&vcard_refused).expect("parse refused card");
    assert_eq!(parsed_refused.keywords, None);
}

#[test]
fn categories_single_tag_variations_and_escaped_delimiters_roundtrip() {
    // Tests single category tags containing plain text, interior spaces, commas, semicolons,
    // backslashes, newlines, and combinations, asserting exact value preservation and fixed-point convergence.
    let test_cases = [
        ("Work", "CATEGORIES:Work\r\n"),
        ("Project Alpha", "CATEGORIES:Project Alpha\r\n"),
        ("Acme, Inc.", "CATEGORIES:Acme\\, Inc.\r\n"),
        ("One, Two, Three", "CATEGORIES:One\\, Two\\, Three\r\n"),
        ("Project;Alpha", "CATEGORIES:Project\\;Alpha\r\n"),
        (
            "Architecture; Core; Platform",
            "CATEGORIES:Architecture\\; Core\\; Platform\r\n",
        ),
        ("Dept\\Core", "CATEGORIES:Dept\\\\Core\r\n"),
        (
            "Path\\\\To\\\\Tag",
            "CATEGORIES:Path\\\\\\\\To\\\\\\\\Tag\r\n",
        ),
        ("Line 1\nLine 2", "CATEGORIES:Line 1\\nLine 2\r\n"),
        (
            "Tag\\, with \\; and \\\\ and \n all four",
            "CATEGORIES:Tag\\\\\\, with \\\\\\; and \\\\\\\\ and \\n all four\r\n",
        ),
    ];

    for (tag, expected_line) in test_cases {
        let card = ContactCard {
            name: Some(Name {
                full: Some("Bob Builder".to_owned()),
                ..Name::default()
            }),
            keywords: Some([(tag.to_owned(), json!(true))].into()),
            ..ContactCard::default()
        };

        let vcard = card_to_vcard(&card);
        assert!(
            vcard.contains(expected_line),
            "Expected {expected_line} in emitted vCard for tag {tag:?}, got:\n{vcard}"
        );

        let parsed = vcard_to_card(&vcard).expect("parse card with single category");
        let kw = parsed.keywords.as_ref().expect("keywords present");
        assert_eq!(
            kw.keys().collect::<Vec<_>>(),
            vec![&tag.to_owned()],
            "Parsed tag must match original {tag:?}"
        );
        assert_eq!(kw[tag], json!(true));

        // Fixed point convergence
        let reemitted = card_to_vcard(&parsed);
        assert_eq!(
            reemitted, vcard,
            "Re-emitted vCard must match for tag {tag:?}"
        );
        let reparsed = vcard_to_card(&reemitted).expect("reparse");
        assert_eq!(
            reparsed.keywords, parsed.keywords,
            "Re-parsed keywords must match for tag {tag:?}"
        );
    }
}

#[test]
fn categories_multiple_tags_sorted_order_and_escaping_roundtrip() {
    // Tests multiple category tags emitted on a single line in lexicographically sorted order,
    // verifying that embedded commas and semicolons within tags do not cause spurious item splits.
    let tags = [
        "Software, Core & Tools",
        "Hardware, Components",
        "Optics; Lasers & Sensors",
        "Finance & Accounting",
        "Executive; Strategy",
    ];

    let mut map = BTreeMap::new();
    for tag in tags {
        map.insert(tag.to_owned(), json!(true));
    }

    let card = ContactCard {
        name: Some(Name {
            full: Some("Charlie Davis".to_owned()),
            ..Name::default()
        }),
        keywords: Some(map),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);

    // Verify sorted order on the single emitted CATEGORIES line (unfolded):
    // "Executive; Strategy" -> Executive\; Strategy
    // "Finance & Accounting" -> Finance & Accounting
    // "Hardware, Components" -> Hardware\, Components
    // "Optics; Lasers & Sensors" -> Optics\; Lasers & Sensors
    // "Software, Core & Tools" -> Software\, Core & Tools
    let unfolded_vcard = unfolded(&vcard);
    let expected_categories_line = "CATEGORIES:Executive\\; Strategy,Finance & Accounting,Hardware\\, Components,Optics\\; Lasers & Sensors,Software\\, Core & Tools";
    assert_eq!(
        line(&unfolded_vcard, "CATEGORIES"),
        expected_categories_line,
        "Expected sorted CATEGORIES line in unfolded vCard"
    );
    assert_eq!(
        unfolded_vcard.matches("CATEGORIES:").count(),
        1,
        "Exactly one CATEGORIES line should be emitted"
    );

    // Parse back and verify exact tag preservation
    let parsed = vcard_to_card(&vcard).expect("parse multiple categories card");
    let kw = parsed.keywords.as_ref().expect("keywords present");
    assert_eq!(kw.len(), 5);
    for tag in tags {
        assert!(
            kw.contains_key(tag),
            "Expected parsed keywords to contain tag {tag:?}, got: {kw:?}"
        );
        assert_eq!(kw[tag], json!(true));
    }

    // Fixed point convergence
    let reemitted = card_to_vcard(&parsed);
    assert_eq!(reemitted, vcard);
    let reparsed = vcard_to_card(&reemitted).expect("reparse");
    assert_eq!(reparsed.keywords, parsed.keywords);
}

#[test]
fn categories_multiple_inbound_lines_merging_deduplication_and_fixed_point() {
    // Tests that multiple inbound CATEGORIES lines (e.g. from vCard imports or multiple providers)
    // are merged into a single deduplicated set, and outbound serialization consolidates them
    // into a single canonical sorted CATEGORIES line.
    let multi_line_vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN:Dana Evans\r\n",
        "CATEGORIES:Development,QA,Release\r\n",
        "CATEGORIES:Release,Ops,Security\r\n",
        "CATEGORIES:Development,Infrastructure,Security,Monitoring\r\n",
        "END:VCARD\r\n",
    );

    let parsed = vcard_to_card(multi_line_vcard).expect("parse multi-line categories");
    let kw = parsed.keywords.as_ref().expect("keywords present");

    let expected_keys = vec![
        "Development",
        "Infrastructure",
        "Monitoring",
        "Ops",
        "QA",
        "Release",
        "Security",
    ];
    assert_eq!(kw.keys().collect::<Vec<_>>(), expected_keys);
    assert!(kw.values().all(|v| v == &json!(true)));

    // Re-emission consolidates into a single sorted line
    let emitted = card_to_vcard(&parsed);
    assert_eq!(
        emitted.matches("CATEGORIES:").count(),
        1,
        "Must emit exactly one consolidated CATEGORIES line"
    );
    assert!(
        emitted.contains(
            "CATEGORIES:Development,Infrastructure,Monitoring,Ops,QA,Release,Security\r\n"
        ),
        "Emitted vCard must have consolidated sorted CATEGORIES line: {emitted}"
    );

    // Fixed point stability across successive passes
    let reparsed = vcard_to_card(&emitted).expect("reparse");
    assert_eq!(reparsed.keywords, parsed.keywords);
    let reemitted = card_to_vcard(&reparsed);
    assert_eq!(reemitted, emitted);
}

#[test]
fn categories_inbound_delimiter_variations_and_empty_item_skipping() {
    // Tests inbound vCards with empty items between/around commas, mixed-case property names,
    // parameters (ALTID, LANGUAGE, custom X- parameters), and parameter casing.
    let vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN:Evan Foster\r\n",
        "categories:Alpha,,Beta,,,Gamma,\r\n",
        "Categories;ALTID=1;LANGUAGE=en:Delta,Epsilon\r\n",
        "CATEGORIES;X-TAG-SYSTEM=CUSTOM;PID=1.1:Zeta,Eta\r\n",
        "END:VCARD\r\n",
    );

    let parsed = vcard_to_card(vcard).expect("parse categories variations");
    let kw = parsed.keywords.as_ref().expect("keywords present");

    let expected_keys = vec!["Alpha", "Beta", "Delta", "Epsilon", "Eta", "Gamma", "Zeta"];
    assert_eq!(kw.keys().collect::<Vec<_>>(), expected_keys);

    // Emitted vCard combines all into one canonical line
    let emitted = card_to_vcard(&parsed);
    assert_eq!(emitted.matches("CATEGORIES:").count(), 1);
    assert!(emitted.contains("CATEGORIES:Alpha,Beta,Delta,Epsilon,Eta,Gamma,Zeta\r\n"));

    let reparsed = vcard_to_card(&emitted).expect("reparse");
    assert_eq!(reparsed.keywords, parsed.keywords);
}

#[test]
fn categories_unicode_and_multibyte_utf8_roundtrip() {
    // Tests non-ASCII and multi-byte UTF-8 categories across various languages and emoji scripts,
    // asserting lossless round-trip fidelity and RFC 2426 line folding without UTF-8 splitting.
    let utf8_tags = [
        "Büro & Verwaltung",
        "Forschung, Entwicklung",
        "Santé, Sécurité",
        "Équipe d'ingénierie",
        "営業部",
        "開発，基盤",
        "مشاريع",
        "🚀 Launch",
        "⭐ VIP",
        "🔥 Urgent, P0",
    ];

    let mut map = BTreeMap::new();
    for tag in utf8_tags {
        map.insert(tag.to_owned(), json!(true));
    }

    let card = ContactCard {
        name: Some(Name {
            full: Some("Fiona Gallagher".to_owned()),
            ..Name::default()
        }),
        keywords: Some(map),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);

    // Verify all emitted lines are valid UTF-8 and <= 77 octets
    for line_str in vcard.split("\r\n") {
        assert!(
            line_str.len() <= 77,
            "Line exceeded 77 octets: {line_str} (len={})",
            line_str.len()
        );
    }

    let parsed = vcard_to_card(&vcard).expect("parse unicode categories");
    let kw = parsed.keywords.as_ref().expect("keywords present");
    assert_eq!(kw.len(), utf8_tags.len());

    for tag in utf8_tags {
        assert!(
            kw.contains_key(tag),
            "Parsed keywords must contain UTF-8 tag {tag:?}, got: {kw:?}"
        );
        assert_eq!(kw[tag], json!(true));
    }

    // Fixed point convergence
    let reemitted = card_to_vcard(&parsed);
    assert_eq!(reemitted, vcard);
    let reparsed = vcard_to_card(&reemitted).expect("reparse");
    assert_eq!(reparsed.keywords, parsed.keywords);
}

#[test]
fn categories_eds_category_list_fidelity_and_states_keyword_invariants() {
    // Tests states_keyword against valid and invalid inputs, verifying EDS whitespace trimming defense.
    // 1. Valid tags: return true
    assert!(states_keyword("Work", &json!(true)));
    assert!(states_keyword("Personal & Family", &json!(true)));
    assert!(states_keyword("Acme, Inc.", &json!(true)));
    assert!(states_keyword("Project;Alpha", &json!(true)));
    assert!(states_keyword("Dept\\Special", &json!(true)));
    assert!(states_keyword("Line 1\nLine 2", &json!(true)));
    assert!(states_keyword("🚀 VIP", &json!(true)));

    // 2. Refused: empty tag
    assert!(!states_keyword("", &json!(true)));

    // 3. Refused: carriage return
    assert!(!states_keyword("tag\rwith_cr", &json!(true)));
    assert!(!states_keyword("\rtag", &json!(true)));
    assert!(!states_keyword("tag\r", &json!(true)));

    // 4. Refused: leading/trailing ASCII whitespace (EDS trims them)
    assert!(!states_keyword(" leading_space", &json!(true)));
    assert!(!states_keyword("trailing_space ", &json!(true)));
    assert!(!states_keyword("\tleading_tab", &json!(true)));
    assert!(!states_keyword("trailing_tab\t", &json!(true)));
    assert!(!states_keyword("\nleading_newline", &json!(true)));
    assert!(!states_keyword("trailing_newline\n", &json!(true)));
    assert!(!states_keyword("\u{b}vertical_tab", &json!(true)));
    assert!(!states_keyword("vertical_tab\u{b}", &json!(true)));
    assert!(!states_keyword("\u{c}form_feed", &json!(true)));
    assert!(!states_keyword("form_feed\u{c}", &json!(true)));

    // 5. Refused: non-boolean-true values (RFC 9553 Set constraint)
    assert!(!states_keyword("Work", &json!(false)));
    assert!(!states_keyword("Work", &json!("true")));
    assert!(!states_keyword("Work", &json!(1)));
    assert!(!states_keyword("Work", &json!(null)));
    assert!(!states_keyword("Work", &json!({})));

    // 6. Card containing mix of valid and refused tags:
    // only valid tags are emitted, refused tags are omitted to prevent EDS trimming corruption.
    let card = ContactCard {
        name: Some(Name {
            full: Some("George Harris".to_owned()),
            ..Name::default()
        }),
        keywords: Some(
            [
                ("Valid Tag A".to_owned(), json!(true)),
                (" leading".to_owned(), json!(true)),
                ("trailing ".to_owned(), json!(true)),
                ("with\rcarriage_return".to_owned(), json!(true)),
                ("false_val".to_owned(), json!(false)),
                ("Valid Tag B".to_owned(), json!(true)),
            ]
            .into(),
        ),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert_eq!(
        line(&vcard, "CATEGORIES"),
        "CATEGORIES:Valid Tag A,Valid Tag B"
    );

    let parsed = vcard_to_card(&vcard).expect("parse mixed card");
    let kw = parsed.keywords.as_ref().expect("keywords present");
    assert_eq!(
        kw.keys().collect::<Vec<_>>(),
        vec!["Valid Tag A", "Valid Tag B"]
    );
}

#[test]
fn nickname_single_and_multiple_entries_eds_slotting_and_roundtrip() {
    // Characterizes NICKNAME cardinality and EDS slotting:
    // RFC 2426 §3.1.3 states NICKNAME on a single comma-separated line, but
    // JSContact (RFC 9553 §2.2.2) keys nicknames individually.
    // jmap-vcard emits one NICKNAME line per keyed entry so each carries an X-JMAP-KEY parameter.
    // Evolution / EDS 3.52 reads E_CONTACT_NICKNAME from the first line, rewrites that line in
    // place upon edit, and leaves parameters intact, while passing subsequent lines through.

    // 1. Single nickname roundtrip
    let mut single_nick = BTreeMap::new();
    single_nick.insert(
        "k1".to_owned(),
        Nickname {
            name: "Vee".to_owned(),
            extra: BTreeMap::new(),
        },
    );
    let card = ContactCard {
        id: Some("C-NICK-SINGLE".into()),
        nicknames: Some(single_nick),
        ..ContactCard::default()
    };
    let vcard = card_to_vcard(&card);
    assert_eq!(line(&vcard, "NICKNAME"), "NICKNAME;X-JMAP-KEY=k1:Vee");

    let parsed = vcard_to_card(&vcard).expect("parse single nickname");
    let parsed_nicks = parsed.nicknames.as_ref().expect("nicknames present");
    assert_eq!(parsed_nicks.len(), 1);
    assert_eq!(parsed_nicks["k1"].name, "Vee");
    assert_eq!(card_to_vcard(&parsed), vcard);

    // 2. Multiple nicknames emitted as distinct lines
    let mut multi_nicks = BTreeMap::new();
    multi_nicks.insert(
        "k1".to_owned(),
        Nickname {
            name: "Vee".to_owned(),
            extra: BTreeMap::new(),
        },
    );
    multi_nicks.insert(
        "k2".to_owned(),
        Nickname {
            name: "Vera the Elder".to_owned(),
            extra: BTreeMap::new(),
        },
    );
    multi_nicks.insert(
        "k3".to_owned(),
        Nickname {
            name: "Chief Architect".to_owned(),
            extra: BTreeMap::new(),
        },
    );
    let multi_card = ContactCard {
        id: Some("C-NICK-MULTI".into()),
        nicknames: Some(multi_nicks),
        ..ContactCard::default()
    };
    let multi_vcard = card_to_vcard(&multi_card);
    assert_eq!(multi_vcard.matches("\r\nNICKNAME;X-JMAP-KEY=").count(), 3);
    assert!(multi_vcard.contains("NICKNAME;X-JMAP-KEY=k1:Vee\r\n"));
    assert!(multi_vcard.contains("NICKNAME;X-JMAP-KEY=k2:Vera the Elder\r\n"));
    assert!(multi_vcard.contains("NICKNAME;X-JMAP-KEY=k3:Chief Architect\r\n"));

    let parsed_multi = vcard_to_card(&multi_vcard).expect("parse multi nickname");
    let p_nicks = parsed_multi.nicknames.as_ref().expect("nicknames present");
    assert_eq!(p_nicks.len(), 3);
    assert_eq!(p_nicks["k1"].name, "Vee");
    assert_eq!(p_nicks["k2"].name, "Vera the Elder");
    assert_eq!(p_nicks["k3"].name, "Chief Architect");
    assert_eq!(card_to_vcard(&parsed_multi), multi_vcard);

    // 3. Inbound multiple unkeyed NICKNAME lines allocate k1, k2, k3
    let raw_unkeyed = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Test User\r\n",
        "NICKNAME:First Nick\r\n",
        "NICKNAME:Second Nick\r\n",
        "NICKNAME:Third Nick\r\n",
        "END:VCARD\r\n"
    );
    let parsed_unkeyed = vcard_to_card(raw_unkeyed).expect("parse unkeyed nicknames");
    let unkeyed_nicks = parsed_unkeyed
        .nicknames
        .as_ref()
        .expect("nicknames present");
    assert_eq!(unkeyed_nicks.len(), 3);
    assert_eq!(unkeyed_nicks["k1"].name, "First Nick");
    assert_eq!(unkeyed_nicks["k2"].name, "Second Nick");
    assert_eq!(unkeyed_nicks["k3"].name, "Third Nick");

    // 4. EDS in-place edit simulation on first line
    let edited_vcard = multi_vcard.replace(
        "NICKNAME;X-JMAP-KEY=k1:Vee\r\n",
        "NICKNAME;X-JMAP-KEY=k1:Vee Updated\r\n",
    );
    let parsed_edited = vcard_to_card(&edited_vcard).expect("parse edited vcard");
    let ed_nicks = parsed_edited.nicknames.as_ref().expect("nicknames present");
    assert_eq!(ed_nicks["k1"].name, "Vee Updated");
    assert_eq!(ed_nicks["k2"].name, "Vera the Elder");
    assert_eq!(ed_nicks["k3"].name, "Chief Architect");
}

#[test]
fn nickname_comma_separated_text_list_inbound_and_escaping_fidelity() {
    // Characterizes comma handling in NICKNAME:
    // 1. Inbound vCard with comma-separated list on a single line (RFC 2426 §3.1.3 text-list):
    //    `NICKNAME:Rob,Robbie,Boss`
    //    entry_text_list reads calcard's parsed values and joins them with commas into a single
    //    Nickname struct ("Rob,Robbie,Boss") because EDS (libebook-contacts 3.52) hands the entire
    //    line back as a single E_CONTACT_NICKNAME string.
    // 2. Outbound emission escapes literal commas as `\,`:
    //    `NICKNAME;X-JMAP-KEY=k1:Rob\,Robbie\,Boss`
    // 3. Reading back the escaped vCard parses back to "Rob,Robbie,Boss", reaching fixed-point convergence.
    let raw_list_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Rob Example\r\n",
        "NICKNAME:Rob,Robbie,Boss\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(raw_list_vcard).expect("parse comma-separated nickname list");
    let nicks = card.nicknames.as_ref().expect("nicknames present");
    assert_eq!(nicks.len(), 1);
    assert_eq!(nicks["k1"].name, "Rob,Robbie,Boss");

    // Outbound emission escapes commas
    let emitted = card_to_vcard(&card);
    assert_eq!(
        line(&emitted, "NICKNAME"),
        "NICKNAME;X-JMAP-KEY=k1:Rob\\,Robbie\\,Boss"
    );

    // Roundtrip back preserves the exact nickname string
    let roundtrip_card = vcard_to_card(&emitted).expect("parse escaped nickname");
    assert_eq!(
        roundtrip_card.nicknames.as_ref().unwrap()["k1"].name,
        "Rob,Robbie,Boss"
    );
    assert_eq!(card_to_vcard(&roundtrip_card), emitted);

    // Inbound mixed escaped and unescaped commas
    let raw_mixed = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:John Smith\r\n",
        "NICKNAME:Smith\\, John,Chief\\, Executive,Boss\r\n",
        "END:VCARD\r\n"
    );
    let card_mixed = vcard_to_card(raw_mixed).expect("parse mixed commas nickname");
    assert_eq!(
        card_mixed.nicknames.as_ref().unwrap()["k1"].name,
        "Smith, John,Chief, Executive,Boss"
    );
}

#[test]
fn nickname_special_characters_escaping_unicode_and_parameters() {
    // Tests nicknames with semicolons, backslashes, newlines, UTF-8 unicode, and parameters:
    let mut nicks = BTreeMap::new();
    nicks.insert(
        "k_special".to_owned(),
        Nickname {
            name: "Nick;Name\\With\nNewline & \"Quotes\"".to_owned(),
            extra: [
                ("pref".to_owned(), json!(1)),
                ("contexts".to_owned(), json!({"work": true})),
            ]
            .into(),
        },
    );
    nicks.insert(
        "k_unicode_jp".to_owned(),
        Nickname {
            name: "たなかさん (田中)".to_owned(),
            extra: BTreeMap::new(),
        },
    );
    nicks.insert(
        "k_unicode_cyrillic".to_owned(),
        Nickname {
            name: "Саша (Александр)".to_owned(),
            extra: BTreeMap::new(),
        },
    );
    nicks.insert(
        "k_unicode_emoji".to_owned(),
        Nickname {
            name: "🌟 SuperStar 🦊".to_owned(),
            extra: BTreeMap::new(),
        },
    );

    let card = ContactCard {
        id: Some("C-NICK-SPECIAL".into()),
        nicknames: Some(nicks),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    let parsed = vcard_to_card(&vcard).expect("parse special nicknames");
    let p_nicks = parsed.nicknames.as_ref().expect("nicknames present");

    assert_eq!(
        p_nicks["k_special"].name,
        "Nick;Name\\With\nNewline & \"Quotes\""
    );
    assert_eq!(p_nicks["k_unicode_jp"].name, "たなかさん (田中)");
    assert_eq!(p_nicks["k_unicode_cyrillic"].name, "Саша (Александр)");
    assert_eq!(p_nicks["k_unicode_emoji"].name, "🌟 SuperStar 🦊");

    // Fixed-point convergence
    assert_eq!(card_to_vcard(&parsed), vcard);

    // Inbound parameters (TYPE, ALTID, LANGUAGE)
    let raw_param_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Elena\r\n",
        "NICKNAME;TYPE=WORK;X-JMAP-KEY=k_work:Office Elena\r\n",
        "NICKNAME;ALTID=1;LANGUAGE=de;X-JMAP-KEY=k_de:Leni\r\n",
        "END:VCARD\r\n"
    );
    let parsed_params = vcard_to_card(raw_param_vcard).expect("parse parameterized nicknames");
    let param_nicks = parsed_params.nicknames.as_ref().expect("nicknames present");
    assert_eq!(param_nicks["k_work"].name, "Office Elena");
    assert_eq!(param_nicks["k_de"].name, "Leni");
}

#[test]
fn nickname_empty_absent_and_predicate_fidelity() {
    // Tests states_nickname predicate and empty/absent nickname handling:
    assert!(states_nickname(&Nickname {
        name: "Nick".into(),
        extra: BTreeMap::new()
    }));
    assert!(!states_nickname(&Nickname {
        name: "".into(),
        extra: BTreeMap::new()
    }));

    // Empty nicknames are not emitted
    let mut nicks = BTreeMap::new();
    nicks.insert(
        "k1".to_owned(),
        Nickname {
            name: "".to_owned(),
            extra: BTreeMap::new(),
        },
    );
    nicks.insert(
        "k2".to_owned(),
        Nickname {
            name: "Valid Nick".to_owned(),
            extra: BTreeMap::new(),
        },
    );
    let card = ContactCard {
        nicknames: Some(nicks),
        ..ContactCard::default()
    };
    let vcard = card_to_vcard(&card);
    assert_eq!(vcard.matches("\r\nNICKNAME").count(), 1);
    assert_eq!(
        line(&vcard, "NICKNAME"),
        "NICKNAME;X-JMAP-KEY=k2:Valid Nick"
    );

    // Inbound empty NICKNAME lines are safely skipped
    let raw_empty = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Test\r\n",
        "NICKNAME:\r\n",
        "NICKNAME;X-JMAP-KEY=k1:\r\n",
        "END:VCARD\r\n"
    );
    let parsed_empty = vcard_to_card(raw_empty).expect("parse empty nickname lines");
    assert_eq!(parsed_empty.nicknames, None);
}

#[test]
fn url_single_and_multiple_properties_eds_slotting_and_roundtrip() {
    // Characterizes URL properties into EDS fields:
    // 1. Single URL (kind: None): maps to EDS E_CONTACT_HOMEPAGE_URL.
    // 2. Multiple URLs (kind: None): emitted as multiple URL;X-JMAP-KEY=... lines.
    //    EDS 3.52 maps the first URL line to E_CONTACT_HOMEPAGE_URL and preserves subsequent lines.
    // 3. Roundtrips preserve all keys and URIs without data loss.

    let mut links = BTreeMap::new();
    links.insert(
        "l1".to_owned(),
        Link {
            uri: "https://alice.example.com".to_owned(),
            kind: None,
            extra: BTreeMap::new(),
        },
    );
    links.insert(
        "l2".to_owned(),
        Link {
            uri: "https://work.example.org/alice".to_owned(),
            kind: None,
            extra: BTreeMap::new(),
        },
    );
    links.insert(
        "l3".to_owned(),
        Link {
            uri: "https://github.com/alice".to_owned(),
            kind: None,
            extra: BTreeMap::new(),
        },
    );

    let card = ContactCard {
        id: Some("C-URL-MULTI".into()),
        links: Some(links),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert_eq!(vcard.matches("\r\nURL;X-JMAP-KEY=").count(), 3);
    assert!(vcard.contains("URL;X-JMAP-KEY=l1:https://alice.example.com\r\n"));
    assert!(vcard.contains("URL;X-JMAP-KEY=l2:https://work.example.org/alice\r\n"));
    assert!(vcard.contains("URL;X-JMAP-KEY=l3:https://github.com/alice\r\n"));

    let parsed = vcard_to_card(&vcard).expect("parse multiple url lines");
    let p_links = parsed.links.as_ref().expect("links present");
    assert_eq!(p_links.len(), 3);
    assert_eq!(p_links["l1"].uri, "https://alice.example.com");
    assert_eq!(p_links["l1"].kind, None);
    assert_eq!(p_links["l2"].uri, "https://work.example.org/alice");
    assert_eq!(p_links["l2"].kind, None);
    assert_eq!(p_links["l3"].uri, "https://github.com/alice");
    assert_eq!(p_links["l3"].kind, None);

    // Fixed-point stability
    assert_eq!(card_to_vcard(&parsed), vcard);

    // Inbound unkeyed multiple URL lines allocate l1, l2, l3
    let raw_unkeyed = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Alice\r\n",
        "URL:https://primary.example.com\r\n",
        "URL:https://secondary.example.com\r\n",
        "URL;X-JMAP-KEY=l9:https://retained.example.com\r\n",
        "END:VCARD\r\n"
    );
    let parsed_unkeyed = vcard_to_card(raw_unkeyed).expect("parse unkeyed urls");
    let unk_links = parsed_unkeyed.links.as_ref().expect("links present");
    assert_eq!(unk_links.len(), 3);
    assert_eq!(unk_links["l1"].uri, "https://primary.example.com");
    assert_eq!(unk_links["l2"].uri, "https://secondary.example.com");
    assert_eq!(unk_links["l9"].uri, "https://retained.example.com");
}

#[test]
fn url_kind_filtering_and_contact_uri_omission() {
    // Product Decision & Rationale:
    // RFC 9553 §2.6.3 defines `kind: "contact"` as a URI for communicating with the person.
    // RFC 9555 §2.6.3 states `kind: "contact"` on vCard 4.0's `CONTACT-URI`.
    // vCard 3.0 has only `URL` (RFC 2426 §3.6.8), which EDS maps to E_CONTACT_HOMEPAGE_URL.
    // Emitting a contact link (or vendor kinds like feed/blog/video) on `URL` would misrepresent
    // it in Evolution's UI as the contact's homepage.
    // Therefore, ONLY `kind: None` (plain website) maps to `URL`. All other kinds emit no line.

    let mut links = BTreeMap::new();
    links.insert(
        "l_web".to_owned(),
        Link {
            uri: "https://alice.example.com".to_owned(),
            kind: None,
            extra: BTreeMap::new(),
        },
    );
    links.insert(
        "l_contact".to_owned(),
        Link {
            uri: "https://contact.example.com/form".to_owned(),
            kind: Some("contact".to_owned()),
            extra: BTreeMap::new(),
        },
    );
    links.insert(
        "l_feed".to_owned(),
        Link {
            uri: "https://alice.example.com/rss.xml".to_owned(),
            kind: Some("feed".to_owned()),
            extra: BTreeMap::new(),
        },
    );
    links.insert(
        "l_news".to_owned(),
        Link {
            uri: "https://news.alice.example.com".to_owned(),
            kind: Some("news".to_owned()),
            extra: BTreeMap::new(),
        },
    );
    links.insert(
        "l_custom".to_owned(),
        Link {
            uri: "https://custom.example.com/profile".to_owned(),
            kind: Some("x-vendor-profile".to_owned()),
            extra: BTreeMap::new(),
        },
    );

    let card = ContactCard {
        id: Some("C-URL-KINDS".into()),
        links: Some(links),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    // Only the plain website link gets a URL line (feed, contact, news, vendor kinds are omitted)
    assert_eq!(vcard.matches("\r\nURL").count(), 1);
    assert_eq!(
        line(&vcard, "URL"),
        "URL;X-JMAP-KEY=l_web:https://alice.example.com"
    );
    assert!(!vcard.contains("contact.example.com"));
    assert!(!vcard.contains("rss.xml"));
    assert!(!vcard.contains("news.alice.example.com"));
    assert!(!vcard.contains("custom.example.com"));

    let parsed = vcard_to_card(&vcard).expect("parse filtered url vcard");
    let p_links = parsed.links.as_ref().expect("links present");
    assert_eq!(p_links.len(), 1);
    assert_eq!(p_links["l_web"].uri, "https://alice.example.com");
    assert_eq!(p_links["l_web"].kind, None);
}

#[test]
fn url_eds_blog_video_and_custom_extensions_characterization() {
    // Characterizes EDS blog and video URL properties:
    // EDS defines E_CONTACT_BLOG_URL (`X-EVOLUTION-BLOG-URL`) and
    // E_CONTACT_VIDEO_URL (`X-EVOLUTION-VIDEO-URL`).
    // jmap-vcard maps them to `links` with kind "blog" and "video",
    // while ignoring vendor URLs without X-EVOLUTION- prefix (X-BLOG-URL, X-VIDEO-URL).
    let raw_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Alice Baker\r\n",
        "URL;X-JMAP-KEY=l1:https://alice.example.com\r\n",
        "X-EVOLUTION-BLOG-URL;X-JMAP-KEY=l2:https://blogs.example.com/alice\r\n",
        "X-EVOLUTION-VIDEO-URL;X-JMAP-KEY=l3:https://videos.example.com/alice\r\n",
        "X-BLOG-URL:https://blogs.vendor.com/ignored\r\n",
        "X-VIDEO-URL:https://videos.vendor.com/ignored\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(raw_vcard).expect("parse vcard with eds blog/video urls");
    let links = parsed.links.as_ref().expect("links present");
    assert_eq!(links.len(), 3);
    assert_eq!(links["l1"].uri, "https://alice.example.com");
    assert_eq!(links["l1"].kind, None);
    assert_eq!(links["l2"].uri, "https://blogs.example.com/alice");
    assert_eq!(links["l2"].kind, Some("blog".to_string()));
    assert_eq!(links["l3"].uri, "https://videos.example.com/alice");
    assert_eq!(links["l3"].kind, Some("video".to_string()));

    let emitted = card_to_vcard(&parsed);
    assert!(emitted.contains("URL;X-JMAP-KEY=l1:https://alice.example.com\r\n"));
    assert!(
        emitted.contains("X-EVOLUTION-BLOG-URL;X-JMAP-KEY=l2:https://blogs.example.com/alice\r\n")
    );
    assert!(
        emitted
            .contains("X-EVOLUTION-VIDEO-URL;X-JMAP-KEY=l3:https://videos.example.com/alice\r\n")
    );
    assert!(!emitted.contains("X-BLOG-URL:"));
    assert!(!emitted.contains("X-VIDEO-URL:"));
    assert_eq!(card_to_vcard(&vcard_to_card(&emitted).unwrap()), emitted);
}

#[test]
fn url_query_parameters_punctuation_and_encoding_fidelity() {
    // Verifies complex URIs containing query strings, semicolons, commas, hashes,
    // authentication userinfo, ports, and percent-encodings:
    // calcard preserves raw URI characters without backslash-escaping URI punctuation (RFC 3986 / RFC 2426 §3.6.8).
    let mut links = BTreeMap::new();
    links.insert(
        "l_complex_query".to_owned(),
        Link {
            uri: "https://api.example.com:8443/v1/search?q=tag:a,b;status:active&filter=x,y;z#top"
                .to_owned(),
            kind: None,
            extra: BTreeMap::new(),
        },
    );
    links.insert(
        "l_auth".to_owned(),
        Link {
            uri: "https://user:p%40ssw%3Brd@secure.example.org:9000/path/to/res?a=1&b=2#sec"
                .to_owned(),
            kind: None,
            extra: BTreeMap::new(),
        },
    );
    links.insert(
        "l_ipv6".to_owned(),
        Link {
            uri: "http://[2001:db8::1]:8080/index.html?token=abc;def,ghi".to_owned(),
            kind: None,
            extra: BTreeMap::new(),
        },
    );

    let card = ContactCard {
        id: Some("C-URL-COMPLEX".into()),
        links: Some(links),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    let parsed = vcard_to_card(&vcard).expect("parse complex urls");
    let p_links = parsed.links.as_ref().expect("links present");

    assert_eq!(
        p_links["l_complex_query"].uri,
        "https://api.example.com:8443/v1/search?q=tag:a,b;status:active&filter=x,y;z#top"
    );
    assert_eq!(
        p_links["l_auth"].uri,
        "https://user:p%40ssw%3Brd@secure.example.org:9000/path/to/res?a=1&b=2#sec"
    );
    assert_eq!(
        p_links["l_ipv6"].uri,
        "http://[2001:db8::1]:8080/index.html?token=abc;def,ghi"
    );

    // Assert fixed-point convergence
    assert_eq!(card_to_vcard(&parsed), vcard);
}

#[test]
fn url_empty_absent_and_predicate_fidelity() {
    // Tests states_link predicate and empty/absent link handling:
    assert!(states_link(&Link {
        uri: "https://example.com".into(),
        kind: None,
        extra: BTreeMap::new()
    }));
    assert!(!states_link(&Link {
        uri: "".into(),
        kind: None,
        extra: BTreeMap::new()
    }));
    assert!(!states_link(&Link {
        uri: "https://example.com".into(),
        kind: Some("contact".into()),
        extra: BTreeMap::new()
    }));
    assert!(!states_link(&Link {
        uri: "https://example.com".into(),
        kind: Some("other".into()),
        extra: BTreeMap::new()
    }));

    // Empty links are not emitted
    let mut links = BTreeMap::new();
    links.insert(
        "l1".to_owned(),
        Link {
            uri: "".to_owned(),
            kind: None,
            extra: BTreeMap::new(),
        },
    );
    links.insert(
        "l2".to_owned(),
        Link {
            uri: "https://valid.example.com".to_owned(),
            kind: None,
            extra: BTreeMap::new(),
        },
    );
    let card = ContactCard {
        links: Some(links),
        ..ContactCard::default()
    };
    let vcard = card_to_vcard(&card);
    assert_eq!(vcard.matches("\r\nURL").count(), 1);
    assert_eq!(
        line(&vcard, "URL"),
        "URL;X-JMAP-KEY=l2:https://valid.example.com"
    );

    // Inbound empty URL lines are safely skipped
    let raw_empty = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Test\r\n",
        "URL:\r\n",
        "URL;X-JMAP-KEY=l1:\r\n",
        "END:VCARD\r\n"
    );
    let parsed_empty = vcard_to_card(raw_empty).expect("parse empty url lines");
    assert_eq!(parsed_empty.links, None);

    // Unmodeled Link fields (contexts, pref, label, mediaType) in extra survive roundtrips untouched
    let mut extra_links = BTreeMap::new();
    extra_links.insert(
        "l_rich".to_owned(),
        Link {
            uri: "https://rich.example.com".to_owned(),
            kind: None,
            extra: [
                ("pref".to_owned(), json!(1)),
                ("contexts".to_owned(), json!({"work": true})),
                ("label".to_owned(), json!("Work Portal")),
                ("mediaType".to_owned(), json!("text/html")),
            ]
            .into(),
        },
    );
    let rich_card = ContactCard {
        links: Some(extra_links),
        ..ContactCard::default()
    };
    let rich_vcard = card_to_vcard(&rich_card);
    let parsed_rich = vcard_to_card(&rich_vcard).expect("parse rich url");
    assert_eq!(
        parsed_rich.links.as_ref().unwrap()["l_rich"].uri,
        "https://rich.example.com"
    );
}

#[test]
fn url_and_calendar_properties_coexistence_and_slotting() {
    // Tests clean separation and coexistence of URL, CALURI, and FBURL:
    // URL -> E_CONTACT_HOMEPAGE_URL (card.links)
    // CALURI -> E_CONTACT_CALENDAR_URI (card.calendars, kind: "calendar")
    // FBURL -> E_CONTACT_FREEBUSY_URL (card.calendars, kind: "freeBusy")
    let mut links = BTreeMap::new();
    links.insert(
        "l1".to_owned(),
        Link {
            uri: "https://alice.example.com".to_owned(),
            kind: None,
            extra: BTreeMap::new(),
        },
    );

    let mut cals = BTreeMap::new();
    cals.insert(
        "c1".to_owned(),
        Calendar {
            uri: "https://cal.example.com/alice.ics".to_owned(),
            kind: Some("calendar".to_owned()),
            extra: BTreeMap::new(),
        },
    );
    cals.insert(
        "c2".to_owned(),
        Calendar {
            uri: "https://cal.example.com/fb/alice.ifb".to_owned(),
            kind: Some("freeBusy".to_owned()),
            extra: BTreeMap::new(),
        },
    );

    let card = ContactCard {
        id: Some("C-URL-CAL-COEXIST".into()),
        links: Some(links),
        calendars: Some(cals),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert_eq!(
        line(&vcard, "URL"),
        "URL;X-JMAP-KEY=l1:https://alice.example.com"
    );
    assert_eq!(
        line(&vcard, "CALURI"),
        "CALURI;X-JMAP-KEY=c1:https://cal.example.com/alice.ics"
    );
    assert_eq!(
        line(&vcard, "FBURL"),
        "FBURL;X-JMAP-KEY=c2:https://cal.example.com/fb/alice.ifb"
    );

    let parsed = vcard_to_card(&vcard).expect("parse coexisting url and calendar lines");
    let p_links = parsed.links.as_ref().expect("links present");
    let p_cals = parsed.calendars.as_ref().expect("calendars present");

    assert_eq!(p_links.len(), 1);
    assert_eq!(p_links["l1"].uri, "https://alice.example.com");
    assert_eq!(p_links["l1"].kind, None);

    assert_eq!(p_cals.len(), 2);
    assert_eq!(p_cals["c1"].uri, "https://cal.example.com/alice.ics");
    assert_eq!(p_cals["c1"].kind.as_deref(), Some("calendar"));
    assert_eq!(p_cals["c2"].uri, "https://cal.example.com/fb/alice.ifb");
    assert_eq!(p_cals["c2"].kind.as_deref(), Some("freeBusy"));

    // Fixed-point stability
    assert_eq!(card_to_vcard(&parsed), vcard);
}

#[test]
fn non_ascii_multilingual_names_and_components_roundtrip() {
    // Tests across diverse world writing systems and scripts:
    // French accents, German umlauts/eszett, Spanish tildes, Icelandic thorn/eth,
    // Polish crossed-L, Russian Cyrillic, Greek, Hebrew, Arabic RTL, Chinese Hanzi,
    // Japanese Kanji/Kana, Korean Hangul, Hindi Devanagari, Vietnamese, and Emoji.
    let test_cases = [
        (
            "French",
            "René François de Chateaubriand",
            Some("Chateaubriand"),
            Some("René"),
            Some("François"),
            Some("de"),
            None,
        ),
        (
            "German",
            "Dr. Jörg Weiß-Müller Jr.",
            Some("Weiß-Müller"),
            Some("Jörg"),
            None,
            Some("Dr."),
            Some("Jr."),
        ),
        (
            "Spanish",
            "María José Carreño Quiñones",
            Some("Carreño Quiñones"),
            Some("María"),
            Some("José"),
            None,
            None,
        ),
        (
            "Icelandic",
            "Guðmundur Þórðarson",
            Some("Þórðarson"),
            Some("Guðmundur"),
            None,
            None,
            None,
        ),
        (
            "Polish",
            "Stanisław Lem",
            Some("Lem"),
            Some("Stanisław"),
            None,
            None,
            None,
        ),
        (
            "Russian Cyrillic",
            "Граф Лев Николаевич Толстой",
            Some("Толстой"),
            Some("Лев"),
            Some("Николаевич"),
            Some("Граф"),
            None,
        ),
        (
            "Greek",
            "Σωκράτης",
            Some("Σωκράτης"),
            None,
            None,
            None,
            None,
        ),
        (
            "Hebrew",
            "שלום עליכם",
            Some("עליכם"),
            Some("שלום"),
            None,
            None,
            None,
        ),
        (
            "Arabic",
            "نجيب محفوظ",
            Some("محفوظ"),
            Some("نجيب"),
            None,
            None,
            None,
        ),
        (
            "Chinese Hanzi",
            "李白",
            Some("李"),
            Some("白"),
            None,
            None,
            None,
        ),
        (
            "Japanese Kanji and Kana",
            "宮崎 駿 (みやざき はやお)",
            Some("宮崎"),
            Some("駿"),
            Some("みやざき はやお"),
            None,
            None,
        ),
        (
            "Korean Hangul",
            "김연아",
            Some("김"),
            Some("연아"),
            None,
            None,
            None,
        ),
        (
            "Hindi Devanagari",
            "रवीन्द्रनाथ ठाकुर",
            Some("ठाकुर"),
            Some("रवीन्द्रनाथ"),
            None,
            None,
            None,
        ),
        (
            "Vietnamese",
            "Nguyễn Du",
            Some("Nguyễn"),
            Some("Du"),
            None,
            None,
            None,
        ),
        (
            "Emoji and Symbols",
            "🧑‍💻 Alice Smith 🚀",
            Some("Smith"),
            Some("Alice"),
            Some("🧑‍💻"),
            None,
            Some("🚀"),
        ),
    ];

    for (label, full_name, surname, given, middle, prefix, suffix) in test_cases {
        let mut components = Vec::new();
        if let Some(s) = surname {
            components.push(NameComponent::new("surname", s));
        }
        if let Some(g) = given {
            components.push(NameComponent::new("given", g));
        }
        if let Some(m) = middle {
            components.push(NameComponent::new("given2", m));
        }
        if let Some(p) = prefix {
            components.push(NameComponent::new("title", p));
        }
        if let Some(suf) = suffix {
            components.push(NameComponent::new("credential", suf));
        }

        let card = ContactCard {
            id: Some(format!("C-NAME-{}", label.replace(' ', "-")).into()),
            name: Some(Name {
                full: Some(full_name.to_owned()),
                components: (!components.is_empty()).then_some(components),
                extra: BTreeMap::new(),
            }),
            ..ContactCard::default()
        };

        let vcard1 = card_to_vcard(&card);
        // Verify line is valid UTF-8 and starts with standard envelope
        assert!(
            vcard1.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"),
            "label: {label}"
        );
        assert!(
            vcard1.contains(&format!("FN:{full_name}")),
            "label: {label}, vcard: {vcard1}"
        );

        let parsed1 =
            vcard_to_card(&vcard1).unwrap_or_else(|e| panic!("{label} parse error: {e:?}"));
        let p_name = parsed1
            .name
            .as_ref()
            .unwrap_or_else(|| panic!("{label} missing name"));
        assert_eq!(p_name.full.as_deref(), Some(full_name), "label: {label}");

        // Verify components match
        if let Some(comps) = &p_name.components {
            let get_comp = |kind: &str| {
                comps
                    .iter()
                    .find(|c| c.kind == kind)
                    .map(|c| c.value.as_str())
            };
            assert_eq!(get_comp("surname"), surname, "surname mismatch for {label}");
            assert_eq!(get_comp("given"), given, "given mismatch for {label}");
            assert_eq!(get_comp("given2"), middle, "middle mismatch for {label}");
            assert_eq!(get_comp("title"), prefix, "prefix mismatch for {label}");
            assert_eq!(
                get_comp("credential"),
                suffix,
                "suffix mismatch for {label}"
            );
        }

        // Fixed-point stability
        let vcard2 = card_to_vcard(&parsed1);
        let parsed2 = vcard_to_card(&vcard2).expect("re-parse second pass");
        let vcard3 = card_to_vcard(&parsed2);
        assert_eq!(vcard2, vcard3, "fixed-point stability for {label}");
    }
}

#[test]
fn non_ascii_multilingual_organization_title_and_role_roundtrip() {
    let mut orgs = BTreeMap::new();
    orgs.insert(
        "o1".to_owned(),
        Organization {
            name: Some("Société Générale & Compagnie".to_owned()),
            units: Some(vec![
                OrgUnit::new("Direction des Systèmes d'Information"),
                OrgUnit::new("Pôle Innovation & Recherche"),
                OrgUnit::new("Équipe Cryptographie"),
            ]),
            extra: BTreeMap::new(),
        },
    );
    orgs.insert(
        "o2".to_owned(),
        Organization {
            name: Some("ООО \"Яндекс\"".to_owned()),
            units: Some(vec![
                OrgUnit::new("Департамент разработки"),
                OrgUnit::new("Группа поисковых технологий"),
            ]),
            extra: BTreeMap::new(),
        },
    );

    let mut titles = BTreeMap::new();
    titles.insert(
        "t1".to_owned(),
        Title {
            name: "Directeur Général Adjoint".to_owned(),
            kind: Some("title".to_owned()),
            extra: BTreeMap::new(),
        },
    );
    titles.insert(
        "t2".to_owned(),
        Title {
            name: "Главный архитектор систем".to_owned(),
            kind: Some("role".to_owned()),
            extra: BTreeMap::new(),
        },
    );
    titles.insert(
        "t3".to_owned(),
        Title {
            name: "開発最高責任者".to_owned(),
            kind: Some("title".to_owned()),
            extra: BTreeMap::new(),
        },
    );

    let card = ContactCard {
        id: Some("C-NONASCII-ORG-TITLE".into()),
        organizations: Some(orgs),
        titles: Some(titles),
        ..ContactCard::default()
    };

    let vcard1 = card_to_vcard(&card);
    let parsed1 = vcard_to_card(&vcard1).expect("parse non-ascii org and titles");

    let p_orgs = parsed1.organizations.as_ref().expect("orgs present");
    assert_eq!(p_orgs.len(), 2);
    assert_eq!(
        p_orgs["o1"].name.as_deref(),
        Some("Société Générale & Compagnie")
    );
    assert_eq!(
        p_orgs["o1"].units.as_ref().unwrap(),
        &[
            OrgUnit::new("Direction des Systèmes d'Information"),
            OrgUnit::new("Pôle Innovation & Recherche"),
            OrgUnit::new("Équipe Cryptographie"),
        ]
    );
    assert_eq!(p_orgs["o2"].name.as_deref(), Some("ООО \"Яндекс\""));
    assert_eq!(
        p_orgs["o2"].units.as_ref().unwrap(),
        &[
            OrgUnit::new("Департамент разработки"),
            OrgUnit::new("Группа поисковых технологий"),
        ]
    );

    let p_titles = parsed1.titles.as_ref().expect("titles present");
    assert_eq!(p_titles.len(), 3);
    assert_eq!(p_titles["t1"].name, "Directeur Général Adjoint");
    assert_eq!(p_titles["t1"].kind, None); // default kind "title"
    assert_eq!(p_titles["t2"].name, "Главный архитектор систем");
    assert_eq!(p_titles["t2"].kind.as_deref(), Some("role"));
    assert_eq!(p_titles["t3"].name, "開発最高責任者");
    assert_eq!(p_titles["t3"].kind, None); // default kind "title"

    // Fixed-point stability
    let vcard2 = card_to_vcard(&parsed1);
    let parsed2 = vcard_to_card(&vcard2).expect("re-parse second pass");
    let vcard3 = card_to_vcard(&parsed2);
    assert_eq!(vcard2, vcard3);
}

#[test]
fn non_ascii_structured_addresses_and_labels_roundtrip() {
    let mut addrs = BTreeMap::new();
    // French address with accents and special characters
    addrs.insert(
        "a_fr".to_owned(),
        Address {
            components: Some(vec![
                AddressComponent::new("postOfficeBox", "Boîte Postale 42"),
                AddressComponent::new("apartment", "Bâtiment B, Étage 3, Porte 12"),
                AddressComponent::new("name", "12 Rue de l'Étoile"),
                AddressComponent::new("locality", "Épinay-sur-Seine"),
                AddressComponent::new("region", "Île-de-France"),
                AddressComponent::new("postcode", "93800"),
                AddressComponent::new("country", "France"),
            ]),
            full: Some(
                "12 Rue de l'Étoile\nBâtiment B, Étage 3\n93800 Épinay-sur-Seine\nFrance"
                    .to_owned(),
            ),
            contexts: Some(json!({"work": true})),
            extra: BTreeMap::new(),
        },
    );
    // German address with umlauts and eszett
    addrs.insert(
        "a_de".to_owned(),
        Address {
            components: Some(vec![
                AddressComponent::new("apartment", "Hinterhaus 2. Stock"),
                AddressComponent::new("name", "Goethestraße 42"),
                AddressComponent::new("locality", "München"),
                AddressComponent::new("region", "Bayern"),
                AddressComponent::new("postcode", "80336"),
                AddressComponent::new("country", "Deutschland"),
            ]),
            full: Some("Goethestraße 42\n80336 München\nDeutschland".to_owned()),
            contexts: Some(json!({"home": true})),
            extra: BTreeMap::new(),
        },
    );
    // Japanese address with Kanji
    addrs.insert(
        "a_ja".to_owned(),
        Address {
            components: Some(vec![
                AddressComponent::new("name", "千代田区千代田1-1"),
                AddressComponent::new("region", "東京都"),
                AddressComponent::new("postcode", "100-8111"),
                AddressComponent::new("country", "日本"),
            ]),
            full: Some("〒100-8111 東京都千代田区千代田1-1\n日本".to_owned()),
            contexts: None,
            extra: BTreeMap::new(),
        },
    );

    let card = ContactCard {
        id: Some("C-NONASCII-ADR".into()),
        addresses: Some(addrs),
        ..ContactCard::default()
    };

    let vcard1 = card_to_vcard(&card);
    let parsed1 = vcard_to_card(&vcard1).expect("parse non-ascii addresses");

    let p_addrs = parsed1.addresses.as_ref().expect("addresses present");
    assert_eq!(p_addrs.len(), 3);

    let a_fr = &p_addrs["a_fr"];
    let comps_fr = a_fr.components.as_ref().expect("french comps");
    let get_comp = |comps: &[AddressComponent], k: &str| -> Option<String> {
        comps.iter().find(|c| c.kind == k).map(|c| c.value.clone())
    };
    assert_eq!(
        get_comp(comps_fr, "postOfficeBox").as_deref(),
        Some("Boîte Postale 42")
    );
    assert_eq!(
        get_comp(comps_fr, "apartment").as_deref(),
        Some("Bâtiment B, Étage 3, Porte 12")
    );
    assert_eq!(
        get_comp(comps_fr, "name").as_deref(),
        Some("12 Rue de l'Étoile")
    );
    assert_eq!(
        get_comp(comps_fr, "locality").as_deref(),
        Some("Épinay-sur-Seine")
    );
    assert_eq!(
        get_comp(comps_fr, "region").as_deref(),
        Some("Île-de-France")
    );
    assert_eq!(get_comp(comps_fr, "postcode").as_deref(), Some("93800"));
    assert_eq!(get_comp(comps_fr, "country").as_deref(), Some("France"));
    assert_eq!(
        a_fr.full.as_deref(),
        Some("12 Rue de l'Étoile\nBâtiment B, Étage 3\n93800 Épinay-sur-Seine\nFrance")
    );

    let a_de = &p_addrs["a_de"];
    let comps_de = a_de.components.as_ref().expect("german comps");
    assert_eq!(
        get_comp(comps_de, "name").as_deref(),
        Some("Goethestraße 42")
    );
    assert_eq!(get_comp(comps_de, "locality").as_deref(), Some("München"));
    assert_eq!(get_comp(comps_de, "region").as_deref(), Some("Bayern"));
    assert_eq!(
        get_comp(comps_de, "country").as_deref(),
        Some("Deutschland")
    );

    let a_ja = &p_addrs["a_ja"];
    let comps_ja = a_ja.components.as_ref().expect("japanese comps");
    assert_eq!(
        get_comp(comps_ja, "name").as_deref(),
        Some("千代田区千代田1-1")
    );
    assert_eq!(get_comp(comps_ja, "region").as_deref(), Some("東京都"));
    assert_eq!(get_comp(comps_ja, "country").as_deref(), Some("日本"));

    // Fixed-point stability
    let vcard2 = card_to_vcard(&parsed1);
    let parsed2 = vcard_to_card(&vcard2).expect("re-parse second pass");
    let vcard3 = card_to_vcard(&parsed2);
    assert_eq!(vcard2, vcard3);
}

#[test]
fn non_ascii_notes_nicknames_categories_and_spouse_roundtrip() {
    let mut notes = BTreeMap::new();
    notes.insert(
        "n1".to_owned(),
        Note {
            note: "München ist eine wunderschöne Stadt mit vielen Parks und Museen.\nRené & Hélène apprécient beaucoup la gastronomie française: café, croissants, crème brûlée.\n∀x ∈ ℝ: x² ≥ 0 (math symbols test 🧑‍💻🚀🌟)".to_owned(),
            extra: BTreeMap::new(),
        },
    );

    let mut nicknames = BTreeMap::new();
    nicknames.insert(
        "k1".to_owned(),
        Nickname {
            name: "Schätzchen".to_owned(),
            extra: BTreeMap::new(),
        },
    );
    nicknames.insert(
        "k2".to_owned(),
        Nickname {
            name: "Маша (Мария)".to_owned(),
            extra: BTreeMap::new(),
        },
    );
    nicknames.insert(
        "k3".to_owned(),
        Nickname {
            name: "たなか (田中)".to_owned(),
            extra: BTreeMap::new(),
        },
    );

    let mut keywords = BTreeMap::new();
    keywords.insert("Amis".to_owned(), json!(true));
    keywords.insert("Collègues de travail".to_owned(), json!(true));
    keywords.insert("Familie & Freunde".to_owned(), json!(true));
    keywords.insert("仕事関係".to_owned(), json!(true));
    keywords.insert("VIP ★ 🌟".to_owned(), json!(true));

    let mut related_to = BTreeMap::new();
    let mut spouse_rel = BTreeMap::new();
    spouse_rel.insert("spouse".to_owned(), json!(true));
    related_to.insert(
        "Hélène Müller-Mayer".to_owned(),
        Relation {
            relation: Some(spouse_rel),
            extra: BTreeMap::new(),
        },
    );

    let card = ContactCard {
        id: Some("C-NONASCII-MIXED".into()),
        notes: Some(notes),
        nicknames: Some(nicknames),
        keywords: Some(keywords),
        related_to: Some(related_to),
        ..ContactCard::default()
    };

    let vcard1 = card_to_vcard(&card);
    let parsed1 = vcard_to_card(&vcard1).expect("parse non-ascii mixed properties");

    let p_notes = parsed1.notes.as_ref().expect("notes present");
    assert_eq!(
        p_notes["n1"].note,
        "München ist eine wunderschöne Stadt mit vielen Parks und Museen.\nRené & Hélène apprécient beaucoup la gastronomie française: café, croissants, crème brûlée.\n∀x ∈ ℝ: x² ≥ 0 (math symbols test 🧑‍💻🚀🌟)"
    );

    let p_nicknames = parsed1.nicknames.as_ref().expect("nicknames present");
    assert_eq!(p_nicknames["k1"].name, "Schätzchen");
    assert_eq!(p_nicknames["k2"].name, "Маша (Мария)");
    assert_eq!(p_nicknames["k3"].name, "たなか (田中)");

    let p_keywords = parsed1.keywords.as_ref().expect("keywords present");
    assert!(p_keywords.contains_key("Amis"));
    assert!(p_keywords.contains_key("Collègues de travail"));
    assert!(p_keywords.contains_key("Familie & Freunde"));
    assert!(p_keywords.contains_key("仕事関係"));
    assert!(p_keywords.contains_key("VIP ★ 🌟"));

    let p_rel = parsed1.related_to.as_ref().expect("related_to present");
    assert!(p_rel.contains_key("Hélène Müller-Mayer"));
    assert!(states_spouse(
        "Hélène Müller-Mayer",
        &p_rel["Hélène Müller-Mayer"]
    ));

    // Fixed-point stability
    let vcard2 = card_to_vcard(&parsed1);
    let parsed2 = vcard_to_card(&vcard2).expect("re-parse second pass");
    let vcard3 = card_to_vcard(&parsed2);
    assert_eq!(vcard2, vcard3);
}

#[test]
fn inbound_vcard_charset_parameter_variations_and_normalization() {
    // Tests inbound vCards carrying CHARSET parameters across case variations
    // and multiple properties. vCard 3.0 specifies UTF-8 unconditionally, so
    // our reader accepts CHARSET=UTF-8 for compatibility with older/buggy clients,
    // while card_to_vcard normalizes outbound output to clean vCard 3.0 without
    // redundant CHARSET parameters.
    let vcard_inbound = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN;CHARSET=UTF-8:René Müller\r\n",
        "N;CHARSET=utf-8:Müller;René;François;Dr.;Jr.\r\n",
        "ORG;CHARSET=UTF-8;X-JMAP-KEY=o1:Société Générale;Pôle Innovation\r\n",
        "TITLE;CHARSET=Utf-8;X-JMAP-KEY=t1:Ingénieur en chef\r\n",
        "ROLE;charset=UTF-8;X-JMAP-KEY=t2:Développeur sénior\r\n",
        "NOTE;CHARSET=UTF-8;X-JMAP-KEY=n1:München ist schön\r\n",
        "NICKNAME;CHARSET=UTF-8;X-JMAP-KEY=k1:Schätzchen\r\n",
        "CATEGORIES;CHARSET=UTF-8:Amis,Collègues,Familie\r\n",
        "X-EVOLUTION-SPOUSE;CHARSET=UTF-8:Hélène Müller\r\n",
        "ADR;TYPE=HOME;CHARSET=UTF-8;X-JMAP-KEY=a1:;;12 Rue de l'Étoile;Paris;;75008;France\r\n",
        "LABEL;TYPE=HOME;CHARSET=UTF-8;X-JMAP-KEY=a1:12 Rue de l'Étoile\\n75008 Paris\\nFrance\r\n",
        "TEL;TYPE=WORK,VOICE;CHARSET=UTF-8;X-JMAP-KEY=p1:+33 1 23 45 67 89\r\n",
        "EMAIL;TYPE=INTERNET;CHARSET=UTF-8;X-JMAP-KEY=e1:rene.muller@example.fr\r\n",
        "END:VCARD\r\n",
    );

    let parsed1 =
        vcard_to_card(vcard_inbound).expect("parse inbound vcard with CHARSET parameters");

    // Assert accurate extraction into JSContact fields
    let name = parsed1.name.as_ref().expect("name present");
    assert_eq!(name.full.as_deref(), Some("René Müller"));
    let comps = name.components.as_ref().expect("name components");
    let get_comp = |kind: &str| {
        comps
            .iter()
            .find(|c| c.kind == kind)
            .map(|c| c.value.as_str())
    };
    assert_eq!(get_comp("given"), Some("René"));
    assert_eq!(get_comp("given2"), Some("François"));
    assert_eq!(get_comp("surname"), Some("Müller"));
    assert_eq!(get_comp("title"), Some("Dr."));
    assert_eq!(get_comp("credential"), Some("Jr."));

    let orgs = parsed1.organizations.as_ref().expect("orgs present");
    assert_eq!(orgs["o1"].name.as_deref(), Some("Société Générale"));
    assert_eq!(
        orgs["o1"].units.as_ref().unwrap(),
        &[OrgUnit::new("Pôle Innovation")]
    );

    let titles = parsed1.titles.as_ref().expect("titles present");
    assert_eq!(titles["t1"].name, "Ingénieur en chef");
    assert_eq!(titles["t1"].kind, None); // default kind "title"
    assert_eq!(titles["t2"].name, "Développeur sénior");
    assert_eq!(titles["t2"].kind.as_deref(), Some("role"));

    let notes = parsed1.notes.as_ref().expect("notes present");
    assert_eq!(notes["n1"].note, "München ist schön");

    let nicks = parsed1.nicknames.as_ref().expect("nicknames present");
    assert_eq!(nicks["k1"].name, "Schätzchen");

    let cats = parsed1.keywords.as_ref().expect("keywords present");
    assert!(cats.contains_key("Amis"));
    assert!(cats.contains_key("Collègues"));
    assert!(cats.contains_key("Familie"));

    let rels = parsed1.related_to.as_ref().expect("related_to present");
    assert!(rels.contains_key("Hélène Müller"));

    let addrs = parsed1.addresses.as_ref().expect("addresses present");
    assert_eq!(
        addrs["a1"].full.as_deref(),
        Some("12 Rue de l'Étoile\n75008 Paris\nFrance")
    );

    let phones = parsed1.phones.as_ref().expect("phones present");
    assert_eq!(phones["p1"].number, "+33 1 23 45 67 89");

    let emails = parsed1.emails.as_ref().expect("emails present");
    assert_eq!(emails["e1"].address, "rene.muller@example.fr");

    // Outbound normalization: standard vCard 3.0 emission must NOT include CHARSET=UTF-8
    let vcard_out = card_to_vcard(&parsed1);
    assert!(
        !vcard_out.contains("CHARSET="),
        "Outbound vCard 3.0 must not carry CHARSET params: {vcard_out}"
    );
    assert!(
        !vcard_out.contains("charset="),
        "Outbound vCard 3.0 must not carry charset params: {vcard_out}"
    );
    assert!(vcard_out.contains("FN:René Müller\r\n"), "{vcard_out}");
    assert!(
        vcard_out.contains("N:Müller;René;François;Dr.;Jr.\r\n"),
        "{vcard_out}"
    );
    assert!(
        vcard_out.contains("ORG;X-JMAP-KEY=o1:Société Générale;Pôle Innovation\r\n"),
        "{vcard_out}"
    );
    assert!(
        vcard_out.contains("NOTE;X-JMAP-KEY=n1:München ist schön\r\n"),
        "{vcard_out}"
    );

    // Fixed-point convergence
    let parsed2 = vcard_to_card(&vcard_out).expect("re-parse outbound vcard");
    let vcard_out2 = card_to_vcard(&parsed2);
    assert_eq!(
        vcard_out, vcard_out2,
        "Emitted vCard must reach fixed point"
    );
}

#[test]
fn inbound_vcard_quoted_printable_encoding_with_charset_utf8_and_latin1() {
    // Tests inbound legacy/vCard 2.1 QUOTED-PRINTABLE encoded properties.
    // 1. QP with explicit CHARSET=UTF-8 (UTF-8 multi-byte octets in =XX)
    // 2. QP with explicit CHARSET=ISO-8859-1 (Single byte Latin-1 in =XX)
    // 3. QP with explicit CHARSET=WINDOWS-1252
    // 4. QP without CHARSET parameter (defaults to Latin-1 per vCard 2.1 RFC 2045)
    let vcard_qp_utf8 = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN;ENCODING=QUOTED-PRINTABLE;CHARSET=UTF-8:Ren=C3=A9=20M=C3=BCller\r\n",
        "N;ENCODING=QUOTED-PRINTABLE;CHARSET=UTF-8:M=C3=BCller;Ren=C3=A9;Fran=C3=A7ois;Dr.;\r\n",
        "ORG;ENCODING=QUOTED-PRINTABLE;CHARSET=UTF-8;X-JMAP-KEY=o1:Soci=C3=A9t=C3=A9=20G=C3=A9n=C3=A9rale;P=C3=B4le=20R&D\r\n",
        "TITLE;ENCODING=QUOTED-PRINTABLE;CHARSET=UTF-8;X-JMAP-KEY=t1:Ing=C3=A9nieur=20en=20chef\r\n",
        "NOTE;ENCODING=QUOTED-PRINTABLE;CHARSET=UTF-8;X-JMAP-KEY=n1:M=C3=BCnchen=20ist=20eine=20sch=C3=B6ne=20Stadt\r\n",
        "X-EVOLUTION-SPOUSE;ENCODING=QUOTED-PRINTABLE;CHARSET=UTF-8:H=C3=A9l=C3=A8ne\r\n",
        "END:VCARD\r\n",
    );

    let parsed_utf8 = vcard_to_card(vcard_qp_utf8).expect("parse QP with CHARSET=UTF-8");
    let name_utf8 = parsed_utf8.name.as_ref().expect("name present");
    assert_eq!(name_utf8.full.as_deref(), Some("René Müller"));
    let comps_utf8 = name_utf8.components.as_ref().expect("components");
    let get_comp_utf8 = |kind: &str| {
        comps_utf8
            .iter()
            .find(|c| c.kind == kind)
            .map(|c| c.value.as_str())
    };
    assert_eq!(get_comp_utf8("given"), Some("René"));
    assert_eq!(get_comp_utf8("given2"), Some("François"));
    assert_eq!(get_comp_utf8("surname"), Some("Müller"));
    assert_eq!(get_comp_utf8("title"), Some("Dr."));

    let orgs_utf8 = parsed_utf8.organizations.as_ref().expect("orgs present");
    assert_eq!(orgs_utf8["o1"].name.as_deref(), Some("Société Générale"));
    assert_eq!(
        orgs_utf8["o1"].units.as_ref().unwrap(),
        &[OrgUnit::new("Pôle R&D")]
    );

    let titles_utf8 = parsed_utf8.titles.as_ref().expect("titles present");
    assert_eq!(titles_utf8["t1"].name, "Ingénieur en chef");

    let notes_utf8 = parsed_utf8.notes.as_ref().expect("notes present");
    assert_eq!(notes_utf8["n1"].note, "München ist eine schöne Stadt");

    let rels_utf8 = parsed_utf8.related_to.as_ref().expect("related_to present");
    assert!(rels_utf8.contains_key("Hélène"));

    // 2. QP with CHARSET=ISO-8859-1 (0xE9 -> é, 0xFC -> ü, 0xF4 -> ô, 0xE8 -> è)
    let vcard_qp_latin1 = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN;ENCODING=QUOTED-PRINTABLE;CHARSET=ISO-8859-1:Ren=E9=20M=FCller\r\n",
        "N;ENCODING=QUOTED-PRINTABLE;CHARSET=ISO-8859-1:M=FCller;Ren=E9;;;\r\n",
        "NOTE;ENCODING=QUOTED-PRINTABLE;CHARSET=ISO-8859-1;X-JMAP-KEY=n1:M=FCnchen=20sch=F6n\r\n",
        "END:VCARD\r\n",
    );

    let parsed_latin1 = vcard_to_card(vcard_qp_latin1).expect("parse QP with CHARSET=ISO-8859-1");
    let name_latin1 = parsed_latin1.name.as_ref().expect("name present");
    assert_eq!(name_latin1.full.as_deref(), Some("René Müller"));
    assert_eq!(
        parsed_latin1.notes.as_ref().unwrap()["n1"].note,
        "München schön"
    );

    // 3. QP with CHARSET=WINDOWS-1252
    let vcard_qp_win1252 = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN;ENCODING=QUOTED-PRINTABLE;CHARSET=WINDOWS-1252:Ren=E9=20M=FCller\r\n",
        "END:VCARD\r\n",
    );
    let parsed_win1252 =
        vcard_to_card(vcard_qp_win1252).expect("parse QP with CHARSET=WINDOWS-1252");
    assert_eq!(
        parsed_win1252.name.as_ref().unwrap().full.as_deref(),
        Some("René Müller")
    );

    // 4. QP without CHARSET parameter (defaults to Latin-1)
    let vcard_qp_no_charset = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN;ENCODING=QUOTED-PRINTABLE:Ren=E9=20M=FCller\r\n",
        "END:VCARD\r\n",
    );
    let parsed_no_charset =
        vcard_to_card(vcard_qp_no_charset).expect("parse QP without CHARSET parameter");
    assert_eq!(
        parsed_no_charset.name.as_ref().unwrap().full.as_deref(),
        Some("René Müller")
    );

    // Outbound emission of QP-decoded cards: must emit clean standard vCard 3.0 UTF-8
    let vcard_out = card_to_vcard(&parsed_utf8);
    assert!(
        !vcard_out.contains("ENCODING="),
        "Outbound vCard 3.0 must not carry ENCODING=QP: {vcard_out}"
    );
    assert!(
        !vcard_out.contains("QUOTED-PRINTABLE"),
        "Outbound vCard 3.0 must not carry QUOTED-PRINTABLE: {vcard_out}"
    );
    assert!(vcard_out.contains("FN:René Müller\r\n"), "{vcard_out}");

    // Fixed-point stability
    let parsed_out = vcard_to_card(&vcard_out).expect("re-parse outbound vcard");
    let vcard_out2 = card_to_vcard(&parsed_out);
    assert_eq!(vcard_out, vcard_out2);
}

#[test]
fn inbound_vcard_quoted_printable_soft_line_breaks_and_escaped_delimiters() {
    // Tests QUOTED-PRINTABLE soft line breaks (=\r\n and =\n) and encoded delimiters:
    // =3D ('='), =3B (';'), =2C (','), =0D=0A (CRLF).
    let vcard_qp_soft_breaks = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN;ENCODING=QUOTED-PRINTABLE;CHARSET=UTF-8:Alice=\r\n",
        "=20Smith\r\n",
        "NOTE;ENCODING=QUOTED-PRINTABLE;CHARSET=UTF-8;X-JMAP-KEY=n1:This is a long note that was folded=\r\n",
        "=20using quoted-printable soft line breaks=\r\n",
        "=20and contains an equals sign (=3D) and a semicolon (=3B).\r\n",
        "ADR;TYPE=HOME;ENCODING=QUOTED-PRINTABLE;CHARSET=UTF-8;X-JMAP-KEY=a1:;;12 Rue de l'=C3=89toile;Paris;;75008;France\r\n",
        "END:VCARD\r\n",
    );

    let parsed = vcard_to_card(vcard_qp_soft_breaks).expect("parse QP with soft line breaks");
    assert_eq!(
        parsed.name.as_ref().unwrap().full.as_deref(),
        Some("Alice Smith")
    );

    let note = &parsed.notes.as_ref().unwrap()["n1"].note;
    assert_eq!(
        note,
        "This is a long note that was folded using quoted-printable soft line breaks and contains an equals sign (=) and a semicolon (;)."
    );

    let addrs = parsed.addresses.as_ref().unwrap();
    let street = addrs["a1"]
        .components
        .as_ref()
        .unwrap()
        .iter()
        .find(|c| c.kind == "name")
        .unwrap();
    assert_eq!(street.value, "12 Rue de l'Étoile");

    // Outbound emission and fixed-point convergence
    let vcard_out = card_to_vcard(&parsed);
    assert!(!vcard_out.contains("ENCODING="));
    let parsed2 = vcard_to_card(&vcard_out).expect("re-parse emitted vcard");
    let vcard_out2 = card_to_vcard(&parsed2);
    assert_eq!(vcard_out, vcard_out2);
}

#[test]
fn inbound_vcard_encoding_parameter_8bit_7bit_and_base64_fidelity() {
    // Tests ENCODING=8BIT, ENCODING=7BIT, ENCODING=b, ENCODING=BASE64, ENCODING=B
    let vcard_encodings = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN;ENCODING=8BIT:René Müller\r\n",
        "N;ENCODING=8BIT:Müller;René;;;\r\n",
        "NOTE;ENCODING=7BIT;X-JMAP-KEY=n1:ASCII note content\r\n",
        "PHOTO;ENCODING=b;TYPE=JPEG:AQIDBA==\r\n",
        "END:VCARD\r\n",
    );

    let parsed =
        vcard_to_card(vcard_encodings).expect("parse vcard with 8bit, 7bit, and b encodings");
    assert_eq!(
        parsed.name.as_ref().unwrap().full.as_deref(),
        Some("René Müller")
    );
    assert_eq!(
        parsed.notes.as_ref().unwrap()["n1"].note,
        "ASCII note content"
    );

    let media = parsed.media.as_ref().expect("media present");
    assert_eq!(media["m1"].kind.as_deref(), Some("photo"));
    assert_eq!(media["m1"].media_type.as_deref(), Some("image/JPEG"));
    assert_eq!(media["m1"].uri, "data:image/JPEG;base64,AQIDBA==");

    // Outbound emission: ENCODING=8BIT and 7BIT are stripped on text; ENCODING=b is emitted on photo
    let vcard_out = card_to_vcard(&parsed);
    assert_eq!(line(&vcard_out, "FN:"), "FN:René Müller");
    assert_eq!(
        line(&vcard_out, "NOTE"),
        "NOTE;X-JMAP-KEY=n1:ASCII note content"
    );
    assert!(line(&vcard_out, "PHOTO").starts_with("PHOTO;"));
    assert!(line(&vcard_out, "PHOTO").contains("ENCODING=b"));
    assert!(line(&vcard_out, "PHOTO").contains("TYPE=JPEG"));
    assert!(line(&vcard_out, "PHOTO").ends_with(":AQIDBA=="));

    // Fixed-point stability
    let parsed2 = vcard_to_card(&vcard_out).expect("re-parse emitted vcard");
    let vcard_out2 = card_to_vcard(&parsed2);
    assert_eq!(vcard_out, vcard_out2);
}

#[test]
fn photo_inline_base64_media_type_variants_and_roundtrip() {
    // Characterizes inline base64 PHOTO roundtrip fidelity across 10 image formats:
    // JPEG, PNG, GIF, WebP, SVG, BMP, TIFF, AVIF, HEIC, Icon.
    let subtypes = [
        ("image/jpeg", "jpeg"),
        ("image/png", "png"),
        ("image/gif", "gif"),
        ("image/webp", "webp"),
        ("image/svg+xml", "svg+xml"),
        ("image/bmp", "bmp"),
        ("image/tiff", "tiff"),
        ("image/avif", "avif"),
        ("image/heic", "heic"),
        ("image/x-icon", "x-icon"),
    ];

    for (media_type, expected_subtype) in subtypes {
        let card = one_photo(
            &format!("data:{media_type};base64,{PAYLOAD}"),
            Some(media_type),
        );
        let vcard = card_to_vcard(&card);
        let photo_str = line(&vcard, "PHOTO;");
        assert!(
            photo_str.contains(&format!("TYPE={expected_subtype}")),
            "{media_type} must state TYPE={expected_subtype} on line: {photo_str}"
        );
        assert!(
            photo_str.contains("ENCODING=b"),
            "Inline photo must state ENCODING=b: {photo_str}"
        );
        assert!(
            photo_str.contains("X-JMAP-KEY=m1"),
            "Inline photo must state X-JMAP-KEY: {photo_str}"
        );
        assert!(
            photo_str.ends_with(&format!(":{PAYLOAD}")),
            "Inline photo must end with payload: {photo_str}"
        );

        // Parse back and verify exact media_type and URI reconstruction
        let parsed = vcard_to_card(&vcard).expect("parse emitted photo vcard");
        let media = parsed.media.as_ref().expect("media present");
        let entry = &media["m1"];
        assert_eq!(entry.kind.as_deref(), Some("photo"));
        assert_eq!(
            entry.media_type.as_deref(),
            Some(media_type),
            "media_type must roundtrip intact for {media_type}"
        );
        assert_eq!(
            entry.uri,
            format!("data:{media_type};base64,{PAYLOAD}"),
            "URI must be reconstructed with exact media_type for {media_type}"
        );

        // Fixed-point convergence
        let vcard2 = card_to_vcard(&parsed);
        let parsed2 = vcard_to_card(&vcard2).expect("second parse");
        let vcard3 = card_to_vcard(&parsed2);
        assert_eq!(
            vcard2, vcard3,
            "vCard must reach fixed point for {media_type}"
        );
    }

    // Inbound uppercase / mixed-case parameters and legacy ENCODING=BASE64
    let vcard_casing = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Photo Case\r\n",
        "PHOTO;TYPE=JPEG;ENCODING=b;X-JMAP-KEY=m1:aGVsbG8tcGhvdG8=\r\n",
        "PHOTO;ENCODING=BASE64;TYPE=PNG;X-JMAP-KEY=m2:aGVsbG8tcGhvdG8=\r\n",
        "PHOTO;encoding=b;type=gif;x-jmap-key=m3:aGVsbG8tcGhvdG8=\r\n",
        "END:VCARD\r\n",
    );
    let parsed_casing = vcard_to_card(vcard_casing).expect("parse casing vcard");
    let media_casing = parsed_casing.media.as_ref().expect("media present");
    assert_eq!(media_casing["m1"].media_type.as_deref(), Some("image/JPEG"));
    assert_eq!(media_casing["m2"].media_type.as_deref(), Some("image/PNG"));
    assert_eq!(media_casing["m3"].media_type.as_deref(), Some("image/gif"));

    let vcard_casing_out = card_to_vcard(&parsed_casing);
    let parsed_casing2 = vcard_to_card(&vcard_casing_out).expect("reparse casing");
    let vcard_casing_out2 = card_to_vcard(&parsed_casing2);
    assert_eq!(vcard_casing_out, vcard_casing_out2);
}

#[test]
fn photo_uri_variant_and_lossy_media_type_characterization() {
    // 1. Valid URI variants: HTTPS, HTTP, file://, URN
    let test_uris = [
        "https://example.com/alice.jpg",
        "http://photos.org/pic.png?size=large&format=raw",
        "file:///home/user/.face",
        "urn:uuid:6a2f7965-728b-47e2-8924-d2e7d7ffba44",
    ];

    for uri in test_uris {
        let card = one_photo(uri, None);
        let vcard = card_to_vcard(&card);
        let unf = unfolded(&vcard);
        let photo_str = line(&unf, "PHOTO;");
        assert_eq!(
            photo_str,
            format!("PHOTO;X-JMAP-KEY=m1;VALUE=uri:{uri}"),
            "URI photo must emit VALUE=uri and key without TYPE or ENCODING"
        );
        assert!(
            !vcard.contains("ENCODING="),
            "URI photo must not have ENCODING: {vcard}"
        );
        assert!(
            !vcard.contains("TYPE="),
            "URI photo must not have TYPE: {vcard}"
        );

        // Parse back
        let parsed = vcard_to_card(&vcard).expect("parse URI photo");
        let media = parsed.media.as_ref().expect("media present");
        let entry = &media["m1"];
        assert_eq!(entry.kind.as_deref(), Some("photo"));
        assert_eq!(entry.uri, uri);
        assert_eq!(entry.media_type, None);

        // Fixed-point convergence
        let vcard2 = card_to_vcard(&parsed);
        assert_eq!(vcard, vcard2);
    }

    // 2. Inbound parameter variations: VALUE=URI, value=uri, mixed case
    for val_param in ["VALUE=URI", "value=uri", "Value=Uri", "VALUE=uri"] {
        let vcard_in = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Uri Photo\r\nPHOTO;{val_param};X-JMAP-KEY=m1:https://cdn.example.com/avatar.webp\r\nEND:VCARD\r\n"
        );
        let parsed = vcard_to_card(&vcard_in).expect("parse uri photo variation");
        let media = parsed.media.as_ref().expect("media present");
        assert_eq!(media["m1"].kind.as_deref(), Some("photo"));
        assert_eq!(media["m1"].uri, "https://cdn.example.com/avatar.webp");
        assert_eq!(media["m1"].media_type, None);

        let vcard_out = card_to_vcard(&parsed);
        assert_eq!(
            line(&vcard_out, "PHOTO;"),
            "PHOTO;X-JMAP-KEY=m1;VALUE=uri:https://cdn.example.com/avatar.webp"
        );
    }

    // 3. Lossy-by-design characterization: media_type on remote URI
    // In JSContact, a remote URI may specify media_type: Some("image/jpeg").
    // But RFC 2426 §3.1.4 states no TYPE on URI references, and EDS reads no TYPE off URI lines.
    // Therefore, card_to_vcard omits TYPE by design, which reads back as media_type: None.
    let card_with_media_type = one_photo("https://example.org/photo.jpg", Some("image/jpeg"));
    let vcard_lossy = card_to_vcard(&card_with_media_type);
    assert_eq!(
        line(&vcard_lossy, "PHOTO;"),
        "PHOTO;X-JMAP-KEY=m1;VALUE=uri:https://example.org/photo.jpg"
    );
    assert!(
        !vcard_lossy.contains("TYPE="),
        "TYPE omitted on URI by design"
    );

    let parsed_lossy = vcard_to_card(&vcard_lossy).expect("parse lossy vcard");
    let entry_lossy = &parsed_lossy.media.as_ref().unwrap()["m1"];
    assert_eq!(
        entry_lossy.media_type, None,
        "media_type on remote URI is unstated across vCard 3.0"
    );
    assert_eq!(entry_lossy.uri, "https://example.org/photo.jpg");

    // Subsequent roundtrips reach fixed point
    let vcard_lossy2 = card_to_vcard(&parsed_lossy);
    assert_eq!(vcard_lossy, vcard_lossy2);
}

#[test]
fn photo_eds_photo_field_replacements_and_same_photo_equality() {
    // 1. same_photo equality comparisons:
    let inline_jpeg = Media {
        kind: Some("photo".to_owned()),
        uri: format!("data:image/jpeg;base64,{PAYLOAD}"),
        media_type: Some("image/jpeg".to_owned()),
        extra: BTreeMap::new(),
    };
    let inline_jpeg_caps = Media {
        kind: Some("photo".to_owned()),
        uri: format!("data:image/JPEG;base64,{PAYLOAD}"),
        media_type: Some("image/JPEG".to_owned()),
        extra: BTreeMap::new(),
    };
    let inline_jpeg_unpadded = Media {
        kind: Some("photo".to_owned()),
        uri: format!("data:image/jpeg;base64,{}", PAYLOAD.trim_end_matches('=')),
        media_type: Some("image/jpeg".to_owned()),
        extra: BTreeMap::new(),
    };
    let inline_png = Media {
        kind: Some("photo".to_owned()),
        uri: format!("data:image/png;base64,{PAYLOAD}"),
        media_type: Some("image/png".to_owned()),
        extra: BTreeMap::new(),
    };
    let inline_diff_payload = Media {
        kind: Some("photo".to_owned()),
        uri: "data:image/jpeg;base64,AQIDBA==".to_owned(),
        media_type: Some("image/jpeg".to_owned()),
        extra: BTreeMap::new(),
    };
    let uri_photo1 = Media {
        kind: Some("photo".to_owned()),
        uri: "https://example.com/alice.jpg".to_owned(),
        media_type: None,
        extra: BTreeMap::new(),
    };
    let uri_photo2 = Media {
        kind: Some("photo".to_owned()),
        uri: "https://example.com/bob.jpg".to_owned(),
        media_type: None,
        extra: BTreeMap::new(),
    };
    let non_photo = Media {
        kind: Some("logo".to_owned()),
        uri: "https://example.com/logo.png".to_owned(),
        media_type: None,
        extra: BTreeMap::new(),
    };

    // Inline comparisons
    assert!(same_photo(&inline_jpeg, &inline_jpeg), "identical inline");
    assert!(
        same_photo(&inline_jpeg, &inline_jpeg_caps),
        "case-insensitive subtype"
    );
    assert!(
        same_photo(&inline_jpeg, &inline_jpeg_unpadded),
        "unpadded vs padded base64"
    );
    assert!(!same_photo(&inline_jpeg, &inline_png), "different subtypes");
    assert!(
        !same_photo(&inline_jpeg, &inline_diff_payload),
        "different payload"
    );

    // URI comparisons
    assert!(same_photo(&uri_photo1, &uri_photo1), "identical URI");
    assert!(!same_photo(&uri_photo1, &uri_photo2), "different URI");

    // Cross-variant and non-photo comparisons
    assert!(!same_photo(&inline_jpeg, &uri_photo1), "inline vs URI");
    assert!(
        same_photo(&non_photo, &non_photo),
        "both non-photos evaluate to None"
    );
    assert!(
        !same_photo(&inline_jpeg, &non_photo),
        "inline photo vs non-photo"
    );

    // 2. EDS photo replacement simulation:
    // When a user changes photo in Evolution UI, EDS rewrites the PHOTO line with no X-JMAP-KEY.
    let initial_card = one_photo(
        &format!("data:image/jpeg;base64,{PAYLOAD}"),
        Some("image/jpeg"),
    );
    let initial_vcard = card_to_vcard(&initial_card);
    assert!(initial_vcard.contains("X-JMAP-KEY=m1"));

    // User chooses a new PNG photo in Evolution
    let eds_edited_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\n",
        "PHOTO;TYPE=png;ENCODING=b:iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==\r\n",
        "END:VCARD\r\n",
    );
    let parsed_eds = vcard_to_card(eds_edited_vcard).expect("parse EDS photo change");
    let media = parsed_eds.media.as_ref().expect("media present");
    let new_entry = &media["m1"];
    assert_eq!(new_entry.media_type.as_deref(), Some("image/png"));

    // same_photo correctly identifies that the user changed the photo
    assert!(
        !same_photo(&initial_card.media.unwrap()["m1"], new_entry),
        "same_photo detects photo update"
    );
}

#[test]
fn photo_multiple_coexisting_entries_and_non_photo_media_filtering() {
    // 1. Multiple PHOTO entries on one card (e.g. inline avatar + remote full picture + inline thumbnail)
    let mut media_map = BTreeMap::new();
    media_map.insert(
        "m1".to_owned(),
        Media {
            kind: Some("photo".to_owned()),
            uri: format!("data:image/jpeg;base64,{PAYLOAD}"),
            media_type: Some("image/jpeg".to_owned()),
            extra: BTreeMap::new(),
        },
    );
    media_map.insert(
        "m2".to_owned(),
        Media {
            kind: Some("photo".to_owned()),
            uri: "https://example.com/profile-large.jpg".to_owned(),
            media_type: None,
            extra: BTreeMap::new(),
        },
    );
    media_map.insert(
        "m3".to_owned(),
        Media {
            kind: Some("photo".to_owned()),
            uri: "data:image/png;base64,AQIDBA==".to_owned(),
            media_type: Some("image/png".to_owned()),
            extra: BTreeMap::new(),
        },
    );

    let multi_card = ContactCard {
        id: Some("C-MULTI-PHOTO".into()),
        media: Some(media_map),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&multi_card);
    let photo_lines: Vec<&str> = vcard
        .split("\r\n")
        .filter(|l| l.starts_with("PHOTO;"))
        .collect();
    assert_eq!(
        photo_lines.len(),
        3,
        "All 3 PHOTO lines must be emitted: {vcard}"
    );
    assert!(
        photo_lines
            .iter()
            .any(|l| l.contains("X-JMAP-KEY=m1") && l.contains("TYPE=jpeg"))
    );
    assert!(
        photo_lines
            .iter()
            .any(|l| l.contains("X-JMAP-KEY=m2") && l.contains("VALUE=uri"))
    );
    assert!(
        photo_lines
            .iter()
            .any(|l| l.contains("X-JMAP-KEY=m3") && l.contains("TYPE=png"))
    );

    // Parse back
    let parsed = vcard_to_card(&vcard).expect("parse multi-photo vcard");
    let parsed_media = parsed.media.as_ref().expect("media present");
    assert_eq!(parsed_media.len(), 3);
    assert_eq!(parsed_media["m1"].media_type.as_deref(), Some("image/jpeg"));
    assert_eq!(
        parsed_media["m2"].uri,
        "https://example.com/profile-large.jpg"
    );
    assert_eq!(parsed_media["m3"].media_type.as_deref(), Some("image/png"));

    // 2. Non-photo media filtering: logos, sounds, documents, and unmapped kinds
    let mut mixed_media = BTreeMap::new();
    mixed_media.insert(
        "m1".to_owned(),
        Media {
            kind: Some("photo".to_owned()),
            uri: format!("data:image/jpeg;base64,{PAYLOAD}"),
            media_type: Some("image/jpeg".to_owned()),
            extra: BTreeMap::new(),
        },
    );
    mixed_media.insert(
        "m2".to_owned(),
        Media {
            kind: Some("logo".to_owned()),
            uri: "https://example.com/corp-logo.png".to_owned(),
            media_type: Some("image/png".to_owned()),
            extra: BTreeMap::new(),
        },
    );
    mixed_media.insert(
        "m3".to_owned(),
        Media {
            kind: Some("sound".to_owned()),
            uri: "https://example.com/pronunciation.ogg".to_owned(),
            media_type: Some("audio/ogg".to_owned()),
            extra: BTreeMap::new(),
        },
    );
    mixed_media.insert(
        "m4".to_owned(),
        Media {
            kind: Some("document".to_owned()),
            uri: "https://example.com/resume.pdf".to_owned(),
            media_type: Some("application/pdf".to_owned()),
            extra: BTreeMap::new(),
        },
    );
    mixed_media.insert(
        "m5".to_owned(),
        Media {
            kind: None,
            uri: "https://example.com/unknown.dat".to_owned(),
            media_type: None,
            extra: BTreeMap::new(),
        },
    );

    // Predicates
    assert!(states_media(&mixed_media["m1"]), "photo is stateable");
    assert!(
        !states_media(&mixed_media["m2"]),
        "logo is not stateable on PHOTO"
    );
    assert!(
        !states_media(&mixed_media["m3"]),
        "sound is not stateable on PHOTO"
    );
    assert!(
        !states_media(&mixed_media["m4"]),
        "document is not stateable on PHOTO"
    );
    assert!(
        !states_media(&mixed_media["m5"]),
        "none kind is not stateable"
    );

    let mixed_card = ContactCard {
        id: Some("C-MIXED-MEDIA".into()),
        media: Some(mixed_media),
        ..ContactCard::default()
    };
    let mixed_vcard = card_to_vcard(&mixed_card);
    assert_eq!(
        mixed_vcard.matches("\r\nPHOTO").count(),
        1,
        "Only the photo entry must emit a PHOTO line: {mixed_vcard}"
    );
    assert!(mixed_vcard.contains("X-JMAP-KEY=m1"));
    assert!(!mixed_vcard.contains("corp-logo.png"));
    assert!(!mixed_vcard.contains("pronunciation.ogg"));
    assert!(!mixed_vcard.contains("resume.pdf"));
}

#[test]
fn photo_edge_cases_empty_malformed_and_large_folded_payloads() {
    // 1. Non-image data URIs: data:application/pdf;base64,... and data:;base64,...
    let pdf_card = one_photo(
        &format!("data:application/pdf;base64,{PAYLOAD}"),
        Some("application/pdf"),
    );
    let pdf_vcard = card_to_vcard(&pdf_card);
    assert_eq!(
        line(&pdf_vcard, "PHOTO;"),
        format!("PHOTO;X-JMAP-KEY=m1;ENCODING=b:{PAYLOAD}"),
        "Non-image data URI emits without TYPE parameter"
    );
    let parsed_pdf = vcard_to_card(&pdf_vcard).expect("parse non-image photo");
    let pdf_entry = &parsed_pdf.media.as_ref().unwrap()["m1"];
    assert_eq!(pdf_entry.media_type, None);
    assert_eq!(pdf_entry.uri, format!("data:;base64,{PAYLOAD}"));

    // 2. EDS TYPE="X-EVOLUTION-UNKNOWN" parsing
    let eds_unknown_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Unknown MIME\r\n",
        "PHOTO;TYPE=\"X-EVOLUTION-UNKNOWN\";ENCODING=b:AQIDBA==\r\n",
        "END:VCARD\r\n",
    );
    let parsed_unknown = vcard_to_card(eds_unknown_vcard).expect("parse X-EVOLUTION-UNKNOWN");
    let unk_entry = &parsed_unknown.media.as_ref().unwrap()["m1"];
    assert_eq!(unk_entry.media_type, None);
    assert_eq!(unk_entry.uri, "data:;base64,AQIDBA==");

    // 3. Inbound empty / degenerate PHOTO lines produce no media entries
    for empty_line in [
        "PHOTO:",
        "PHOTO;ENCODING=b:",
        "PHOTO;VALUE=uri:",
        "PHOTO;TYPE=jpeg;ENCODING=b:",
        "PHOTO;X-JMAP-KEY=m1:",
    ] {
        let vcard_empty = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Empty Photo\r\n{empty_line}\r\nEND:VCARD\r\n"
        );
        let parsed_empty = vcard_to_card(&vcard_empty).expect("parse empty photo line");
        assert!(
            parsed_empty.media.is_none() || parsed_empty.media.as_ref().unwrap().is_empty(),
            "Degenerate line {empty_line:?} must produce no media entries"
        );
    }

    // 4. Invalid data URIs in JSContact are rejected by states_media and emit no line
    for invalid_uri in [
        "data:image/jpeg,%89PNG%0D%0A",
        "data:image/jpeg;base64,invalid base64!@#$",
        "data:image/jpeg",
        "data:",
        "",
    ] {
        let invalid_card = one_photo(invalid_uri, None);
        assert!(
            !states_media(&invalid_card.media.as_ref().unwrap()["m1"]),
            "{invalid_uri:?} must not be stateable"
        );
        let vcard_invalid = card_to_vcard(&invalid_card);
        assert!(
            !vcard_invalid.contains("PHOTO"),
            "{invalid_uri:?} must state no PHOTO line"
        );
    }

    // 5. Large 2KB binary image with multi-line 75-octet folding
    let mut raw_bytes = Vec::with_capacity(2048);
    // Construct simulated PNG binary payload
    raw_bytes.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    for i in 0..2040 {
        raw_bytes.push(((i * 37 + 13) % 256) as u8);
    }
    let encoded_payload = base64::engine::general_purpose::STANDARD.encode(&raw_bytes);
    let large_card = one_photo(
        &format!("data:image/png;base64,{encoded_payload}"),
        Some("image/png"),
    );

    let large_vcard = card_to_vcard(&large_card);
    let fold_count = large_vcard.matches("\r\n ").count();
    assert!(
        fold_count > 30,
        "2KB payload must fold across 30+ continuation lines (fold_count = {fold_count})"
    );

    let parsed_large = vcard_to_card(&large_vcard).expect("parse folded large photo");
    let large_entry = &parsed_large.media.as_ref().unwrap()["m1"];
    assert_eq!(large_entry.media_type.as_deref(), Some("image/png"));

    let parsed_bytes = base64::engine::general_purpose::STANDARD
        .decode(
            large_entry
                .uri
                .strip_prefix("data:image/png;base64,")
                .unwrap(),
        )
        .expect("decode reconstructed payload");
    assert_eq!(
        parsed_bytes, raw_bytes,
        "Binary payload must roundtrip byte-for-byte"
    );

    // Fixed-point stability
    let large_vcard2 = card_to_vcard(&parsed_large);
    let parsed_large2 = vcard_to_card(&large_vcard2).expect("second parse");
    let large_vcard3 = card_to_vcard(&parsed_large2);
    assert_eq!(
        large_vcard2, large_vcard3,
        "Large folded photo reaches fixed point"
    );
}

#[test]
fn standard_vcard_properties_dropped_by_design_characterization_and_rationale() {
    // Audit of standard vCard 3.0 properties that Evolution/EDS has no E_CONTACT_*
    // field or active editor UI for: GEO, TZ, MAILER, PRODID, REV, SORT-STRING, CLASS, SOUND, LOGO.
    //
    // Contract & Rationale:
    // 1. Inbound vCards containing these standard RFC 2426 properties parse safely without errors.
    // 2. Standard mapped properties (FN, N, EMAIL, TEL, ADR, ORG, TITLE, ROLE, NOTE, URL, PHOTO, etc.)
    //    are extracted with 100% fidelity.
    // 3. Unmodeled standard properties are NOT synthesized into JSContact models or extra maps,
    //    preventing pollution of standard JMAP JSON schemas.
    // 4. Outbound serialization emits clean standard vCard 3.0 without unmapped property lines.
    // 5. Exporter-owned metadata (PRODID, REV) and client-owned flags (MAILER, CLASS) are not preserved
    //    across saves, ensuring server-side updated timestamps and generator integrity remain authoritative.
    // 6. Roundtrip operations achieve fixed-point convergence (card2 == card and vcard2 == vcard3).

    let vcard_full = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:std-prop-contact-001\r\n",
        "X-JMAP-UID:550e8400-e29b-41d4-a716-446655440000\r\n",
        "FN:Dr. Albert Einstein\r\n",
        "N:Einstein;Albert;;Dr.;\r\n",
        "NICKNAME;X-JMAP-KEY=k1:Bertie\r\n",
        "EMAIL;TYPE=WORK,PREF;X-JMAP-KEY=e1:albert.einstein@ias.edu\r\n",
        "TEL;TYPE=WORK,VOICE;X-JMAP-KEY=p1:+1-609-734-8000\r\n",
        "ADR;TYPE=WORK;X-JMAP-KEY=a1:;;1 Einstein Dr;Princeton;NJ;08540;USA\r\n",
        "LABEL;TYPE=WORK;X-JMAP-KEY=a1:1 Einstein Dr\\nPrinceton\\, NJ 08540\\nUSA\r\n",
        "ORG;X-JMAP-KEY=o1:Institute for Advanced Study;School of Natural Sciences\r\n",
        "TITLE;X-JMAP-KEY=t1:Professor of Theoretical Physics\r\n",
        "ROLE;X-JMAP-KEY=t2:Researcher\r\n",
        "NOTE;X-JMAP-KEY=n1:General Relativity and Quantum Mechanics\r\n",
        "URL;X-JMAP-KEY=l1:https://www.ias.edu/scholars/einstein\r\n",
        "CALURI;X-JMAP-KEY=c1:https://cal.ias.edu/einstein\r\n",
        "FBURL;X-JMAP-KEY=c2:https://cal.ias.edu/freebusy/einstein\r\n",
        "PHOTO;TYPE=JPEG;ENCODING=b;X-JMAP-KEY=m1:/9j/4AAQSkZJRg==\r\n",
        "CATEGORIES:Physics,Nobel Laureate,Relativity\r\n",
        "BDAY;X-JMAP-KEY=y1:1879-03-14\r\n",
        "X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=y2:1903-01-06\r\n",
        "X-EVOLUTION-SPOUSE:Mileva Marić\r\n",
        "X-JABBER;TYPE=WORK;X-JMAP-KEY=s1:einstein@jabber.ias.edu\r\n",
        // Standard unmapped properties to audit:
        "GEO:40.331575;-74.667232\r\n",
        "TZ:-05:00\r\n",
        "MAILER:Evolution 3.52 / JMAP Client\r\n",
        "PRODID:-//Institute for Advanced Study//IAS Contacts 3.0//EN\r\n",
        "REV:2026-08-22T00:07:47Z\r\n",
        "SORT-STRING:Einstein\r\n",
        "CLASS:CONFIDENTIAL\r\n",
        "SOUND;TYPE=BASIC;ENCODING=b:AQIDBA==\r\n",
        "SOUND;VALUE=uri:https://example.com/einstein_pronunciation.wav\r\n",
        "LOGO;TYPE=PNG;ENCODING=b:iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==\r\n",
        "LOGO;VALUE=uri:https://www.ias.edu/logo.png\r\n",
        "END:VCARD\r\n",
    );

    let parsed =
        vcard_to_card(vcard_full).expect("parse vcard containing standard unmapped properties");

    // Mapped properties are 100% intact
    assert_eq!(parsed.id.as_ref().unwrap().as_str(), "std-prop-contact-001");
    assert_eq!(
        parsed.uid.as_deref(),
        Some("550e8400-e29b-41d4-a716-446655440000")
    );
    assert_eq!(
        parsed.name.as_ref().unwrap().full.as_deref(),
        Some("Dr. Albert Einstein")
    );
    assert_eq!(
        parsed.nicknames.as_ref().unwrap()["k1"].name.as_str(),
        "Bertie"
    );
    assert_eq!(
        parsed.emails.as_ref().unwrap()["e1"].address.as_str(),
        "albert.einstein@ias.edu"
    );
    assert_eq!(
        parsed.phones.as_ref().unwrap()["p1"].number.as_str(),
        "+1-609-734-8000"
    );
    assert_eq!(
        parsed.organizations.as_ref().unwrap()["o1"].name.as_deref(),
        Some("Institute for Advanced Study")
    );
    assert_eq!(
        parsed.notes.as_ref().unwrap()["n1"].note.as_str(),
        "General Relativity and Quantum Mechanics"
    );
    assert_eq!(
        parsed.links.as_ref().unwrap()["l1"].uri.as_str(),
        "https://www.ias.edu/scholars/einstein"
    );
    assert_eq!(
        parsed.calendars.as_ref().unwrap()["c1"].uri.as_str(),
        "https://cal.ias.edu/einstein"
    );
    assert_eq!(
        parsed.anniversaries.as_ref().unwrap()["y1"].kind.as_str(),
        "birth"
    );
    assert!(
        parsed
            .related_to
            .as_ref()
            .unwrap()
            .contains_key("Mileva Marić")
    );
    assert_eq!(
        parsed.online_services.as_ref().unwrap()["s1"]
            .service
            .as_deref(),
        Some("Jabber")
    );

    // Media contains ONLY the PHOTO — SOUND and LOGO lines are safely ignored
    let media = parsed.media.as_ref().expect("media map present");
    assert_eq!(media.len(), 1, "Only PHOTO is read into media map");
    let photo = &media["m1"];
    assert_eq!(photo.kind.as_deref(), Some("photo"));
    assert_eq!(photo.media_type.as_deref(), Some("image/JPEG"));
    assert_eq!(
        photo.uri.as_str(),
        "data:image/JPEG;base64,/9j/4AAQSkZJRg=="
    );

    // Outbound vCard contains all mapped lines and none of the unmapped standard lines
    let emitted = card_to_vcard(&parsed);
    assert!(emitted.contains("FN:Dr. Albert Einstein\r\n"));
    assert!(emitted.contains("PHOTO;X-JMAP-KEY=m1;TYPE=JPEG;ENCODING=b:/9j/4AAQSkZJRg==\r\n"));
    assert!(!emitted.contains("GEO:"), "GEO must not be emitted");
    assert!(!emitted.contains("TZ:"), "TZ must not be emitted");
    assert!(!emitted.contains("MAILER:"), "MAILER must not be emitted");
    assert!(!emitted.contains("PRODID:"), "PRODID must not be emitted");
    assert!(!emitted.contains("REV:"), "REV must not be emitted");
    assert!(
        !emitted.contains("SORT-STRING:"),
        "SORT-STRING must not be emitted"
    );
    assert!(!emitted.contains("CLASS:"), "CLASS must not be emitted");
    assert!(!emitted.contains("SOUND"), "SOUND must not be emitted");
    assert!(!emitted.contains("LOGO"), "LOGO must not be emitted");

    // Fixed-point convergence
    let parsed2 = vcard_to_card(&emitted).expect("second parse");
    let emitted2 = card_to_vcard(&parsed2);
    let parsed3 = vcard_to_card(&emitted2).expect("third parse");
    let emitted3 = card_to_vcard(&parsed3);
    assert_eq!(
        parsed2, parsed3,
        "ContactCard reaches fixed point on second pass"
    );
    assert_eq!(
        emitted2, emitted3,
        "Emitted vCard reaches fixed point on second pass"
    );
}

#[test]
fn standard_properties_individual_variations_and_parameters() {
    // Verify parsing tolerance and safe omission across individual standard properties:
    // GEO, TZ, MAILER, PRODID, REV, SORT-STRING, CLASS, SOUND, LOGO with parameters.

    // 1. GEO variations: lat;lon, signed floats, precision, custom parameters
    for geo_val in [
        "GEO:37.386013;-122.082932",
        "GEO:-33.8688;151.2093",
        "GEO:0.0;0.0",
        "GEO;X-PRECISION=HIGH:51.5074;-0.1278",
        "GEO;VALUE=text:48.8566;2.3522",
    ] {
        let vcard =
            format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Geo User\r\n{geo_val}\r\nEND:VCARD\r\n");
        let parsed = vcard_to_card(&vcard).expect("parse GEO");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Geo User")
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("GEO:") && !emitted.contains("GEO;"),
            "GEO must not be emitted"
        );
    }

    // 2. TZ variations: UTC offsets, IANA text names, abbreviations, lowercase
    for tz_val in [
        "TZ:-05:00",
        "TZ:+01:00",
        "TZ:+05:30",
        "TZ;VALUE=text:America/New_York",
        "TZ;VALUE=text:Europe/Zurich",
        "TZ;VALUE=TEXT:EST",
        "TZ;VALUE=text:UTC",
        "tz:America/Tokyo",
    ] {
        let vcard =
            format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:TZ User\r\n{tz_val}\r\nEND:VCARD\r\n");
        let parsed = vcard_to_card(&vcard).expect("parse TZ");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("TZ User")
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("TZ:") && !emitted.contains("TZ;"),
            "TZ must not be emitted"
        );
    }

    // 3. MAILER variations: client identifiers and version strings
    for mailer_val in [
        "MAILER:Evolution 3.52.0",
        "MAILER:Mozilla Thunderbird 128.0",
        "MAILER:Apple Mail (2.3654.120.0.1)",
        "MAILER:PigeonMail/2.1",
        "mailer:Custom Agent",
    ] {
        let vcard = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Mailer User\r\n{mailer_val}\r\nEND:VCARD\r\n"
        );
        let parsed = vcard_to_card(&vcard).expect("parse MAILER");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Mailer User")
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("MAILER:") && !emitted.contains("MAILER;"),
            "MAILER must not be emitted"
        );
    }

    // 4. PRODID variations: FPI strings and application identifiers
    for prodid_val in [
        "PRODID:-//Apple Inc.//macOS 14.5//EN",
        "PRODID:-//Google Inc.//Google Contacts//EN",
        "PRODID:-//Evolution Data Server//3.52.0//EN",
        "PRODID:-//Mozilla.org/NONSGML Mozilla Address Book V1.0//EN",
        "prodid:-//Example Corp.//EN",
    ] {
        let vcard = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Prodid User\r\n{prodid_val}\r\nEND:VCARD\r\n"
        );
        let parsed = vcard_to_card(&vcard).expect("parse PRODID");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Prodid User")
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("PRODID:") && !emitted.contains("PRODID;"),
            "PRODID must not be emitted"
        );
    }

    // 5. REV variations: ISO-8601 timestamps, basic and extended formats
    for rev_val in [
        "REV:2026-08-22T00:07:47Z",
        "REV:20260822T000747Z",
        "REV:1995-10-31T22:27:10.123Z",
        "REV;VALUE=date-time:2024-01-15T12:00:00Z",
        "rev:2026-08-22T00:00:00Z",
    ] {
        let vcard =
            format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Rev User\r\n{rev_val}\r\nEND:VCARD\r\n");
        let parsed = vcard_to_card(&vcard).expect("parse REV");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Rev User")
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("REV:") && !emitted.contains("REV;"),
            "REV must not be emitted"
        );
    }

    // 6. SORT-STRING variations: simple strings, non-ASCII, spaces, escaped delimiters
    for sort_val in [
        "SORT-STRING:Einstein",
        "SORT-STRING:Mueller",
        "SORT-STRING:Müller\\, Hans",
        "SORT-STRING:van Beethoven",
        "sort-string:Bach",
    ] {
        let vcard =
            format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Sort User\r\n{sort_val}\r\nEND:VCARD\r\n");
        let parsed = vcard_to_card(&vcard).expect("parse SORT-STRING");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Sort User")
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("SORT-STRING:") && !emitted.contains("SORT-STRING;"),
            "SORT-STRING must not be emitted"
        );
    }

    // 7. CLASS variations: access classifications
    for class_val in [
        "CLASS:PUBLIC",
        "CLASS:PRIVATE",
        "CLASS:CONFIDENTIAL",
        "CLASS;X-CUSTOM=1:PUBLIC",
        "class:private",
    ] {
        let vcard =
            format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Class User\r\n{class_val}\r\nEND:VCARD\r\n");
        let parsed = vcard_to_card(&vcard).expect("parse CLASS");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Class User")
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("CLASS:") && !emitted.contains("CLASS;"),
            "CLASS must not be emitted"
        );
    }

    // 8. SOUND variations: inline binary audio and remote URIs
    for sound_val in [
        "SOUND;TYPE=BASIC;ENCODING=b:AQIDBA==",
        "SOUND;TYPE=WAV;ENCODING=b:UklGRg==",
        "SOUND;VALUE=uri:https://example.com/pronunciation.wav",
        "SOUND;VALUE=URI:file:///sounds/name.au",
        "sound;value=uri:https://example.com/audio.mp3",
    ] {
        let vcard =
            format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Sound User\r\n{sound_val}\r\nEND:VCARD\r\n");
        let parsed = vcard_to_card(&vcard).expect("parse SOUND");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Sound User")
        );
        assert!(parsed.media.is_none() || parsed.media.as_ref().unwrap().is_empty());
        let emitted = card_to_vcard(&parsed);
        assert!(!emitted.contains("SOUND"), "SOUND must not be emitted");
    }

    // 9. LOGO variations: inline binary image and remote URIs
    for logo_val in [
        "LOGO;TYPE=JPEG;ENCODING=b:/9j/4AAQSkZJRg==",
        "LOGO;TYPE=PNG;ENCODING=b:iVBORw0KGgo==",
        "LOGO;VALUE=uri:https://example.com/logo.png",
        "LOGO;VALUE=URI:https://example.com/corporate_logo.svg",
        "logo;value=uri:https://example.com/icon.gif",
    ] {
        let vcard =
            format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Logo User\r\n{logo_val}\r\nEND:VCARD\r\n");
        let parsed = vcard_to_card(&vcard).expect("parse LOGO");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Logo User")
        );
        assert!(parsed.media.is_none() || parsed.media.as_ref().unwrap().is_empty());
        let emitted = card_to_vcard(&parsed);
        assert!(!emitted.contains("LOGO"), "LOGO must not be emitted");
    }
}

#[test]
fn jscontact_sound_and_logo_media_entries_server_preservation() {
    // When a JSContact ContactCard on the JMAP server contains non-photo media entries
    // (such as kind: "sound", kind: "logo", kind: "document"), states_media ensures that
    // ONLY kind: "photo" entries are emitted onto vCard 3.0 PHOTO lines.
    //
    // Non-photo media entries get no vCard line and are omitted on the wire format to
    // prevent confusing Evolution's contact editor (which only supports E_CONTACT_PHOTO).
    // During JMAP sync, PatchObject preserves the untouched sound/logo entries on the server.

    let mut media = BTreeMap::new();
    media.insert(
        "m_photo".to_owned(),
        Media {
            kind: Some("photo".to_owned()),
            media_type: Some("image/jpeg".to_owned()),
            uri: "data:image/jpeg;base64,/9j/4AAQSkZJRg==".to_owned(),
            extra: BTreeMap::new(),
        },
    );
    media.insert(
        "m_logo".to_owned(),
        Media {
            kind: Some("logo".to_owned()),
            media_type: Some("image/png".to_owned()),
            uri: "https://example.com/company_logo.png".to_owned(),
            extra: BTreeMap::new(),
        },
    );
    media.insert(
        "m_sound".to_owned(),
        Media {
            kind: Some("sound".to_owned()),
            media_type: Some("audio/wav".to_owned()),
            uri: "data:audio/wav;base64,UklGRg==".to_owned(),
            extra: BTreeMap::new(),
        },
    );
    media.insert(
        "m_doc".to_owned(),
        Media {
            kind: Some("document".to_owned()),
            media_type: Some("application/pdf".to_owned()),
            uri: "https://example.com/resume.pdf".to_owned(),
            extra: BTreeMap::new(),
        },
    );

    // states_media predicate validation
    assert!(states_media(&media["m_photo"]), "photo must be stateable");
    assert!(
        !states_media(&media["m_logo"]),
        "logo must not be stateable on vCard 3.0 PHOTO"
    );
    assert!(
        !states_media(&media["m_sound"]),
        "sound must not be stateable on vCard 3.0 PHOTO"
    );
    assert!(
        !states_media(&media["m_doc"]),
        "document must not be stateable on vCard 3.0 PHOTO"
    );

    let card = ContactCard {
        name: Some(Name {
            full: Some("Media Test Contact".to_owned()),
            ..Name::default()
        }),
        media: Some(media),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);

    // Exactly one PHOTO line emitted
    assert_eq!(
        vcard.matches("PHOTO").count(),
        1,
        "Exactly one PHOTO line must be emitted"
    );
    assert!(vcard.contains("PHOTO;X-JMAP-KEY=m_photo;TYPE=jpeg;ENCODING=b:/9j/4AAQSkZJRg==\r\n"));
    assert!(!vcard.contains("LOGO"), "LOGO must not be emitted");
    assert!(!vcard.contains("SOUND"), "SOUND must not be emitted");
    assert!(!vcard.contains("company_logo.png"));
    assert!(!vcard.contains("audio/wav"));
    assert!(!vcard.contains("resume.pdf"));

    // Reading back parses only the photo
    let parsed = vcard_to_card(&vcard).expect("parse back");
    let parsed_media = parsed.media.as_ref().expect("media present");
    assert_eq!(parsed_media.len(), 1);
    assert_eq!(parsed_media["m_photo"].kind.as_deref(), Some("photo"));
    assert_eq!(
        parsed_media["m_photo"].uri.as_str(),
        "data:image/jpeg;base64,/9j/4AAQSkZJRg=="
    );
}

#[test]
fn standard_properties_case_insensitivity_and_empty_values() {
    // Verify that empty or whitespace-only lines for all 9 standard properties
    // are safely handled without panicking or creating empty structs.

    for empty_line in [
        "GEO:",
        "GEO:   ",
        "geo:",
        "TZ:",
        "TZ:   ",
        "tz:",
        "MAILER:",
        "mailer:   ",
        "PRODID:",
        "prodid:   ",
        "REV:",
        "rev:   ",
        "SORT-STRING:",
        "sort-string:   ",
        "CLASS:",
        "class:   ",
        "SOUND:",
        "sound:   ",
        "SOUND;VALUE=uri:",
        "SOUND;ENCODING=b:",
        "LOGO:",
        "logo:   ",
        "LOGO;VALUE=uri:",
        "LOGO;ENCODING=b:",
    ] {
        let vcard =
            format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Empty Test\r\n{empty_line}\r\nEND:VCARD\r\n");
        let parsed = vcard_to_card(&vcard)
            .unwrap_or_else(|e| panic!("Failed to parse empty line {empty_line:?}: {e}"));
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Empty Test")
        );
        let emitted = card_to_vcard(&parsed);
        assert_eq!(
            emitted,
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Empty Test\r\nEND:VCARD\r\n"
        );
    }
}

#[test]
fn phone_mobile_type_synonym_and_permutations_characterization() {
    // Audit & characterization: `TYPE=MOBILE` is widely emitted in the wild (Android,
    // iOS, Outlook, feature phones) as a synonym for vCard 3.0 standard `TYPE=CELL`.
    // Verify that inbound `TYPE=MOBILE` parses into JSContact `features: {"mobile": true}`
    // identically to `TYPE=CELL`, normalizes outbound to standard `TYPE=CELL`, and
    // achieves fixed-point roundtrip stability.
    let vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:test-phone-mobile-synonym\r\n",
        "FN:Mobile Synonym Test\r\n",
        "TEL;X-JMAP-KEY=p_bare_mobile;TYPE=MOBILE:+1-555-0101\r\n",
        "TEL;X-JMAP-KEY=p_lower_mobile;TYPE=mobile:+1-555-0102\r\n",
        "TEL;X-JMAP-KEY=p_mixed_mobile;type=Mobile:+1-555-0103\r\n",
        "TEL;X-JMAP-KEY=p_work_mobile_plain;TYPE=WORK,MOBILE:+1-555-0104\r\n",
        "TEL;X-JMAP-KEY=p_home_mobile;TYPE=HOME,MOBILE:+1-555-0105\r\n",
        "TEL;X-JMAP-KEY=p_pref_mobile;TYPE=MOBILE,PREF:+1-555-0106\r\n",
        "TEL;X-JMAP-KEY=p_work_mobile_pref;TYPE=WORK,MOBILE;TYPE=PREF:+1-555-0107\r\n",
        "TEL;X-JMAP-KEY=p_mobile_voice;TYPE=MOBILE,VOICE:+1-555-0108\r\n",
        "TEL;X-JMAP-KEY=p_mobile_fax;TYPE=MOBILE,FAX:+1-555-0109\r\n",
        "END:VCARD\r\n",
    );

    let card = vcard_to_card(vcard).expect("parse mobile synonym vcard");
    let phones = card.phones.as_ref().expect("phones map");
    assert_eq!(phones.len(), 9);

    // 1. TEL;TYPE=MOBILE -> features: {"mobile": true}
    let p_bare = &phones["p_bare_mobile"];
    assert_eq!(p_bare.number, "+1-555-0101");
    assert_eq!(p_bare.contexts, None);
    assert_eq!(p_bare.features, Some(json!({"mobile": true})));
    assert_eq!(p_bare.pref, None);
    assert!(states_phone_feature(p_bare.features.as_ref(), "mobile"));

    // 2. TEL;TYPE=mobile -> lowercase
    let p_lower = &phones["p_lower_mobile"];
    assert_eq!(p_lower.number, "+1-555-0102");
    assert_eq!(p_lower.features, Some(json!({"mobile": true})));

    // 3. TEL;type=Mobile -> mixed case
    let p_mixed = &phones["p_mixed_mobile"];
    assert_eq!(p_mixed.number, "+1-555-0103");
    assert_eq!(p_mixed.features, Some(json!({"mobile": true})));

    // 4. TEL;TYPE=WORK,MOBILE -> work context + mobile feature
    let p_wm = &phones["p_work_mobile_plain"];
    assert_eq!(p_wm.number, "+1-555-0104");
    assert_eq!(p_wm.contexts, Some(json!({"work": true})));
    assert_eq!(p_wm.features, Some(json!({"mobile": true})));
    assert!(states_context(p_wm.contexts.as_ref(), "work"));
    assert!(states_phone_feature(p_wm.features.as_ref(), "mobile"));

    // 5. TEL;TYPE=HOME,MOBILE -> private context + mobile feature
    let p_hm = &phones["p_home_mobile"];
    assert_eq!(p_hm.number, "+1-555-0105");
    assert_eq!(p_hm.contexts, Some(json!({"private": true})));
    assert_eq!(p_hm.features, Some(json!({"mobile": true})));
    assert!(states_context(p_hm.contexts.as_ref(), "private"));
    assert!(states_phone_feature(p_hm.features.as_ref(), "mobile"));

    // 6. TEL;TYPE=MOBILE,PREF -> pref: 1 + mobile feature
    let p_pm = &phones["p_pref_mobile"];
    assert_eq!(p_pm.number, "+1-555-0106");
    assert_eq!(p_pm.pref, Some(1));
    assert_eq!(p_pm.features, Some(json!({"mobile": true})));

    // 7. TEL;TYPE=WORK,MOBILE;TYPE=PREF -> work + mobile + pref
    let p_wmp = &phones["p_work_mobile_pref"];
    assert_eq!(p_wmp.number, "+1-555-0107");
    assert_eq!(p_wmp.pref, Some(1));
    assert_eq!(p_wmp.contexts, Some(json!({"work": true})));
    assert_eq!(p_wmp.features, Some(json!({"mobile": true})));

    // 8. TEL;TYPE=MOBILE,VOICE -> mobile + voice, narrowed to mobile
    let p_mv = &phones["p_mobile_voice"];
    assert_eq!(p_mv.number, "+1-555-0108");
    assert_eq!(p_mv.features, Some(json!({"mobile": true, "voice": true})));
    assert!(states_phone_feature(p_mv.features.as_ref(), "mobile"));
    assert!(!states_phone_feature(p_mv.features.as_ref(), "voice"));

    // 9. TEL;TYPE=MOBILE,FAX -> mobile + fax, narrowed to mobile
    let p_mf = &phones["p_mobile_fax"];
    assert_eq!(p_mf.number, "+1-555-0109");
    assert_eq!(p_mf.features, Some(json!({"mobile": true, "fax": true})));
    assert!(states_phone_feature(p_mf.features.as_ref(), "mobile"));
    assert!(!states_phone_feature(p_mf.features.as_ref(), "fax"));

    // Outbound emission: TYPE=MOBILE normalizes to standard vCard 3.0 TYPE=CELL
    let emitted = card_to_vcard(&card);
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_bare_mobile"),
        "TEL;X-JMAP-KEY=p_bare_mobile;TYPE=CELL:+1-555-0101"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_lower_mobile"),
        "TEL;X-JMAP-KEY=p_lower_mobile;TYPE=CELL:+1-555-0102"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_mixed_mobile"),
        "TEL;X-JMAP-KEY=p_mixed_mobile;TYPE=CELL:+1-555-0103"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_work_mobile_plain"),
        "TEL;X-JMAP-KEY=p_work_mobile_plain;TYPE=WORK,CELL:+1-555-0104"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_home_mobile"),
        "TEL;X-JMAP-KEY=p_home_mobile;TYPE=HOME,CELL:+1-555-0105"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_pref_mobile"),
        "TEL;X-JMAP-KEY=p_pref_mobile;TYPE=CELL,PREF:+1-555-0106"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_work_mobile_pref"),
        "TEL;X-JMAP-KEY=p_work_mobile_pref;TYPE=WORK,CELL,PREF:+1-555-0107"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_mobile_voice"),
        "TEL;X-JMAP-KEY=p_mobile_voice;TYPE=CELL:+1-555-0108"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_mobile_fax"),
        "TEL;X-JMAP-KEY=p_mobile_fax;TYPE=CELL:+1-555-0109"
    );

    // Fixed-point convergence
    let card2 = vcard_to_card(&emitted).expect("parse roundtrip vcard");
    let emitted2 = card_to_vcard(&card2);
    assert_eq!(emitted, emitted2);
}

#[test]
fn phone_nineteen_eds_fields_complete_matrix_and_roundtrip() {
    // Comprehensive test verifying all 19 EDS phone fields from libebook-contacts 3.52:
    //  1. E_CONTACT_PHONE_PRIMARY      (31) -> TEL;TYPE=PREF
    //  2. E_CONTACT_PHONE_BUSINESS     (17) -> TEL;TYPE=WORK,VOICE
    //  3. E_CONTACT_PHONE_BUSINESS_2   (18) -> TEL;TYPE=WORK (2nd work phone)
    //  4. E_CONTACT_PHONE_BUSINESS_FAX (19) -> TEL;TYPE=WORK,FAX
    //  5. E_CONTACT_PHONE_HOME         (23) -> TEL;TYPE=HOME,VOICE
    //  6. E_CONTACT_PHONE_HOME_2       (24) -> TEL;TYPE=HOME (2nd home phone)
    //  7. E_CONTACT_PHONE_HOME_FAX     (25) -> TEL;TYPE=HOME,FAX
    //  8. E_CONTACT_PHONE_MOBILE       (27) -> TEL;TYPE=CELL / TEL;TYPE=MOBILE
    //  9. E_CONTACT_PHONE_PAGER        (30) -> TEL;TYPE=PAGER
    // 10. E_CONTACT_PHONE_OTHER        (28) -> TEL;TYPE=VOICE / bare TEL
    // 11. E_CONTACT_PHONE_OTHER_FAX    (29) -> TEL;TYPE=FAX
    // 12. E_CONTACT_PHONE_CAR          (21) -> TEL;TYPE=CAR
    // 13. E_CONTACT_PHONE_ISDN         (26) -> TEL;TYPE=ISDN
    // 14. E_CONTACT_PHONE_CALLBACK     (20) -> TEL;TYPE=CALLBACK
    // 15. E_CONTACT_PHONE_COMPANY      (22) -> TEL;TYPE=COMPANY
    // 16. E_CONTACT_PHONE_RADIO        (32) -> TEL;TYPE=RADIO
    // 17. E_CONTACT_PHONE_TELEX        (33) -> TEL;TYPE=TELEX
    // 18. E_CONTACT_PHONE_TTYTDD       (34) -> TEL;TYPE=TTYTDD
    // 19. E_CONTACT_PHONE_ASSISTANT    (16) -> TEL;TYPE=ASSISTANT
    let vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:test-19-eds-phone-fields\r\n",
        "FN:EDS Phone Matrix Test\r\n",
        "TEL;X-JMAP-KEY=p01_primary;TYPE=PREF:+1-555-0100\r\n",
        "TEL;X-JMAP-KEY=p02_business;TYPE=WORK,VOICE:+1-555-0101\r\n",
        "TEL;X-JMAP-KEY=p03_business_2;TYPE=WORK:+1-555-0102\r\n",
        "TEL;X-JMAP-KEY=p04_business_fax;TYPE=WORK,FAX:+1-555-0103\r\n",
        "TEL;X-JMAP-KEY=p05_home;TYPE=HOME,VOICE:+1-555-0104\r\n",
        "TEL;X-JMAP-KEY=p06_home_2;TYPE=HOME:+1-555-0105\r\n",
        "TEL;X-JMAP-KEY=p07_home_fax;TYPE=HOME,FAX:+1-555-0106\r\n",
        "TEL;X-JMAP-KEY=p08_mobile_cell;TYPE=CELL:+1-555-0107\r\n",
        "TEL;X-JMAP-KEY=p09_mobile_syn;TYPE=MOBILE:+1-555-0108\r\n",
        "TEL;X-JMAP-KEY=p10_pager;TYPE=PAGER:+1-555-0109\r\n",
        "TEL;X-JMAP-KEY=p11_other_voice;TYPE=VOICE:+1-555-0110\r\n",
        "TEL;X-JMAP-KEY=p12_other_bare:+1-555-0111\r\n",
        "TEL;X-JMAP-KEY=p13_other_fax;TYPE=FAX:+1-555-0112\r\n",
        "TEL;X-JMAP-KEY=p14_car;TYPE=CAR:+1-555-0113\r\n",
        "TEL;X-JMAP-KEY=p15_isdn;TYPE=ISDN:+1-555-0114\r\n",
        "TEL;X-JMAP-KEY=p16_callback;TYPE=CALLBACK:+1-555-0115\r\n",
        "TEL;X-JMAP-KEY=p17_company;TYPE=COMPANY:+1-555-0116\r\n",
        "TEL;X-JMAP-KEY=p18_radio;TYPE=RADIO:+1-555-0117\r\n",
        "TEL;X-JMAP-KEY=p19_telex;TYPE=TELEX:+1-555-0118\r\n",
        "TEL;X-JMAP-KEY=p20_ttytdd;TYPE=TTYTDD:+1-555-0119\r\n",
        "TEL;X-JMAP-KEY=p21_assistant;TYPE=ASSISTANT:+1-555-0120\r\n",
        "END:VCARD\r\n",
    );

    let card = vcard_to_card(vcard).expect("parse 19-field EDS vcard");
    let phones = card.phones.as_ref().expect("phones map");
    assert_eq!(phones.len(), 21);

    // 1. Primary: pref: 1
    assert_eq!(phones["p01_primary"].pref, Some(1));
    assert_eq!(phones["p01_primary"].number, "+1-555-0100");

    // 2. Business: work context + voice feature
    assert_eq!(phones["p02_business"].contexts, Some(json!({"work": true})));
    assert_eq!(
        phones["p02_business"].features,
        Some(json!({"voice": true}))
    );

    // 3. Business 2: work context + no feature
    assert_eq!(
        phones["p03_business_2"].contexts,
        Some(json!({"work": true}))
    );
    assert_eq!(phones["p03_business_2"].features, None);

    // 4. Business Fax: work context + fax feature
    assert_eq!(
        phones["p04_business_fax"].contexts,
        Some(json!({"work": true}))
    );
    assert_eq!(
        phones["p04_business_fax"].features,
        Some(json!({"fax": true}))
    );

    // 5. Home: private context + voice feature
    assert_eq!(phones["p05_home"].contexts, Some(json!({"private": true})));
    assert_eq!(phones["p05_home"].features, Some(json!({"voice": true})));

    // 6. Home 2: private context + no feature
    assert_eq!(
        phones["p06_home_2"].contexts,
        Some(json!({"private": true}))
    );
    assert_eq!(phones["p06_home_2"].features, None);

    // 7. Home Fax: private context + fax feature
    assert_eq!(
        phones["p07_home_fax"].contexts,
        Some(json!({"private": true}))
    );
    assert_eq!(phones["p07_home_fax"].features, Some(json!({"fax": true})));

    // 8. Mobile (CELL): mobile feature
    assert_eq!(
        phones["p08_mobile_cell"].features,
        Some(json!({"mobile": true}))
    );

    // 9. Mobile (MOBILE synonym): mobile feature
    assert_eq!(
        phones["p09_mobile_syn"].features,
        Some(json!({"mobile": true}))
    );

    // 10. Pager: pager feature
    assert_eq!(phones["p10_pager"].features, Some(json!({"pager": true})));

    // 11. Other Voice: voice feature
    assert_eq!(
        phones["p11_other_voice"].features,
        Some(json!({"voice": true}))
    );

    // 12. Other Bare: untyped
    assert_eq!(phones["p12_other_bare"].features, None);
    assert_eq!(phones["p12_other_bare"].contexts, None);

    // 13. Other Fax: fax feature
    assert_eq!(phones["p13_other_fax"].features, Some(json!({"fax": true})));

    // 14-21. Specialized telephony lines (CAR, ISDN, CALLBACK, COMPANY, RADIO, TELEX, TTYTDD, ASSISTANT)
    for (key, num) in [
        ("p14_car", "+1-555-0113"),
        ("p15_isdn", "+1-555-0114"),
        ("p16_callback", "+1-555-0115"),
        ("p17_company", "+1-555-0116"),
        ("p18_radio", "+1-555-0117"),
        ("p19_telex", "+1-555-0118"),
        ("p20_ttytdd", "+1-555-0119"),
        ("p21_assistant", "+1-555-0120"),
    ] {
        let p = &phones[key];
        assert_eq!(p.number, num);
        assert!(states_phone(p));
    }

    // Outbound emission: primary phone is sorted first due to pref: 1
    let emitted = card_to_vcard(&card);

    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p01_primary"),
        "TEL;X-JMAP-KEY=p01_primary;TYPE=PREF:+1-555-0100"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p02_business"),
        "TEL;X-JMAP-KEY=p02_business;TYPE=WORK,VOICE:+1-555-0101"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p03_business_2"),
        "TEL;X-JMAP-KEY=p03_business_2;TYPE=WORK:+1-555-0102"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p04_business_fax"),
        "TEL;X-JMAP-KEY=p04_business_fax;TYPE=WORK,FAX:+1-555-0103"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p05_home"),
        "TEL;X-JMAP-KEY=p05_home;TYPE=HOME,VOICE:+1-555-0104"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p06_home_2"),
        "TEL;X-JMAP-KEY=p06_home_2;TYPE=HOME:+1-555-0105"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p07_home_fax"),
        "TEL;X-JMAP-KEY=p07_home_fax;TYPE=HOME,FAX:+1-555-0106"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p08_mobile_cell"),
        "TEL;X-JMAP-KEY=p08_mobile_cell;TYPE=CELL:+1-555-0107"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p09_mobile_syn"),
        "TEL;X-JMAP-KEY=p09_mobile_syn;TYPE=CELL:+1-555-0108"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p10_pager"),
        "TEL;X-JMAP-KEY=p10_pager;TYPE=PAGER:+1-555-0109"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p11_other_voice"),
        "TEL;X-JMAP-KEY=p11_other_voice;TYPE=VOICE:+1-555-0110"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p12_other_bare"),
        "TEL;X-JMAP-KEY=p12_other_bare:+1-555-0111"
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p13_other_fax"),
        "TEL;X-JMAP-KEY=p13_other_fax;TYPE=FAX:+1-555-0112"
    );

    // Multi-roundtrip fixed point stability
    let card2 = vcard_to_card(&emitted).expect("parse roundtrip 2");
    let emitted2 = card_to_vcard(&card2);
    assert_eq!(emitted, emitted2);
}

#[test]
fn phone_whitespace_punctuation_and_uri_schemes_handling() {
    // Test formatted numbers, spaces, visual separators, tel: URI, and leading/trailing whitespace
    let vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:test-phone-formats\r\n",
        "FN:Phone Formats Test\r\n",
        "TEL;X-JMAP-KEY=p_spaced;TYPE=WORK:   +1 (555) 012-3456   \r\n",
        "TEL;X-JMAP-KEY=p_dotted;TYPE=HOME:123.456.7890\r\n",
        "TEL;X-JMAP-KEY=p_dashed;TYPE=CELL:555-867-5309\r\n",
        "TEL;X-JMAP-KEY=p_parens;TYPE=PAGER:(800) 555-0199\r\n",
        "TEL;X-JMAP-KEY=p_uri;TYPE=WORK,VOICE:tel:+1-555-0123;ext=100\r\n",
        "TEL;X-JMAP-KEY=p_bare_empty:\r\n",
        "END:VCARD\r\n",
    );

    let card = vcard_to_card(vcard).expect("parse phone formats vcard");
    let phones = card.phones.as_ref().expect("phones map");

    // Exact string representation is preserved, empty phone numbers are dropped
    assert_eq!(phones.len(), 5);
    assert_eq!(phones["p_spaced"].number, "   +1 (555) 012-3456   ");
    assert_eq!(phones["p_dotted"].number, "123.456.7890");
    assert_eq!(phones["p_dashed"].number, "555-867-5309");
    assert_eq!(phones["p_parens"].number, "(800) 555-0199");
    assert_eq!(phones["p_uri"].number, "tel:+1-555-0123;ext=100");

    assert!(!phones.contains_key("p_bare_empty"));

    // Predicates: states_phone
    let empty_phone = ContactPhone::default();
    let whitespace_phone = ContactPhone {
        number: "   ".to_string(),
        ..ContactPhone::default()
    };
    assert!(!states_phone(&empty_phone));
    assert!(states_phone(&whitespace_phone)); // Non-empty string; states_phone checks !is_empty()

    // Outbound emission & roundtrip
    let emitted = card_to_vcard(&card);
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_spaced"),
        "TEL;X-JMAP-KEY=p_spaced;TYPE=WORK:   +1 (555) 012-3456   "
    );
    assert_eq!(
        line(&emitted, "TEL;X-JMAP-KEY=p_uri"),
        "TEL;X-JMAP-KEY=p_uri;TYPE=WORK,VOICE:tel:+1-555-0123\\;ext=100"
    );

    let card2 = vcard_to_card(&emitted).expect("parse roundtrip");
    assert_eq!(
        card2.phones.as_ref().unwrap()["p_uri"].number,
        "tel:+1-555-0123;ext=100"
    );
    let emitted2 = card_to_vcard(&card2);
    assert_eq!(emitted, emitted2);
}

#[test]
fn phone_multi_token_and_case_insensitive_type_matrix_roundtrip() {
    let vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:test-phone-multi-token-matrix\r\n",
        "FN:Phone Multi Token Test\r\n",
        "TEL;X-JMAP-KEY=p_wc;TYPE=work,cell:+1-555-1001\r\n",
        "TEL;X-JMAP-KEY=p_wc_sep;TYPE=WORK;TYPE=CELL:+1-555-1002\r\n",
        "TEL;X-JMAP-KEY=p_hm;TYPE=home,mobile:+1-555-1003\r\n",
        "TEL;X-JMAP-KEY=p_hm_sep;TYPE=HOME;TYPE=MOBILE:+1-555-1004\r\n",
        "TEL;X-JMAP-KEY=p_wpv;type=work,pref,voice:+1-555-1005\r\n",
        "TEL;X-JMAP-KEY=p_wpv_sep;TYPE=WORK;type=pref;TYPE=VOICE:+1-555-1006\r\n",
        "TEL;X-JMAP-KEY=p_wvf;TYPE=WORK,FAX,CELL:+1-555-1007\r\n",
        "TEL;X-JMAP-KEY=p_hpf;TYPE=HOME,PAGER,FAX:+1-555-1008\r\n",
        "TEL;X-JMAP-KEY=p_vfv;TYPE=VOICE,FAX,VIDEO:+1-555-1009\r\n",
        "END:VCARD\r\n",
    );

    let card = vcard_to_card(vcard).expect("parse multi token vcard");
    let phones = card.phones.as_ref().expect("phones map");
    assert_eq!(phones.len(), 9);

    // Comma-separated vs semicolon-separated equivalence
    assert_eq!(phones["p_wc"].contexts, phones["p_wc_sep"].contexts);
    assert_eq!(phones["p_wc"].features, phones["p_wc_sep"].features);

    assert_eq!(phones["p_hm"].contexts, phones["p_hm_sep"].contexts);
    assert_eq!(phones["p_hm"].features, phones["p_hm_sep"].features);

    assert_eq!(phones["p_wpv"].contexts, phones["p_wpv_sep"].contexts);
    assert_eq!(phones["p_wpv"].features, phones["p_wpv_sep"].features);
    assert_eq!(phones["p_wpv"].pref, phones["p_wpv_sep"].pref);

    // Feature narrowing precedence:
    // WORK,FAX,CELL -> CELL
    assert_eq!(
        phones["p_wvf"].features,
        Some(json!({"mobile": true, "fax": true}))
    );
    assert!(states_phone_feature(
        phones["p_wvf"].features.as_ref(),
        "mobile"
    ));
    assert!(!states_phone_feature(
        phones["p_wvf"].features.as_ref(),
        "fax"
    ));

    // HOME,PAGER,FAX -> PAGER
    assert_eq!(
        phones["p_hpf"].features,
        Some(json!({"pager": true, "fax": true}))
    );
    assert!(states_phone_feature(
        phones["p_hpf"].features.as_ref(),
        "pager"
    ));
    assert!(!states_phone_feature(
        phones["p_hpf"].features.as_ref(),
        "fax"
    ));

    // VOICE,FAX,VIDEO -> FAX
    assert_eq!(
        phones["p_vfv"].features,
        Some(json!({"voice": true, "fax": true, "video": true}))
    );
    assert!(states_phone_feature(
        phones["p_vfv"].features.as_ref(),
        "fax"
    ));
    assert!(!states_phone_feature(
        phones["p_vfv"].features.as_ref(),
        "voice"
    ));
    assert!(!states_phone_feature(
        phones["p_vfv"].features.as_ref(),
        "video"
    ));

    // Outbound emission & convergence
    let emitted = card_to_vcard(&card);
    let card2 = vcard_to_card(&emitted).expect("parse emitted");
    let emitted2 = card_to_vcard(&card2);
    assert_eq!(emitted, emitted2);
}

#[test]
fn evolution_manager_and_assistant_relations_roundtrip() {
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:c-mgr-asst-001\r\n",
        "FN:Taylor Swift\r\n",
        "X-EVOLUTION-SPOUSE:Austin Swift\r\n",
        "X-EVOLUTION-MANAGER:Scott Borchetta\r\n",
        "X-EVOLUTION-ASSISTANT:Tree Paine\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse manager and assistant");
    let related = card.related_to.as_ref().expect("related_to present");
    assert_eq!(related.len(), 3);
    assert_eq!(
        related["Austin Swift"].relation,
        Some([("spouse".to_string(), json!(true))].into())
    );
    assert_eq!(
        related["Scott Borchetta"].relation,
        Some([("manager".to_string(), json!(true))].into())
    );
    assert_eq!(
        related["Tree Paine"].relation,
        Some([("assistant".to_string(), json!(true))].into())
    );

    let emitted = card_to_vcard(&card);
    assert!(emitted.contains("X-EVOLUTION-SPOUSE:Austin Swift\r\n"));
    assert!(emitted.contains("X-EVOLUTION-MANAGER:Scott Borchetta\r\n"));
    assert!(emitted.contains("X-EVOLUTION-ASSISTANT:Tree Paine\r\n"));

    let card2 = vcard_to_card(&emitted).expect("parse re-emitted");
    assert_eq!(card2, card);
    let emitted2 = card_to_vcard(&card2);
    assert_eq!(emitted2, emitted);
}

#[test]
fn evolution_blog_and_video_urls_links_roundtrip() {
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:c-blog-video-001\r\n",
        "FN:Morgan Lee\r\n",
        "URL;X-JMAP-KEY=l1:https://morgan.example.com\r\n",
        "X-EVOLUTION-BLOG-URL;X-JMAP-KEY=l2:https://blogs.example.com/morgan\r\n",
        "X-EVOLUTION-VIDEO-URL;X-JMAP-KEY=l3:https://stream.example.com/morgan/live\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse blog and video links");
    let links = card.links.as_ref().expect("links present");
    assert_eq!(links.len(), 3);
    assert_eq!(links["l1"].uri, "https://morgan.example.com");
    assert_eq!(links["l1"].kind, None);
    assert_eq!(links["l2"].uri, "https://blogs.example.com/morgan");
    assert_eq!(links["l2"].kind, Some("blog".to_string()));
    assert_eq!(links["l3"].uri, "https://stream.example.com/morgan/live");
    assert_eq!(links["l3"].kind, Some("video".to_string()));

    let emitted = card_to_vcard(&card);
    assert!(emitted.contains("URL;X-JMAP-KEY=l1:https://morgan.example.com\r\n"));
    assert!(
        emitted.contains("X-EVOLUTION-BLOG-URL;X-JMAP-KEY=l2:https://blogs.example.com/morgan\r\n")
    );
    assert!(emitted.contains(
        "X-EVOLUTION-VIDEO-URL;X-JMAP-KEY=l3:https://stream.example.com/morgan/live\r\n"
    ));

    let card2 = vcard_to_card(&emitted).expect("parse re-emitted");
    assert_eq!(card2, card);
    let emitted2 = card_to_vcard(&card2);
    assert_eq!(emitted2, emitted);
}

#[test]
fn evolution_remaining_x_properties_coexistence_and_predicates() {
    // Tests predicates: states_manager, states_assistant, states_spouse, states_link
    let rel_valid_mgr = Relation {
        relation: Some([("manager".to_string(), json!(true))].into()),
        extra: BTreeMap::new(),
    };
    let rel_valid_asst = Relation {
        relation: Some([("assistant".to_string(), json!(true))].into()),
        extra: BTreeMap::new(),
    };
    let rel_valid_spouse = Relation {
        relation: Some([("spouse".to_string(), json!(true))].into()),
        extra: BTreeMap::new(),
    };
    let rel_other = Relation {
        relation: Some([("colleague".to_string(), json!(true))].into()),
        extra: BTreeMap::new(),
    };
    let rel_non_bool = Relation {
        relation: Some([("manager".to_string(), json!(1))].into()),
        extra: BTreeMap::new(),
    };
    let rel_empty = Relation {
        relation: None,
        extra: BTreeMap::new(),
    };

    // Valid person names
    assert!(states_manager("Sarah Connor", &rel_valid_mgr));
    assert!(states_assistant("John Connor", &rel_valid_asst));
    assert!(states_spouse("Kyle Reese", &rel_valid_spouse));
    assert!(states_manager("Élise Müller", &rel_valid_mgr));
    assert!(states_assistant("山田 太郎", &rel_valid_asst));

    // Cross-relation checks
    assert!(!states_manager("Sarah Connor", &rel_valid_asst));
    assert!(!states_manager("Sarah Connor", &rel_valid_spouse));
    assert!(!states_assistant("John Connor", &rel_valid_mgr));
    assert!(!states_assistant("John Connor", &rel_valid_spouse));
    assert!(!states_spouse("Kyle Reese", &rel_valid_mgr));
    assert!(!states_spouse("Kyle Reese", &rel_valid_asst));
    assert!(!states_manager("Sarah Connor", &rel_other));
    assert!(!states_assistant("John Connor", &rel_other));
    assert!(!states_manager("Sarah Connor", &rel_non_bool));
    assert!(!states_manager("Sarah Connor", &rel_empty));

    // Invalid person names (empty, URI, whitespace-edged, CR)
    assert!(!states_manager("", &rel_valid_mgr));
    assert!(!states_manager("   ", &rel_valid_mgr));
    assert!(!states_manager(" Sarah", &rel_valid_mgr));
    assert!(!states_manager("Sarah ", &rel_valid_mgr));
    assert!(!states_manager("Sarah\rConnor", &rel_valid_mgr));
    assert!(!states_manager("urn:uuid:12345", &rel_valid_mgr));
    assert!(!states_manager("mailto:sarah@example.com", &rel_valid_mgr));
    assert!(!states_manager("https://example.com/sarah", &rel_valid_mgr));

    assert!(!states_assistant("", &rel_valid_asst));
    assert!(!states_assistant(" John", &rel_valid_asst));
    assert!(!states_assistant("urn:uuid:67890", &rel_valid_asst));

    // states_link across kinds
    assert!(states_link(&Link {
        uri: "https://example.com".to_string(),
        kind: None,
        extra: BTreeMap::new(),
    }));
    assert!(states_link(&Link {
        uri: "https://blogs.example.com".to_string(),
        kind: Some("blog".to_string()),
        extra: BTreeMap::new(),
    }));
    assert!(states_link(&Link {
        uri: "https://video.example.com".to_string(),
        kind: Some("video".to_string()),
        extra: BTreeMap::new(),
    }));
    assert!(!states_link(&Link {
        uri: "".to_string(),
        kind: None,
        extra: BTreeMap::new(),
    }));
    assert!(!states_link(&Link {
        uri: "".to_string(),
        kind: Some("blog".to_string()),
        extra: BTreeMap::new(),
    }));
    assert!(!states_link(&Link {
        uri: "https://example.com/contact".to_string(),
        kind: Some("contact".to_string()),
        extra: BTreeMap::new(),
    }));
    assert!(!states_link(&Link {
        uri: "https://example.com/rss".to_string(),
        kind: Some("feed".to_string()),
        extra: BTreeMap::new(),
    }));
}

#[test]
fn multiple_relations_on_single_person_and_multi_relation_cards() {
    // Tests a person who is both manager and assistant, or spouse and manager
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:c-multi-rel-001\r\n",
        "FN:Jordan Multi\r\n",
        "X-EVOLUTION-SPOUSE:Taylor Brooks\r\n",
        "X-EVOLUTION-MANAGER:Taylor Brooks\r\n",
        "X-EVOLUTION-ASSISTANT:Alex Morgan\r\n",
        "X-EVOLUTION-MANAGER:Alex Morgan\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse multi relations");
    let related = card.related_to.as_ref().expect("related_to present");
    assert_eq!(related.len(), 2);

    let taylor = &related["Taylor Brooks"];
    assert_eq!(
        taylor.relation,
        Some(
            [
                ("spouse".to_string(), json!(true)),
                ("manager".to_string(), json!(true)),
            ]
            .into()
        )
    );

    let alex = &related["Alex Morgan"];
    assert_eq!(
        alex.relation,
        Some(
            [
                ("assistant".to_string(), json!(true)),
                ("manager".to_string(), json!(true)),
            ]
            .into()
        )
    );

    let emitted = card_to_vcard(&card);
    assert!(emitted.contains("X-EVOLUTION-SPOUSE:Taylor Brooks\r\n"));
    assert!(emitted.contains("X-EVOLUTION-MANAGER:Taylor Brooks\r\n"));
    assert!(emitted.contains("X-EVOLUTION-MANAGER:Alex Morgan\r\n"));
    assert!(emitted.contains("X-EVOLUTION-ASSISTANT:Alex Morgan\r\n"));

    let card2 = vcard_to_card(&emitted).expect("parse re-emitted");
    assert_eq!(card2, card);
    let emitted2 = card_to_vcard(&card2);
    assert_eq!(emitted2, emitted);
}

#[test]
fn evolution_links_and_relations_case_insensitivity_and_whitespace() {
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:c-case-001\r\n",
        "fn:Case Insensitive\r\n",
        "x-evolution-manager:Boss Man\r\n",
        "X-Evolution-Assistant:Helper Person\r\n",
        "x-evolution-blog-url;X-JMAP-KEY=l_b:https://blogs.example.com/case\r\n",
        "X-Evolution-Video-Url;X-JMAP-KEY=l_v:https://video.example.com/case\r\n",
        // Empty lines that should be ignored
        "X-EVOLUTION-MANAGER:\r\n",
        "X-EVOLUTION-ASSISTANT:   \r\n",
        "X-EVOLUTION-BLOG-URL:\r\n",
        "X-EVOLUTION-VIDEO-URL:   \r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse mixed-case vcard");
    let related = card.related_to.as_ref().expect("related_to present");
    assert_eq!(related.len(), 2);
    assert!(related.contains_key("Boss Man"));
    assert!(related.contains_key("Helper Person"));

    let links = card.links.as_ref().expect("links present");
    assert_eq!(links.len(), 2);
    assert_eq!(links["l_b"].uri, "https://blogs.example.com/case");
    assert_eq!(links["l_b"].kind, Some("blog".to_string()));
    assert_eq!(links["l_v"].uri, "https://video.example.com/case");
    assert_eq!(links["l_v"].kind, Some("video".to_string()));

    let emitted = card_to_vcard(&card);
    assert!(emitted.contains("X-EVOLUTION-MANAGER:Boss Man\r\n"));
    assert!(emitted.contains("X-EVOLUTION-ASSISTANT:Helper Person\r\n"));
    assert!(
        emitted.contains("X-EVOLUTION-BLOG-URL;X-JMAP-KEY=l_b:https://blogs.example.com/case\r\n")
    );
    assert!(
        emitted.contains("X-EVOLUTION-VIDEO-URL;X-JMAP-KEY=l_v:https://video.example.com/case\r\n")
    );

    let card2 = vcard_to_card(&emitted).expect("parse re-emitted");
    assert_eq!(card2, card);
    let emitted2 = card_to_vcard(&card2);
    assert_eq!(emitted2, emitted);
}

#[test]
fn email_four_slots_and_attribute_list_matrix_roundtrip() {
    // Audit & characterization of EDS EMAIL slotting:
    // EDS exposes 4 individual string fields:
    //   - E_CONTACT_EMAIL_1 (field 8)
    //   - E_CONTACT_EMAIL_2 (field 9)
    //   - E_CONTACT_EMAIL_3 (field 10)
    //   - E_CONTACT_EMAIL_4 (field 11)
    // plus the E_CONTACT_EMAIL (field 97) attribute list holding all EMAIL properties (lines 1..=4 and 5+).
    //
    // Outbound emission: card_to_vcard sorts emails by (pref.unwrap_or(u32::MAX), key) so:
    //   1st emitted line -> E_CONTACT_EMAIL_1 (primary email in Evolution)
    //   2nd emitted line -> E_CONTACT_EMAIL_2
    //   3rd emitted line -> E_CONTACT_EMAIL_3
    //   4th emitted line -> E_CONTACT_EMAIL_4
    //   5th+ emitted lines -> E_CONTACT_EMAIL attribute list
    // All lines carry X-JMAP-KEY, context TYPE, and TYPE=PREF when preferred.

    // 1. Six emails: e_work_pri is preferred (pref: 1), remaining 5 are unranked (pref: None)
    // and emitted in sorted key order.
    let card = ContactCard {
        card_type: Some("Card".to_owned()),
        version: Some("1.0".to_owned()),
        emails: Some(
            [
                (
                    "e_work_pri".to_owned(),
                    ContactEmail {
                        address: "vera.work@example.com".to_owned(),
                        contexts: Some(json!({"work": true})),
                        pref: Some(1),
                        ..ContactEmail::default()
                    },
                ),
                (
                    "e_home".to_owned(),
                    ContactEmail {
                        address: "vera.home@example.com".to_owned(),
                        contexts: Some(json!({"private": true})),
                        pref: None,
                        ..ContactEmail::default()
                    },
                ),
                (
                    "e_direct".to_owned(),
                    ContactEmail {
                        address: "vera.direct@example.com".to_owned(),
                        contexts: Some(json!({"work": true})),
                        pref: None,
                        ..ContactEmail::default()
                    },
                ),
                (
                    "e_billing".to_owned(),
                    ContactEmail {
                        address: "billing@example.com".to_owned(),
                        contexts: Some(json!({"work": true, "private": true})),
                        pref: None,
                        ..ContactEmail::default()
                    },
                ),
                (
                    "e_support".to_owned(),
                    ContactEmail {
                        address: "support@example.com".to_owned(),
                        contexts: None,
                        pref: None,
                        ..ContactEmail::default()
                    },
                ),
                (
                    "e_archive".to_owned(),
                    ContactEmail {
                        address: "archive@example.com".to_owned(),
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

    let emitted = card_to_vcard(&card);
    let email_lines: Vec<&str> = emitted.lines().filter(|l| l.starts_with("EMAIL")).collect();
    assert_eq!(
        email_lines.len(),
        6,
        "all 6 emails must be emitted: {emitted}"
    );

    // Slot 1 (E_CONTACT_EMAIL_1): e_work_pri (pref: 1)
    assert!(
        email_lines[0].contains("X-JMAP-KEY=e_work_pri")
            && email_lines[0].contains("vera.work@example.com")
            && email_lines[0].contains("TYPE=WORK")
            && email_lines[0].contains("PREF"),
        "Slot 1 (E_CONTACT_EMAIL_1) must be e_work_pri: {}",
        email_lines[0]
    );

    // Slot 2 (E_CONTACT_EMAIL_2): e_archive (unranked, sorted key order)
    assert!(
        email_lines[1].contains("X-JMAP-KEY=e_archive")
            && email_lines[1].contains("archive@example.com")
            && !email_lines[1].contains("PREF"),
        "Slot 2 (E_CONTACT_EMAIL_2) must be e_archive: {}",
        email_lines[1]
    );

    // Slot 3 (E_CONTACT_EMAIL_3): e_billing (unranked)
    assert!(
        email_lines[2].contains("X-JMAP-KEY=e_billing")
            && email_lines[2].contains("billing@example.com")
            && !email_lines[2].contains("PREF"),
        "Slot 3 (E_CONTACT_EMAIL_3) must be e_billing: {}",
        email_lines[2]
    );

    // Slot 4 (E_CONTACT_EMAIL_4): e_direct (unranked)
    assert!(
        email_lines[3].contains("X-JMAP-KEY=e_direct")
            && email_lines[3].contains("vera.direct@example.com")
            && email_lines[3].contains("TYPE=WORK")
            && !email_lines[3].contains("PREF"),
        "Slot 4 (E_CONTACT_EMAIL_4) must be e_direct: {}",
        email_lines[3]
    );

    // Slot 5 (E_CONTACT_EMAIL list entry 5): e_home (unranked)
    assert!(
        email_lines[4].contains("X-JMAP-KEY=e_home")
            && email_lines[4].contains("vera.home@example.com")
            && email_lines[4].contains("TYPE=HOME")
            && !email_lines[4].contains("PREF"),
        "Slot 5 (E_CONTACT_EMAIL list entry 5) must be e_home: {}",
        email_lines[4]
    );

    // Slot 6 (E_CONTACT_EMAIL list entry 6): e_support (unranked)
    assert!(
        email_lines[5].contains("X-JMAP-KEY=e_support")
            && email_lines[5].contains("support@example.com")
            && !email_lines[5].contains("PREF"),
        "Slot 6 (E_CONTACT_EMAIL list entry 6) must be e_support: {}",
        email_lines[5]
    );

    // Inbound parse: all 6 emails are preserved with their exact addresses, contexts, and keys
    let card2 = vcard_to_card(&emitted).expect("parse back 6 emails");
    assert_eq!(card2, card);

    // Re-emission converges to identical fixed point
    let emitted2 = card_to_vcard(&card2);
    assert_eq!(emitted2, emitted);

    // 2. Unkeyed inbound vCard with 5 emails allocates e1..e5 in document order
    let unkeyed_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Multi Email Contact\r\n",
        "EMAIL;TYPE=WORK,PREF:one@example.com\r\n",
        "EMAIL;TYPE=HOME:two@example.com\r\n",
        "EMAIL;TYPE=WORK:three@example.com\r\n",
        "EMAIL:four@example.com\r\n",
        "EMAIL;TYPE=OTHER:five@example.com\r\n",
        "END:VCARD\r\n"
    );
    let unkeyed_card = vcard_to_card(unkeyed_vcard).expect("parse unkeyed");
    let unkeyed_emails = unkeyed_card.emails.as_ref().expect("emails");
    assert_eq!(unkeyed_emails.len(), 5);
    assert_eq!(unkeyed_emails["e1"].address, "one@example.com");
    assert_eq!(unkeyed_emails["e1"].pref, Some(1));
    assert_eq!(unkeyed_emails["e2"].address, "two@example.com");
    assert_eq!(unkeyed_emails["e2"].pref, None);
    assert_eq!(unkeyed_emails["e3"].address, "three@example.com");
    assert_eq!(unkeyed_emails["e4"].address, "four@example.com");
    assert_eq!(unkeyed_emails["e5"].address, "five@example.com");

    let unkeyed_emitted = card_to_vcard(&unkeyed_card);
    let unkeyed_card2 = vcard_to_card(&unkeyed_emitted).expect("re-parse");
    assert_eq!(unkeyed_card2, unkeyed_card);
}

#[test]
fn address_three_label_slots_work_home_other_and_adr_pairing_matrix() {
    // Audit & characterization of EDS Address and Label slots:
    // EDS provides 3 primary address slots (each with 7 subfields = 21 fields) + 3 synthetic label string fields:
    //   - Work slot:  E_CONTACT_ADDRESS_WORK (field 5)  / E_CONTACT_ADDRESS_LABEL_WORK (field 14)  [TYPE=WORK]
    //   - Home slot:  E_CONTACT_ADDRESS_HOME (field 4)  / E_CONTACT_ADDRESS_LABEL_HOME (field 13)  [TYPE=HOME]
    //   - Other slot: E_CONTACT_ADDRESS_OTHER (field 6) / E_CONTACT_ADDRESS_LABEL_OTHER (field 15) [TYPE=OTHER / bare]
    //
    // Test matrix covers:
    // 1. All 3 slots with structured ADR + matching standalone LABEL lines.
    // 2. All 3 slots with standalone LABEL only (no structured ADR).
    // 3. All 3 slots with structured ADR only (no LABEL).
    // 4. Mixed slots (Work: ADR+LABEL, Home: LABEL only, Other: ADR only).
    // 5. In-place modification of synthetic label fields in EDS.
    // 6. PREF interplay across address slots: highest preference emitted first with TYPE=PREF.
    // 7. Unkeyed inbound vCards pairing ADR and LABEL by context fallback (WORK, HOME, OTHER/bare).

    // --- 1. All 3 slots with structured ADR + matching standalone LABEL ---
    let mut extra_work = BTreeMap::new();
    extra_work.insert("pref".to_owned(), json!(1));

    let card_all3 = ContactCard {
        card_type: Some("Card".to_owned()),
        version: Some("1.0".to_owned()),
        addresses: Some(
            [
                (
                    "a_work".to_owned(),
                    Address {
                        components: Some(vec![
                            AddressComponent::new("name", "Hauptstraße 1"),
                            AddressComponent::new("locality", "Berlin"),
                            AddressComponent::new("postcode", "10115"),
                            AddressComponent::new("country", "Germany"),
                        ]),
                        contexts: Some(json!({"work": true})),
                        full: Some("Hauptstraße 1\n10115 Berlin\nGermany".to_owned()),
                        extra: extra_work,
                    },
                ),
                (
                    "a_home".to_owned(),
                    Address {
                        components: Some(vec![
                            AddressComponent::new("name", "Heimweg 2"),
                            AddressComponent::new("locality", "München"),
                            AddressComponent::new("postcode", "80331"),
                            AddressComponent::new("country", "Germany"),
                        ]),
                        contexts: Some(json!({"private": true})),
                        full: Some("Heimweg 2\n80331 München\nGermany".to_owned()),
                        extra: BTreeMap::new(),
                    },
                ),
                (
                    "a_other".to_owned(),
                    Address {
                        components: Some(vec![
                            AddressComponent::new("postOfficeBox", "Postfach 42"),
                            AddressComponent::new("locality", "Hamburg"),
                            AddressComponent::new("postcode", "20095"),
                            AddressComponent::new("country", "Germany"),
                        ]),
                        contexts: None,
                        full: Some("Postfach 42\n20095 Hamburg\nGermany".to_owned()),
                        extra: BTreeMap::new(),
                    },
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ..ContactCard::default()
    };

    let emitted_all3 = card_to_vcard(&card_all3);

    // Verify emission order: a_work (pref: 1) is first, followed by a_home and a_other
    let adr_lines: Vec<&str> = emitted_all3
        .lines()
        .filter(|l| l.starts_with("ADR"))
        .collect();
    let label_lines: Vec<&str> = emitted_all3
        .lines()
        .filter(|l| l.starts_with("LABEL"))
        .collect();
    assert_eq!(adr_lines.len(), 3, "3 ADR lines: {emitted_all3}");
    assert_eq!(label_lines.len(), 3, "3 LABEL lines: {emitted_all3}");

    // Work slot (E_CONTACT_ADDRESS_WORK / E_CONTACT_ADDRESS_LABEL_WORK):
    assert!(
        adr_lines[0].contains("X-JMAP-KEY=a_work")
            && adr_lines[0].contains("TYPE=WORK")
            && adr_lines[0].contains("PREF"),
        "Work ADR must be 1st: {}",
        adr_lines[0]
    );
    assert!(
        label_lines[0].contains("X-JMAP-KEY=a_work")
            && label_lines[0].contains("TYPE=WORK")
            && label_lines[0].contains("PREF"),
        "Work LABEL must be 1st: {}",
        label_lines[0]
    );

    // Home slot (E_CONTACT_ADDRESS_HOME / E_CONTACT_ADDRESS_LABEL_HOME):
    assert!(
        adr_lines[1].contains("X-JMAP-KEY=a_home")
            && adr_lines[1].contains("TYPE=HOME")
            && !adr_lines[1].contains("PREF"),
        "Home ADR: {}",
        adr_lines[1]
    );
    assert!(
        label_lines[1].contains("X-JMAP-KEY=a_home") && label_lines[1].contains("TYPE=HOME"),
        "Home LABEL: {}",
        label_lines[1]
    );

    // Other slot (E_CONTACT_ADDRESS_OTHER / E_CONTACT_ADDRESS_LABEL_OTHER):
    assert!(
        adr_lines[2].contains("X-JMAP-KEY=a_other") && !adr_lines[2].contains("TYPE="),
        "Other ADR (unslotted): {}",
        adr_lines[2]
    );
    assert!(
        label_lines[2].contains("X-JMAP-KEY=a_other") && !label_lines[2].contains("TYPE="),
        "Other LABEL (unslotted): {}",
        label_lines[2]
    );

    // Inbound parse: all 3 addresses are restored with their structured components AND full labels
    let parsed_all3 = vcard_to_card(&emitted_all3).expect("parse back all3");
    assert_eq!(parsed_all3, card_all3);

    // Roundtrip fixed-point
    let re_emitted_all3 = card_to_vcard(&parsed_all3);
    assert_eq!(re_emitted_all3, emitted_all3);

    // --- 2. All 3 slots with standalone LABEL only (no structured ADR) ---
    let card_labels_only = ContactCard {
        card_type: Some("Card".to_owned()),
        version: Some("1.0".to_owned()),
        addresses: Some(
            [
                (
                    "a1".to_owned(),
                    Address {
                        components: None,
                        contexts: Some(json!({"work": true})),
                        full: Some("Work Label Only\nBerlin".to_owned()),
                        extra: BTreeMap::new(),
                    },
                ),
                (
                    "a2".to_owned(),
                    Address {
                        components: None,
                        contexts: Some(json!({"private": true})),
                        full: Some("Home Label Only\nMünchen".to_owned()),
                        extra: BTreeMap::new(),
                    },
                ),
                (
                    "a3".to_owned(),
                    Address {
                        components: None,
                        contexts: None,
                        full: Some("Other Label Only\nHamburg".to_owned()),
                        extra: BTreeMap::new(),
                    },
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ..ContactCard::default()
    };

    let emitted_labels = card_to_vcard(&card_labels_only);
    assert!(
        !emitted_labels.contains("ADR"),
        "no ADR lines should be emitted"
    );
    let lbl_lines: Vec<&str> = emitted_labels
        .lines()
        .filter(|l| l.starts_with("LABEL"))
        .collect();
    assert_eq!(lbl_lines.len(), 3);
    assert!(lbl_lines[0].contains("TYPE=WORK") && lbl_lines[0].contains("Work Label Only"));
    assert!(lbl_lines[1].contains("TYPE=HOME") && lbl_lines[1].contains("Home Label Only"));
    assert!(!lbl_lines[2].contains("TYPE=") && lbl_lines[2].contains("Other Label Only"));

    let parsed_labels = vcard_to_card(&emitted_labels).expect("parse labels only");
    assert_eq!(parsed_labels, card_labels_only);

    // --- 3. Mixed slots (Work: ADR+LABEL, Home: LABEL only, Other: ADR only) ---
    let card_mixed = ContactCard {
        card_type: Some("Card".to_owned()),
        version: Some("1.0".to_owned()),
        addresses: Some(
            [
                (
                    "a_work".to_owned(),
                    Address {
                        components: Some(vec![AddressComponent::new("name", "Work Street 10")]),
                        contexts: Some(json!({"work": true})),
                        full: Some("Work Street 10\nBerlin".to_owned()),
                        extra: BTreeMap::new(),
                    },
                ),
                (
                    "a_home".to_owned(),
                    Address {
                        components: None,
                        contexts: Some(json!({"private": true})),
                        full: Some("Home Label Only\nCologne".to_owned()),
                        extra: BTreeMap::new(),
                    },
                ),
                (
                    "a_other".to_owned(),
                    Address {
                        components: Some(vec![AddressComponent::new("locality", "Frankfurt")]),
                        contexts: None,
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

    let emitted_mixed = card_to_vcard(&card_mixed);
    let parsed_mixed = vcard_to_card(&emitted_mixed).expect("parse mixed slots");
    assert_eq!(parsed_mixed, card_mixed);

    // --- 4. Unkeyed vCard with WORK, HOME, and OTHER ADR + LABEL lines ---
    // Simulates an EDS contact export or foreign vCard without X-JMAP-KEY:
    // label_entry must pair each LABEL with its matching ADR by context (WORK -> WORK, HOME -> HOME, OTHER/bare -> OTHER).
    let unkeyed_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Unkeyed Address Contact\r\n",
        "ADR;TYPE=WORK:;;Hauptstraße 1;Berlin;;10115;Germany\r\n",
        "LABEL;TYPE=WORK:Hauptstraße 1\\n10115 Berlin\\nGermany\r\n",
        "ADR;TYPE=HOME:;;Heimweg 2;München;;80331;Germany\r\n",
        "LABEL;TYPE=HOME:Heimweg 2\\n80331 München\\nGermany\r\n",
        "ADR;TYPE=OTHER:;;Postfach 42;Hamburg;;20095;Germany\r\n",
        "LABEL;TYPE=OTHER:Postfach 42\\n20095 Hamburg\\nGermany\r\n",
        "LABEL:Bare Label Without Type\\nBerlin\r\n",
        "END:VCARD\r\n"
    );

    let unkeyed_card = vcard_to_card(unkeyed_vcard).expect("parse unkeyed addresses");
    let unkeyed_addrs = unkeyed_card.addresses.as_ref().expect("addresses");
    // a1 (WORK), a2 (HOME), a3 (OTHER), a4 (bare label)
    assert_eq!(
        unkeyed_addrs.len(),
        4,
        "expected 4 distinct address entries"
    );
    assert_eq!(unkeyed_addrs["a1"].contexts, Some(json!({"work": true})));
    assert_eq!(
        unkeyed_addrs["a1"].full.as_deref(),
        Some("Hauptstraße 1\n10115 Berlin\nGermany")
    );
    assert_eq!(unkeyed_addrs["a2"].contexts, Some(json!({"private": true})));
    assert_eq!(
        unkeyed_addrs["a2"].full.as_deref(),
        Some("Heimweg 2\n80331 München\nGermany")
    );
    assert_eq!(unkeyed_addrs["a3"].contexts, None);
    assert_eq!(
        unkeyed_addrs["a3"].full.as_deref(),
        Some("Postfach 42\n20095 Hamburg\nGermany")
    );
    assert_eq!(unkeyed_addrs["a4"].contexts, None);
    assert_eq!(
        unkeyed_addrs["a4"].full.as_deref(),
        Some("Bare Label Without Type\nBerlin")
    );

    // Re-emission converges to fixed point
    let unkeyed_emitted = card_to_vcard(&unkeyed_card);
    let unkeyed_card2 = vcard_to_card(&unkeyed_emitted).expect("re-parse");
    assert_eq!(unkeyed_card2, unkeyed_card);
}

#[test]
fn email_and_address_label_edge_cases_and_parameter_permutations() {
    // 1. Multi-line labels with escaped delimiters: newlines, commas, semicolons, backslashes
    let card = ContactCard {
        card_type: Some("Card".to_owned()),
        version: Some("1.0".to_owned()),
        addresses: Some(
            [(
                "a_complex".to_owned(),
                Address {
                    components: Some(vec![
                        AddressComponent::new("name", "Suite 400, Floor 2"),
                        AddressComponent::new("locality", "San Francisco; Bay Area"),
                        AddressComponent::new("country", "United States \\ USA"),
                    ]),
                    contexts: Some(json!({"work": true})),
                    full: Some(
                        "Acme Corp, Suite 400\\nDept; Ops\\nSan Francisco, CA\\nUSA".to_owned(),
                    ),
                    extra: BTreeMap::new(),
                },
            )]
            .into_iter()
            .collect(),
        ),
        emails: Some(
            [(
                "e1".to_owned(),
                ContactEmail {
                    address: "complex+user@example.com".to_owned(),
                    contexts: Some(json!({"work": true})),
                    pref: Some(1),
                    ..ContactEmail::default()
                },
            )]
            .into_iter()
            .collect(),
        ),
        ..ContactCard::default()
    };

    let emitted = card_to_vcard(&card);
    let card2 = vcard_to_card(&emitted).expect("parse complex delimiters");
    assert_eq!(card2, card);
    let emitted2 = card_to_vcard(&card2);
    assert_eq!(emitted2, emitted);

    // 2. Inbound vCard with mixed-case and lowercase parameters (type=work,pref, TYPE=HOME, type=other)
    let mixed_case_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Case Permutations\r\n",
        "email;type=work,pref:work@case.example\r\n",
        "EMAIL;TYPE=HOME:home@case.example\r\n",
        "email;type=other:other@case.example\r\n",
        "adr;type=work,pref:;;100 Work St;City;;12345;US\r\n",
        "label;type=work,pref:100 Work St\\nCity\\nUS\r\n",
        "ADR;TYPE=HOME:;;200 Home Rd;City;;12345;US\r\n",
        "LABEL;TYPE=HOME:200 Home Rd\\nCity\\nUS\r\n",
        "adr;type=other:;;300 Other Ave;City;;12345;US\r\n",
        "label;type=other:300 Other Ave\\nCity\\nUS\r\n",
        // Empty lines that must be safely ignored
        "EMAIL:\r\n",
        "ADR:;;;;;;\r\n",
        "LABEL:\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(mixed_case_vcard).expect("parse mixed case");
    let emails = parsed.emails.as_ref().expect("emails");
    assert_eq!(emails.len(), 3);
    assert_eq!(emails["e1"].address, "work@case.example");
    assert_eq!(emails["e1"].pref, Some(1));
    assert_eq!(emails["e2"].address, "home@case.example");
    assert_eq!(emails["e3"].address, "other@case.example");

    let addrs = parsed.addresses.as_ref().expect("addresses");
    assert_eq!(addrs.len(), 3);
    assert_eq!(addrs["a1"].contexts, Some(json!({"work": true})));
    assert_eq!(addrs["a1"].extra.get("pref"), Some(&json!(1)));
    assert_eq!(addrs["a2"].contexts, Some(json!({"private": true})));
    assert_eq!(addrs["a3"].contexts, None);

    let re_emitted = card_to_vcard(&parsed);
    let parsed2 = vcard_to_card(&re_emitted).expect("re-parse");
    assert_eq!(parsed2, parsed);
}

#[test]
fn vcard_21_outlook_representative_fixture_import_and_normalization() {
    // Representative vCard 2.1 exported by legacy Microsoft Outlook:
    // - VERSION:2.1
    // - Bare parameter type words: TEL;WORK;VOICE, TEL;HOME;VOICE, TEL;CELL;VOICE, TEL;WORK;FAX
    // - EMAIL;PREF;INTERNET and EMAIL;INTERNET
    // - ADR;WORK;PREF and LABEL;WORK;PREF;ENCODING=QUOTED-PRINTABLE
    // - NOTE;ENCODING=QUOTED-PRINTABLE with German umlauts and multi-line soft breaks
    // - PHOTO;JPEG;ENCODING=BASE64
    // - Standard fields (N, FN, ORG, TITLE, BDAY, URL, REV)
    let outlook_vcard_21 = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:2.1\r\n",
        "N:Mustermann;Erika;;Dr.;\r\n",
        "FN:Dr. Erika Mustermann\r\n",
        "ORG:Musterfirma GmbH;Entwicklung;Software\r\n",
        "TITLE:Leitende Entwicklerin\r\n",
        "NOTE;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:Dies ist eine Notiz mit Umlauten: =C3=84, =C3=96, =C3=9C, =C3=A4, =C3=B6=\r\n",
        ", =C3=BC, =C3=9F.=0D=0AZweite Zeile mit Semikolon; und Komma, und Backslash\\\r\n",
        "TEL;WORK;VOICE:+49-89-1234567\r\n",
        "TEL;HOME;VOICE:+49-89-7654321\r\n",
        "TEL;CELL;VOICE:+49-170-1234567\r\n",
        "TEL;WORK;FAX:+49-89-1234568\r\n",
        "ADR;WORK;PREF;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:;;Musterstra=C3=9Fe 123;M=C3=BCnchen;Bayern;80331;Deutschland\r\n",
        "LABEL;WORK;PREF;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:Musterstra=C3=9Fe 123=0D=0A80331 M=C3=BCnchen=0D=0ADeutschland\r\n",
        "EMAIL;PREF;INTERNET:erika@musterfirma.de\r\n",
        "EMAIL;INTERNET:erika.mustermann@home.de\r\n",
        "URL;WORK:https://www.musterfirma.de\r\n",
        "BDAY:1975-04-12\r\n",
        "PHOTO;JPEG;ENCODING=BASE64:\r\n",
        " /9j/4AAQSkZJRgABAQEASABIAAD/2wBDAP//////////////////////////////////////\r\n",
        " //////////////////////////////////////////////////////wgALCAABAAEBAREA\r\n",
        " /8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABPxA=\r\n",
        "REV:20230115T120000Z\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(outlook_vcard_21).expect("parse outlook 2.1 vcard");

    // 1. Name verification
    let name = card.name.as_ref().expect("name");
    assert_eq!(name.full.as_deref(), Some("Dr. Erika Mustermann"));
    let components = name.components.as_ref().expect("components");
    assert_eq!(components[0].kind, "title");
    assert_eq!(components[0].value, "Dr.");
    assert_eq!(components[1].kind, "given");
    assert_eq!(components[1].value, "Erika");
    assert_eq!(components[2].kind, "surname");
    assert_eq!(components[2].value, "Mustermann");

    // 2. Organization & Title
    let orgs = card.organizations.as_ref().expect("organizations");
    assert_eq!(orgs["o1"].name.as_deref(), Some("Musterfirma GmbH"));
    let units = orgs["o1"].units.as_ref().expect("units");
    assert_eq!(units.len(), 2);
    assert_eq!(units[0].name, "Entwicklung");
    assert_eq!(units[1].name, "Software");
    let titles = card.titles.as_ref().expect("titles");
    assert_eq!(titles["t1"].name, "Leitende Entwicklerin");

    // 3. Note with QP decoded umlauts and soft line break
    let notes = card.notes.as_ref().expect("notes");
    assert_eq!(
        notes["n1"].note,
        "Dies ist eine Notiz mit Umlauten: Ä, Ö, Ü, ä, ö, ü, ß.\r\nZweite Zeile mit Semikolon; und Komma, und Backslash\\"
    );

    // 4. Telephones with bare 2.1 type words
    let phones = card.phones.as_ref().expect("phones");
    assert_eq!(phones.len(), 4);
    assert_eq!(phones["p1"].number, "+49-89-1234567");
    assert_eq!(phones["p1"].contexts, Some(json!({"work": true})));
    assert_eq!(phones["p1"].features, Some(json!({"voice": true})));

    assert_eq!(phones["p2"].number, "+49-89-7654321");
    assert_eq!(phones["p2"].contexts, Some(json!({"private": true})));
    assert_eq!(phones["p2"].features, Some(json!({"voice": true})));

    assert_eq!(phones["p3"].number, "+49-170-1234567");
    assert_eq!(
        phones["p3"].features,
        Some(json!({"mobile": true, "voice": true}))
    );

    assert_eq!(phones["p4"].number, "+49-89-1234568");
    assert_eq!(phones["p4"].contexts, Some(json!({"work": true})));
    assert_eq!(phones["p4"].features, Some(json!({"fax": true})));

    // 5. Address & Label with PREF and QP decoding
    let addrs = card.addresses.as_ref().expect("addresses");
    assert_eq!(addrs.len(), 1);
    let addr = &addrs["a1"];
    assert_eq!(addr.contexts, Some(json!({"work": true})));
    assert_eq!(addr.extra.get("pref"), Some(&json!(1)));
    assert_eq!(
        addr.full.as_deref(),
        Some("Musterstraße 123\r\n80331 München\r\nDeutschland")
    );
    let addr_comps = addr.components.as_ref().expect("address components");
    assert_eq!(addr_comps[0].kind, "name");
    assert_eq!(addr_comps[0].value, "Musterstraße 123");
    assert_eq!(addr_comps[1].kind, "locality");
    assert_eq!(addr_comps[1].value, "München");
    assert_eq!(addr_comps[2].kind, "region");
    assert_eq!(addr_comps[2].value, "Bayern");
    assert_eq!(addr_comps[3].kind, "postcode");
    assert_eq!(addr_comps[3].value, "80331");
    assert_eq!(addr_comps[4].kind, "country");
    assert_eq!(addr_comps[4].value, "Deutschland");

    // 6. Emails with PREF
    let emails = card.emails.as_ref().expect("emails");
    assert_eq!(emails.len(), 2);
    assert_eq!(emails["e1"].address, "erika@musterfirma.de");
    assert_eq!(emails["e1"].pref, Some(1));
    assert_eq!(emails["e2"].address, "erika.mustermann@home.de");
    assert_eq!(emails["e2"].pref, None);

    // 7. URL & Birthday
    let links = card.links.as_ref().expect("links");
    assert_eq!(links["l1"].uri, "https://www.musterfirma.de");
    let anniv = card.anniversaries.as_ref().expect("anniversaries");
    assert_eq!(anniv["y1"].kind.as_str(), "birth");
    assert_eq!(
        anniversary_date(&anniv["y1"]),
        Some("1975-04-12".to_owned())
    );

    // 8. Photo with JPEG subtype inferred from bare 2.1 parameter
    let media = card.media.as_ref().expect("media");
    assert_eq!(media.len(), 1);
    let photo = &media["m1"];
    assert_eq!(photo.kind.as_deref(), Some("photo"));
    assert_eq!(photo.media_type.as_deref(), Some("image/JPEG"));
    assert!(photo.uri.starts_with("data:image/JPEG;base64,"));

    // 9. Outbound emission normalizes strictly to vCard 3.0
    let emitted = card_to_vcard(&card);
    assert!(emitted.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"));
    assert!(!emitted.contains("VERSION:2.1"));
    assert!(!emitted.contains("QUOTED-PRINTABLE"));
    assert!(!emitted.contains("INTERNET"));
    assert!(emitted.contains("TYPE=JPEG") && emitted.contains("ENCODING=b"));
    assert!(emitted.contains("TYPE=WORK,VOICE:"));
    assert!(emitted.contains("TYPE=HOME,VOICE:"));
    assert!(emitted.contains("TYPE=CELL:"));
    assert!(emitted.contains("TYPE=WORK,FAX:"));
    assert!(
        emitted.contains("EMAIL;X-JMAP-KEY=e1;TYPE=PREF:erika@musterfirma.de")
            || emitted.contains("EMAIL;TYPE=PREF;X-JMAP-KEY=e1:erika@musterfirma.de")
    );

    // 10. Fixed-point roundtrip stability (re-emitting normalized 3.0 reaches fixpoint)
    let card2 = vcard_to_card(&emitted).expect("parse normalized 3.0 vcard");
    let emitted2 = card_to_vcard(&card2);
    assert_eq!(emitted2, emitted);
    let card3 = vcard_to_card(&emitted2).expect("parse fixpoint vcard");
    assert_eq!(card3, card2);
}

#[test]
fn vcard_21_feature_phone_nokia_sony_ericsson_fixtures_import() {
    // Real-world vCard 2.1 from Nokia and Sony Ericsson feature phones:
    // - Bare TEL types: TEL;VOICE;HOME, TEL;VOICE;WORK, TEL;CELL, TEL;MOBILE, TEL;FAX;WORK, TEL;PAGER
    // - Bare EMAIL;INTERNET
    // - Soft line wrapped QP note with =0D=0A
    // - Unmapped SOUND property safely ignored
    let feature_phone_vcard_21 = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:2.1\r\n",
        "N:Smith;John\r\n",
        "FN:John Smith\r\n",
        "TEL;VOICE;HOME:555-1111\r\n",
        "TEL;VOICE;WORK:555-2222\r\n",
        "TEL;CELL:555-3333\r\n",
        "TEL;MOBILE:555-4444\r\n",
        "TEL;FAX;WORK:555-5555\r\n",
        "TEL;PAGER:555-6666\r\n",
        "EMAIL;INTERNET:john.smith@example.com\r\n",
        "ADR;HOME:;;123 Main St;Springfield;IL;62701;USA\r\n",
        "NOTE;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:Met at the conference=0D=0APromised to follow up=\r\n",
        " next week about project.\r\n",
        "SOUND;WAVE;BASE64:\r\n",
        " UklGRiQAAABXQVZFZm10IBAAAAABAAEARKwAAIhYAQACABAAZGF0YQAAAAA=\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(feature_phone_vcard_21).expect("parse feature phone 2.1");

    let name = card.name.as_ref().expect("name");
    assert_eq!(name.full.as_deref(), Some("John Smith"));

    let phones = card.phones.as_ref().expect("phones");
    assert_eq!(phones.len(), 6);
    assert_eq!(phones["p1"].number, "555-1111");
    assert_eq!(phones["p1"].contexts, Some(json!({"private": true})));
    assert_eq!(phones["p1"].features, Some(json!({"voice": true})));

    assert_eq!(phones["p2"].number, "555-2222");
    assert_eq!(phones["p2"].contexts, Some(json!({"work": true})));
    assert_eq!(phones["p2"].features, Some(json!({"voice": true})));

    assert_eq!(phones["p3"].number, "555-3333");
    assert_eq!(phones["p3"].features, Some(json!({"mobile": true})));

    assert_eq!(phones["p4"].number, "555-4444");
    assert_eq!(phones["p4"].features, Some(json!({"mobile": true})));

    assert_eq!(phones["p5"].number, "555-5555");
    assert_eq!(phones["p5"].contexts, Some(json!({"work": true})));
    assert_eq!(phones["p5"].features, Some(json!({"fax": true})));

    assert_eq!(phones["p6"].number, "555-6666");
    assert_eq!(phones["p6"].features, Some(json!({"pager": true})));

    let notes = card.notes.as_ref().expect("notes");
    assert_eq!(
        notes["n1"].note,
        "Met at the conference\r\nPromised to follow up next week about project."
    );

    // SOUND is an audio property, not a PHOTO picture, so it is ignored on parse
    assert!(card.media.is_none());

    // Outbound emission normalizes to vCard 3.0
    let emitted = card_to_vcard(&card);
    assert!(emitted.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"));
    assert!(emitted.contains("555-3333") && emitted.contains("TYPE=CELL"));
    assert!(emitted.contains("555-4444") && emitted.contains("TYPE=CELL"));
    assert!(emitted.contains("555-6666") && emitted.contains("TYPE=PAGER"));

    let card2 = vcard_to_card(&emitted).expect("re-parse");
    assert_eq!(card2, card);
}

#[test]
fn vcard_21_legacy_charsets_iso_8859_1_and_windows_1252_import() {
    // 1. ISO-8859-1 German fixture
    let iso_vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:2.1\r\n",
        "N;CHARSET=ISO-8859-1;ENCODING=QUOTED-PRINTABLE:M=FCller;Hans;;;\r\n",
        "FN;CHARSET=ISO-8859-1;ENCODING=QUOTED-PRINTABLE:Hans M=FCller\r\n",
        "ORG;CHARSET=ISO-8859-1:M=FCller AG\r\n",
        "NOTE;CHARSET=ISO-8859-1;ENCODING=QUOTED-PRINTABLE:Gr=FC=DFe aus Z=FCrich=0D=0AFreundliche Empfehlung\r\n",
        "EMAIL;INTERNET;PREF:hans.mueller@example.ch\r\n",
        "TEL;HOME:044 123 45 67\r\n",
        "ADR;HOME;CHARSET=ISO-8859-1;ENCODING=QUOTED-PRINTABLE:;;Bahnhofstrasse 10;Z=FCrich;;8001;Schweiz\r\n",
        "END:VCARD\r\n"
    );

    let card1 = vcard_to_card(iso_vcard).expect("parse iso-8859-1 vcard 2.1");
    let name1 = card1.name.as_ref().expect("name1");
    assert_eq!(name1.full.as_deref(), Some("Hans Müller"));
    let components1 = name1.components.as_ref().expect("components1");
    assert_eq!(components1[0].value, "Hans");
    assert_eq!(components1[1].value, "Müller");

    let notes1 = card1.notes.as_ref().expect("notes1");
    assert_eq!(
        notes1["n1"].note,
        "Grüße aus Zürich\r\nFreundliche Empfehlung"
    );

    let addrs1 = card1.addresses.as_ref().expect("addrs1");
    let addr_comps1 = addrs1["a1"].components.as_ref().expect("addr comps1");
    assert_eq!(addr_comps1[0].value, "Bahnhofstrasse 10");
    assert_eq!(addr_comps1[1].value, "Zürich");
    assert_eq!(addr_comps1[2].value, "8001");
    assert_eq!(addr_comps1[3].value, "Schweiz");

    let emitted1 = card_to_vcard(&card1);
    assert!(!emitted1.contains("CHARSET"));
    assert!(!emitted1.contains("QUOTED-PRINTABLE"));
    let card1_re = vcard_to_card(&emitted1).expect("re-parse iso");
    assert_eq!(card1_re, card1);

    // 2. Windows-1252 French fixture with Euro sign (=80)
    let win_vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:2.1\r\n",
        "N;CHARSET=WINDOWS-1252;ENCODING=QUOTED-PRINTABLE:Fran=E7ois;Ren=E9;;;\r\n",
        "FN;CHARSET=WINDOWS-1252;ENCODING=QUOTED-PRINTABLE:Ren=E9 Fran=E7ois\r\n",
        "NOTE;CHARSET=WINDOWS-1252;ENCODING=QUOTED-PRINTABLE:Co=FBt: 100 =80=0D=0APrix net\r\n",
        "EMAIL;INTERNET:rene@example.fr\r\n",
        "END:VCARD\r\n"
    );

    let card2 = vcard_to_card(win_vcard).expect("parse windows-1252 vcard 2.1");
    let name2 = card2.name.as_ref().expect("name2");
    assert_eq!(name2.full.as_deref(), Some("René François"));
    let notes2 = card2.notes.as_ref().expect("notes2");
    assert_eq!(notes2["n1"].note, "Coût: 100 €\r\nPrix net");

    let emitted2 = card_to_vcard(&card2);
    let card2_re = vcard_to_card(&emitted2).expect("re-parse win");
    assert_eq!(card2_re, card2);
}

#[test]
fn vcard_21_bare_type_words_and_combinations_matrix() {
    // Tests exhaustive combinations of bare type words across TEL, EMAIL, ADR, LABEL
    let bare_words_vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:2.1\r\n",
        "FN:Bare Parameter Matrix\r\n",
        // TEL combinations
        "TEL;WORK;VOICE:+1-555-0101\r\n",
        "TEL;HOME;FAX:+1-555-0102\r\n",
        "TEL;WORK;FAX:+1-555-0103\r\n",
        "TEL;HOME;VOICE:+1-555-0104\r\n",
        "TEL;CELL:+1-555-0105\r\n",
        "TEL;MOBILE:+1-555-0106\r\n",
        "TEL;PAGER:+1-555-0107\r\n",
        "TEL;VIDEO:+1-555-0108\r\n",
        "TEL;PREF;WORK;VOICE:+1-555-0100\r\n",
        // EMAIL combinations
        "EMAIL;WORK;INTERNET;PREF:work.primary@matrix.example\r\n",
        "EMAIL;HOME;INTERNET:home@matrix.example\r\n",
        "EMAIL;INTERNET:general@matrix.example\r\n",
        // ADR combinations with bare context/type
        "ADR;WORK;POSTAL;PARCEL;DOM:;;100 Work Blvd;Work City;Work State;10001;USA\r\n",
        "ADR;HOME;POSTAL:;;200 Home Lane;Home Town;Home State;20002;USA\r\n",
        "ADR;POSTAL;PREF:;;300 Postal Box;Other City;;30003;USA\r\n",
        // LABEL with bare type and QP
        "LABEL;WORK;PREF;ENCODING=QUOTED-PRINTABLE:100 Work Blvd=0D=0AWork City, Work State 10001\r\n",
        "LABEL;HOME;ENCODING=QUOTED-PRINTABLE:200 Home Lane=0D=0AHome Town, Home State 20002\r\n",
        "LABEL;POSTAL;PREF;ENCODING=QUOTED-PRINTABLE:300 Postal Box=0D=0AOther City 30003\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(bare_words_vcard).expect("parse bare words matrix");

    // 1. Phone assertions
    let phones = card.phones.as_ref().expect("phones");
    assert_eq!(phones.len(), 9);
    assert_eq!(phones["p1"].number, "+1-555-0101");
    assert_eq!(phones["p1"].contexts, Some(json!({"work": true})));
    assert_eq!(phones["p1"].features, Some(json!({"voice": true})));

    assert_eq!(phones["p2"].number, "+1-555-0102");
    assert_eq!(phones["p2"].contexts, Some(json!({"private": true})));
    assert_eq!(phones["p2"].features, Some(json!({"fax": true})));

    assert_eq!(phones["p3"].number, "+1-555-0103");
    assert_eq!(phones["p3"].contexts, Some(json!({"work": true})));
    assert_eq!(phones["p3"].features, Some(json!({"fax": true})));

    assert_eq!(phones["p4"].number, "+1-555-0104");
    assert_eq!(phones["p4"].contexts, Some(json!({"private": true})));
    assert_eq!(phones["p4"].features, Some(json!({"voice": true})));

    assert_eq!(phones["p5"].number, "+1-555-0105");
    assert_eq!(phones["p5"].features, Some(json!({"mobile": true})));

    assert_eq!(phones["p6"].number, "+1-555-0106");
    assert_eq!(phones["p6"].features, Some(json!({"mobile": true})));

    assert_eq!(phones["p7"].number, "+1-555-0107");
    assert_eq!(phones["p7"].features, Some(json!({"pager": true})));

    assert_eq!(phones["p8"].number, "+1-555-0108");
    assert_eq!(phones["p8"].features, Some(json!({"video": true})));

    assert_eq!(phones["p9"].number, "+1-555-0100");
    assert_eq!(phones["p9"].pref, Some(1));
    assert_eq!(phones["p9"].contexts, Some(json!({"work": true})));
    assert_eq!(phones["p9"].features, Some(json!({"voice": true})));

    // 2. Email assertions
    let emails = card.emails.as_ref().expect("emails");
    assert_eq!(emails.len(), 3);
    assert_eq!(emails["e1"].address, "work.primary@matrix.example");
    assert_eq!(emails["e1"].pref, Some(1));
    assert_eq!(emails["e1"].contexts, Some(json!({"work": true})));
    assert_eq!(emails["e2"].address, "home@matrix.example");
    assert_eq!(emails["e2"].contexts, Some(json!({"private": true})));
    assert_eq!(emails["e3"].address, "general@matrix.example");
    assert_eq!(emails["e3"].contexts, None);

    // 3. Address assertions with paired labels
    let addrs = card.addresses.as_ref().expect("addresses");
    assert_eq!(addrs.len(), 3);
    assert_eq!(addrs["a1"].contexts, Some(json!({"work": true})));
    assert_eq!(
        addrs["a1"].full.as_deref(),
        Some("100 Work Blvd\r\nWork City, Work State 10001")
    );
    assert_eq!(addrs["a1"].extra.get("pref"), Some(&json!(1)));

    assert_eq!(addrs["a2"].contexts, Some(json!({"private": true})));
    assert_eq!(
        addrs["a2"].full.as_deref(),
        Some("200 Home Lane\r\nHome Town, Home State 20002")
    );

    assert_eq!(addrs["a3"].contexts, None);
    assert_eq!(
        addrs["a3"].full.as_deref(),
        Some("300 Postal Box\r\nOther City 30003")
    );
    assert_eq!(addrs["a3"].extra.get("pref"), Some(&json!(1)));

    // 4. Outbound roundtrip
    let emitted = card_to_vcard(&card);
    let card2 = vcard_to_card(&emitted).expect("re-parse bare words matrix");
    assert_eq!(card2, card);
}

#[test]
fn vcard_21_photo_formats_and_encoding_permutations() {
    let dummy_base64 = concat!(
        " /9j/4AAQSkZJRgABAQEASABIAAD/2wBDAP//////////////////////////////////////\r\n",
        " //////////////////////////////////////////////////////wgALCAABAAEBAREA\r\n",
        " /8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABPxA="
    );

    // 1. PHOTO;JPEG;ENCODING=BASE64
    let vcard_jpeg = format!(
        "BEGIN:VCARD\r\nVERSION:2.1\r\nFN:Photo JPEG\r\nPHOTO;JPEG;ENCODING=BASE64:\r\n{dummy_base64}\r\nEND:VCARD\r\n"
    );
    let card_jpeg = vcard_to_card(&vcard_jpeg).expect("parse photo jpeg");
    let photo_jpeg = &card_jpeg.media.as_ref().expect("media")["m1"];
    assert_eq!(photo_jpeg.media_type.as_deref(), Some("image/JPEG"));
    assert!(photo_jpeg.uri.starts_with("data:image/JPEG;base64,"));
    let emitted_jpeg = card_to_vcard(&card_jpeg);
    assert!(emitted_jpeg.contains("TYPE=JPEG") && emitted_jpeg.contains("ENCODING=b"));

    // 2. PHOTO;GIF;BASE64
    let vcard_gif = format!(
        "BEGIN:VCARD\r\nVERSION:2.1\r\nFN:Photo GIF\r\nPHOTO;GIF;BASE64:\r\n{dummy_base64}\r\nEND:VCARD\r\n"
    );
    let card_gif = vcard_to_card(&vcard_gif).expect("parse photo gif");
    let photo_gif = &card_gif.media.as_ref().expect("media")["m1"];
    assert_eq!(photo_gif.media_type.as_deref(), Some("image/GIF"));
    assert!(photo_gif.uri.starts_with("data:image/GIF;base64,"));
    let emitted_gif = card_to_vcard(&card_gif);
    assert!(emitted_gif.contains("TYPE=GIF") && emitted_gif.contains("ENCODING=b"));

    // 3. PHOTO;PNG;ENCODING=BASE64
    let vcard_png = format!(
        "BEGIN:VCARD\r\nVERSION:2.1\r\nFN:Photo PNG\r\nPHOTO;PNG;ENCODING=BASE64:\r\n{dummy_base64}\r\nEND:VCARD\r\n"
    );
    let card_png = vcard_to_card(&vcard_png).expect("parse photo png");
    let photo_png = &card_png.media.as_ref().expect("media")["m1"];
    assert_eq!(photo_png.media_type.as_deref(), Some("image/PNG"));
    assert!(photo_png.uri.starts_with("data:image/PNG;base64,"));
    let emitted_png = card_to_vcard(&card_png);
    assert!(emitted_png.contains("TYPE=PNG") && emitted_png.contains("ENCODING=b"));

    // 4. PHOTO;TYPE=JPEG;ENCODING=BASE64
    let vcard_type_jpeg = format!(
        "BEGIN:VCARD\r\nVERSION:2.1\r\nFN:Photo Type JPEG\r\nPHOTO;TYPE=JPEG;ENCODING=BASE64:\r\n{dummy_base64}\r\nEND:VCARD\r\n"
    );
    let card_type_jpeg = vcard_to_card(&vcard_type_jpeg).expect("parse photo type jpeg");
    let photo_type_jpeg = &card_type_jpeg.media.as_ref().expect("media")["m1"];
    assert_eq!(photo_type_jpeg.media_type.as_deref(), Some("image/JPEG"));
    let emitted_type_jpeg = card_to_vcard(&card_type_jpeg);
    assert!(emitted_type_jpeg.contains("TYPE=JPEG") && emitted_type_jpeg.contains("ENCODING=b"));
}

#[test]
fn vcard_21_quoted_printable_soft_line_breaks_and_continuation() {
    // Tests quoted-printable soft line wrapping (=\r\n) and encoded delimiter bytes (=3D, =3B, =2C, =0D=0A)
    let qp_vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:2.1\r\n",
        "N;ENCODING=QUOTED-PRINTABLE:O=27Connor;Timothy=3B Jr.;;;\r\n",
        "FN;ENCODING=QUOTED-PRINTABLE:Timothy O=27Connor=2C Jr.\r\n",
        "ORG;ENCODING=QUOTED-PRINTABLE:Acme=2C Inc.;Research =26 Development=3B Labs\r\n",
        "NOTE;ENCODING=QUOTED-PRINTABLE:This is a very long note that was exported by=\r\n",
        " an older email client using Quoted-Printable encoding and soft line breaks=\r\n",
        " to wrap text across physical lines without breaking words.=0D=0AKey=3DValue=\r\n",
        " pair; and a list: one, two, three.\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(qp_vcard).expect("parse qp soft breaks");

    let name = card.name.as_ref().expect("name");
    assert_eq!(name.full.as_deref(), Some("Timothy O'Connor, Jr."));
    let components = name.components.as_ref().expect("components");
    assert_eq!(components[0].kind, "given");
    assert_eq!(components[0].value, "Timothy; Jr.");
    assert_eq!(components[1].kind, "surname");
    assert_eq!(components[1].value, "O'Connor");

    let orgs = card.organizations.as_ref().expect("orgs");
    assert_eq!(orgs["o1"].name.as_deref(), Some("Acme, Inc."));
    let units = orgs["o1"].units.as_ref().expect("units");
    assert_eq!(units[0].name, "Research & Development; Labs");

    let notes = card.notes.as_ref().expect("notes");
    assert_eq!(
        notes["n1"].note,
        concat!(
            "This is a very long note that was exported by",
            " an older email client using Quoted-Printable encoding and soft line breaks",
            " to wrap text across physical lines without breaking words.\r\n",
            "Key=Value pair; and a list: one, two, three."
        )
    );

    // Emitting to 3.0 produces clean RFC 2426 backslash-escaped delimiters and line folding
    let emitted = card_to_vcard(&card);
    assert!(emitted.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"));
    assert!(!emitted.contains("QUOTED-PRINTABLE"));
    assert!(emitted.contains("Acme\\, Inc."));
    assert!(emitted.contains("Research & Development\\; Labs"));

    let card2 = vcard_to_card(&emitted).expect("re-parse qp emission");
    assert_eq!(card2, card);
}

#[test]
fn fixpoint_roundtrip_characterization_and_oscillation_diagnostics() {
    // Characterizes the multi-stage roundtrip contract:
    // vCard₁ (raw inbound)
    //   -> Card₁ (JSContact)
    //   -> vCard₂ (Export₁)
    //   -> Card₂ (EContact₂ / JSContact)
    //   -> vCard₃ (Export₂)
    //   -> Card₃ (EContact₃ / JSContact)
    //   -> vCard₄ (Export₃)
    //
    // Fixed-Point Stability Invariants:
    // 1. Export₂ (vCard₃) == Export₃ (vCard₄) byte-identical.
    // 2. Card₂ (EContact₂) == Card₃ (EContact₃) structurally identical.
    let complex_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:fixpoint-characterization-01\r\n",
        "X-JMAP-UID:urn:uuid:fixpoint-0001\r\n",
        "FN:Dr. Alexander Viktor von Humboldt, Jr.\r\n",
        "N:von Humboldt;Alexander;Viktor;Dr.;Jr.\r\n",
        "NICKNAME;X-JMAP-KEY=k1:Alex\r\n",
        "NICKNAME;X-JMAP-KEY=k2:Explorer\r\n",
        "EMAIL;X-JMAP-KEY=e1;TYPE=WORK,PREF:alex.work@academy.example.org\r\n",
        "EMAIL;X-JMAP-KEY=e2;TYPE=HOME:alex.home@humboldt.example\r\n",
        "EMAIL;X-JMAP-KEY=e3:alex.personal@domain.example\r\n",
        "TEL;X-JMAP-KEY=p1;TYPE=WORK,CELL,PREF:+49-30-10001\r\n",
        "TEL;X-JMAP-KEY=p2;TYPE=WORK,FAX:+49-30-10002\r\n",
        "TEL;X-JMAP-KEY=p3;TYPE=HOME,VOICE:+49-30-20001\r\n",
        "TEL;X-JMAP-KEY=p4;TYPE=PAGER:+49-30-30001\r\n",
        "ADR;X-JMAP-KEY=a1;TYPE=WORK,PREF:PO Box 100;Suite 500;Unter den Linden 6;Berlin;Berlin;10099;Germany\r\n",
        "LABEL;X-JMAP-KEY=a1;TYPE=WORK,PREF:PO Box 100\\nSuite 500\\nUnter den Linden 6\\n10099 Berlin\\nGermany\r\n",
        "ADR;X-JMAP-KEY=a2;TYPE=HOME:;;Jägerstraße 22;Berlin;Berlin;10117;Germany\r\n",
        "LABEL;X-JMAP-KEY=a2;TYPE=HOME:Jägerstraße 22\\n10117 Berlin\\nGermany\r\n",
        "ORG;X-JMAP-KEY=o1:Prussian Academy of Sciences;Natural Philosophy;Cosmology Division;Expedition Team\r\n",
        "TITLE;X-JMAP-KEY=t1:Director of Scientific Expeditions\r\n",
        "ROLE;X-JMAP-KEY=t2:Principal Investigator\r\n",
        "NOTE;X-JMAP-KEY=n1:Expedition notes & field observations:\\n1. Chimborazo barometric survey\\n2. Orinoco river mapping\r\n",
        "BDAY;X-JMAP-KEY=b1:1769-09-14\r\n",
        "X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=w1:1799-06-05\r\n",
        "URL;X-JMAP-KEY=l1:https://humboldt.example.org\r\n",
        "X-EVOLUTION-BLOG-URL;X-JMAP-KEY=l2:https://expeditions.humboldt.example.org/blog\r\n",
        "X-EVOLUTION-VIDEO-URL;X-JMAP-KEY=l3:https://archive.example.org/lectures/kosmos.mp4\r\n",
        "CALURI;X-JMAP-KEY=c1:https://calendar.humboldt.example.org/expeditions\r\n",
        "FBURL;X-JMAP-KEY=c2:https://freebusy.humboldt.example.org/schedule\r\n",
        "PHOTO;X-JMAP-KEY=m1;VALUE=uri:https://archive.example.org/portraits/humboldt.jpg\r\n",
        "CATEGORIES:Academy,Astronomy,Botany,Expedition,Geology,Science\r\n",
        "X-EVOLUTION-SPOUSE:Aimé Bonpland\r\n",
        "X-EVOLUTION-MANAGER:Johann Wolfgang von Goethe\r\n",
        "X-EVOLUTION-ASSISTANT:Carl Sigismund Kunth\r\n",
        "X-MATRIX;X-JMAP-KEY=im1;TYPE=WORK:alex:matrix.academy.example.org\r\n",
        "X-JABBER;X-JMAP-KEY=im2;TYPE=HOME:humboldt@jabber.example.org\r\n",
        // Standard unmapped properties safely dropped
        "GEO:52.5186,13.3932\r\n",
        "TZ:Europe/Berlin\r\n",
        "PRODID:-//Prussian Academy//Cosmology v1.0//EN\r\n",
        "REV:1804-08-01T12:00:00Z\r\n",
        "END:VCARD\r\n"
    );

    // Pass 1: Parse raw inbound vCard -> Card₁
    let card1 = vcard_to_card(complex_vcard).expect("pass 1 parse");

    // Pass 2: Card₁ -> Export₁ (vCard₂)
    let vcard2 = card_to_vcard(&card1);

    // Pass 3: vCard₂ -> Card₂ (EContact₂)
    let card2 = vcard_to_card(&vcard2).expect("pass 2 parse");

    // Pass 4: Card₂ -> Export₂ (vCard₃)
    let vcard3 = card_to_vcard(&card2);

    // Pass 5: vCard₃ -> Card₃ (EContact₃)
    let card3 = vcard_to_card(&vcard3).expect("pass 3 parse");

    // Pass 6: Card₃ -> Export₃ (vCard₄)
    let vcard4 = card_to_vcard(&card3);

    // Assertion 1: Export₂ == Export₃ byte-identical
    assert_eq!(
        vcard3, vcard4,
        "Export₂ and Export₃ must be byte-identical fixed-points"
    );

    // Assertion 2: Card₂ == Card₃ structurally identical
    assert_eq!(
        card2, card3,
        "EContact₂ and EContact₃ must be structurally identical fixed-points"
    );
}

#[test]
fn fixpoint_convergence_across_all_contact_property_domains_matrix() {
    // Tests fixpoint convergence (Export₂ == Export₃ byte-identical, Card₂ == Card₃)
    // across all 15 discrete property domains.
    let domain_vcards = [
        (
            "names",
            concat!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\n",
                "FN:Prof. Dr. Maria-José Carreño-Quiroga, PhD\r\n",
                "N:Carreño-Quiroga;Maria;José;Prof. Dr.;PhD\r\n",
                "END:VCARD\r\n"
            ),
        ),
        (
            "nicknames",
            concat!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\n",
                "FN:Nickname Matrix\r\n",
                "NICKNAME;X-JMAP-KEY=k1:MJ\r\n",
                "NICKNAME;X-JMAP-KEY=k2:Quiroga, The Great\r\n",
                "NICKNAME;X-JMAP-KEY=k3:🌟 Star\r\n",
                "END:VCARD\r\n"
            ),
        ),
        (
            "emails",
            concat!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\n",
                "FN:Email Matrix\r\n",
                "EMAIL;X-JMAP-KEY=e1;TYPE=WORK,PREF:primary.work@example.com\r\n",
                "EMAIL;X-JMAP-KEY=e2;TYPE=HOME:home.address@example.org\r\n",
                "EMAIL;X-JMAP-KEY=e3;TYPE=WORK:secondary.work@example.com\r\n",
                "EMAIL;X-JMAP-KEY=e4:general@example.net\r\n",
                "EMAIL;X-JMAP-KEY=e5:fifth@example.net\r\n",
                "EMAIL;X-JMAP-KEY=e6:sixth@example.net\r\n",
                "END:VCARD\r\n"
            ),
        ),
        (
            "telephony",
            concat!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\n",
                "FN:Telephony 19 Fields Matrix\r\n",
                "TEL;X-JMAP-KEY=p1;TYPE=WORK,CELL,PREF:+1-555-0101\r\n",
                "TEL;X-JMAP-KEY=p2;TYPE=WORK,FAX:+1-555-0102\r\n",
                "TEL;X-JMAP-KEY=p3;TYPE=WORK,VOICE:+1-555-0103\r\n",
                "TEL;X-JMAP-KEY=p4;TYPE=HOME,CELL:+1-555-0104\r\n",
                "TEL;X-JMAP-KEY=p5;TYPE=HOME,FAX:+1-555-0105\r\n",
                "TEL;X-JMAP-KEY=p6;TYPE=HOME,VOICE:+1-555-0106\r\n",
                "TEL;X-JMAP-KEY=p7;TYPE=PAGER:+1-555-0107\r\n",
                "TEL;X-JMAP-KEY=p8;TYPE=VIDEO:+1-555-0108\r\n",
                "TEL;X-JMAP-KEY=p9;TYPE=CAR:+1-555-0109\r\n",
                "TEL;X-JMAP-KEY=p10;TYPE=ISDN:+1-555-0110\r\n",
                "TEL;X-JMAP-KEY=p11;TYPE=TTYTDD:+1-555-0111\r\n",
                "END:VCARD\r\n"
            ),
        ),
        (
            "addresses_and_labels",
            concat!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\n",
                "FN:Address Matrix\r\n",
                "ADR;X-JMAP-KEY=a1;TYPE=WORK,PREF:PO Box 42;Suite 100;100 Market St;San Francisco;CA;94105;USA\r\n",
                "LABEL;X-JMAP-KEY=a1;TYPE=WORK,PREF:PO Box 42\\nSuite 100\\n100 Market St\\nSan Francisco\\, CA 94105\\nUSA\r\n",
                "ADR;X-JMAP-KEY=a2;TYPE=HOME:;;742 Evergreen Terrace;Springfield;OR;97477;USA\r\n",
                "LABEL;X-JMAP-KEY=a2;TYPE=HOME:742 Evergreen Terrace\\nSpringfield\\, OR 97477\\nUSA\r\n",
                "ADR;X-JMAP-KEY=a3;TYPE=OTHER:;;Postlagernd;Berlin;;10115;Germany\r\n",
                "LABEL;X-JMAP-KEY=a3;TYPE=OTHER:Postlagernd\\n10115 Berlin\\nGermany\r\n",
                "END:VCARD\r\n"
            ),
        ),
        (
            "organizations",
            concat!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\n",
                "FN:Org Matrix\r\n",
                "ORG;X-JMAP-KEY=o1:Enterprise Corp;Cloud Division;Storage Group;Flash Core Team\r\n",
                "ORG;X-JMAP-KEY=o2:;Freelance Consulting;Remote Office\r\n",
                "END:VCARD\r\n"
            ),
        ),
        (
            "titles_and_roles",
            concat!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\n",
                "FN:Titles Matrix\r\n",
                "TITLE;X-JMAP-KEY=t1:Chief Technology Officer\r\n",
                "TITLE;X-JMAP-KEY=t2:Vice President of Engineering\r\n",
                "ROLE;X-JMAP-KEY=t3:Executive Committee Chair\r\n",
                "ROLE;X-JMAP-KEY=t4:Open Standards Representative\r\n",
                "END:VCARD\r\n"
            ),
        ),
        (
            "notes_escaping",
            concat!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\n",
                "FN:Note Escaping Matrix\r\n",
                "NOTE;X-JMAP-KEY=n1:Line 1\\nLine 2\\r\\nWith \\; and \\, and \\\\ backslashes.\\n∀x ∈ ℝ: x² ≥ 0\r\n",
                "NOTE;X-JMAP-KEY=n2:Second note with emojis 🚀 🌟 🌍\r\n",
                "END:VCARD\r\n"
            ),
        ),
        (
            "anniversaries",
            concat!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\n",
                "FN:Anniversaries Matrix\r\n",
                "BDAY;X-JMAP-KEY=b1:1980-05-20\r\n",
                "X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=w1:2010-09-15\r\n",
                "END:VCARD\r\n"
            ),
        ),
        (
            "links_blogs_videos",
            concat!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\n",
                "FN:Links Matrix\r\n",
                "URL;X-JMAP-KEY=l1:https://example.com/profile?id=123&sort=asc;view=full\r\n",
                "X-EVOLUTION-BLOG-URL;X-JMAP-KEY=l2:https://blog.example.com/tech\r\n",
                "X-EVOLUTION-VIDEO-URL;X-JMAP-KEY=l3:https://video.example.com/channel/live\r\n",
                "END:VCARD\r\n"
            ),
        ),
        (
            "calendars",
            concat!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\n",
                "FN:Calendar Matrix\r\n",
                "CALURI;X-JMAP-KEY=c1:https://cal.example.org/user/calendar.ics\r\n",
                "FBURL;X-JMAP-KEY=c2:https://cal.example.org/freebusy/user.vfb\r\n",
                "END:VCARD\r\n"
            ),
        ),
        (
            "photos",
            concat!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\n",
                "FN:Photos Matrix\r\n",
                "PHOTO;X-JMAP-KEY=m1;VALUE=uri:https://example.com/avatar.png\r\n",
                "END:VCARD\r\n"
            ),
        ),
        (
            "online_services",
            concat!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\n",
                "FN:Online Services Matrix\r\n",
                "X-AIM;X-JMAP-KEY=im1;TYPE=WORK:screenname1\r\n",
                "X-GADUGADU;X-JMAP-KEY=im2;TYPE=HOME:123456\r\n",
                "X-GOOGLE-TALK;X-JMAP-KEY=im3;TYPE=WORK:user@gmail.com\r\n",
                "X-GROUPWISE;X-JMAP-KEY=im4;TYPE=WORK:gwuser\r\n",
                "X-ICQ;X-JMAP-KEY=im5;TYPE=HOME:98765432\r\n",
                "X-JABBER;X-JMAP-KEY=im6;TYPE=WORK:user@jabber.org\r\n",
                "X-MSN;X-JMAP-KEY=im7;TYPE=HOME:user@hotmail.com\r\n",
                "X-MATRIX;X-JMAP-KEY=im8;TYPE=WORK:user:matrix.org\r\n",
                "X-SKYPE;X-JMAP-KEY=im9;TYPE=WORK:skype.handle\r\n",
                "X-YAHOO;X-JMAP-KEY=im10;TYPE=HOME:yahoo_user\r\n",
                "END:VCARD\r\n"
            ),
        ),
        (
            "relations",
            concat!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\n",
                "FN:Relations Matrix\r\n",
                "X-EVOLUTION-SPOUSE:Maria Carreño\r\n",
                "X-EVOLUTION-MANAGER:Chief Officer Smith\r\n",
                "X-EVOLUTION-ASSISTANT:Assistant Johnson\r\n",
                "END:VCARD\r\n"
            ),
        ),
        (
            "categories_keywords",
            concat!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\n",
                "FN:Categories Matrix\r\n",
                "CATEGORIES:Architecture,Engineering,OpenSource,Standards,VIP\r\n",
                "END:VCARD\r\n"
            ),
        ),
    ];

    for (domain, vcard_input) in domain_vcards {
        let card1 = vcard_to_card(vcard_input)
            .unwrap_or_else(|e| panic!("domain {domain} failed initial parse: {e}"));
        let vcard2 = card_to_vcard(&card1);
        let card2 = vcard_to_card(&vcard2)
            .unwrap_or_else(|e| panic!("domain {domain} failed second parse: {e}"));
        let vcard3 = card_to_vcard(&card2);
        let card3 = vcard_to_card(&vcard3)
            .unwrap_or_else(|e| panic!("domain {domain} failed third parse: {e}"));
        let vcard4 = card_to_vcard(&card3);

        assert_eq!(
            vcard3, vcard4,
            "Domain '{domain}' failed vCard fixpoint stability (Export₂ != Export₃)"
        );
        assert_eq!(
            card2, card3,
            "Domain '{domain}' failed JSContact fixpoint stability (Card₂ != Card₃)"
        );
    }
}

#[test]
fn trailing_whitespace_filed_bug_minimal_input_named_regression() {
    // Minimal reproduction input recorded in docs/BACKLOG.md:
    // "BEGIN:VCARD\r\nVERSION:3.0\r\nNICKNAME;ENCODING=b:! \r\nEND:VCARD\r\n"
    let input = "BEGIN:VCARD\r\nVERSION:3.0\r\nNICKNAME;ENCODING=b:! \r\nEND:VCARD\r\n";

    let card1 = vcard_to_card(input).expect("initial parse must succeed");
    let vcard1 = card_to_vcard(&card1);

    let card2 = vcard_to_card(&vcard1).expect("second parse must succeed");
    let vcard2 = card_to_vcard(&card2);

    let card3 = vcard_to_card(&vcard2).expect("third parse must succeed");
    let vcard3 = card_to_vcard(&card3);

    assert_eq!(
        vcard2, vcard3,
        "vCard fixpoint failure for minimal BACKLOG input (Export₂ != Export₃):\nExport₂:\n{vcard2}\nExport₃:\n{vcard3}"
    );
    assert_eq!(
        card2, card3,
        "JSContact fixpoint failure for minimal BACKLOG input (Card₂ != Card₃):\nCard₂: {card2:?}\nCard₃: {card3:?}"
    );
}

#[test]
fn trailing_whitespace_on_property_values_across_all_domains_fixpoint() {
    let input = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Test Contact \r\n",
        "N:Family ;Given ;Additional ;Prefix ;Suffix \r\n",
        "NICKNAME:Nick \r\n",
        "EMAIL;TYPE=WORK,PREF:user@example.com \r\n",
        "TEL;TYPE=WORK,VOICE:555-1234 \r\n",
        "ADR;TYPE=WORK:;;123 Main St ;Anytown ;State ;12345 ;Country \r\n",
        "LABEL;TYPE=WORK:123 Main St \\nAnytown \\nState 12345 \r\n",
        "ORG:Acme Corp ;Engineering ;Platform ;Floor 3 \r\n",
        "TITLE:Senior Architect \r\n",
        "ROLE:Lead Engineer \r\n",
        "NOTE:Important note text \\nWith second line \r\n",
        "URL:https://example.com/contact \r\n",
        "CATEGORIES:Architecture,Engineering\r\n",
        "X-EVOLUTION-SPOUSE:Maria Carreño \r\n",
        "X-EVOLUTION-MANAGER:Chief Manager \r\n",
        "X-EVOLUTION-ASSISTANT:Senior Assistant \r\n",
        "X-EVOLUTION-BLOG-URL:https://blog.example.com \r\n",
        "X-EVOLUTION-VIDEO-URL:https://video.example.com \r\n",
        "END:VCARD\r\n"
    );

    let card1 = vcard_to_card(input).expect("initial parse must succeed");
    let vcard1 = card_to_vcard(&card1);

    let card2 = vcard_to_card(&vcard1).expect("second parse must succeed");
    let vcard2 = card_to_vcard(&card2);

    let card3 = vcard_to_card(&vcard2).expect("third parse must succeed");
    let vcard3 = card_to_vcard(&card3);

    assert_eq!(
        vcard2, vcard3,
        "vCard fixpoint failure for multi-domain trailing whitespace (Export₂ != Export₃):\nExport₂:\n{vcard2}\nExport₃:\n{vcard3}"
    );
    assert_eq!(
        card2, card3,
        "JSContact fixpoint failure for multi-domain trailing whitespace (Card₂ != Card₃)"
    );
}

#[test]
fn trailing_whitespace_only_property_values_fixpoint_and_filtering() {
    let input = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN: \r\n",
        "N: ; ; ; ; \r\n",
        "NICKNAME: \r\n",
        "EMAIL: \r\n",
        "TEL: \r\n",
        "ADR:;; ; ; ; ; \r\n",
        "LABEL: \r\n",
        "ORG: ; ; \r\n",
        "TITLE: \r\n",
        "ROLE: \r\n",
        "NOTE: \r\n",
        "CATEGORIES: \r\n",
        "END:VCARD\r\n"
    );

    let card1 = vcard_to_card(input).expect("initial parse of whitespace-only values must succeed");
    let vcard1 = card_to_vcard(&card1);

    let card2 = vcard_to_card(&vcard1).expect("second parse must succeed");
    let vcard2 = card_to_vcard(&card2);

    let card3 = vcard_to_card(&vcard2).expect("third parse must succeed");
    let vcard3 = card_to_vcard(&card3);

    assert_eq!(
        vcard2, vcard3,
        "vCard fixpoint failure for whitespace-only values (Export₂ != Export₃):\nExport₂:\n{vcard2}\nExport₃:\n{vcard3}"
    );
    assert_eq!(
        card2, card3,
        "JSContact fixpoint failure for whitespace-only values (Card₂ != Card₃)"
    );
}

#[test]
fn trailing_whitespace_with_vcard_21_and_quoted_printable_fixpoint() {
    let input = concat!(
        "BEGIN:VCARD\r\nVERSION:2.1\r\n",
        "FN:Legacy Contact \r\n",
        "TEL;WORK;VOICE:555-0100 \r\n",
        "EMAIL;INTERNET:legacy@example.com \r\n",
        "NOTE;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:Line 1=20 =\r\nLine 2 \r\n",
        "END:VCARD\r\n"
    );

    let card1 = vcard_to_card(input)
        .expect("initial parse of vCard 2.1 with trailing whitespace must succeed");
    let vcard1 = card_to_vcard(&card1);

    let card2 = vcard_to_card(&vcard1).expect("second parse must succeed");
    let vcard2 = card_to_vcard(&card2);

    let card3 = vcard_to_card(&vcard2).expect("third parse must succeed");
    let vcard3 = card_to_vcard(&card3);

    assert_eq!(
        vcard2, vcard3,
        "vCard fixpoint failure for vCard 2.1 QP with trailing whitespace (Export₂ != Export₃):\nExport₂:\n{vcard2}\nExport₃:\n{vcard3}"
    );
    assert_eq!(
        card2, card3,
        "JSContact fixpoint failure for vCard 2.1 QP with trailing whitespace (Card₂ != Card₃)"
    );
}

#[test]
fn name_with_empty_full_string_and_components_reaches_fixed_point() {
    let card = ContactCard {
        name: Some(Name {
            components: Some(vec![NameComponent {
                kind: "given".to_string(),
                value: "TestGiven".to_string(),
                extra: BTreeMap::new(),
            }]),
            full: Some("".to_string()),
            extra: BTreeMap::new(),
        }),
        ..Default::default()
    };

    let vcard1 = card_to_vcard(&card);
    let card2 = vcard_to_card(&vcard1).expect("second parse must succeed");
    let vcard2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&vcard2).expect("third parse must succeed");
    let vcard3 = card_to_vcard(&card3);

    assert_eq!(
        vcard2, vcard3,
        "vCard fixpoint failure for empty full name with components (Export₂ != Export₃):\nExport₂:\n{vcard2}\nExport₃:\n{vcard3}"
    );
    assert_eq!(
        card2, card3,
        "JSContact fixpoint failure for empty full name with components (Card₂ != Card₃)"
    );
}

#[test]
fn file_as_basic_evolution_x_property_roundtrip() {
    let input = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:file-as-001\r\n",
        "FN:John Doe\r\n",
        "N:Doe;John;;;\r\n",
        "X-EVOLUTION-FILE-AS:Doe\\, John (Personal)\r\n",
        "EMAIL;X-JMAP-KEY=e1:john@example.com\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(input).expect("parse vcard with file-as");
    let name = card.name.as_ref().expect("name object present");
    assert_eq!(name.full, Some("John Doe".to_string()));
    assert_eq!(
        name.extra.get("fileAs"),
        Some(&json!("Doe, John (Personal)"))
    );
    assert!(states_file_as(card.name.as_ref()));

    let emitted = card_to_vcard(&card);
    assert!(
        emitted.contains("X-EVOLUTION-FILE-AS:Doe\\, John (Personal)"),
        "emitted vCard must contain X-EVOLUTION-FILE-AS: {emitted}"
    );

    let card2 = vcard_to_card(&emitted).expect("second parse");
    let emitted2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&emitted2).expect("third parse");
    let emitted3 = card_to_vcard(&card3);

    assert_eq!(emitted, emitted2, "Fixpoint convergence Export₁ == Export₂");
    assert_eq!(
        emitted2, emitted3,
        "Fixpoint convergence Export₂ == Export₃"
    );
    assert_eq!(card2, card3, "Fixpoint convergence Card₂ == Card₃");
}

#[test]
fn file_as_inbound_synonyms_file_as_and_x_file_as() {
    // 1. FILE-AS (vCard 4.0 / RFC 6350 / alternative extension)
    let vcard1 = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:file-as-syn-001\r\n",
        "FN:Alice Smith\r\n",
        "FILE-AS:Smith\\, Alice\r\n",
        "END:VCARD\r\n"
    );
    let card1 = vcard_to_card(vcard1).expect("parse FILE-AS");
    assert_eq!(
        card1.name.as_ref().unwrap().extra.get("fileAs"),
        Some(&json!("Smith, Alice"))
    );
    let emitted1 = card_to_vcard(&card1);
    assert!(
        emitted1.contains("X-EVOLUTION-FILE-AS:Smith\\, Alice"),
        "Outbound normalizes to X-EVOLUTION-FILE-AS: {emitted1}"
    );

    // 2. X-FILE-AS (Outlook / generic extension)
    let vcard2 = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:file-as-syn-002\r\n",
        "FN:Bob Jones\r\n",
        "X-FILE-AS:Jones\\, Bob (Work)\r\n",
        "END:VCARD\r\n"
    );
    let card2 = vcard_to_card(vcard2).expect("parse X-FILE-AS");
    assert_eq!(
        card2.name.as_ref().unwrap().extra.get("fileAs"),
        Some(&json!("Jones, Bob (Work)"))
    );
    let emitted2 = card_to_vcard(&card2);
    assert!(
        emitted2.contains("X-EVOLUTION-FILE-AS:Jones\\, Bob (Work)"),
        "Outbound normalizes to X-EVOLUTION-FILE-AS: {emitted2}"
    );

    // 3. Case-insensitivity: lowercase and mixed-case property names
    let vcard3 = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:file-as-syn-003\r\n",
        "FN:Charlie Brown\r\n",
        "x-evolution-file-as:Brown\\, Charlie\r\n",
        "END:VCARD\r\n"
    );
    let card3 = vcard_to_card(vcard3).expect("parse lowercase x-evolution-file-as");
    assert_eq!(
        card3.name.as_ref().unwrap().extra.get("fileAs"),
        Some(&json!("Brown, Charlie"))
    );
    let emitted3 = card_to_vcard(&card3);
    assert!(emitted3.contains("X-EVOLUTION-FILE-AS:Brown\\, Charlie"));
}

#[test]
fn file_as_and_sort_string_coexistence_without_clobbering() {
    // Both X-EVOLUTION-FILE-AS and SORT-STRING present in input:
    // X-EVOLUTION-FILE-AS maps to name.extra["fileAs"], while SORT-STRING is dropped
    // from vCard 3.0 output and does NOT clobber fileAs.
    let input = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:file-as-sort-001\r\n",
        "FN:Albert Einstein\r\n",
        "N:Einstein;Albert;;;\r\n",
        "SORT-STRING:Einstein\r\n",
        "X-EVOLUTION-FILE-AS:Einstein\\, Prof. Albert (IAS)\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(input).expect("parse vcard with file-as and sort-string");
    let name = card.name.as_ref().expect("name object present");
    assert_eq!(
        name.extra.get("fileAs"),
        Some(&json!("Einstein, Prof. Albert (IAS)"))
    );

    let emitted = card_to_vcard(&card);
    assert!(
        emitted.contains("X-EVOLUTION-FILE-AS:Einstein\\, Prof. Albert (IAS)"),
        "emitted vCard must contain X-EVOLUTION-FILE-AS: {emitted}"
    );
    assert!(
        !emitted.contains("SORT-STRING"),
        "emitted vCard must NOT contain SORT-STRING: {emitted}"
    );

    // If JSContact Name contains both sortAs and fileAs in extra, both survive in memory
    let mut extra = BTreeMap::new();
    extra.insert("fileAs".to_string(), json!("Einstein, Albert"));
    extra.insert("sortAs".to_string(), json!("Einstein"));
    let card_dual = ContactCard {
        uid: Some("dual-sort-file-01".to_string()),
        name: Some(Name {
            full: Some("Albert Einstein".to_string()),
            components: None,
            extra,
        }),
        ..Default::default()
    };

    let emitted_dual = card_to_vcard(&card_dual);
    assert!(emitted_dual.contains("X-EVOLUTION-FILE-AS:Einstein\\, Albert"));
    assert!(!emitted_dual.contains("SORT-STRING"));

    let card_reparsed = vcard_to_card(&emitted_dual).expect("re-parse dual");
    assert_eq!(
        card_reparsed.name.as_ref().unwrap().extra.get("fileAs"),
        Some(&json!("Einstein, Albert"))
    );
}

#[test]
fn file_as_escaping_special_characters_and_unicode() {
    let input = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:file-as-esc-001\r\n",
        "FN:Hans Müller\r\n",
        "X-EVOLUTION-FILE-AS:Müller\\, Dr. Hans \\; (Büro / Zürich)\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(input).expect("parse escaped file-as");
    assert_eq!(
        card.name.as_ref().unwrap().extra.get("fileAs"),
        Some(&json!("Müller, Dr. Hans ; (Büro / Zürich)"))
    );

    let emitted = card_to_vcard(&card);
    assert!(
        emitted.contains("X-EVOLUTION-FILE-AS:Müller\\, Dr. Hans \\; (Büro / Zürich)"),
        "escaped special characters preserved: {emitted}"
    );

    let card2 = vcard_to_card(&emitted).expect("second parse");
    assert_eq!(card, card2);
}

#[test]
fn file_as_card_level_and_name_level_emission() {
    // 1. file_as in card.name.extra with "file_as" underscore key
    let card1 = ContactCard {
        uid: Some("fa-001".to_string()),
        name: Some(Name {
            full: Some("John Doe".to_string()),
            components: None,
            extra: {
                let mut m = BTreeMap::new();
                m.insert("file_as".to_string(), json!("Doe, John (Name Extra)"));
                m
            },
        }),
        ..Default::default()
    };
    let emitted1 = card_to_vcard(&card1);
    assert!(emitted1.contains("X-EVOLUTION-FILE-AS:Doe\\, John (Name Extra)"));
    assert!(states_file_as(card1.name.as_ref()));

    // 2. fileAs in card.extra
    let card2 = ContactCard {
        uid: Some("fa-002".to_string()),
        name: None,
        extra: {
            let mut m = BTreeMap::new();
            m.insert("fileAs".to_string(), json!("Doe, John (Card Extra)"));
            m
        },
        ..Default::default()
    };
    let emitted2 = card_to_vcard(&card2);
    assert!(emitted2.contains("X-EVOLUTION-FILE-AS:Doe\\, John (Card Extra)"));

    // 3. Empty string / whitespace-only fileAs is filtered and states_file_as is false
    let card3 = ContactCard {
        uid: Some("fa-003".to_string()),
        name: Some(Name {
            full: Some("John Doe".to_string()),
            components: None,
            extra: {
                let mut m = BTreeMap::new();
                m.insert("fileAs".to_string(), json!("   "));
                m
            },
        }),
        ..Default::default()
    };
    let emitted3 = card_to_vcard(&card3);
    assert!(!emitted3.contains("X-EVOLUTION-FILE-AS"));
    assert!(!states_file_as(card3.name.as_ref()));
    assert!(!states_file_as(None));

    // 4. Card with ONLY X-EVOLUTION-FILE-AS (no FN or N)
    let vcard4 = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:fa-only-004\r\n",
        "X-EVOLUTION-FILE-AS:Anonymous Entity\r\n",
        "END:VCARD\r\n"
    );
    let card4 = vcard_to_card(vcard4).expect("parse file-as only vcard");
    assert_eq!(
        card4.name.as_ref().unwrap().extra.get("fileAs"),
        Some(&json!("Anonymous Entity"))
    );
    let emitted4 = card_to_vcard(&card4);
    assert!(emitted4.contains("X-EVOLUTION-FILE-AS:Anonymous Entity"));
}

#[test]
fn apple_property_groups_representative_icloud_fixture_import_and_roundtrip() {
    // Representative iCloud / macOS Contacts exported vCard 3.0 with Apple-style property groups
    // and `X-ABLabel` parameter annotations (`itemN.PROPERTY` + `itemN.X-ABLabel:_$!<Label>!$_`).
    let icloud_vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "PRODID:-//Apple Inc.//macOS 14.5//EN\r\n",
        "N:Appleseed;John;M.;Mr.;Esq.\r\n",
        "FN:John M. Appleseed\r\n",
        "NICKNAME:Johnny\r\n",
        "item1.EMAIL;type=INTERNET;type=pref:john.appleseed@work.example.com\r\n",
        "item1.X-ABLabel:_$!<Work>!$_\r\n",
        "item2.EMAIL;type=INTERNET:john.appleseed@home.example.org\r\n",
        "item2.X-ABLabel:_$!<Home>!$_\r\n",
        "item3.TEL;type=pref:(555) 555-0100\r\n",
        "item3.X-ABLabel:_$!<Mobile>!$_\r\n",
        "item4.TEL:(555) 555-0200\r\n",
        "item4.X-ABLabel:_$!<Work>!$_\r\n",
        "item5.TEL:(555) 555-0300\r\n",
        "item5.X-ABLabel:_$!<HomeFAX>!$_\r\n",
        "item6.TEL:(555) 555-0400\r\n",
        "item6.X-ABLabel:_$!<Main>!$_\r\n",
        "item7.ADR;type=pref:;;1 Infinite Loop;Cupertino;CA;95014;USA\r\n",
        "item7.X-ABLabel:_$!<Work>!$_\r\n",
        "item8.ADR:;;123 Homestead Ave;Sunnyvale;CA;94086;USA\r\n",
        "item8.X-ABLabel:_$!<Home>!$_\r\n",
        "item9.URL:https://www.apple.com\r\n",
        "item9.X-ABLabel:_$!<HomePage>!$_\r\n",
        "item10.X-ABRELATEDNAMES;type=pref:Jane Appleseed\r\n",
        "item10.X-ABLabel:_$!<Spouse>!$_\r\n",
        "item11.X-ABRELATEDNAMES:Bob Smith\r\n",
        "item11.X-ABLabel:_$!<Manager>!$_\r\n",
        "item12.X-ABRELATEDNAMES:Alice Assistant\r\n",
        "item12.X-ABLabel:_$!<Assistant>!$_\r\n",
        "item13.X-ABDATE:2018-06-20\r\n",
        "item13.X-ABLabel:_$!<Anniversary>!$_\r\n",
        "NOTE:Representative Apple Contact export.\\nMulti-line note.\r\n",
        "CATEGORIES:VIP,Family\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(icloud_vcard).expect("parse iCloud vcard");

    // Verify Name & Nickname
    assert_eq!(
        card.name.as_ref().unwrap().full.as_deref(),
        Some("John M. Appleseed")
    );
    assert_eq!(
        card.nicknames
            .as_ref()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .name,
        "Johnny"
    );

    // Verify Emails & X-ABLabel mapped contexts
    let emails = card.emails.as_ref().expect("emails");
    assert_eq!(emails.len(), 2);
    let work_email = emails
        .values()
        .find(|e| e.address.contains("work"))
        .expect("work email");
    assert_eq!(work_email.contexts.as_ref(), Some(&json!({"work": true})));
    assert_eq!(work_email.pref, Some(1));
    let home_email = emails
        .values()
        .find(|e| e.address.contains("home"))
        .expect("home email");
    assert_eq!(
        home_email.contexts.as_ref(),
        Some(&json!({"private": true}))
    );

    // Verify Telephones & X-ABLabel mapped features / contexts
    let phones = card.phones.as_ref().expect("phones");
    assert_eq!(phones.len(), 4);
    let mobile = phones
        .values()
        .find(|p| p.number.contains("0100"))
        .expect("mobile phone");
    assert_eq!(mobile.features.as_ref(), Some(&json!({"mobile": true})));
    assert_eq!(mobile.pref, Some(1));

    let work_phone = phones
        .values()
        .find(|p| p.number.contains("0200"))
        .expect("work phone");
    assert_eq!(work_phone.contexts.as_ref(), Some(&json!({"work": true})));

    let fax_phone = phones
        .values()
        .find(|p| p.number.contains("0300"))
        .expect("fax phone");
    assert_eq!(fax_phone.features.as_ref(), Some(&json!({"fax": true})));
    assert_eq!(fax_phone.contexts.as_ref(), Some(&json!({"private": true})));

    let main_phone = phones
        .values()
        .find(|p| p.number.contains("0400"))
        .expect("main phone");
    assert_eq!(main_phone.features.as_ref(), Some(&json!({"voice": true})));
    assert_eq!(main_phone.contexts.as_ref(), Some(&json!({"work": true})));

    // Verify Addresses & X-ABLabel mapped contexts
    let addresses = card.addresses.as_ref().expect("addresses");
    assert_eq!(addresses.len(), 2);
    let work_adr = addresses
        .values()
        .find(|a| {
            a.components
                .as_ref()
                .unwrap()
                .iter()
                .any(|c| c.value.contains("Infinite Loop"))
        })
        .expect("work adr");
    assert_eq!(work_adr.contexts.as_ref(), Some(&json!({"work": true})));
    assert_eq!(work_adr.extra.get("pref"), Some(&json!(1)));

    let home_adr = addresses
        .values()
        .find(|a| {
            a.components
                .as_ref()
                .unwrap()
                .iter()
                .any(|c| c.value.contains("Homestead"))
        })
        .expect("home adr");
    assert_eq!(home_adr.contexts.as_ref(), Some(&json!({"private": true})));

    // Verify Links
    let links = card.links.as_ref().expect("links");
    assert_eq!(links.len(), 1);
    let link = links.values().next().unwrap();
    assert_eq!(link.uri, "https://www.apple.com");
    assert_eq!(link.kind, None);

    // Verify Relations (X-ABRELATEDNAMES)
    let related = card.related_to.as_ref().expect("related_to");
    assert_eq!(related.len(), 3);
    assert_eq!(
        related
            .get("Jane Appleseed")
            .and_then(|r| r.relation.as_ref()),
        Some(
            &[("spouse".to_string(), Value::Bool(true))]
                .into_iter()
                .collect()
        )
    );
    assert_eq!(
        related.get("Bob Smith").and_then(|r| r.relation.as_ref()),
        Some(
            &[("manager".to_string(), Value::Bool(true))]
                .into_iter()
                .collect()
        )
    );
    assert_eq!(
        related
            .get("Alice Assistant")
            .and_then(|r| r.relation.as_ref()),
        Some(
            &[("assistant".to_string(), Value::Bool(true))]
                .into_iter()
                .collect()
        )
    );

    // Verify Anniversaries (X-ABDATE)
    let anniversaries = card.anniversaries.as_ref().expect("anniversaries");
    assert_eq!(anniversaries.len(), 1);
    let ann = anniversaries.values().next().unwrap();
    assert_eq!(ann.kind, "wedding");
    assert_eq!(
        ann.date.as_ref(),
        Some(&json!({"@type": "PartialDate", "year": 2018, "month": 6, "day": 20}))
    );

    // Outbound vCard 3.0 emission check
    let emitted = card_to_vcard(&card);
    assert!(emitted.contains("FN:John M. Appleseed"));
    assert!(
        emitted.contains("TEL;X-JMAP-KEY=p1;TYPE=CELL,PREF:(555) 555-0100")
            || emitted.contains("TEL;TYPE=CELL,PREF;X-JMAP-KEY=p1:(555) 555-0100")
            || emitted.contains("TEL;X-JMAP-KEY=p1;TYPE=PREF,CELL:(555) 555-0100")
    );
    assert!(
        emitted.contains("EMAIL;X-JMAP-KEY=e1;TYPE=WORK,PREF:john.appleseed@work.example.com")
            || emitted
                .contains("EMAIL;TYPE=WORK,PREF;X-JMAP-KEY=e1:john.appleseed@work.example.com")
            || emitted
                .contains("EMAIL;X-JMAP-KEY=e1;TYPE=PREF,WORK:john.appleseed@work.example.com")
    );
    assert!(
        emitted
            .contains("ADR;X-JMAP-KEY=a1;TYPE=WORK,PREF:;;1 Infinite Loop;Cupertino;CA;95014;USA")
            || emitted.contains(
                "ADR;TYPE=WORK,PREF;X-JMAP-KEY=a1:;;1 Infinite Loop;Cupertino;CA;95014;USA"
            )
            || emitted.contains(
                "ADR;X-JMAP-KEY=a1;TYPE=PREF,WORK:;;1 Infinite Loop;Cupertino;CA;95014;USA"
            )
    );
    assert!(emitted.contains("X-EVOLUTION-SPOUSE:Jane Appleseed"));
    assert!(emitted.contains("X-EVOLUTION-MANAGER:Bob Smith"));
    assert!(emitted.contains("X-EVOLUTION-ASSISTANT:Alice Assistant"));
    assert!(
        emitted.contains("X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=y1:2018-06-20")
            || emitted.contains("X-EVOLUTION-ANNIVERSARY:2018-06-20")
    );

    // Multi-pass round-trip fixpoint verification
    let card2 = vcard_to_card(&emitted).expect("second parse");
    let emitted2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&emitted2).expect("third parse");
    let emitted3 = card_to_vcard(&card3);

    assert_eq!(
        emitted2, emitted3,
        "Export₂ must be byte-identical to Export₃"
    );
    assert_eq!(
        card2, card3,
        "Card₂ must be structurally identical to Card₃"
    );
}

#[test]
fn apple_property_groups_custom_labels_and_extended_relations() {
    let input = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN:Custom Labels Contact\r\n",
        "item1.TEL:+1-555-0999\r\n",
        "item1.X-ABLabel:Direct Line\r\n",
        "item2.TEL:+1-555-0888\r\n",
        "item2.X-ABLabel:_$!<WorkFAX>!$_\r\n",
        "item3.TEL:+1-555-0777\r\n",
        "item3.X-ABLabel:_$!<Pager>!$_\r\n",
        "item4.EMAIL:emergency@example.com\r\n",
        "item4.X-ABLabel:Emergency\r\n",
        "item5.URL:https://custom.example.org\r\n",
        "item5.X-ABLabel:Personal Portfolio\r\n",
        "item6.X-ABRELATEDNAMES:Charlie Partner\r\n",
        "item6.X-ABLabel:_$!<Partner>!$_\r\n",
        "item7.X-ABRELATEDNAMES:Dan Colleague\r\n",
        "item7.X-ABLabel:Colleague\r\n",
        "item8.X-ABDATE:2015-11-28\r\n",
        "item8.X-ABLabel:First Met\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(input).expect("parse custom labels");

    // TEL with custom label
    let phones = card.phones.as_ref().unwrap();
    let direct_phone = phones.values().find(|p| p.number == "+1-555-0999").unwrap();
    assert_eq!(direct_phone.extra.get("label"), Some(&json!("Direct Line")));

    // TEL with WorkFAX
    let workfax_phone = phones.values().find(|p| p.number == "+1-555-0888").unwrap();
    assert_eq!(workfax_phone.features.as_ref(), Some(&json!({"fax": true})));
    assert_eq!(
        workfax_phone.contexts.as_ref(),
        Some(&json!({"work": true}))
    );

    // TEL with Pager
    let pager_phone = phones.values().find(|p| p.number == "+1-555-0777").unwrap();
    assert_eq!(pager_phone.features.as_ref(), Some(&json!({"pager": true})));

    // EMAIL with custom label
    let email = card.emails.as_ref().unwrap().values().next().unwrap();
    assert_eq!(email.extra.get("label"), Some(&json!("Emergency")));

    // URL with custom label
    let link = card.links.as_ref().unwrap().values().next().unwrap();
    assert_eq!(link.extra.get("label"), Some(&json!("Personal Portfolio")));

    // Extended relations
    let related = card.related_to.as_ref().unwrap();
    assert_eq!(
        related
            .get("Charlie Partner")
            .and_then(|r| r.relation.as_ref()),
        Some(
            &[("spouse".to_string(), Value::Bool(true))]
                .into_iter()
                .collect()
        )
    );
    assert_eq!(
        related
            .get("Dan Colleague")
            .and_then(|r| r.relation.as_ref()),
        Some(
            &[("colleague".to_string(), Value::Bool(true))]
                .into_iter()
                .collect()
        )
    );

    // Custom date anniversary
    let ann = card
        .anniversaries
        .as_ref()
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(ann.kind, "first met");
    assert_eq!(
        ann.date.as_ref(),
        Some(&json!({"@type": "PartialDate", "year": 2015, "month": 11, "day": 28}))
    );
}

#[test]
fn apple_property_groups_variations_and_boundary_cases() {
    // 1. Case insensitivity in X-ABLabel and property names
    let vcard1 = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Case Test\r\n",
        "itemA.tel:+1-555-1111\r\n",
        "itemA.x-ablabel:_$!<mobile>!$_\r\n",
        "itemB.email:test@example.com\r\n",
        "itemB.X-ABLABEL:work\r\n",
        "itemC.adr:;;123 Test St;City;ST;12345;USA\r\n",
        "itemC.x-AbLabel:_$!<Home>!$_\r\n",
        "END:VCARD\r\n"
    );
    let card1 = vcard_to_card(vcard1).expect("parse case test");
    assert_eq!(
        card1
            .phones
            .as_ref()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .features
            .as_ref(),
        Some(&json!({"mobile": true}))
    );
    assert_eq!(
        card1
            .emails
            .as_ref()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .contexts
            .as_ref(),
        Some(&json!({"work": true}))
    );
    assert_eq!(
        card1
            .addresses
            .as_ref()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .contexts
            .as_ref(),
        Some(&json!({"private": true}))
    );

    // 2. Orphaned X-ABLabel and properties without labels
    let vcard2 = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Orphan Test\r\n",
        "item99.X-ABLabel:Orphaned Label\r\n",
        "TEL:+1-555-2222\r\n",
        "EMAIL:plain@example.com\r\n",
        "END:VCARD\r\n"
    );
    let card2 = vcard_to_card(vcard2).expect("parse orphan test");
    assert_eq!(card2.phones.as_ref().unwrap().len(), 1);
    assert_eq!(card2.emails.as_ref().unwrap().len(), 1);

    // 3. Round-trip fixed point
    let emitted = card_to_vcard(&card1);
    let card1_re = vcard_to_card(&emitted).expect("re-parse");
    let emitted_re = card_to_vcard(&card1_re);
    assert_eq!(emitted, emitted_re);
}

#[test]
fn im_scheme_long_tail_aliases_and_canonical_uri_resolution() {
    use jmap_vcard::contact::{online_service_handle, online_service_uri};

    // 1. Verify canonical URI generation for all 10 mapped services
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

    // 2. Verify handle extraction and roundtrip across all 18 supported URI scheme aliases
    for (service, uri, expected_handle, header) in [
        ("AIM", "aim:alice_aim", "alice_aim", "X-AIM"),
        ("AIM", "aol:alice_aol", "alice_aol", "X-AIM"),
        ("Gadu-Gadu", "gg:123456", "123456", "X-GADUGADU"),
        ("Gadu-Gadu", "gadugadu:123456", "123456", "X-GADUGADU"),
        ("Gadu-Gadu", "gadu:123456", "123456", "X-GADUGADU"),
        (
            "Google Talk",
            "xmpp:gtalk_user@example.com",
            "gtalk_user@example.com",
            "X-GOOGLE-TALK",
        ),
        (
            "Google Talk",
            "gtalk:gtalk_user2@example.com",
            "gtalk_user2@example.com",
            "X-GOOGLE-TALK",
        ),
        ("GroupWise", "groupwise:dave_gw", "dave_gw", "X-GROUPWISE"),
        (
            "GroupWise",
            "novell:dave_novell",
            "dave_novell",
            "X-GROUPWISE",
        ),
        ("ICQ", "icq:67890123", "67890123", "X-ICQ"),
        (
            "Jabber",
            "xmpp:dave_xmpp@example.com",
            "dave_xmpp@example.com",
            "X-JABBER",
        ),
        (
            "Jabber",
            "jabber:dave_jabber@example.com",
            "dave_jabber@example.com",
            "X-JABBER",
        ),
        ("MSN", "msn:bob@example.com", "bob@example.com", "X-MSN"),
        ("MSN", "msnim:bob@example.com", "bob@example.com", "X-MSN"),
        (
            "Matrix",
            "matrix:@elena:matrix.example",
            "@elena:matrix.example",
            "X-MATRIX",
        ),
        ("Skype", "skype:grace_skype", "grace_skype", "X-SKYPE"),
        ("Yahoo", "yahoo:carol_yahoo", "carol_yahoo", "X-YAHOO"),
        ("Yahoo", "ymsgr:carol_ymsgr", "carol_ymsgr", "X-YAHOO"),
    ] {
        let card = at_uri(Some(service), uri);
        let vcard = card_to_vcard(&card);
        assert_eq!(
            line(&vcard, header),
            format!("{header};X-JMAP-KEY=s1;TYPE=HOME:{expected_handle}"),
            "service: {service}, uri: {uri}"
        );

        let parsed = vcard_to_card(&vcard).expect("parse");
        let services = parsed.online_services.expect("online services");
        assert_eq!(services["s1"].user.as_deref(), Some(expected_handle));
        assert_eq!(services["s1"].uri, None);
        assert_eq!(services["s1"].service.as_deref(), Some(service));

        let service_entry = OnlineService {
            service: Some(service.to_owned()),
            uri: Some(uri.to_owned()),
            user: None,
            extra: BTreeMap::new(),
        };
        assert_eq!(
            online_service_handle(&service_entry),
            Some(expected_handle),
            "handle extraction for {service} with uri {uri}"
        );
    }
}

#[test]
fn im_scheme_action_query_and_invalid_handle_rejection() {
    use jmap_vcard::contact::states_online_service;

    for (service, uri, header) in [
        ("AIM", "aim:goim?screenname=alice", "X-AIM"),
        ("AIM", "aol:chat?user=alice", "X-AIM"),
        ("Gadu-Gadu", "gg:chat?uin=12345", "X-GADUGADU"),
        (
            "Google Talk",
            "gtalk:chat?jid=bob@example.com",
            "X-GOOGLE-TALK",
        ),
        ("GroupWise", "novell:message?user=carol", "X-GROUPWISE"),
        ("ICQ", "icq:message?uin=12345678", "X-ICQ"),
        ("Jabber", "jabber:user@example.com?message", "X-JABBER"),
        ("MSN", "msnim:add?contact=elena@example.com", "X-MSN"),
        ("Matrix", "matrix:u/frank:matrix.org", "X-MATRIX"),
        ("Skype", "skype:echo123?call", "X-SKYPE"),
        ("Yahoo", "ymsgr:sendim?heidi", "X-YAHOO"),
        ("Yahoo", "ymsgr:chat?heidi", "X-YAHOO"),
    ] {
        let card = at_uri(Some(service), uri);
        let vcard = card_to_vcard(&card);
        assert!(
            !vcard.contains(header),
            "{service} uri '{uri}' was unexpectedly drawn on {header}: {vcard}"
        );

        let service_entry = OnlineService {
            service: Some(service.to_owned()),
            uri: Some(uri.to_owned()),
            user: None,
            extra: BTreeMap::new(),
        };
        assert!(
            !states_online_service(&service_entry),
            "{service} uri '{uri}' should be rejected by states_online_service"
        );
    }
}

#[test]
fn twitter_sip_and_unslotted_social_services_characterization_and_rationale() {
    // 1. Contact on server with slotted and unslotted/unmodeled onlineServices
    let mut card = ContactCard::default();
    let mut services = BTreeMap::new();
    services.insert(
        "s_jabber".to_owned(),
        OnlineService {
            service: Some("Jabber".to_owned()),
            user: Some("vera@jabber.example".to_owned()),
            ..OnlineService::default()
        },
    );
    services.insert(
        "s_twitter".to_owned(),
        OnlineService {
            service: Some("Twitter".to_owned()),
            user: Some("@vera_tw".to_owned()),
            uri: Some("https://twitter.com/vera_tw".to_owned()),
            ..OnlineService::default()
        },
    );
    services.insert(
        "s_sip".to_owned(),
        OnlineService {
            service: Some("SIP".to_owned()),
            uri: Some("sip:vera@example.com".to_owned()),
            ..OnlineService::default()
        },
    );
    services.insert(
        "s_telegram".to_owned(),
        OnlineService {
            service: Some("Telegram".to_owned()),
            user: Some("vera_tg".to_owned()),
            uri: Some("tg://resolve?domain=vera_tg".to_owned()),
            ..OnlineService::default()
        },
    );
    services.insert(
        "s_discord".to_owned(),
        OnlineService {
            service: Some("Discord".to_owned()),
            user: Some("vera#1234".to_owned()),
            ..OnlineService::default()
        },
    );
    services.insert(
        "s_signal".to_owned(),
        OnlineService {
            service: Some("Signal".to_owned()),
            user: Some("+1555123456".to_owned()),
            ..OnlineService::default()
        },
    );
    services.insert(
        "s_mastodon".to_owned(),
        OnlineService {
            service: Some("Mastodon".to_owned()),
            user: Some("@vera@social.example".to_owned()),
            uri: Some("acct:vera@social.example".to_owned()),
            ..OnlineService::default()
        },
    );
    card.online_services = Some(services);

    // 2. Outbound vCard contains ONLY mapped slotted services (Jabber)
    let vcard = card_to_vcard(&card);
    assert!(
        vcard.contains("X-JABBER;X-JMAP-KEY=s_jabber;TYPE=HOME:vera@jabber.example\r\n"),
        "{vcard}"
    );
    assert!(!vcard.contains("X-TWITTER"), "{vcard}");
    assert!(!vcard.contains("X-SIP"), "{vcard}");
    assert!(!vcard.contains("X-TELEGRAM"), "{vcard}");
    assert!(!vcard.contains("X-DISCORD"), "{vcard}");
    assert!(!vcard.contains("X-SIGNAL"), "{vcard}");
    assert!(!vcard.contains("X-MASTODON"), "{vcard}");

    // 3. Inbound vCard with unslotted lines parses safely without corrupting mapped services
    let vcard_with_unslotted = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Test User\r\n",
        "X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera@jabber.example\r\n",
        "X-TWITTER:@vera_tw\r\n",
        "X-SIP:sip:vera@example.com\r\n",
        "X-TELEGRAM:vera_tg\r\n",
        "X-DISCORD:vera#1234\r\n",
        "END:VCARD\r\n"
    );
    let parsed = vcard_to_card(vcard_with_unslotted).expect("parse vcard");
    let parsed_services = parsed.online_services.as_ref().expect("online_services");
    assert_eq!(parsed_services.len(), 1);
    assert_eq!(
        parsed_services["s1"].user.as_deref(),
        Some("vera@jabber.example")
    );
    assert_eq!(parsed_services["s1"].service.as_deref(), Some("Jabber"));

    // 4. Fixed-point stability across round-trips
    let export1 = card_to_vcard(&parsed);
    let card2 = vcard_to_card(&export1).expect("re-parse");
    let export2 = card_to_vcard(&card2);
    assert_eq!(export1, export2);
}

#[test]
fn logo_and_key_vcard_lines_and_server_preservation_characterization() {
    // Audit & characterization of LOGO (RFC 2426 §3.5.3 / E_CONTACT_LOGO) and KEY (RFC 2426 §3.7.2 / E_CONTACT_X509_CERT / E_CONTACT_PGP_CERT):
    //
    // 1. Inbound vCard with LOGO, KEY (X509, PGP, URI), PHOTO, and standard contact properties
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:pas-id-test-logo-key-001\r\n",
        "FN:Dr. Vera Marie Oldenburg\r\n",
        "N:Oldenburg;Vera;Marie;Dr.;\r\n",
        "EMAIL;X-JMAP-KEY=e1;TYPE=WORK,PREF:vera@example.com\r\n",
        "TEL;X-JMAP-KEY=p1;TYPE=WORK,CELL:+1-555-0199\r\n",
        "ADR;X-JMAP-KEY=a1;TYPE=WORK:;;Hauptstr. 1;Berlin;;10115;Germany\r\n",
        "PHOTO;X-JMAP-KEY=m1;TYPE=JPEG;ENCODING=b:/9j/4AAQSkZJRg==\r\n",
        // LOGO lines: inline base64 and remote URI
        "LOGO;TYPE=PNG;ENCODING=b:iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==\r\n",
        "LOGO;VALUE=uri:https://example.com/corporate_logo.png\r\n",
        // KEY lines: X.509 certificate in base64, PGP public key in base64, URI reference, case-insensitive
        "KEY;TYPE=X509;ENCODING=b:MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA0Y123\r\n",
        "KEY;TYPE=PGP;ENCODING=b:mQGNBF+1234567890abcdef\r\n",
        "KEY;VALUE=uri:https://example.com/keys/vera.asc\r\n",
        "key;type=x509;encoding=b:MIIB...case_insensitive\r\n",
        "KEY:bare-untyped-key-payload-string\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse vcard with logo and key");

    // Verify contact name, email, phone, address, and PHOTO are parsed intact
    assert_eq!(
        card.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Dr. Vera Marie Oldenburg")
    );
    assert_eq!(card.emails.as_ref().map(|e| e.len()), Some(1));
    assert_eq!(card.phones.as_ref().map(|p| p.len()), Some(1));
    assert_eq!(card.addresses.as_ref().map(|a| a.len()), Some(1));

    // Media map contains ONLY the PHOTO — LOGO lines are safely dropped on parse
    let media = card.media.as_ref().expect("media present");
    assert_eq!(media.len(), 1);
    assert_eq!(media["m1"].kind.as_deref(), Some("photo"));
    assert_eq!(media["m1"].media_type.as_deref(), Some("image/JPEG"));
    assert!(media["m1"].uri.starts_with("data:image/JPEG;base64,"));

    // JSContact extra is empty (no unmodeled junk leaked)
    assert!(card.extra.is_empty());

    // Outbound emission: emits standard PHOTO line and strictly omits LOGO and KEY
    let emitted = card_to_vcard(&card);
    assert!(
        emitted.contains("PHOTO;X-JMAP-KEY=m1;TYPE=JPEG;ENCODING=b:/9j/4AAQSkZJRg==")
            || emitted.contains("PHOTO;X-JMAP-KEY=m1;TYPE=jpeg;ENCODING=b:/9j/4AAQSkZJRg=="),
        "PHOTO must be emitted: {emitted}"
    );
    assert!(
        !emitted.contains("LOGO"),
        "LOGO must not be emitted: {emitted}"
    );
    assert!(
        !emitted.contains("KEY;"),
        "KEY; must not be emitted: {emitted}"
    );
    assert!(
        !emitted.contains("KEY:"),
        "KEY: must not be emitted: {emitted}"
    );

    // Multi-pass roundtrip fixed-point stability
    let card2 = vcard_to_card(&emitted).expect("re-parse emitted vcard");
    assert_eq!(card2, card);
    let emitted2 = card_to_vcard(&card2);
    assert_eq!(emitted2, emitted);
}

#[test]
fn crypto_keys_and_logo_server_state_untouched_characterization() {
    use jmap_vcard::contact::{same_photo, states_media};

    // 1. Server-side contact card carrying cryptoKeys in extra and mixed media (photo, logo, sound)
    let mut card = ContactCard {
        card_type: Some("Card".to_owned()),
        version: Some("1.0".to_owned()),
        ..ContactCard::default()
    };

    // JSContact RFC 9553 §2.7.1 cryptoKeys
    let mut crypto_keys = BTreeMap::new();
    crypto_keys.insert(
        "k1".to_owned(),
        json!({
            "@type": "CryptoKey",
            "kind": "key",
            "uri": "data:application/x-x509-ca-cert;base64,MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8A",
            "mediaType": "application/x-x509-ca-cert"
        }),
    );
    crypto_keys.insert(
        "k2".to_owned(),
        json!({
            "@type": "CryptoKey",
            "kind": "key",
            "uri": "https://keys.openpgp.org/vks/v1/by-fingerprint/1234567890",
            "mediaType": "application/pgp-keys"
        }),
    );
    card.extra
        .insert("cryptoKeys".to_owned(), json!(crypto_keys));

    // JSContact RFC 9553 §2.6.4 media
    let mut media = BTreeMap::new();
    let photo = Media {
        kind: Some("photo".to_owned()),
        uri: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==".to_owned(),
        media_type: Some("image/png".to_owned()),
        extra: BTreeMap::new(),
    };
    let logo = Media {
        kind: Some("logo".to_owned()),
        uri: "https://example.com/corp_logo.svg".to_owned(),
        media_type: Some("image/svg+xml".to_owned()),
        extra: BTreeMap::new(),
    };
    let sound = Media {
        kind: Some("sound".to_owned()),
        uri: "https://example.com/pronunciation.ogg".to_owned(),
        media_type: Some("audio/ogg".to_owned()),
        extra: BTreeMap::new(),
    };
    media.insert("m_photo".to_owned(), photo.clone());
    media.insert("m_logo".to_owned(), logo.clone());
    media.insert("m_sound".to_owned(), sound.clone());
    card.media = Some(media);

    // 2. Predicate validation: states_media answers true ONLY for photo
    assert!(states_media(&photo), "photo must be stateable");
    assert!(!states_media(&logo), "logo must NOT be stateable on PHOTO");
    assert!(
        !states_media(&sound),
        "sound must NOT be stateable on PHOTO"
    );

    // 3. same_photo equality comparisons
    assert!(same_photo(&photo, &photo));
    assert!(same_photo(&logo, &sound)); // Both evaluate to None -> true
    assert!(!same_photo(&photo, &logo)); // Photo vs None -> false

    // 4. Outbound vCard emission: emits PHOTO and omits LOGO, SOUND, and cryptoKeys
    let vcard = card_to_vcard(&card);
    assert!(
        vcard.contains("PHOTO;X-JMAP-KEY=m_photo;TYPE=png;ENCODING=b:")
            || vcard.contains("PHOTO;X-JMAP-KEY=m_photo;TYPE=PNG;ENCODING=b:")
    );
    assert!(!vcard.contains("LOGO"), "{vcard}");
    assert!(!vcard.contains("SOUND"), "{vcard}");
    assert!(!vcard.contains("KEY;"), "{vcard}");
    assert!(!vcard.contains("KEY:"), "{vcard}");
    assert!(!vcard.contains("cryptoKeys"), "{vcard}");

    // 5. Inbound parse from emitted vCard
    let parsed = vcard_to_card(&vcard).expect("parse");
    let parsed_media = parsed.media.as_ref().expect("media");
    assert_eq!(parsed_media.len(), 1);
    assert_eq!(parsed_media["m_photo"].kind.as_deref(), Some("photo"));
    assert!(parsed.extra.is_empty());
}

#[test]
fn key_and_logo_edge_cases_and_malformed_payloads() {
    // 1. Folded long base64 KEY and LOGO lines (75 octets)
    let folded_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Folded Key and Logo User\r\n",
        "KEY;TYPE=X509;ENCODING=b:MIIDhzCCAm+gAwIBAgIJAOnL/n8c3hB/MA0GCSqGSIb3DQEBCwUA\r\n",
        " MBUxEzARBgNVBAMMCkZha2UgQ0EgMDEeFw0yMDAxMDEwMDAwMDBaFw0zMDAxMDEwMDAwMDBa\r\n",
        " MBUxEzARBgNVBAMMCkZha2UgQ0EgMDE=\r\n",
        "LOGO;TYPE=JPEG;ENCODING=b:/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAP//////////////////\r\n",
        " ////////////////////////////////////////////////////////////////////wgAL\r\n",
        "CAABAAEBAREA/8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABPxA=\r\n",
        "END:VCARD\r\n"
    );
    let card1 = vcard_to_card(folded_vcard).expect("parse folded key/logo");
    assert_eq!(
        card1.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Folded Key and Logo User")
    );
    assert!(card1.media.is_none());

    // 2. Empty KEY and LOGO property lines
    let empty_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Empty Props User\r\n",
        "KEY:\r\n",
        "KEY;TYPE=X509;ENCODING=b:\r\n",
        "LOGO:\r\n",
        "LOGO;TYPE=PNG:\r\n",
        "LOGO;VALUE=uri:\r\n",
        "END:VCARD\r\n"
    );
    let card2 = vcard_to_card(empty_vcard).expect("parse empty key/logo");
    assert_eq!(
        card2.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Empty Props User")
    );
    assert!(card2.media.is_none());

    // 3. Round-trip stability
    let emitted1 = card_to_vcard(&card1);
    let card1_re = vcard_to_card(&emitted1).expect("re-parse");
    let emitted1_re = card_to_vcard(&card1_re);
    assert_eq!(emitted1, emitted1_re);
}

#[test]
fn proptest_generator_sync_coverage_and_domain_invariants() {
    // 1. Contact with name-level fileAs and sortAs, card-level fileAs and cryptoKeys
    let mut name_extra = BTreeMap::new();
    name_extra.insert("fileAs".to_string(), json!("Doe, John (Name Extra)"));
    name_extra.insert("sortAs".to_string(), json!("DOE"));

    let mut card_extra = BTreeMap::new();
    card_extra.insert("fileAs".to_string(), json!("Doe, John (Card Extra)"));
    card_extra.insert(
        "cryptoKeys".to_string(),
        json!({"k1": {"kind": "pgp", "uri": "https://example.com/key.asc"}}),
    );
    card_extra.insert("unmodeledCustom".to_string(), json!("preserve_me"));

    let mut email_extra = BTreeMap::new();
    email_extra.insert("label".to_string(), json!("Support"));

    let mut phone_extra = BTreeMap::new();
    phone_extra.insert("label".to_string(), json!("Direct Line"));

    let mut adr_extra = BTreeMap::new();
    adr_extra.insert("label".to_string(), json!("Warehouse"));
    adr_extra.insert("pref".to_string(), json!(1));

    let mut link_extra = BTreeMap::new();
    link_extra.insert("label".to_string(), json!("Portfolio"));

    let mut rel_extra = BTreeMap::new();
    rel_extra.insert("label".to_string(), json!("Emergency Contact"));

    let card = ContactCard {
        id: Some("C-GEN-SYNC".into()),
        name: Some(Name {
            full: Some("John Doe".into()),
            components: Some(vec![
                NameComponent::new("given", "John"),
                NameComponent::new("surname", "Doe"),
            ]),
            extra: name_extra,
        }),
        emails: Some(
            [(
                "e1".to_string(),
                ContactEmail {
                    address: "support@example.com".into(),
                    contexts: Some(json!({"work": true})),
                    pref: Some(1),
                    extra: email_extra,
                },
            )]
            .into(),
        ),
        phones: Some(
            [(
                "p1".to_string(),
                ContactPhone {
                    number: "+1-555-0199".into(),
                    features: Some(json!({"voice": true})),
                    contexts: Some(json!({"work": true})),
                    pref: Some(1),
                    extra: phone_extra,
                },
            )]
            .into(),
        ),
        addresses: Some(
            [(
                "a1".to_string(),
                Address {
                    components: Some(vec![
                        AddressComponent::new("name", "100 Industrial Pkwy"),
                        AddressComponent::new("locality", "Springfield"),
                    ]),
                    contexts: Some(json!({"work": true})),
                    full: Some("100 Industrial Pkwy, Springfield".into()),
                    extra: adr_extra,
                },
            )]
            .into(),
        ),
        links: Some(
            [(
                "l1".to_string(),
                Link {
                    uri: "https://portfolio.example.com".into(),
                    kind: Some("website".into()),
                    extra: link_extra,
                },
            )]
            .into(),
        ),
        related_to: Some(
            [(
                "r1".to_string(),
                Relation {
                    relation: Some([("partner".to_string(), json!(true))].into()),
                    extra: rel_extra,
                },
            )]
            .into(),
        ),
        extra: card_extra,
        ..ContactCard::default()
    };

    // Export₁ must emit X-EVOLUTION-FILE-AS (preferring name-level fileAs) and standard properties
    let vcard1 = card_to_vcard(&card);
    assert!(vcard1.contains("X-EVOLUTION-FILE-AS:Doe\\, John (Name Extra)"));
    assert!(!vcard1.contains("cryptoKeys"));
    assert!(!vcard1.contains("unmodeledCustom"));
    assert!(!vcard1.contains("KEY:"));

    // Parse₁ extracts fields cleanly
    let parsed1 = vcard_to_card(&vcard1).expect("parse vcard1");
    let name1 = parsed1.name.as_ref().expect("name1");
    assert_eq!(
        name1.extra.get("fileAs").and_then(|v| v.as_str()),
        Some("Doe, John (Name Extra)")
    );

    // Multi-pass round-trip reaches exact fixed point
    let vcard2 = card_to_vcard(&parsed1);
    let parsed2 = vcard_to_card(&vcard2).expect("parse vcard2");
    let vcard3 = card_to_vcard(&parsed2);
    assert_eq!(vcard2, vcard3, "Export₂ == Export₃ fixpoint invariant");
    assert_eq!(parsed1, parsed2, "Card₂ == Card₃ fixpoint invariant");
}

#[test]
fn apple_property_group_block_generator_roundtrips_to_fixpoint() {
    let raw_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Apple Groups Contact\r\n",
        "item1.TEL;type=pref:+1-555-0100\r\n",
        "item1.X-ABLabel:_$!<Mobile>!$_\r\n",
        "item2.TEL:+1-555-0200\r\n",
        "item2.X-ABLabel:Direct Line\r\n",
        "item3.EMAIL;type=pref:alice@example.com\r\n",
        "item3.X-ABLabel:_$!<Work>!$_\r\n",
        "item4.ADR;type=pref:;;123 Main St;Springfield;IL;62701;USA\r\n",
        "item4.X-ABLabel:_$!<Work>!$_\r\n",
        "item5.URL:https://alice.example.com\r\n",
        "item5.X-ABLabel:_$!<HomePage>!$_\r\n",
        "item6.X-ABRELATEDNAMES;type=pref:Bob Smith\r\n",
        "item6.X-ABLabel:_$!<Spouse>!$_\r\n",
        "item7.X-AB-RELATED-NAMES:Charlie Brown\r\n",
        "item7.X-ABLabel:Colleague\r\n",
        "item8.X-ABDATE;type=pref:2018-06-15\r\n",
        "item8.X-ABLabel:_$!<Anniversary>!$_\r\n",
        "item9.X-AB-DATE:2020-01-01\r\n",
        "item9.X-ABLabel:First Met\r\n",
        "END:VCARD\r\n"
    );

    let parsed1 = vcard_to_card(raw_vcard).expect("parse apple groups raw vcard");
    let phones = parsed1.phones.as_ref().expect("phones");
    assert_eq!(phones.len(), 2);

    let emails = parsed1.emails.as_ref().expect("emails");
    assert_eq!(emails.len(), 1);

    let addresses = parsed1.addresses.as_ref().expect("addresses");
    assert_eq!(addresses.len(), 1);

    let links = parsed1.links.as_ref().expect("links");
    assert_eq!(links.len(), 1);

    let relations = parsed1.related_to.as_ref().expect("relations");
    assert_eq!(relations.len(), 2);

    let anniversaries = parsed1.anniversaries.as_ref().expect("anniversaries");
    assert_eq!(anniversaries.len(), 2);

    // Outbound emission normalizes to vCard 3.0 standard properties and reaches fixed point
    let vcard1 = card_to_vcard(&parsed1);
    let parsed2 = vcard_to_card(&vcard1).expect("parse vcard1");
    let vcard2 = card_to_vcard(&parsed2);
    let parsed3 = vcard_to_card(&vcard2).expect("parse vcard2");
    let vcard3 = card_to_vcard(&parsed3);

    assert_eq!(vcard2, vcard3, "Export₂ == Export₃ fixpoint invariant");
    assert_eq!(parsed2, parsed3, "Card₂ == Card₃ fixpoint invariant");
}

#[test]
fn vcard_40_google_contacts_representative_fixture_import_and_roundtrip() {
    let google_v4 = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "N:Doe;John;Michael;Mr.;Esq.\r\n",
        "FN:Mr. John Michael Doe Esq.\r\n",
        "NICKNAME:Johnny\r\n",
        "PHOTO:data:image/jpeg;base64,/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAP//////////////////////////////////////////////////////////////////////////////////////wgALCAABAAEBAREA/8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABPxA=\r\n",
        "BDAY:19800101\r\n",
        "ANNIVERSARY:20100615\r\n",
        "GENDER:M\r\n",
        "EMAIL;TYPE=work:john.doe@company.com\r\n",
        "EMAIL;TYPE=home:johndoe@gmail.com\r\n",
        "TEL;TYPE=\"work,voice\";VALUE=uri:tel:+1-555-555-0100\r\n",
        "TEL;TYPE=\"cell,voice\";VALUE=uri:tel:+1-555-555-0101\r\n",
        "TEL;TYPE=\"home,voice\";VALUE=uri:tel:+1-555-555-0102\r\n",
        "ADR;TYPE=work:;;100 Work St.;Tech City;CA;94000;United States\r\n",
        "ADR;TYPE=home:;;200 Home Ave.;Home Town;CA;94001;United States\r\n",
        "ORG:Alphabet Inc.;Google LLC;Core Systems\r\n",
        "TITLE:Senior Staff Engineer\r\n",
        "ROLE:Engineering Lead\r\n",
        "NOTE:Met at Open Source Summit in 2024.\\nGreat discussion on JMAP.\r\n",
        "URL:https://john.doe.example.com\r\n",
        "IMPP:xmpp:johndoe@chat.example.com\r\n",
        "CATEGORIES:Colleagues,Engineering,VIP\r\n",
        "CLIENTPIDMAP:1;urn:uuid:53e374d9-337e-4727-8803-a1e9c14e0556\r\n",
        "UID:urn:uuid:4fbe8750-df3e-4725-b220-e0c62ba1e9f8\r\n",
        "REV:20260823T120000Z\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(google_v4).expect("parse Google Contacts vCard 4.0");

    // Name and components
    let name = parsed.name.as_ref().expect("name");
    assert_eq!(name.full.as_deref(), Some("Mr. John Michael Doe Esq."));
    let comps = name.components.as_ref().expect("components");
    assert_eq!(comps.len(), 5);

    // Nicknames
    let nicknames = parsed.nicknames.as_ref().expect("nicknames");
    assert_eq!(nicknames.len(), 1);
    assert_eq!(nicknames["k1"].name, "Johnny");

    // Photo
    let media = parsed.media.as_ref().expect("media");
    assert_eq!(media.len(), 1);
    let photo = &media["m1"];
    assert_eq!(photo.kind.as_deref(), Some("photo"));
    assert!(photo.uri.starts_with("data:image/jpeg;base64,"));
    assert_eq!(photo.media_type.as_deref(), Some("image/jpeg"));

    // Anniversaries (BDAY:19800101 and ANNIVERSARY:20100615)
    let anniversaries = parsed.anniversaries.as_ref().expect("anniversaries");
    assert_eq!(anniversaries.len(), 2);
    let bday = anniversaries
        .values()
        .find(|a| a.kind == "birth")
        .expect("birth");
    assert_eq!(bday.date.as_ref().unwrap()["year"], 1980);
    assert_eq!(bday.date.as_ref().unwrap()["month"], 1);
    assert_eq!(bday.date.as_ref().unwrap()["day"], 1);
    let anniv = anniversaries
        .values()
        .find(|a| a.kind == "wedding")
        .expect("wedding");
    assert_eq!(anniv.date.as_ref().unwrap()["year"], 2010);
    assert_eq!(anniv.date.as_ref().unwrap()["month"], 6);
    assert_eq!(anniv.date.as_ref().unwrap()["day"], 15);

    // Emails
    let emails = parsed.emails.as_ref().expect("emails");
    assert_eq!(emails.len(), 2);
    let work_email = emails
        .values()
        .find(|e| e.address == "john.doe@company.com")
        .unwrap();
    assert_eq!(work_email.contexts.as_ref().unwrap()["work"], true);
    let home_email = emails
        .values()
        .find(|e| e.address == "johndoe@gmail.com")
        .unwrap();
    assert_eq!(home_email.contexts.as_ref().unwrap()["private"], true);

    // Phones (TYPE="work,voice", TYPE="cell,voice", TYPE="home,voice")
    let phones = parsed.phones.as_ref().expect("phones");
    assert_eq!(phones.len(), 3);
    let work_phone = phones
        .values()
        .find(|p| p.number == "tel:+1-555-555-0100")
        .unwrap();
    assert_eq!(work_phone.contexts.as_ref().unwrap()["work"], true);
    assert_eq!(work_phone.features.as_ref().unwrap()["voice"], true);
    let cell_phone = phones
        .values()
        .find(|p| p.number == "tel:+1-555-555-0101")
        .unwrap();
    assert_eq!(cell_phone.features.as_ref().unwrap()["mobile"], true);
    assert_eq!(cell_phone.features.as_ref().unwrap()["voice"], true);
    let home_phone = phones
        .values()
        .find(|p| p.number == "tel:+1-555-555-0102")
        .unwrap();
    assert_eq!(home_phone.contexts.as_ref().unwrap()["private"], true);
    assert_eq!(home_phone.features.as_ref().unwrap()["voice"], true);

    // Addresses
    let addresses = parsed.addresses.as_ref().expect("addresses");
    assert_eq!(addresses.len(), 2);

    // Organization with units
    let orgs = parsed.organizations.as_ref().expect("organizations");
    assert_eq!(orgs.len(), 1);
    let org = &orgs["o1"];
    assert_eq!(org.name.as_deref(), Some("Alphabet Inc."));
    let units = org.units.as_ref().expect("units");
    assert_eq!(units.len(), 2);
    assert_eq!(units[0].name, "Google LLC");
    assert_eq!(units[1].name, "Core Systems");

    // Titles
    let titles = parsed.titles.as_ref().expect("titles");
    assert_eq!(titles.len(), 2);
    let title = titles.values().find(|t| t.kind.is_none()).unwrap();
    assert_eq!(title.name, "Senior Staff Engineer");
    let role = titles
        .values()
        .find(|t| t.kind.as_deref() == Some("role"))
        .unwrap();
    assert_eq!(role.name, "Engineering Lead");

    // Notes
    let notes = parsed.notes.as_ref().expect("notes");
    assert_eq!(notes.len(), 1);
    assert_eq!(
        notes["n1"].note,
        "Met at Open Source Summit in 2024.\nGreat discussion on JMAP."
    );

    // Links
    let links = parsed.links.as_ref().expect("links");
    assert_eq!(links.len(), 1);
    assert_eq!(links["l1"].uri, "https://john.doe.example.com");

    // Online services (IMPP:xmpp:johndoe@chat.example.com -> Jabber)
    let services = parsed.online_services.as_ref().expect("online_services");
    assert_eq!(services.len(), 1);
    let service = &services["s1"];
    assert_eq!(service.service.as_deref(), Some("Jabber"));
    assert_eq!(service.user.as_deref(), Some("johndoe@chat.example.com"));

    // Categories
    let keywords = parsed.keywords.as_ref().expect("keywords");
    assert_eq!(keywords.len(), 3);
    assert!(keywords.contains_key("Colleagues"));
    assert!(keywords.contains_key("Engineering"));
    assert!(keywords.contains_key("VIP"));

    // Unmapped/metadata properties: GENDER, CLIENTPIDMAP, REV do not pollute extra
    assert!(
        parsed.extra.is_empty(),
        "extra should be clean: {:?}",
        parsed.extra
    );

    // Round-trip fixpoint convergence
    let export1 = card_to_vcard(&parsed);
    assert!(export1.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"));
    assert!(export1.contains("PHOTO;X-JMAP-KEY=m1;TYPE=jpeg;ENCODING=b:"));
    assert!(export1.contains("X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY="));
    assert!(export1.contains("X-JABBER;X-JMAP-KEY="));

    let card2 = vcard_to_card(&export1).expect("parse export1");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃ fixpoint invariant");
    assert_eq!(card2, card3, "Card₂ == Card₃ fixpoint invariant");
}

#[test]
fn vcard_40_ios_share_sheet_fixture_import_and_roundtrip() {
    let ios_v4 = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "PRODID:-//Apple Inc.//iOS 17.5//EN\r\n",
        "N:Appleseed;John;;;\r\n",
        "FN:John Appleseed\r\n",
        "NICKNAME:Johnny\r\n",
        "PHOTO:data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==\r\n",
        "BDAY:19850412\r\n",
        "ANNIVERSARY:20150920\r\n",
        "RELATED;TYPE=spouse:Jane Appleseed\r\n",
        "RELATED;TYPE=manager:Tim Cook\r\n",
        "RELATED;TYPE=assistant:Siri\r\n",
        "EMAIL;TYPE=INTERNET;TYPE=HOME;TYPE=pref:john.appleseed@icloud.com\r\n",
        "EMAIL;TYPE=INTERNET;TYPE=WORK:john.appleseed@apple.com\r\n",
        "TEL;TYPE=CELL;TYPE=VOICE;TYPE=pref:tel:+1-800-692-7753\r\n",
        "TEL;TYPE=WORK;TYPE=VOICE:tel:+1-408-996-1010\r\n",
        "ADR;TYPE=WORK;TYPE=pref:;;1 Apple Park Way;Cupertino;CA;95014;United States\r\n",
        "URL:https://www.apple.com\r\n",
        "NOTE:iOS Share Sheet contact export\r\n",
        "CATEGORIES:VIP,Work\r\n",
        "UID:urn:uuid:ab12cd34-ef56-7890-abcd-ef1234567890\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(ios_v4).expect("parse iOS vCard 4.0");

    assert_eq!(
        parsed.name.as_ref().unwrap().full.as_deref(),
        Some("John Appleseed")
    );

    // Photo
    let media = parsed.media.as_ref().expect("media");
    let photo = &media["m1"];
    assert_eq!(photo.media_type.as_deref(), Some("image/png"));

    // Anniversaries
    let anniversaries = parsed.anniversaries.as_ref().expect("anniversaries");
    assert_eq!(anniversaries.len(), 2);

    // Relations (RELATED;TYPE=spouse, manager, assistant)
    let relations = parsed.related_to.as_ref().expect("relations");
    assert_eq!(relations.len(), 3);
    assert_eq!(
        relations["Jane Appleseed"].relation.as_ref().unwrap()["spouse"],
        true
    );
    assert_eq!(
        relations["Tim Cook"].relation.as_ref().unwrap()["manager"],
        true
    );
    assert_eq!(
        relations["Siri"].relation.as_ref().unwrap()["assistant"],
        true
    );

    // Emails
    let emails = parsed.emails.as_ref().expect("emails");
    assert_eq!(emails.len(), 2);
    let pref_email = emails
        .values()
        .find(|e| e.address == "john.appleseed@icloud.com")
        .unwrap();
    assert_eq!(pref_email.pref, Some(1));

    // Phones
    let phones = parsed.phones.as_ref().expect("phones");
    assert_eq!(phones.len(), 2);
    let pref_phone = phones
        .values()
        .find(|p| p.number == "tel:+1-800-692-7753")
        .unwrap();
    assert_eq!(pref_phone.pref, Some(1));

    // Outbound emission normalizes to standard vCard 3.0
    let export1 = card_to_vcard(&parsed);
    assert!(export1.contains("X-EVOLUTION-SPOUSE:Jane Appleseed\r\n"));
    assert!(export1.contains("X-EVOLUTION-MANAGER:Tim Cook\r\n"));
    assert!(export1.contains("X-EVOLUTION-ASSISTANT:Siri\r\n"));
    assert!(export1.contains("X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY="));
    assert!(export1.contains("PHOTO;X-JMAP-KEY=m1;TYPE=png;ENCODING=b:"));

    // Multi-stage fixpoint convergence
    let card2 = vcard_to_card(&export1).expect("parse export1");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃ fixpoint invariant");
    assert_eq!(card2, card3, "Card₂ == Card₃ fixpoint invariant");
}

#[test]
fn vcard_40_nextcloud_and_carddav_fixture_import_and_roundtrip() {
    let nextcloud_v4 = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "PRODID:-//Nextcloud Contacts v5.3.0//EN\r\n",
        "UID:12345678-1234-1234-1234-123456789abc\r\n",
        "FN:Alice Wonderland\r\n",
        "N:Wonderland;Alice;;;\r\n",
        "EMAIL;TYPE=work:alice@wonderland.org\r\n",
        "TEL;TYPE=cell:tel:+49-170-1234567\r\n",
        "ADR;TYPE=home:;;Rabbit Hole 1;Fantasy City;Bavaria;80331;Germany\r\n",
        "PHOTO:https://wonderland.org/photos/alice.jpg\r\n",
        "BDAY:19900520\r\n",
        "ANNIVERSARY:20200815\r\n",
        "IMPP;TYPE=home:xmpp:alice@jabber.de\r\n",
        "IMPP;TYPE=work:matrix:alice:matrix.org\r\n",
        "IMPP:skype:alice_wonder\r\n",
        "URL:https://alice.wonderland.org\r\n",
        "NOTE:Nextcloud vCard 4.0 export with UTF-8 umlauts: München, Gräfelfing\r\n",
        "CATEGORIES:Friends,Open Source\r\n",
        "TZ:Europe/Berlin\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(nextcloud_v4).expect("parse Nextcloud vCard 4.0");

    assert_eq!(
        parsed.name.as_ref().unwrap().full.as_deref(),
        Some("Alice Wonderland")
    );

    // Photo (URI reference)
    let media = parsed.media.as_ref().expect("media");
    assert_eq!(media.len(), 1);
    let photo = &media["m1"];
    assert_eq!(photo.uri, "https://wonderland.org/photos/alice.jpg");

    // Online services (IMPP across Jabber, Matrix, Skype)
    let services = parsed.online_services.as_ref().expect("online_services");
    assert_eq!(services.len(), 3);
    let jabber = services
        .values()
        .find(|s| s.service.as_deref() == Some("Jabber"))
        .unwrap();
    assert_eq!(jabber.user.as_deref(), Some("alice@jabber.de"));
    let matrix = services
        .values()
        .find(|s| s.service.as_deref() == Some("Matrix"))
        .unwrap();
    assert_eq!(matrix.user.as_deref(), Some("alice:matrix.org"));
    let skype = services
        .values()
        .find(|s| s.service.as_deref() == Some("Skype"))
        .unwrap();
    assert_eq!(skype.user.as_deref(), Some("alice_wonder"));

    // Anniversaries
    let anniversaries = parsed.anniversaries.as_ref().expect("anniversaries");
    assert_eq!(anniversaries.len(), 2);

    // Note with Unicode umlauts
    let notes = parsed.notes.as_ref().expect("notes");
    assert_eq!(
        notes["n1"].note,
        "Nextcloud vCard 4.0 export with UTF-8 umlauts: München, Gräfelfing"
    );

    // TZ dropped safely
    assert!(parsed.extra.is_empty(), "extra should be clean");

    // Outbound emission & fixpoint
    let export1 = card_to_vcard(&parsed);
    assert!(
        export1.contains("PHOTO;X-JMAP-KEY=m1;VALUE=uri:https://wonderland.org/photos/alice.jpg")
    );
    assert!(export1.contains("X-JABBER;X-JMAP-KEY="));
    assert!(export1.contains("X-MATRIX;X-JMAP-KEY="));
    assert!(export1.contains("X-SKYPE;X-JMAP-KEY="));

    let card2 = vcard_to_card(&export1).expect("parse export1");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃ fixpoint invariant");
    assert_eq!(card2, card3, "Card₂ == Card₃ fixpoint invariant");
}

#[test]
fn vcard_40_photo_data_uris_and_mediatype_parameters() {
    // 1. Direct data: URI with image/jpeg
    let vcard_data_jpeg = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:Photo Data JPEG\r\n",
        "PHOTO:data:image/jpeg;base64,/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAP//////////////////////////////////////////////////////////////////////////////////////wgALCAABAAEBAREA/8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABPxA=\r\n",
        "END:VCARD\r\n"
    );
    let card1 = vcard_to_card(vcard_data_jpeg).expect("parse data jpeg");
    let media1 = card1.media.as_ref().expect("media");
    let p1 = &media1["m1"];
    assert!(p1.uri.starts_with("data:image/jpeg;base64,"));
    assert_eq!(p1.media_type.as_deref(), Some("image/jpeg"));

    // 2. Direct data: URI with explicit MEDIATYPE parameter
    let vcard_mediatype = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:Photo MEDIATYPE PNG\r\n",
        "PHOTO;MEDIATYPE=image/png:data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==\r\n",
        "END:VCARD\r\n"
    );
    let card2 = vcard_to_card(vcard_mediatype).expect("parse mediatype png");
    let media2 = card2.media.as_ref().expect("media");
    let p2 = &media2["m1"];
    assert!(p2.uri.starts_with("data:image/png;base64,"));
    assert_eq!(p2.media_type.as_deref(), Some("image/png"));

    // 3. Direct HTTP URI without VALUE=uri
    let vcard_http = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:Photo HTTP Direct\r\n",
        "PHOTO:https://example.com/direct_photo.webp\r\n",
        "END:VCARD\r\n"
    );
    let card3 = vcard_to_card(vcard_http).expect("parse direct http photo");
    let media3 = card3.media.as_ref().expect("media");
    let p3 = &media3["m1"];
    assert_eq!(p3.uri, "https://example.com/direct_photo.webp");

    // 4. HTTP URI with MEDIATYPE parameter
    let vcard_http_mediatype = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:Photo HTTP MEDIATYPE\r\n",
        "PHOTO;MEDIATYPE=image/jpeg:https://example.com/direct_photo.jpg\r\n",
        "END:VCARD\r\n"
    );
    let card4 = vcard_to_card(vcard_http_mediatype).expect("parse http mediatype photo");
    let media4 = card4.media.as_ref().expect("media");
    let p4 = &media4["m1"];
    assert_eq!(p4.uri, "https://example.com/direct_photo.jpg");
    assert_eq!(p4.media_type.as_deref(), Some("image/jpeg"));

    // All roundtrip cleanly to fixed points
    for card in [&card1, &card2, &card3, &card4] {
        let export1 = card_to_vcard(card);
        let c2 = vcard_to_card(&export1).expect("parse export1");
        let export2 = card_to_vcard(&c2);
        let c3 = vcard_to_card(&export2).expect("parse export2");
        let export3 = card_to_vcard(&c3);
        assert_eq!(export2, export3);
        assert_eq!(c2, c3);
    }
}

#[test]
fn vcard_40_impp_all_supported_services_and_action_query_rejection() {
    let vcard_impp = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:IMPP Matrix Contact\r\n",
        "IMPP:xmpp:alice@jabber.org\r\n",
        "IMPP;TYPE=home:skype:alice_skype\r\n",
        "IMPP;TYPE=work:matrix:alice:matrix.org\r\n",
        "IMPP:aim:alice_aim\r\n",
        "IMPP:icq:12345678\r\n",
        "IMPP:msn:alice@hotmail.com\r\n",
        "IMPP:yahoo:alice_yahoo\r\n",
        "IMPP:groupwise:alice_gw\r\n",
        "IMPP:gg:87654321\r\n",
        "IMPP:gtalk:alice.gtalk@gmail.com\r\n",
        // Action and query URIs that must be rejected safely
        "IMPP:skype:echo123?call\r\n",
        "IMPP:xmpp:alice?message\r\n",
        "IMPP:aim:goim?screenname=alice\r\n",
        "IMPP:unknownservice:user123\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(vcard_impp).expect("parse IMPP vCard 4.0");
    let services = parsed.online_services.as_ref().expect("online_services");
    assert_eq!(
        services.len(),
        10,
        "all 10 mapped services should be extracted"
    );

    let services_by_name: BTreeMap<_, _> = services
        .values()
        .map(|s| (s.service.as_deref().unwrap(), s.user.as_deref().unwrap()))
        .collect();

    assert_eq!(services_by_name["Jabber"], "alice@jabber.org");
    assert_eq!(services_by_name["Skype"], "alice_skype");
    assert_eq!(services_by_name["Matrix"], "alice:matrix.org");
    assert_eq!(services_by_name["AIM"], "alice_aim");
    assert_eq!(services_by_name["ICQ"], "12345678");
    assert_eq!(services_by_name["MSN"], "alice@hotmail.com");
    assert_eq!(services_by_name["Yahoo"], "alice_yahoo");
    assert_eq!(services_by_name["GroupWise"], "alice_gw");
    assert_eq!(services_by_name["Gadu-Gadu"], "87654321");
    assert_eq!(services_by_name["Google Talk"], "alice.gtalk@gmail.com");

    // Outbound roundtrip
    let export1 = card_to_vcard(&parsed);
    let card2 = vcard_to_card(&export1).expect("parse export1");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃ fixpoint invariant");
    assert_eq!(card2, card3, "Card₂ == Card₃ fixpoint invariant");
}

#[test]
fn vcard_40_related_and_anniversary_variations() {
    let vcard_related = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:Related and Anniversary Contact\r\n",
        "RELATED;TYPE=spouse:Jane Spouse\r\n",
        "RELATED;TYPE=partner:Alex Partner\r\n",
        "RELATED;TYPE=manager:Boss Man\r\n",
        "RELATED;TYPE=assistant:Help Desk\r\n",
        "RELATED;TYPE=co-worker:Colleague Bob\r\n",
        "RELATED:Friend Dave\r\n",
        "ANNIVERSARY:19990518\r\n",
        "ANNIVERSARY:20050820\r\n",
        "BDAY:19881125\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(vcard_related).expect("parse related v4");
    let relations = parsed.related_to.as_ref().expect("relations");

    assert_eq!(
        relations["Jane Spouse"].relation.as_ref().unwrap()["spouse"],
        true
    );
    assert_eq!(
        relations["Alex Partner"].relation.as_ref().unwrap()["spouse"],
        true
    );
    assert_eq!(
        relations["Boss Man"].relation.as_ref().unwrap()["manager"],
        true
    );
    assert_eq!(
        relations["Help Desk"].relation.as_ref().unwrap()["assistant"],
        true
    );
    assert_eq!(
        relations["Colleague Bob"].relation.as_ref().unwrap()["co-worker"],
        true
    );
    assert_eq!(
        relations["Friend Dave"].relation.as_ref().unwrap()["contact"],
        true
    );

    let anniversaries = parsed.anniversaries.as_ref().expect("anniversaries");
    assert_eq!(anniversaries.len(), 3);

    // Outbound roundtrip
    let export1 = card_to_vcard(&parsed);
    let card2 = vcard_to_card(&export1).expect("parse export1");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃ fixpoint invariant");
    assert_eq!(card2, card3, "Card₂ == Card₃ fixpoint invariant");
}

#[test]
fn vcard_40_type_parameters_quoted_and_comma_delimited() {
    let vcard_types = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:Quoted Types Contact\r\n",
        "TEL;TYPE=\"work,voice\":+1-555-0100\r\n",
        "TEL;TYPE=\"home,cell\";PREF=1:+1-555-0200\r\n",
        "TEL;TYPE=\"WORK,FAX\":+1-555-0300\r\n",
        "EMAIL;TYPE=\"WORK\";PREF=1:work@example.com\r\n",
        "EMAIL;TYPE=\"HOME\":home@example.com\r\n",
        "ADR;TYPE=\"WORK\";PREF=1:;;100 Corp Way;City;ST;12345;USA\r\n",
        "ADR;TYPE=\"HOME\":;;200 Res Rd;City;ST;12345;USA\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(vcard_types).expect("parse quoted types");

    let phones = parsed.phones.as_ref().expect("phones");
    assert_eq!(phones.len(), 3);
    let p1 = phones.values().find(|p| p.number == "+1-555-0100").unwrap();
    assert_eq!(p1.contexts.as_ref().unwrap()["work"], true);
    assert_eq!(p1.features.as_ref().unwrap()["voice"], true);

    let p2 = phones.values().find(|p| p.number == "+1-555-0200").unwrap();
    assert_eq!(p2.contexts.as_ref().unwrap()["private"], true);
    assert_eq!(p2.features.as_ref().unwrap()["mobile"], true);
    assert_eq!(p2.pref, Some(1));

    let p3 = phones.values().find(|p| p.number == "+1-555-0300").unwrap();
    assert_eq!(p3.contexts.as_ref().unwrap()["work"], true);
    assert_eq!(p3.features.as_ref().unwrap()["fax"], true);

    let emails = parsed.emails.as_ref().expect("emails");
    assert_eq!(emails.len(), 2);
    let e1 = emails
        .values()
        .find(|e| e.address == "work@example.com")
        .unwrap();
    assert_eq!(e1.contexts.as_ref().unwrap()["work"], true);
    assert_eq!(e1.pref, Some(1));

    let addresses = parsed.addresses.as_ref().expect("addresses");
    assert_eq!(addresses.len(), 2);
    let a1 = addresses
        .values()
        .find(|a| a.extra.get("pref") == Some(&serde_json::json!(1)))
        .unwrap();
    assert_eq!(a1.contexts.as_ref().unwrap()["work"], true);

    // Outbound roundtrip
    let export1 = card_to_vcard(&parsed);
    let card2 = vcard_to_card(&export1).expect("parse export1");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃ fixpoint invariant");
    assert_eq!(card2, card3, "Card₂ == Card₃ fixpoint invariant");
}

#[test]
fn agent_property_variations_nested_vcards_uris_and_fixpoint() {
    // RFC 2426 §3.5.4 AGENT property: specifies information about another person
    // who acts on behalf of the individual associated with the vCard.
    //
    // AGENT can be:
    // 1. A URI resolving to a vCard (e.g. `AGENT;VALUE=uri:http://...` or `AGENT:http://...`)
    // 2. A structured value containing a nested vCard (with BEGIN:VCARD...END:VCARD)
    // 3. A plain text name or unescaped representation
    //
    // EDS has no E_CONTACT_AGENT and Evolution's contact editor has no UI for agent.
    // In JSContact (RFC 9553 §2.1.8 / §2.7.2), an agent relation resides in `relatedTo`
    // with relation type "agent".
    //
    // Contract:
    // - Inbound AGENT lines (nested vCards, URIs, plain text, folded escaping) parse safely
    //   without corrupting surrounding properties (FN, N, EMAIL, TEL, ADR, NOTE, etc.).
    // - AGENT is safely dropped on parse and does not pollute `card.extra` or `card.related_to`.
    // - Outbound vCard 3.0 strictly omits AGENT lines.
    // - Round-trips reach fixed-point stability (Export₂ == Export₃, Card₂ == Card₃).

    let agent_permutations = [
        // 1. Standard RFC 2426 URI format
        "AGENT;VALUE=uri:https://example.com/agents/smith.vcf\r\n",
        // 2. CID URI format (email attachment)
        "AGENT;VALUE=uri:CID:agent-007@example.com\r\n",
        // 3. Direct URI without explicit VALUE parameter
        "AGENT:https://example.com/direct_agent.vcf\r\n",
        // 4. Case-insensitive property and parameter names
        "agent;value=uri:https://example.com/lowercase_agent.vcf\r\n",
        "Agent;Value=URI:https://example.com/mixed_case_agent.vcf\r\n",
        // 5. Escaped inline nested vCard with \n newlines
        "AGENT:BEGIN:VCARD\\nVERSION:3.0\\nFN:Nested Agent Smith\\nTEL;TYPE=WORK:+1-555-0199\\nEND:VCARD\\n\r\n",
        // 6. Escaped inline nested vCard with escaped delimiters (\;, \,, \\)
        "AGENT:BEGIN:VCARD\\nFN:Agent\\; Special\\nNOTE:Handling commas\\, and backslashes\\\\\\nEND:VCARD\\n\r\n",
        // 7. Plain text name (informal exporter)
        "AGENT:John Q. Agent\\, Esq.\r\n",
        // 8. With vendor parameters
        "AGENT;X-JMAP-KEY=ag1;X-VENDOR-STATUS=ACTIVE:https://example.com/agent.vcf\r\n",
        // 9. Empty and whitespace-only AGENT lines
        "AGENT:\r\n",
        "AGENT:   \r\n",
        "AGENT;VALUE=uri:\r\n",
    ];

    for (idx, agent_line) in agent_permutations.iter().enumerate() {
        let vcard = format!(
            concat!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\n",
                "FN:Principal Contact\r\n",
                "N:Contact;Principal;;;\r\n",
                "EMAIL;TYPE=WORK;X-JMAP-KEY=e1:principal@example.com\r\n",
                "TEL;TYPE=WORK,VOICE;X-JMAP-KEY=p1:+1-555-0100\r\n",
                "ADR;TYPE=WORK;X-JMAP-KEY=a1:;;100 Executive Way;Capital;DC;20001;USA\r\n",
                "NOTE;X-JMAP-KEY=n1:Principal contact notes.\r\n",
                "{}",
                "CATEGORIES:VIP,Executive\r\n",
                "END:VCARD\r\n"
            ),
            agent_line
        );

        let parsed = vcard_to_card(&vcard).unwrap_or_else(|e| {
            panic!("[Case {idx}] failed to parse vCard with AGENT:\n{agent_line}\nError: {e:?}")
        });

        // 1. Verify standard fields are parsed completely and intact
        assert_eq!(
            parsed.name.as_ref().and_then(|n| n.full.as_deref()),
            Some("Principal Contact"),
            "[Case {idx}] full name intact"
        );
        let emails = parsed.emails.as_ref().expect("emails");
        assert_eq!(emails["e1"].address, "principal@example.com");
        let phones = parsed.phones.as_ref().expect("phones");
        assert_eq!(phones["p1"].number, "+1-555-0100");
        let addresses = parsed.addresses.as_ref().expect("addresses");
        assert_eq!(
            addresses["a1"]
                .components
                .as_ref()
                .unwrap()
                .iter()
                .find(|c| c.kind == "locality")
                .map(|c| c.value.as_str()),
            Some("Capital")
        );
        let notes = parsed.notes.as_ref().expect("notes");
        assert_eq!(notes["n1"].note, "Principal contact notes.");
        let keywords = parsed.keywords.as_ref().expect("keywords");
        assert!(keywords.contains_key("VIP") && keywords.contains_key("Executive"));

        // 2. Verify AGENT did not pollute extra or create spurious relations
        assert!(
            parsed.extra.is_empty(),
            "[Case {idx}] card.extra should be empty, got: {:?}",
            parsed.extra
        );
        assert!(
            parsed.related_to.is_none(),
            "[Case {idx}] related_to should be None for unmapped AGENT, got: {:?}",
            parsed.related_to
        );

        // 3. Outbound emission strictly omits AGENT
        let export1 = card_to_vcard(&parsed);
        assert!(
            !export1.contains("AGENT"),
            "[Case {idx}] Export₁ must not contain AGENT:\n{export1}"
        );
        assert!(
            export1.contains("FN:Principal Contact"),
            "[Case {idx}] Export₁ must contain FN"
        );
        assert!(
            export1.contains("principal@example.com"),
            "[Case {idx}] Export₁ must contain email"
        );

        // 4. Fixed-point stability across round-trips
        let card2 = vcard_to_card(&export1).expect("parse export1");
        let export2 = card_to_vcard(&card2);
        let card3 = vcard_to_card(&export2).expect("parse export2");
        let export3 = card_to_vcard(&card3);

        assert_eq!(
            export2, export3,
            "[Case {idx}] Export₂ == Export₃ fixpoint invariant"
        );
        assert_eq!(
            card2, card3,
            "[Case {idx}] Card₂ == Card₃ fixpoint invariant"
        );
    }
}

#[test]
fn sound_property_variations_audio_encodings_uris_and_fixpoint() {
    // RFC 2426 §3.6.3 / RFC 6350 §6.6.5 SOUND property: specifies digital audio
    // pronunciation or voice signature for the contact's name.
    //
    // SOUND can be:
    // 1. Inline base64 binary audio (TYPE=BASIC for 8-bit mu-law, TYPE=WAV, TYPE=MP3, TYPE=OGG)
    // 2. Remote URI (e.g. `SOUND;VALUE=uri:http://...` or `SOUND:http://...`)
    // 3. Local file URI (e.g. `SOUND;VALUE=uri:file:///sounds/name.au`)
    // 4. vCard 4.0 data URI (`SOUND:data:audio/ogg;base64,...`) or MEDIATYPE param
    // 5. vCard 2.1 syntax (`SOUND;WAVE;BASE64:...`)
    //
    // EDS has no E_CONTACT_SOUND and Evolution's contact editor provides no audio controls.
    // In JSContact (RFC 9553 §2.6.4), sounds reside in `media` with `kind: "sound"`.
    //
    // Contract:
    // - Inbound SOUND lines parse safely and do NOT corrupt surrounding properties.
    // - PHOTO (if present) is extracted cleanly without collision from SOUND.
    // - SOUND is never misparsed as a photo or stored in `card.media` or `card.extra`.
    // - Outbound vCard 3.0 strictly omits SOUND lines.
    // - Round-trips reach fixed-point stability (Export₂ == Export₃, Card₂ == Card₃).

    let sound_permutations = [
        // 1. Standard RFC 2426 BASIC (8-bit mu-law) inline binary
        "SOUND;TYPE=BASIC;ENCODING=b:AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHw==\r\n",
        // 2. Standard RFC 2426 WAV inline binary
        "SOUND;TYPE=WAV;ENCODING=b:UklGRi4AAABXQVZFZm10IBAAAAABAAEAQB8AAEAfAAABAAgAZGF0YQAAAAA=\r\n",
        // 3. MP3 inline binary audio
        "SOUND;TYPE=MP3;ENCODING=b:SUQzBAAAAAAAI1RTU0UAAAAPAAADTGF2ZjU4Ljc2LjEwMAAAAAAAAAAAAAAA\r\n",
        // 4. OGG inline binary audio
        "SOUND;TYPE=OGG;ENCODING=b:T2dnUwACAAAAAAAAAABAAAABAAAAAKs1N1E=\r\n",
        // 5. Case variations in parameter names
        "sound;type=basic;encoding=b:AQIDBA==\r\n",
        "Sound;Type=WAV;Encoding=B:UklGRg==\r\n",
        // 6. Remote HTTP/HTTPS URIs
        "SOUND;VALUE=uri:https://example.com/audio/pronunciation.wav\r\n",
        "SOUND:https://example.com/audio/pronunciation.mp3\r\n",
        // 7. Local file URI
        "SOUND;VALUE=uri:file:///usr/share/sounds/names/alice.au\r\n",
        // 8. vCard 4.0 data URI
        "SOUND:data:audio/ogg;base64,T2dnUwACAAAAAAAAAABAAAABAAAAAKs1N1E=\r\n",
        // 9. vCard 4.0 MEDIATYPE parameter
        "SOUND;MEDIATYPE=audio/ogg:https://example.com/sound.ogg\r\n",
        // 10. vCard 2.1 legacy syntax
        "SOUND;WAVE;BASE64:\r\n  UklGRgAAAA==\r\n",
        // 11. Multi-line 75-octet folded base64 payload
        concat!(
            "SOUND;TYPE=WAV;ENCODING=b:UklGRi4AAABXQVZFZm10IBAAAAABAAEAQB8AAEAfAAABAAg\r\n",
            " AZGF0YQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\r\n",
            " AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\r\n"
        ),
        // 12. Empty and whitespace-only SOUND lines
        "SOUND:\r\n",
        "SOUND:   \r\n",
        "SOUND;VALUE=uri:\r\n",
        "SOUND;ENCODING=b:\r\n",
    ];

    let photo_payload = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

    for (idx, sound_line) in sound_permutations.iter().enumerate() {
        let vcard = format!(
            concat!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\n",
                "FN:Alice Sound Speaker\r\n",
                "N:Speaker;Alice;;;\r\n",
                "EMAIL;TYPE=WORK;X-JMAP-KEY=e1:alice@example.com\r\n",
                "PHOTO;TYPE=PNG;ENCODING=b;X-JMAP-KEY=m1:{}\r\n",
                "{}",
                "NOTE;X-JMAP-KEY=n1:Has pronunciation audio guide.\r\n",
                "END:VCARD\r\n"
            ),
            photo_payload, sound_line
        );

        let parsed = vcard_to_card(&vcard).unwrap_or_else(|e| {
            panic!("[Case {idx}] failed to parse vCard with SOUND:\n{sound_line}\nError: {e:?}")
        });

        // 1. Verify FN, N, EMAIL, NOTE are parsed intact
        assert_eq!(
            parsed.name.as_ref().and_then(|n| n.full.as_deref()),
            Some("Alice Sound Speaker"),
            "[Case {idx}] full name intact"
        );
        let emails = parsed.emails.as_ref().expect("emails");
        assert_eq!(emails["e1"].address, "alice@example.com");
        let notes = parsed.notes.as_ref().expect("notes");
        assert_eq!(notes["n1"].note, "Has pronunciation audio guide.");

        // 2. Verify media contains ONLY the photo, not the sound
        let media = parsed.media.as_ref().expect("media");
        assert_eq!(
            media.len(),
            1,
            "[Case {idx}] media should contain only 1 photo entry, got: {media:?}"
        );
        let photo_entry = &media["m1"];
        assert_eq!(
            photo_entry.kind.as_deref(),
            Some("photo"),
            "[Case {idx}] media kind must be photo"
        );
        assert!(
            photo_entry.uri.contains(photo_payload)
                || photo_entry.uri.starts_with("data:image/png;base64,"),
            "[Case {idx}] photo uri must retain PNG image payload"
        );

        // 3. Verify card.extra is empty
        assert!(
            parsed.extra.is_empty(),
            "[Case {idx}] card.extra should be empty, got: {:?}",
            parsed.extra
        );

        // 4. Outbound emission contains PHOTO but strictly omits SOUND
        let export1 = card_to_vcard(&parsed);
        assert!(
            export1.contains("PHOTO"),
            "[Case {idx}] Export₁ must contain PHOTO:\n{export1}"
        );
        assert!(
            !export1.contains("SOUND"),
            "[Case {idx}] Export₁ must NOT contain SOUND:\n{export1}"
        );

        // 5. Fixed-point stability across round-trips
        let card2 = vcard_to_card(&export1).expect("parse export1");
        let export2 = card_to_vcard(&card2);
        let card3 = vcard_to_card(&export2).expect("parse export2");
        let export3 = card_to_vcard(&card3);

        assert_eq!(
            export2, export3,
            "[Case {idx}] Export₂ == Export₃ fixpoint invariant"
        );
        assert_eq!(
            card2, card3,
            "[Case {idx}] Card₂ == Card₃ fixpoint invariant"
        );
    }
}

#[test]
fn jscontact_agent_relations_and_sound_media_server_preservation() {
    // Tests server-side JSContact card carrying:
    // 1. `related_to` with relation type "agent" (RFC 9553 §2.1.8 & §2.7.2)
    // 2. `related_to` with other unmodeled relation types ("child", "colleague", "friend")
    // 3. `media` with `kind: "sound"` (RFC 9553 §2.6.4)
    // 4. `media` with `kind: "logo"`
    // 5. `media` with `kind: "photo"`
    // 6. `extra["cryptoKeys"]` (RFC 9553 §2.7.1)
    //
    // Invariants:
    // - `states_spouse`, `states_manager`, and `states_assistant` evaluate to false for "agent".
    // - `states_media` strictly evaluates to true ONLY for `kind: "photo"`.
    // - `card_to_vcard` emits ONLY PHOTO and mapped relations (X-EVOLUTION-SPOUSE, etc.),
    //   completely omitting AGENT, SOUND, LOGO, and cryptoKeys.
    // - During JMAP sync, `jmap-book-sync`'s `PatchObject` safely preserves untouched
    //   server-side `relatedTo` agent relations, `sound`/`logo` media entries, and `cryptoKeys`.

    let mut related_to = BTreeMap::new();
    // Mapped relations (have EDS slots)
    related_to.insert(
        "Eve Spouse".to_owned(),
        Relation {
            relation: Some([("spouse".to_owned(), serde_json::json!(true))].into()),
            extra: BTreeMap::new(),
        },
    );
    related_to.insert(
        "Bob Manager".to_owned(),
        Relation {
            relation: Some([("manager".to_owned(), serde_json::json!(true))].into()),
            extra: BTreeMap::new(),
        },
    );
    related_to.insert(
        "Carol Assistant".to_owned(),
        Relation {
            relation: Some([("assistant".to_owned(), serde_json::json!(true))].into()),
            extra: BTreeMap::new(),
        },
    );
    // Unmapped relations (no EDS slots — agent, child, colleague)
    related_to.insert(
        "David Agent".to_owned(),
        Relation {
            relation: Some([("agent".to_owned(), serde_json::json!(true))].into()),
            extra: BTreeMap::new(),
        },
    );
    related_to.insert(
        "Frank Child".to_owned(),
        Relation {
            relation: Some([("child".to_owned(), serde_json::json!(true))].into()),
            extra: BTreeMap::new(),
        },
    );
    related_to.insert(
        "Grace Colleague".to_owned(),
        Relation {
            relation: Some([("colleague".to_owned(), serde_json::json!(true))].into()),
            extra: BTreeMap::new(),
        },
    );

    let mut media = BTreeMap::new();
    let photo = Media {
        kind: Some("photo".to_owned()),
        uri: "data:image/jpeg;base64,/9j/4AAQSkZJRg==".to_owned(),
        media_type: Some("image/jpeg".to_owned()),
        extra: BTreeMap::new(),
    };
    let sound = Media {
        kind: Some("sound".to_owned()),
        uri: "https://example.com/pronounce.mp3".to_owned(),
        media_type: Some("audio/mpeg".to_owned()),
        extra: BTreeMap::new(),
    };
    let logo = Media {
        kind: Some("logo".to_owned()),
        uri: "https://example.com/corp_logo.png".to_owned(),
        media_type: Some("image/png".to_owned()),
        extra: BTreeMap::new(),
    };
    media.insert("m_photo".to_owned(), photo.clone());
    media.insert("m_sound".to_owned(), sound.clone());
    media.insert("m_logo".to_owned(), logo.clone());

    let mut extra = BTreeMap::new();
    extra.insert(
        "cryptoKeys".to_owned(),
        serde_json::json!({
            "k1": {
                "kind": "pgp",
                "uri": "https://keys.example.com/alice.asc"
            }
        }),
    );

    let card = ContactCard {
        id: Some("C-SERVER-PRESERVE".into()),
        name: Some(Name {
            full: Some("Alice Representative".into()),
            ..Name::default()
        }),
        related_to: Some(related_to),
        media: Some(media),
        extra,
        ..ContactCard::default()
    };

    // 1. Verify predicate evaluations
    assert!(states_media(&photo), "photo must be stateable on PHOTO");
    assert!(
        !states_media(&sound),
        "sound must not be stateable on PHOTO"
    );
    assert!(!states_media(&logo), "logo must not be stateable on PHOTO");

    let rel_agent = &card.related_to.as_ref().unwrap()["David Agent"];
    assert!(
        !states_spouse("David Agent", rel_agent),
        "agent is not a spouse"
    );
    assert!(
        !states_manager("David Agent", rel_agent),
        "agent is not a manager"
    );
    assert!(
        !states_assistant("David Agent", rel_agent),
        "agent is not an assistant"
    );

    // 2. Emit vCard 3.0
    let vcard = card_to_vcard(&card);

    // Mapped fields must be present
    assert!(vcard.contains("FN:Alice Representative"), "{vcard}");
    assert!(vcard.contains("X-EVOLUTION-SPOUSE:Eve Spouse"), "{vcard}");
    assert!(vcard.contains("X-EVOLUTION-MANAGER:Bob Manager"), "{vcard}");
    assert!(
        vcard.contains("X-EVOLUTION-ASSISTANT:Carol Assistant"),
        "{vcard}"
    );
    assert!(
        vcard.contains("PHOTO;X-JMAP-KEY=m_photo;TYPE=jpeg;ENCODING=b:"),
        "{vcard}"
    );

    // Unmapped fields must strictly be omitted from wire format
    assert!(
        !vcard.contains("AGENT"),
        "AGENT must not be emitted:\n{vcard}"
    );
    assert!(
        !vcard.contains("David Agent"),
        "David Agent must not be emitted:\n{vcard}"
    );
    assert!(
        !vcard.contains("Frank Child"),
        "Frank Child must not be emitted:\n{vcard}"
    );
    assert!(
        !vcard.contains("Grace Colleague"),
        "Grace Colleague must not be emitted:\n{vcard}"
    );
    assert!(
        !vcard.contains("SOUND"),
        "SOUND must not be emitted:\n{vcard}"
    );
    assert!(
        !vcard.contains("pronounce.mp3"),
        "sound URI must not be emitted:\n{vcard}"
    );
    assert!(
        !vcard.contains("LOGO"),
        "LOGO must not be emitted:\n{vcard}"
    );
    assert!(
        !vcard.contains("corp_logo.png"),
        "logo URI must not be emitted:\n{vcard}"
    );
    assert!(
        !vcard.contains("\r\nKEY"),
        "KEY must not be emitted:\n{vcard}"
    );
    assert!(
        !vcard.contains("cryptoKeys"),
        "cryptoKeys must not be emitted:\n{vcard}"
    );

    // 3. Multi-stage fixed-point roundtrip
    let card2 = vcard_to_card(&vcard).expect("parse emitted vcard");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃ fixpoint invariant");
    assert_eq!(card2, card3, "Card₂ == Card₃ fixpoint invariant");
}

#[test]
fn agent_and_sound_coexisting_in_full_master_card_roundtrip() {
    // Tests a master contact card containing the full spectrum of supported properties
    // coexisting with multiple AGENT and SOUND lines:
    // - Standard mapped fields: UID, FN, N, NICKNAME, EMAIL, TEL, ADR, ORG, TITLE, ROLE,
    //   NOTE, URL, BDAY, PHOTO, CATEGORIES, X-EVOLUTION-SPOUSE, X-EVOLUTION-MANAGER,
    //   X-EVOLUTION-ASSISTANT, X-EVOLUTION-FILE-AS, X-JABBER, X-SKYPE.
    // - Unmapped properties: AGENT (URI, nested vCard, plain text), SOUND (BASIC base64, WAV URI).
    //
    // Asserts 100% roundtrip fidelity of all mapped fields, clean omission of AGENT/SOUND,
    // and multi-pass fixed-point convergence.

    let vcard_input = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:master-agent-sound-001\r\n",
        "FN:Dr. Elena Rostova\r\n",
        "N:Rostova;Elena;Sergeevna;Dr.;Ph.D.\r\n",
        "NICKNAME:Lena,Elenochka\r\n",
        "X-EVOLUTION-FILE-AS:Rostova, Elena (Global Tech)\r\n",
        "EMAIL;TYPE=WORK,PREF;X-JMAP-KEY=e1:elena.rostova@globaltech.example.com\r\n",
        "EMAIL;TYPE=HOME;X-JMAP-KEY=e2:elena.personal@example.org\r\n",
        "TEL;TYPE=WORK,CELL,PREF;X-JMAP-KEY=p1:+1-555-0100\r\n",
        "TEL;TYPE=WORK,FAX;X-JMAP-KEY=p2:+1-555-0101\r\n",
        "TEL;TYPE=HOME,VOICE;X-JMAP-KEY=p3:+1-555-0102\r\n",
        "ADR;TYPE=WORK,PREF;X-JMAP-KEY=a1:Suite 400;Floor 4;100 Innovation Blvd;Tech City;CA;94016;USA\r\n",
        "LABEL;TYPE=WORK,PREF;X-JMAP-KEY=a1:100 Innovation Blvd, Suite 400\\nTech City, CA 94016\r\n",
        "ORG;X-JMAP-KEY=o1:Global Tech;Advanced Research;Quantum Optics\r\n",
        "TITLE;X-JMAP-KEY=t1:Chief Quantum Architect\r\n",
        "ROLE;X-JMAP-KEY=t2:Principal Investigator\r\n",
        "BDAY;X-JMAP-KEY=y1:1982-11-25\r\n",
        "X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=y2:2010-06-18\r\n",
        "URL;X-JMAP-KEY=l1:https://quantum.globaltech.example.com/rostova\r\n",
        "X-EVOLUTION-BLOG-URL;X-JMAP-KEY=l2:https://quantumthinking.blog/elena\r\n",
        "X-JABBER;TYPE=WORK;X-JMAP-KEY=s1:elena@jabber.globaltech.example.com\r\n",
        "X-SKYPE;TYPE=HOME;X-JMAP-KEY=s2:elena_rostova_skype\r\n",
        "X-EVOLUTION-SPOUSE:Mikhail Rostov\r\n",
        "X-EVOLUTION-MANAGER:Victor Vance\r\n",
        "X-EVOLUTION-ASSISTANT:Natalia Romanova\r\n",
        "NOTE;X-JMAP-KEY=n1:Leading the 2026 quantum photonics initiative.\r\n",
        "CATEGORIES:Research,Quantum,Executive\r\n",
        "PHOTO;TYPE=PNG;ENCODING=b;X-JMAP-KEY=m1:iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==\r\n",
        // AGENT lines (nested vCard and URI)
        "AGENT:BEGIN:VCARD\\nVERSION:3.0\\nFN:Alex Agent\\nTEL:+1-555-0999\\nEND:VCARD\\n\r\n",
        "AGENT;VALUE=uri:https://example.com/agents/rostova_rep.vcf\r\n",
        // SOUND lines (binary audio and URI)
        "SOUND;TYPE=BASIC;ENCODING=b:AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHw==\r\n",
        "SOUND;VALUE=uri:https://example.com/audio/rostova_pronunciation.wav\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(vcard_input).expect("parse master card with AGENT and SOUND");

    // 1. Verify structured name
    let name = parsed.name.as_ref().expect("name");
    assert_eq!(name.full.as_deref(), Some("Dr. Elena Rostova"));
    assert_eq!(
        name.extra.get("fileAs"),
        Some(&serde_json::json!("Rostova, Elena (Global Tech)"))
    );

    // 2. Verify telephony, emails, addresses
    assert_eq!(parsed.phones.as_ref().unwrap().len(), 3);
    assert_eq!(parsed.emails.as_ref().unwrap().len(), 2);
    assert_eq!(parsed.addresses.as_ref().unwrap().len(), 1);

    // 3. Verify organization and titles
    let org = &parsed.organizations.as_ref().unwrap()["o1"];
    assert_eq!(org.name.as_deref(), Some("Global Tech"));
    assert_eq!(org.units.as_ref().unwrap().len(), 2);
    assert_eq!(parsed.titles.as_ref().unwrap().len(), 2);

    // 4. Verify relations (Spouse, Manager, Assistant)
    let relations = parsed.related_to.as_ref().expect("relations");
    assert_eq!(relations.len(), 3);
    assert!(relations.contains_key("Mikhail Rostov"));
    assert!(relations.contains_key("Victor Vance"));
    assert!(relations.contains_key("Natalia Romanova"));
    assert!(
        !relations.contains_key("Alex Agent"),
        "AGENT must not create relation"
    );

    // 5. Verify media has only photo
    let media = parsed.media.as_ref().expect("media");
    assert_eq!(media.len(), 1);
    assert_eq!(media["m1"].kind.as_deref(), Some("photo"));

    // 6. Export and assert complete exclusion of AGENT and SOUND
    let export1 = card_to_vcard(&parsed);
    assert!(
        !export1.contains("AGENT"),
        "Export₁ must not contain AGENT:\n{export1}"
    );
    assert!(
        !export1.contains("SOUND"),
        "Export₁ must not contain SOUND:\n{export1}"
    );
    assert!(
        export1.contains("PHOTO;X-JMAP-KEY=m1;TYPE=PNG;ENCODING=b:")
            || export1.contains("PHOTO;X-JMAP-KEY=m1;TYPE=png;ENCODING=b:"),
        "Export₁ must retain PHOTO:\n{export1}"
    );
    assert!(
        export1.contains("X-EVOLUTION-SPOUSE:Mikhail Rostov"),
        "Export₁ must retain spouse"
    );

    // 7. Fixed-point multi-stage roundtrip
    let card2 = vcard_to_card(&export1).expect("parse export1");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃ fixpoint invariant");
    assert_eq!(card2, card3, "Card₂ == Card₃ fixpoint invariant");
}

fn read_fixture(file_name: &str) -> String {
    let path = format!("{}/tests/fixtures/{file_name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

struct RealExporterTestCase {
    name: &'static str,
    fixture_file: &'static str,
    exporter_name: &'static str,
    expected_full_name: &'static str,
    expected_surname: &'static str,
    expected_given_name: &'static str,
    expected_email_count: usize,
    expected_phone_count: usize,
    expected_address_count: usize,
    expected_org_name: Option<&'static str>,
    expected_org_units_count: usize,
    expected_title_count: usize,
    expected_anniversaries_count: usize,
    expected_relations_count: usize,
    expected_has_photo: bool,
    expected_categories_count: usize,
    unmapped_vendor_properties_dropped_on_export: &'static [&'static str],
}

#[test]
fn real_exporter_fixture_corpus_table_driven_roundtrip() {
    let corpus = [
        RealExporterTestCase {
            name: "Google Contacts Export (vCard 3.0 with Apple Group Labels)",
            fixture_file: "google_contacts_export.vcf",
            exporter_name: "Google Contacts",
            expected_full_name: "Dr. Jane Marie Doe",
            expected_surname: "Doe",
            expected_given_name: "Jane",
            expected_email_count: 2,
            expected_phone_count: 3,
            expected_address_count: 2,
            expected_org_name: Some("Alphabet Inc."),
            expected_org_units_count: 2,
            expected_title_count: 2,
            expected_anniversaries_count: 2, // BDAY + Anniversary
            expected_relations_count: 3,     // Spouse, Manager, Assistant
            expected_has_photo: true,
            expected_categories_count: 3,
            unmapped_vendor_properties_dropped_on_export: &[
                "X-GENDER",
                "X-PHONETIC-FIRST-NAME",
                "X-PHONETIC-LAST-NAME",
            ],
        },
        RealExporterTestCase {
            name: "Google Contacts Export (vCard 4.0 with Data URIs, IMPP & Related)",
            fixture_file: "google_contacts_vcard40_export.vcf",
            exporter_name: "Google Contacts (vCard 4.0)",
            expected_full_name: "Dr. Jane Marie Doe",
            expected_surname: "Doe",
            expected_given_name: "Jane",
            expected_email_count: 2,
            expected_phone_count: 3,
            expected_address_count: 2,
            expected_org_name: Some("Alphabet Inc."),
            expected_org_units_count: 2,
            expected_title_count: 2,
            expected_anniversaries_count: 2, // BDAY + Anniversary
            expected_relations_count: 3,     // Spouse, Manager, Assistant
            expected_has_photo: true,
            expected_categories_count: 3,
            unmapped_vendor_properties_dropped_on_export: &["GENDER", "CLIENTPIDMAP", "PRODID"],
        },
        RealExporterTestCase {
            name: "Apple iCloud & macOS Contacts Export (vCard 3.0 with X-ABLabel Groups)",
            fixture_file: "icloud_macos_export.vcf",
            exporter_name: "Apple iCloud / macOS Contacts",
            expected_full_name: "Prof. Alicia Katherine Vance",
            expected_surname: "Vance",
            expected_given_name: "Alicia",
            expected_email_count: 2,
            expected_phone_count: 4, // Mobile, Work, WorkFAX, Main
            expected_address_count: 2,
            expected_org_name: Some("MIT"),
            expected_org_units_count: 2,
            expected_title_count: 2,
            expected_anniversaries_count: 2, // BDAY + Anniversary
            expected_relations_count: 3,     // Spouse, Manager, Assistant
            expected_has_photo: true,
            expected_categories_count: 3,
            unmapped_vendor_properties_dropped_on_export: &["PRODID", "X-ABShowAs"],
        },
        RealExporterTestCase {
            name: "Microsoft Outlook Modern Export (vCard 3.0 with Design & Office Extensions)",
            fixture_file: "outlook_vcard30_export.vcf",
            exporter_name: "Microsoft Outlook 16.0 / M365",
            expected_full_name: "Mr. Erik Magnus Lindqvist",
            expected_surname: "Lindqvist",
            expected_given_name: "Erik",
            expected_email_count: 3,
            expected_phone_count: 4, // Work, Home, Cell, WorkFAX
            expected_address_count: 2,
            expected_org_name: Some("Nordic Solutions AB"),
            expected_org_units_count: 3, // Cloud Infrastructure, Platform Security, Executive Team
            expected_title_count: 2,
            expected_anniversaries_count: 1, // BDAY
            expected_relations_count: 0,
            expected_has_photo: false,
            expected_categories_count: 3,
            unmapped_vendor_properties_dropped_on_export: &[
                "X-MS-OL-DESIGN",
                "X-MS-CARDPICTURE",
                "X-MS-TEL-ASSISTANT",
                "X-MS-IMADDRESS",
                "PRODID",
            ],
        },
        RealExporterTestCase {
            name: "Microsoft Outlook Classic Export (vCard 2.1 with Quoted-Printable & Bare Types)",
            fixture_file: "outlook_vcard21_export.vcf",
            exporter_name: "Microsoft Outlook 2.1 / Legacy",
            expected_full_name: "Dr. Wolfgang Klaus Müller",
            expected_surname: "Müller",
            expected_given_name: "Wolfgang",
            expected_email_count: 2,
            expected_phone_count: 4, // Work, Home, Cell, WorkFAX
            expected_address_count: 2,
            expected_org_name: Some("Hanseatische Handelsgesellschaft mbH"),
            expected_org_units_count: 3,
            expected_title_count: 1,
            expected_anniversaries_count: 1, // BDAY
            expected_relations_count: 0,
            expected_has_photo: false,
            expected_categories_count: 0,
            unmapped_vendor_properties_dropped_on_export: &["ENCODING=QUOTED-PRINTABLE", "CHARSET"],
        },
        RealExporterTestCase {
            name: "Nextcloud & CardDAV Export (vCard 4.0 with Data URIs, IMPP & Related)",
            fixture_file: "nextcloud_carddav_export.vcf",
            exporter_name: "Nextcloud Contacts / SabreDAV",
            expected_full_name: "Dr. Camille Sylvie Laurent",
            expected_surname: "Laurent",
            expected_given_name: "Camille",
            expected_email_count: 2,
            expected_phone_count: 4, // Work, Home, Cell, WorkFAX
            expected_address_count: 2,
            expected_org_name: Some("INRIA Paris"),
            expected_org_units_count: 2,
            expected_title_count: 2,
            expected_anniversaries_count: 2, // BDAY + Anniversary
            expected_relations_count: 2,     // Spouse, Assistant
            expected_has_photo: true,
            expected_categories_count: 3,
            unmapped_vendor_properties_dropped_on_export: &["GENDER", "PRODID"],
        },
        RealExporterTestCase {
            name: "GNOME Evolution Native Export (vCard 3.0 with File-As & Slotted Attributes)",
            fixture_file: "evolution_native_export.vcf",
            exporter_name: "GNOME Evolution / EDS",
            expected_full_name: "Henri François Dubois",
            expected_surname: "Dubois",
            expected_given_name: "Henri",
            expected_email_count: 2,
            expected_phone_count: 4, // Work, Home, Cell, WorkFAX
            expected_address_count: 2,
            expected_org_name: Some("Aéronautique Spatiale SA"),
            expected_org_units_count: 3,
            expected_title_count: 2,
            expected_anniversaries_count: 2, // BDAY + Anniversary
            expected_relations_count: 3,     // Spouse, Manager, Assistant
            expected_has_photo: true,
            expected_categories_count: 3,
            unmapped_vendor_properties_dropped_on_export: &[],
        },
    ];

    for case in &corpus {
        assert!(!case.exporter_name.is_empty(), "Exporter name specified");
        let vcard_text = read_fixture(case.fixture_file);

        // 1. Inbound Parsing to JSContact / EContact model
        let card = vcard_to_card(&vcard_text).unwrap_or_else(|e| {
            panic!(
                "Failed to parse fixture {} ({}): {e}",
                case.fixture_file, case.exporter_name
            )
        });

        // 2. Validate Parsed Mapped Surface
        let name = card.name.as_ref().expect("name present");
        assert_eq!(
            name.full.as_deref(),
            Some(case.expected_full_name),
            "Full name mismatch for {} ({})",
            case.name,
            case.exporter_name
        );

        let surname_comp = name
            .components
            .as_ref()
            .and_then(|comps| comps.iter().find(|c| c.kind == "surname"));
        assert_eq!(
            surname_comp.map(|c| c.value.as_str()),
            Some(case.expected_surname),
            "Surname mismatch for {}",
            case.name
        );

        let given_comp = name
            .components
            .as_ref()
            .and_then(|comps| comps.iter().find(|c| c.kind == "given"));
        assert_eq!(
            given_comp.map(|c| c.value.as_str()),
            Some(case.expected_given_name),
            "Given name mismatch for {}",
            case.name
        );

        let emails = card.emails.as_ref().expect("emails map present");
        assert_eq!(
            emails.len(),
            case.expected_email_count,
            "Email count mismatch for {}",
            case.name
        );

        let phones = card.phones.as_ref().expect("phones map present");
        assert_eq!(
            phones.len(),
            case.expected_phone_count,
            "Phone count mismatch for {}",
            case.name
        );

        let addresses = card.addresses.as_ref().expect("addresses map present");
        assert_eq!(
            addresses.len(),
            case.expected_address_count,
            "Address count mismatch for {}",
            case.name
        );

        if let Some(expected_org) = case.expected_org_name {
            let orgs = card.organizations.as_ref().expect("organizations present");
            let primary_org = orgs.values().next().expect("primary org");
            assert_eq!(
                primary_org.name.as_deref(),
                Some(expected_org),
                "Org name mismatch for {}",
                case.name
            );
            assert_eq!(
                primary_org.units.as_ref().map(|u| u.len()).unwrap_or(0),
                case.expected_org_units_count,
                "Org units count mismatch for {}",
                case.name
            );
        }

        assert_eq!(
            card.titles.as_ref().map(|t| t.len()).unwrap_or(0),
            case.expected_title_count,
            "Title count mismatch for {}",
            case.name
        );

        assert_eq!(
            card.anniversaries.as_ref().map(|a| a.len()).unwrap_or(0),
            case.expected_anniversaries_count,
            "Anniversaries count mismatch for {}",
            case.name
        );

        assert_eq!(
            card.related_to.as_ref().map(|r| r.len()).unwrap_or(0),
            case.expected_relations_count,
            "Relations count mismatch for {}",
            case.name
        );

        if case.expected_has_photo {
            let media = card.media.as_ref().expect("media present");
            assert!(
                media.values().any(|m| m.kind.as_deref() == Some("photo")),
                "Expected photo missing for {}",
                case.name
            );
        }

        assert_eq!(
            card.keywords.as_ref().map(|k| k.len()).unwrap_or(0),
            case.expected_categories_count,
            "Categories count mismatch for {}",
            case.name
        );

        // 3. First Export (Export₁) to canonical RFC 2426 vCard 3.0
        let export1 = card_to_vcard(&card);
        assert!(
            export1.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"),
            "Export₁ must start with vCard 3.0 envelope for {}:\n{export1}",
            case.name
        );
        assert!(
            export1.ends_with("END:VCARD\r\n"),
            "Export₁ must end with END:VCARD for {}:\n{export1}",
            case.name
        );

        // Verify unmapped vendor properties are cleanly dropped
        for dropped_prop in case.unmapped_vendor_properties_dropped_on_export {
            assert!(
                !export1.contains(dropped_prop),
                "Export₁ must drop unmapped vendor property '{}' for {}:\n{export1}",
                dropped_prop,
                case.name
            );
        }

        // 4. Multi-Stage Round-Trip Fixpoint Execution
        let card2 = vcard_to_card(&export1)
            .unwrap_or_else(|e| panic!("Failed to parse Export₁ for {}: {e}", case.name));
        let export2 = card_to_vcard(&card2);
        let card3 = vcard_to_card(&export2)
            .unwrap_or_else(|e| panic!("Failed to parse Export₂ for {}: {e}", case.name));
        let export3 = card_to_vcard(&card3);

        // 5. Standing Fixpoint Invariants
        assert_eq!(
            export2, export3,
            "Export₂ == Export₃ fixpoint invariant violated for {}",
            case.name
        );
        assert_eq!(
            card2, card3,
            "Card₂ == Card₃ fixpoint invariant violated for {}",
            case.name
        );

        // 6. Lossless Preservation of Mapped Surface (card2 vs card3 and card vs card2)
        assert_eq!(
            card2.name, card3.name,
            "Name preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            card2.nicknames, card3.nicknames,
            "Nicknames preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            card2.organizations, card3.organizations,
            "Organizations preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            card2.titles, card3.titles,
            "Titles preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            card2.keywords, card3.keywords,
            "Keywords preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            card2.anniversaries, card3.anniversaries,
            "Anniversaries preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            card2.related_to, card3.related_to,
            "Relations preserved losslessly for {}",
            case.name
        );
        assert_eq!(
            card2.links, card3.links,
            "Links preserved losslessly for {}",
            case.name
        );
    }
}

#[test]
fn real_exporter_fixture_google_contacts_detailed_roundtrip() {
    let vcard_text = read_fixture("google_contacts_export.vcf");
    let card = vcard_to_card(&vcard_text).expect("parse Google Contacts fixture");

    // 1. Verify phonetic names and gender do not pollute extra
    assert!(
        card.extra.is_empty(),
        "card.extra must be clean, got: {:?}",
        card.extra
    );

    // 2. Verify Apple group labels resolved to native contexts
    let emails = card.emails.as_ref().unwrap();
    let work_email = emails
        .values()
        .find(|e| e.address == "jane.doe@research.example")
        .expect("work email");
    assert_eq!(
        work_email.contexts.as_ref().and_then(|c| c.get("work")),
        Some(&serde_json::json!(true))
    );

    let home_email = emails
        .values()
        .find(|e| e.address == "janedoe.personal@private.example")
        .expect("home email");
    assert_eq!(
        home_email.contexts.as_ref().and_then(|c| c.get("private")),
        Some(&serde_json::json!(true))
    );

    // 3. Verify telephony features
    let phones = card.phones.as_ref().unwrap();
    let cell_phone = phones
        .values()
        .find(|p| p.number == "+1 (650) 555-0142")
        .expect("cell phone");
    assert_eq!(
        cell_phone.features.as_ref().and_then(|f| f.get("mobile")),
        Some(&serde_json::json!(true))
    );

    let fax_phone = phones
        .values()
        .find(|p| p.number == "+1 (650) 555-0198")
        .expect("fax phone");
    assert_eq!(
        fax_phone.features.as_ref().and_then(|f| f.get("fax")),
        Some(&serde_json::json!(true))
    );
    assert_eq!(
        fax_phone.contexts.as_ref().and_then(|c| c.get("work")),
        Some(&serde_json::json!(true))
    );

    // 4. Verify relations
    let relations = card.related_to.as_ref().unwrap();
    assert!(states_spouse(
        "John Michael Doe",
        &relations["John Michael Doe"]
    ));
    assert!(states_manager(
        "Dr. Alan Turing",
        &relations["Dr. Alan Turing"]
    ));
    assert!(states_assistant("Sarah Connor", &relations["Sarah Connor"]));

    // 5. Verify multiline note
    let notes = card.notes.as_ref().unwrap();
    let note_text = &notes.values().next().unwrap().note;
    assert!(note_text.contains("NeurIPS 2024"));
    assert!(note_text.contains("transformer optimizations"));

    // 6. Export and fixpoint verification
    let export1 = card_to_vcard(&card);
    assert!(!export1.contains("X-GENDER"));
    assert!(!export1.contains("X-PHONETIC"));
    assert!(export1.contains("X-EVOLUTION-SPOUSE:John Michael Doe"));
    assert!(export1.contains("X-EVOLUTION-MANAGER:Dr. Alan Turing"));
    assert!(export1.contains("X-EVOLUTION-ASSISTANT:Sarah Connor"));

    let card2 = vcard_to_card(&export1).expect("parse export1");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃");
    assert_eq!(card2, card3, "Card₂ == Card₃");
}

#[test]
fn real_exporter_fixture_google_contacts_vcard40_detailed_roundtrip() {
    let vcard_text = read_fixture("google_contacts_vcard40_export.vcf");
    let card = vcard_to_card(&vcard_text).expect("parse Google Contacts vCard 4.0 fixture");

    // 1. Verify GENDER, CLIENTPIDMAP, PRODID, and REV do not pollute extra
    assert!(
        card.extra.is_empty(),
        "card.extra must be clean, got: {:?}",
        card.extra
    );

    // 2. Verify email properties and PREF rank
    let emails = card.emails.as_ref().unwrap();
    let work_email = emails
        .values()
        .find(|e| e.address == "jane.doe@research.example")
        .expect("work email");
    assert_eq!(
        work_email.contexts.as_ref().and_then(|c| c.get("work")),
        Some(&serde_json::json!(true))
    );
    assert_eq!(work_email.pref, Some(1));

    let home_email = emails
        .values()
        .find(|e| e.address == "janedoe.personal@private.example")
        .expect("home email");
    assert_eq!(
        home_email.contexts.as_ref().and_then(|c| c.get("private")),
        Some(&serde_json::json!(true))
    );

    // 3. Verify telephony features and quoted TYPE parameters
    let phones = card.phones.as_ref().unwrap();
    let work_phone = phones
        .values()
        .find(|p| p.number == "+1 (650) 555-0199")
        .expect("work phone");
    assert_eq!(
        work_phone.features.as_ref().and_then(|f| f.get("voice")),
        Some(&serde_json::json!(true))
    );
    assert_eq!(
        work_phone.contexts.as_ref().and_then(|c| c.get("work")),
        Some(&serde_json::json!(true))
    );
    assert_eq!(work_phone.pref, Some(1));

    let cell_phone = phones
        .values()
        .find(|p| p.number == "+1 (650) 555-0142")
        .expect("cell phone");
    assert_eq!(
        cell_phone.features.as_ref().and_then(|f| f.get("mobile")),
        Some(&serde_json::json!(true))
    );

    let fax_phone = phones
        .values()
        .find(|p| p.number == "+1 (650) 555-0198")
        .expect("fax phone");
    assert_eq!(
        fax_phone.features.as_ref().and_then(|f| f.get("fax")),
        Some(&serde_json::json!(true))
    );
    assert_eq!(
        fax_phone.contexts.as_ref().and_then(|c| c.get("work")),
        Some(&serde_json::json!(true))
    );

    // 4. Verify postal addresses and inline LABEL parameter extraction
    let addresses = card.addresses.as_ref().unwrap();
    let work_adr = addresses
        .values()
        .find(|a| a.contexts.as_ref().and_then(|c| c.get("work")) == Some(&serde_json::json!(true)))
        .expect("work adr");
    assert!(
        work_adr
            .full
            .as_ref()
            .unwrap()
            .contains("1600 Amphitheatre Pkwy")
    );
    assert_eq!(work_adr.extra.get("pref"), Some(&serde_json::json!(1)));

    // 5. Verify IMPP online services mapped to Jabber / Google Talk
    let services = card.online_services.as_ref().unwrap();
    let impp = services
        .values()
        .find(|s| s.user.as_deref() == Some("janedoe@google.example"))
        .expect("IMPP service");
    assert_eq!(impp.service.as_deref(), Some("Jabber"));

    // 6. Verify relations (Spouse, Manager, Assistant)
    let relations = card.related_to.as_ref().unwrap();
    assert!(states_spouse(
        "John Michael Doe",
        &relations["John Michael Doe"]
    ));
    assert!(states_manager(
        "Dr. Alan Turing",
        &relations["Dr. Alan Turing"]
    ));
    assert!(states_assistant("Sarah Connor", &relations["Sarah Connor"]));

    // 7. Verify hyphenated anniversary dates
    let anniversaries = card.anniversaries.as_ref().unwrap();
    let bday = anniversaries
        .values()
        .find(|a| a.kind == "birth")
        .expect("birthday");
    assert_eq!(
        bday.date,
        Some(serde_json::json!({"@type": "PartialDate", "year": 1982, "month": 7, "day": 15}))
    );

    let anniversary = anniversaries
        .values()
        .find(|a| a.kind == "wedding")
        .expect("anniversary");
    assert_eq!(
        anniversary.date,
        Some(serde_json::json!({"@type": "PartialDate", "year": 2010, "month": 9, "day": 18}))
    );

    // 8. Verify inline data URI photo
    let media = card.media.as_ref().unwrap();
    let photo = media
        .values()
        .find(|m| m.kind.as_deref() == Some("photo"))
        .expect("photo");
    assert_eq!(photo.media_type.as_deref(), Some("image/jpeg"));
    assert!(photo.uri.starts_with("data:image/jpeg;base64,"));

    // 9. Export to canonical vCard 3.0 and verify field normalizations
    let export1 = card_to_vcard(&card);
    assert!(export1.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"));
    assert!(!export1.contains("GENDER"));
    assert!(!export1.contains("CLIENTPIDMAP"));
    assert!(!export1.contains("PRODID"));
    assert!(export1.contains("X-EVOLUTION-SPOUSE:John Michael Doe"));
    assert!(export1.contains("X-EVOLUTION-MANAGER:Dr. Alan Turing"));
    assert!(export1.contains("X-EVOLUTION-ASSISTANT:Sarah Connor"));
    assert!(export1.contains("PHOTO;"));
    assert!(export1.contains("ENCODING=b"));

    // 10. Multi-stage roundtrip fixpoint execution
    let card2 = vcard_to_card(&export1).expect("parse export1");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃");
    assert_eq!(card2, card3, "Card₂ == Card₃");
}

#[test]
fn real_exporter_fixture_apple_icloud_macos_detailed_roundtrip() {
    let vcard_text = read_fixture("icloud_macos_export.vcf");
    let card = vcard_to_card(&vcard_text).expect("parse Apple iCloud fixture");

    // 1. Verify MAIN phone mapped to voice + work
    let phones = card.phones.as_ref().unwrap();
    let main_phone = phones
        .values()
        .find(|p| p.number == "+1 (555) 999-0000")
        .expect("main phone");
    assert_eq!(
        main_phone.features.as_ref().and_then(|f| f.get("voice")),
        Some(&serde_json::json!(true))
    );
    assert_eq!(
        main_phone.contexts.as_ref().and_then(|c| c.get("work")),
        Some(&serde_json::json!(true))
    );

    // 2. Verify structured address with escaped comma
    let addresses = card.addresses.as_ref().unwrap();
    let work_adr = addresses
        .values()
        .find(|a| a.contexts.as_ref().and_then(|c| c.get("work")) == Some(&serde_json::json!(true)))
        .expect("work adr");
    let street = work_adr
        .components
        .as_ref()
        .unwrap()
        .iter()
        .find(|c| c.kind == "name")
        .unwrap();
    assert_eq!(street.value, "500 Science Drive, Suite 3B");

    // 3. Verify relations & anniversary
    let relations = card.related_to.as_ref().unwrap();
    assert!(states_spouse("Robert Vance", &relations["Robert Vance"]));
    assert!(states_manager(
        "Dean Marcus Cole",
        &relations["Dean Marcus Cole"]
    ));
    assert!(states_assistant(
        "Elena Rostova",
        &relations["Elena Rostova"]
    ));

    let anniversaries = card.anniversaries.as_ref().unwrap();
    let wedding = anniversaries
        .values()
        .find(|a| a.kind == "wedding")
        .expect("wedding anniversary");
    assert_eq!(anniversary_date(wedding), Some("2004-06-20".into()));

    // 4. Export and fixpoint verification
    let export1 = card_to_vcard(&card);
    assert!(!export1.contains("PRODID"));
    assert!(!export1.contains("X-ABShowAs"));

    let card2 = vcard_to_card(&export1).expect("parse export1");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃");
    assert_eq!(card2, card3, "Card₂ == Card₃");
}

#[test]
fn real_exporter_fixture_outlook_modern_vcard30_detailed_roundtrip() {
    let vcard_text = read_fixture("outlook_vcard30_export.vcf");
    let card = vcard_to_card(&vcard_text).expect("parse Outlook 3.0 fixture");

    // 1. Verify 4-unit ORG hierarchy (Company, Unit 1, Unit 2, Office)
    let orgs = card.organizations.as_ref().unwrap();
    let org = orgs.values().next().unwrap();
    assert_eq!(org.name.as_deref(), Some("Nordic Solutions AB"));
    let units = org.units.as_ref().unwrap();
    assert_eq!(units.len(), 3);
    assert_eq!(units[0].name, "Cloud Infrastructure");
    assert_eq!(units[1].name, "Platform Security");
    assert_eq!(units[2].name, "Executive Team");

    // 2. Verify postal addresses and labels
    let addresses = card.addresses.as_ref().unwrap();
    let work_adr = addresses
        .values()
        .find(|a| a.contexts.as_ref().and_then(|c| c.get("work")) == Some(&serde_json::json!(true)))
        .expect("work adr");
    assert!(
        work_adr
            .full
            .as_ref()
            .unwrap()
            .contains("Stureplan 4, 5 tr")
    );

    // 3. Export and ensure X-MS-* extensions are cleanly dropped
    let export1 = card_to_vcard(&card);
    assert!(!export1.contains("X-MS-OL-DESIGN"));
    assert!(!export1.contains("X-MS-CARDPICTURE"));
    assert!(!export1.contains("X-MS-TEL-ASSISTANT"));
    assert!(!export1.contains("X-MS-IMADDRESS"));

    let card2 = vcard_to_card(&export1).expect("parse export1");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃");
    assert_eq!(card2, card3, "Card₂ == Card₃");
}

#[test]
fn real_exporter_fixture_outlook_classic_vcard21_detailed_roundtrip() {
    let vcard_text = read_fixture("outlook_vcard21_export.vcf");
    let card = vcard_to_card(&vcard_text).expect("parse Outlook 2.1 fixture");

    // 1. Verify German umlauts decoded from QP
    let name = card.name.as_ref().unwrap();
    assert_eq!(name.full.as_deref(), Some("Dr. Wolfgang Klaus Müller"));

    let notes = card.notes.as_ref().unwrap();
    let note_text = &notes.values().next().unwrap().note;
    assert!(note_text.contains("Großkunde für Logistiklösungen"));
    assert!(note_text.contains("telefonische Kontaktaufnahme"));

    // 2. Verify QP address decoded
    let addresses = card.addresses.as_ref().unwrap();
    let work_adr = addresses
        .values()
        .find(|a| a.contexts.as_ref().and_then(|c| c.get("work")) == Some(&serde_json::json!(true)))
        .expect("work adr");
    assert!(work_adr.full.as_ref().unwrap().contains("Gebäude B"));

    // 3. Export to strict vCard 3.0 UTF-8
    let export1 = card_to_vcard(&card);
    assert!(export1.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"));
    assert!(!export1.contains("QUOTED-PRINTABLE"));
    assert!(!export1.contains("CHARSET"));

    let card2 = vcard_to_card(&export1).expect("parse export1");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃");
    assert_eq!(card2, card3, "Card₂ == Card₃");
}

#[test]
fn real_exporter_fixture_nextcloud_carddav_vcard40_detailed_roundtrip() {
    let vcard_text = read_fixture("nextcloud_carddav_export.vcf");
    let card = vcard_to_card(&vcard_text).expect("parse Nextcloud fixture");

    // 1. Verify data URI photo
    let media = card.media.as_ref().unwrap();
    let photo = media
        .values()
        .find(|m| m.kind.as_deref() == Some("photo"))
        .unwrap();
    assert_eq!(photo.media_type.as_deref(), Some("image/jpeg"));
    assert!(photo.uri.starts_with("data:image/jpeg;base64,"));

    // 2. Verify IMPP online services
    let services = card.online_services.as_ref().unwrap();
    assert!(
        services
            .values()
            .any(|s| s.service.as_deref() == Some("Matrix"))
    );
    assert!(
        services
            .values()
            .any(|s| s.service.as_deref() == Some("Jabber"))
    );

    // 3. Export to canonical vCard 3.0
    let export1 = card_to_vcard(&card);
    assert!(export1.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"));
    assert!(!export1.contains("GENDER"));
    assert!(!export1.contains("PRODID"));

    let card2 = vcard_to_card(&export1).expect("parse export1");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃");
    assert_eq!(card2, card3, "Card₂ == Card₃");
}

#[test]
fn real_exporter_fixture_evolution_native_vcard30_detailed_roundtrip() {
    let vcard_text = read_fixture("evolution_native_export.vcf");
    let card = vcard_to_card(&vcard_text).expect("parse Evolution fixture");

    // 1. Verify File-As
    let name = card.name.as_ref().unwrap();
    assert_eq!(
        name.extra.get("fileAs"),
        Some(&serde_json::json!("Dubois, Henri (Aéronautique)"))
    );

    // 2. Verify EDS relations, blogs, videos, and slotted IMs
    let relations = card.related_to.as_ref().unwrap();
    assert!(states_spouse(
        "Marie-Claire Dubois",
        &relations["Marie-Claire Dubois"]
    ));
    assert!(states_manager(
        "Philippe Martin",
        &relations["Philippe Martin"]
    ));
    assert!(states_assistant(
        "Corinne Petit",
        &relations["Corinne Petit"]
    ));

    let links = card.links.as_ref().unwrap();
    assert!(links.values().any(|l| l.kind.as_deref() == Some("blog")));
    assert!(links.values().any(|l| l.kind.as_deref() == Some("video")));

    let services = card.online_services.as_ref().unwrap();
    assert!(
        services
            .values()
            .any(|s| s.service.as_deref() == Some("Jabber"))
    );
    assert!(
        services
            .values()
            .any(|s| s.service.as_deref() == Some("Matrix"))
    );

    // 3. Export and assert complete retention
    let export1 = card_to_vcard(&card);
    let unfolded_export1 = unfolded(&export1);
    assert!(unfolded_export1.contains("X-EVOLUTION-FILE-AS:Dubois\\, Henri (Aéronautique)"));
    assert!(unfolded_export1.contains("X-EVOLUTION-SPOUSE:Marie-Claire Dubois"));
    assert!(unfolded_export1.contains("X-EVOLUTION-MANAGER:Philippe Martin"));
    assert!(unfolded_export1.contains("X-EVOLUTION-ASSISTANT:Corinne Petit"));
    assert!(
        unfolded_export1.contains("X-EVOLUTION-BLOG-URL")
            && unfolded_export1.contains("https://blog.henridubois.example")
    );
    assert!(
        unfolded_export1.contains("X-EVOLUTION-VIDEO-URL")
            && unfolded_export1.contains("https://video.aerospatiale.example/u/hdubois")
    );

    let card2 = vcard_to_card(&export1).expect("parse export1");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃");
    assert_eq!(card2, card3, "Card₂ == Card₃");
}

#[test]
fn hyphenated_dates_bday_and_anniversary_variations_fidelity() {
    // 1. Standard BDAY extended format (RFC 2426 §3.1.5)
    let vcard_bday_extended = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Alice Date\r\n",
        "BDAY:1985-04-12\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(vcard_bday_extended).expect("parse BDAY extended");
    let anniversaries = card.anniversaries.expect("anniversaries");
    let bday = anniversaries
        .values()
        .find(|a| a.kind == "birth")
        .expect("bday");
    assert_eq!(
        bday.date,
        Some(json!({"@type": "PartialDate", "year": 1985, "month": 4, "day": 12}))
    );

    // 2. BDAY with explicit VALUE=date parameter
    let vcard_bday_value_date = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Alice Date\r\n",
        "BDAY;VALUE=date:1985-04-12\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(vcard_bday_value_date).expect("parse BDAY VALUE=date");
    let anniversaries = card.anniversaries.expect("anniversaries");
    let bday = anniversaries
        .values()
        .find(|a| a.kind == "birth")
        .expect("bday");
    assert_eq!(
        bday.date,
        Some(json!({"@type": "PartialDate", "year": 1985, "month": 4, "day": 12}))
    );

    // 3. BDAY with VALUE=DATE uppercase and lowercase
    let vcard_bday_value_date_lower = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Alice Date\r\n",
        "BDAY;value=DATE:1985-04-12\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(vcard_bday_value_date_lower).expect("parse BDAY value=DATE");
    let anniversaries = card.anniversaries.expect("anniversaries");
    let bday = anniversaries
        .values()
        .find(|a| a.kind == "birth")
        .expect("bday");
    assert_eq!(
        bday.date,
        Some(json!({"@type": "PartialDate", "year": 1985, "month": 4, "day": 12}))
    );

    // 4. BDAY with VALUE=date-and-or-time
    let vcard_bday_date_and_or_time = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:Alice Date\r\n",
        "BDAY;VALUE=date-and-or-time:1985-04-12\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(vcard_bday_date_and_or_time).expect("parse BDAY date-and-or-time");
    let anniversaries = card.anniversaries.expect("anniversaries");
    let bday = anniversaries
        .values()
        .find(|a| a.kind == "birth")
        .expect("bday");
    assert_eq!(
        bday.date,
        Some(json!({"@type": "PartialDate", "year": 1985, "month": 4, "day": 12}))
    );

    // 5. BDAY with extended ISO 8601 timestamps (UTC and timezone offsets)
    let vcard_bday_utc = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Alice Date\r\n",
        "BDAY:1985-04-12T10:30:00Z\r\n",
        "END:VCARD\r\n"
    );
    let card_utc = vcard_to_card(vcard_bday_utc).expect("parse BDAY UTC");
    let anniversaries = card_utc.anniversaries.expect("anniversaries");
    let bday = anniversaries
        .values()
        .find(|a| a.kind == "birth")
        .expect("bday");
    assert_eq!(
        bday.date,
        Some(json!({"@type": "PartialDate", "year": 1985, "month": 4, "day": 12}))
    );

    let vcard_bday_tz_plus = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Alice Date\r\n",
        "BDAY:1985-04-12T10:30:00+02:00\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(vcard_bday_tz_plus).expect("parse BDAY TZ +02:00");
    let anniversaries = card.anniversaries.expect("anniversaries");
    let bday = anniversaries
        .values()
        .find(|a| a.kind == "birth")
        .expect("bday");
    assert_eq!(
        bday.date,
        Some(json!({"@type": "PartialDate", "year": 1985, "month": 4, "day": 12}))
    );

    let vcard_bday_tz_minus = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Alice Date\r\n",
        "BDAY:1985-04-12T10:30:00-05:00\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(vcard_bday_tz_minus).expect("parse BDAY TZ -05:00");
    let anniversaries = card.anniversaries.expect("anniversaries");
    let bday = anniversaries
        .values()
        .find(|a| a.kind == "birth")
        .expect("bday");
    assert_eq!(
        bday.date,
        Some(json!({"@type": "PartialDate", "year": 1985, "month": 4, "day": 12}))
    );

    // 6. Grouped property item1.BDAY
    let vcard_bday_grouped = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Alice Date\r\n",
        "item1.BDAY;VALUE=date:1985-04-12\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(vcard_bday_grouped).expect("parse item1.BDAY");
    let anniversaries = card.anniversaries.expect("anniversaries");
    let bday = anniversaries
        .values()
        .find(|a| a.kind == "birth")
        .expect("bday");
    assert_eq!(
        bday.date,
        Some(json!({"@type": "PartialDate", "year": 1985, "month": 4, "day": 12}))
    );

    // 7. ANNIVERSARY extended format (RFC 6350 §6.2.6)
    let vcard_anniv_extended = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:Alice Date\r\n",
        "ANNIVERSARY:2015-09-20\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(vcard_anniv_extended).expect("parse ANNIVERSARY extended");
    let anniversaries = card.anniversaries.expect("anniversaries");
    let anniv = anniversaries
        .values()
        .find(|a| a.kind == "wedding")
        .expect("anniv");
    assert_eq!(
        anniv.date,
        Some(json!({"@type": "PartialDate", "year": 2015, "month": 9, "day": 20}))
    );

    // 8. ANNIVERSARY with explicit VALUE=date parameter
    let vcard_anniv_value_date = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:Alice Date\r\n",
        "ANNIVERSARY;VALUE=date:2015-09-20\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(vcard_anniv_value_date).expect("parse ANNIVERSARY VALUE=date");
    let anniversaries = card.anniversaries.expect("anniversaries");
    let anniv = anniversaries
        .values()
        .find(|a| a.kind == "wedding")
        .expect("anniv");
    assert_eq!(
        anniv.date,
        Some(json!({"@type": "PartialDate", "year": 2015, "month": 9, "day": 20}))
    );

    // 9. ANNIVERSARY with timestamp
    let vcard_anniv_timestamp = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:Alice Date\r\n",
        "ANNIVERSARY:2015-09-20T14:00:00Z\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(vcard_anniv_timestamp).expect("parse ANNIVERSARY timestamp");
    let anniversaries = card.anniversaries.expect("anniversaries");
    let anniv = anniversaries
        .values()
        .find(|a| a.kind == "wedding")
        .expect("anniv");
    assert_eq!(
        anniv.date,
        Some(json!({"@type": "PartialDate", "year": 2015, "month": 9, "day": 20}))
    );

    // 10. Coexisting BDAY, ANNIVERSARY, X-EVOLUTION-ANNIVERSARY, and Apple X-ABDATE in one card
    let vcard_all_dates = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Alice Date\r\n",
        "BDAY;VALUE=date:1985-04-12\r\n",
        "X-EVOLUTION-ANNIVERSARY:2010-06-15\r\n",
        "item1.X-ABDATE:2018-11-22\r\n",
        "item1.X-ABLabel:_$!<Anniversary>!$_\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(vcard_all_dates).expect("parse all dates");
    let anniversaries = card.anniversaries.as_ref().expect("anniversaries");
    assert_eq!(anniversaries.len(), 3);
    let bday = anniversaries
        .values()
        .find(|a| a.kind == "birth")
        .expect("birth");
    assert_eq!(
        bday.date,
        Some(json!({"@type": "PartialDate", "year": 1985, "month": 4, "day": 12}))
    );
    let wedding_dates: Vec<_> = anniversaries
        .values()
        .filter(|a| a.kind == "wedding")
        .filter_map(|a| a.date.as_ref())
        .collect();
    assert_eq!(wedding_dates.len(), 2);
    assert!(
        wedding_dates
            .iter()
            .any(|d| d["year"] == 2010 && d["month"] == 6 && d["day"] == 15)
    );
    assert!(
        wedding_dates
            .iter()
            .any(|d| d["year"] == 2018 && d["month"] == 11 && d["day"] == 22)
    );

    // 11. Multi-pass fixed-point stability
    let export1 = card_to_vcard(&card);
    let card2 = vcard_to_card(&export1).expect("parse export1");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃");
    assert_eq!(card2, card3, "Card₂ == Card₃");
}
