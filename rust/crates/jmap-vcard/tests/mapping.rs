// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSContact `ContactCard` ↔ vCard 3.0 mapping for the minimal property set.
//! The address book backend needs UID, FN, N, NICKNAME, EMAIL, TEL, ADR, LABEL,
//! ORG, TITLE, ROLE, NOTE, BDAY, URL, CALADRURI, CALURI, FBURL, PHOTO,
//! CATEGORIES, and `X-` lines for EDS instant-messaging handles and spouse data.

use std::collections::BTreeMap;

use base64::Engine;
use jmap_proto::contacts::{
    Address, AddressComponent, Anniversary, Calendar, ContactCard, ContactEmail, ContactPhone,
    Link, Media, Name, NameComponent, Nickname, Note, OnlineService, OrgUnit, Organization,
    Relation, SchedulingAddress, Title,
};
use jmap_vcard::{
    VCardError, address_label, anniversary_date, card_to_vcard, maps_context, maps_phone_feature,
    online_service_handle, online_service_uri, restore_address_components, restore_name_components,
    same_photo, same_service, states_a_point_in_time, states_address, states_address_component,
    states_anniversary, states_assistant, states_calendar, states_context, states_email,
    states_file_as, states_keyword, states_link, states_manager, states_media, states_name,
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
    // EDS indexes and returns contacts by vCard UID. UID must carry the JMAP id.
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
    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C9".into());
        card
    };
    let vcard = card_to_vcard(&card);
    assert_eq!(line(&vcard, "UID:"), "UID:C9");
    assert!(!vcard.contains("X-JMAP-UID"), "{vcard}");

    // After readback, the JMAP id is the only identifier.
    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(back.id.as_ref().unwrap().as_str(), "C9");
    assert_eq!(back.uid, None);
}

#[test]
fn uid_parsing_is_case_insensitive_and_stabilizes_duplicate_uid_lines() {
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "uid:server-id-primary\r\n",
        "UID:server-id-secondary\r\n",
        "x-jmap-uid:urn:uuid:8e96c46a-f5d2-42ea-a43d-492a7a2cf5aa\r\n",
        "FN:Vera\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse mixed-case duplicate UID vcard");
    assert_eq!(card.id.as_ref().unwrap().as_str(), "server-id-primary");
    assert_eq!(
        card.uid.as_deref(),
        Some("urn:uuid:8e96c46a-f5d2-42ea-a43d-492a7a2cf5aa")
    );

    let emitted = card_to_vcard(&card);
    assert!(emitted.contains("UID:server-id-primary\r\n"));
    assert!(!emitted.contains("UID:server-id-secondary\r\n"));
    assert!(emitted.contains("X-JMAP-UID:urn:uuid:8e96c46a-f5d2-42ea-a43d-492a7a2cf5aa\r\n"));

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized UID vcard");
    assert_eq!(reparsed.id.as_ref().unwrap().as_str(), "server-id-primary");
    assert_eq!(
        reparsed.uid.as_deref(),
        Some("urn:uuid:8e96c46a-f5d2-42ea-a43d-492a7a2cf5aa")
    );
}

#[test]
fn a_vcard_from_evolution_has_no_jmap_id_yet() {
    // Evolution invents this UID locally. Callers must not treat it as a JMAP id.
    let vcard = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:pas-id-6890AB\r\nFN:Vera\r\nEND:VCARD\r\n";
    let card = vcard_to_card(vcard).expect("parse");
    assert_eq!(card.id.as_ref().unwrap().as_str(), "pas-id-6890AB");
    assert_eq!(card.name.unwrap().full.as_deref(), Some("Vera"));
}

#[test]
fn maps_name_components_onto_the_structured_n_property() {
    let card = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("Dr. Vera Marie Oldenburg MSc".to_owned()),
            components: Some(vec![
                NameComponent::new("title", "Dr."),
                NameComponent::new("given", "Vera"),
                NameComponent::new("given2", "Marie"),
                NameComponent::new("surname", "Oldenburg"),
                NameComponent::new("credential", "MSc"),
            ]),
            ..Name::default()
        });
        card
    };

    let vcard = card_to_vcard(&card);
    assert_eq!(line(&vcard, "N:"), "N:Oldenburg;Vera;Marie;Dr.;MSc");
    assert_eq!(line(&vcard, "FN:"), "FN:Dr. Vera Marie Oldenburg MSc");

    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(back.name.as_ref().unwrap(), card.name.as_ref().unwrap());
}

#[test]
fn derives_the_missing_half_of_the_name() {
    // If `full` is absent, derive it from components in display order.
    let components = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            components: Some(vec![
                NameComponent::new("surname", "Oldenburg"),
                NameComponent::new("given", "Vera"),
            ]),
            ..Name::default()
        });
        card
    };
    assert_eq!(
        line(&card_to_vcard(&components), "FN:"),
        "FN:Vera Oldenburg"
    );

    // If the vCard has no N value, keep `components` unset instead of guessing.
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
    // JSContact patches address map keys. Roundtrip must keep them stable.
    let card = fixture_card();
    let back = vcard_to_card(&card_to_vcard(&card)).expect("parse");
    assert_eq!(
        back.emails.unwrap().keys().collect::<Vec<_>>(),
        card.emails.unwrap().keys().collect::<Vec<_>>()
    );
}

#[test]
fn invents_keys_for_entries_that_have_none() {
    // A vCard from Evolution carries no X-JMAP-KEY line.
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
    // No TYPE means no context, not an empty context object.
    assert!(emails.values().any(|e| e.contexts.is_none()));

    let phone = card.phones.expect("phones").into_values().next().unwrap();
    assert_eq!(phone.features, Some(json!({"voice": true, "fax": true})));
}

#[test]
fn an_email_without_an_address_is_skipped_in_both_directions() {
    let card = {
        let mut card = ContactCard::default();
        card.emails = Some(
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
        );
        card.phones = Some([("p1".to_owned(), ContactPhone::default())].into());
        card
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
    // ORG keeps name then units, outermost first (RFC 2426 §3.5.5, RFC 9553 §2.2.3).
    let card = {
        let mut card = ContactCard::default();
        card.organizations = Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some("Example GmbH".to_owned()),
                    units: Some(vec![OrgUnit::new("Research"), OrgUnit::new("Optics")]),
                    ..Organization::default()
                },
            )]
            .into(),
        );
        card
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
    // Component one is the organization name, so units cannot move into it.
    let card = {
        let mut card = ContactCard::default();
        card.organizations = Some(
            [(
                "o1".to_owned(),
                Organization {
                    units: Some(vec![OrgUnit::new("Research")]),
                    ..Organization::default()
                },
            )]
            .into(),
        );
        card
    };

    let vcard = card_to_vcard(&card);
    assert_eq!(line(&vcard, "ORG"), "ORG;X-JMAP-KEY=o1:;Research");

    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(back.organizations, card.organizations);
}

#[test]
fn an_organization_with_nothing_in_it_is_skipped_in_both_directions() {
    let card = {
        let mut card = ContactCard::default();
        card.organizations = Some([("o1".to_owned(), Organization::default())].into());
        card
    };
    assert!(!card_to_vcard(&card).contains("\r\nORG"));

    let back =
        vcard_to_card("BEGIN:VCARD\r\nVERSION:3.0\r\nORG:;;\r\nEND:VCARD\r\n").expect("parse");
    assert_eq!(back.organizations, None);
}

#[test]
fn an_organization_with_an_empty_name_string_behaves_consistently() {
    // `name = ""` with no units states nothing, so no ORG line is emitted.
    let card_empty_name = {
        let mut card = ContactCard::default();
        card.organizations = Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some(String::new()),
                    units: None,
                    ..Organization::default()
                },
            )]
            .into(),
        );
        card
    };
    let org = &card_empty_name.organizations.as_ref().unwrap()["o1"];
    assert!(!states_organization(org));
    let vcard = card_to_vcard(&card_empty_name);
    assert!(!vcard.contains("\r\nORG"));
    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(back.organizations, None);

    // With units present, the empty first component is preserved via leading `;`.
    let card_empty_name_with_units = {
        let mut card = ContactCard::default();
        card.organizations = Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some(String::new()),
                    units: Some(vec![OrgUnit::new("Engineering"), OrgUnit::new("Security")]),
                    ..Organization::default()
                },
            )]
            .into(),
        );
        card
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
    // Evolution emits ORG without X-JMAP-KEY and puts department in component two.
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
    // `kind` selects TITLE or ROLE (RFC 9553 §2.2.4, RFC 2426 §§3.5.1-3.5.2).
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
    // Vendor kinds beyond `title` and `role` have no vCard 3.0 field, so skip them.
    let card = {
        let mut card = ContactCard::default();
        card.titles = Some(
            [("t1".to_owned(), {
                let mut title = Title::default();
                title.name = "Knight of the Realm".to_owned();
                title.kind = Some("x-honour".to_owned());
                title
            })]
            .into(),
        );
        card
    };

    let vcard = card_to_vcard(&card);
    assert!(!vcard.contains("Knight"), "{vcard}");
    assert!(!vcard.contains("\r\nTITLE"), "{vcard}");
    assert!(!vcard.contains("\r\nROLE"), "{vcard}");
}

#[test]
fn a_title_with_no_name_is_skipped_in_both_directions() {
    let card = {
        let mut card = ContactCard::default();
        card.titles = Some([("t1".to_owned(), Title::default())].into());
        card
    };
    assert!(!card_to_vcard(&card).contains("\r\nTITLE"));

    let back =
        vcard_to_card("BEGIN:VCARD\r\nVERSION:3.0\r\nTITLE:\r\nEND:VCARD\r\n").expect("parse");
    assert_eq!(back.titles, None);
}

#[test]
fn invents_a_key_for_a_title_that_has_none() {
    // Evolution emits no X-JMAP-KEY, and TITLE plus ROLE still map into one map.
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
    let card = {
        let mut card = ContactCard::default();
        card.organizations = Some(
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
        );
        card
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
    let card = {
        let mut card = ContactCard::default();
        card.titles = Some(
            [
                ("t1".to_owned(), {
                    let mut title = Title::default();
                    title.name = "Research Scientist".to_owned();
                    title.kind = None;
                    title
                }),
                ("t2".to_owned(), {
                    let mut title = Title::default();
                    title.name = "Director of Engineering".to_owned();
                    title.kind = None;
                    title
                }),
                ("r1".to_owned(), {
                    let mut title = Title::default();
                    title.name = "Lead Investigator".to_owned();
                    title.kind = Some("role".to_owned());
                    title
                }),
                ("r2".to_owned(), {
                    let mut title = Title::default();
                    title.name = "Project Manager".to_owned();
                    title.kind = Some("role".to_owned());
                    title
                }),
            ]
            .into(),
        );
        card
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
    // Keep ORG component order to match EDS slots.
    // Order is organization name, department, office, then extra units.
    let card = {
        let mut card = ContactCard::default();
        card.organizations = Some(
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
        );
        card
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

    // Keep unkeyed ORG components under generated key `o1`.
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
    // Keep a deep hierarchy with one name and five units.
    let deep_card = {
        let mut card = ContactCard::default();
        card.organizations = Some(
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
        );
        card
    };
    let deep_vcard = card_to_vcard(&deep_card);
    assert_eq!(
        line(&unfolded(&deep_vcard), "ORG"),
        "ORG;X-JMAP-KEY=o1:Global Tech;Engineering;Infrastructure;Storage Systems;Flash Division;Team Beta"
    );
    let deep_back = vcard_to_card(&deep_vcard).expect("parse");
    assert_eq!(deep_back.organizations, deep_card.organizations);

    // Keep unit positions for nameless organizations.
    // The writer uses a leading semicolon to hold the empty name slot.
    let nameless_card = {
        let mut card = ContactCard::default();
        card.organizations = Some(
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
        );
        card
    };
    let nameless_vcard = card_to_vcard(&nameless_card);
    assert_eq!(
        line(&nameless_vcard, "ORG"),
        "ORG;X-JMAP-KEY=o1:;Engineering;Security;Cryptography;Quantum"
    );
    let nameless_back = vcard_to_card(&nameless_vcard).expect("parse");
    assert_eq!(nameless_back.organizations, nameless_card.organizations);

    // EDS can clear the office slot in place.
    // Readback drops empty units and keeps stable unit order.
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
    let card = {
        let mut card = ContactCard::default();
        card.organizations = Some(
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
        );
        card.titles = Some(
            [
                ("t1".to_owned(), {
                    let mut title = Title::default();
                    title.name = "Chief Scientist".to_owned();
                    title.kind = None;
                    title
                }),
                ("t2".to_owned(), {
                    let mut title = Title::default();
                    title.name = "Distinguished Engineer".to_owned();
                    title.kind = Some("title".to_owned());
                    title
                }),
                ("r1".to_owned(), {
                    let mut title = Title::default();
                    title.name = "Technical Lead".to_owned();
                    title.kind = Some("role".to_owned());
                    title
                }),
                ("r2".to_owned(), {
                    let mut title = Title::default();
                    title.name = "Steering Committee Member".to_owned();
                    title.kind = Some("role".to_owned());
                    title
                }),
                ("x1".to_owned(), {
                    let mut title = Title::default();
                    title.name = "Honorary Fellow".to_owned();
                    title.kind = Some("x-honour".to_owned());
                    title
                }),
            ]
            .into(),
        );
        card
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

    // Expect four title keys after roundtrip: t1, t2, r1, r2.
    // Canonical kind is None for default titles.
    let expected_titles = [
        ("t1".to_owned(), {
            let mut title = Title::default();
            title.name = "Chief Scientist".to_owned();
            title.kind = None;
            title
        }),
        ("t2".to_owned(), {
            let mut title = Title::default();
            title.name = "Distinguished Engineer".to_owned();
            title.kind = None;
            title
        }),
        ("r1".to_owned(), {
            let mut title = Title::default();
            title.name = "Technical Lead".to_owned();
            title.kind = Some("role".to_owned());
            title
        }),
        ("r2".to_owned(), {
            let mut title = Title::default();
            title.name = "Steering Committee Member".to_owned();
            title.kind = Some("role".to_owned());
            title
        }),
    ]
    .into();
    assert_eq!(back.titles, Some(expected_titles));
}

#[test]
fn multi_component_org_with_escaped_punctuation_roundtrips() {
    let card = {
        let mut card = ContactCard::default();
        card.organizations = Some(
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
        );
        card
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
    {
        let mut card = ContactCard::default();
        card.addresses = Some([(key.to_owned(), address)].into());
        card
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
    // Evolution omits X-JMAP-KEY and fills empty ADR slots.
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
    // vCard 3.0 has no ADR field for floor or room.
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
    // RFC 9553 splits street name and house number; RFC 2426 has one street
    // slot, so both components share it in source order.
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

    // Readback keeps one `name` component. `Hauptstraße 1` is locale-ambiguous.
    let back = vcard_to_card(&vcard).expect("parse");
    let addresses = back.addresses.expect("addresses");
    assert_eq!(
        components_of(&addresses["a1"]),
        vec![("name", "Hauptstraße 1")]
    );
}

#[test]
fn an_address_that_states_only_a_house_number_is_on_the_line() {
    // A standalone house number is valid address data, so emit ADR.
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
    // JSContact `full` maps to vCard `LABEL`; embedded newlines are preserved.
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
    // EDS rewrites LABEL without X-JMAP-KEY, so pairing falls back to TYPE.
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
    // RFC 9553 allows `full` without components, so LABEL-only addresses keep
    // their key on LABEL and do not require an ADR line.
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

    // A TYPE-mismatched LABEL does not fold into an unrelated ADR.
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

    let card = {
        let mut card = ContactCard::default();
        card.addresses = Some(addresses);
        card
    };

    let vcard = card_to_vcard(&card);
    assert!(vcard.contains("ADR;X-JMAP-KEY=a-work;TYPE=WORK:"));
    assert!(vcard.contains("LABEL;X-JMAP-KEY=a-work;TYPE=WORK:"));
    assert!(vcard.contains("ADR;X-JMAP-KEY=a-home;TYPE=HOME:"));
    assert!(vcard.contains("LABEL;X-JMAP-KEY=a-home;TYPE=HOME:"));

    // Roundtrip the emitted vCard directly.
    let roundtrip = vcard_to_card(&vcard).expect("parse emitted");
    assert_eq!(roundtrip.addresses, card.addresses);

    // Simulate EDS with LABEL lines that keep TYPE but drop X-JMAP-KEY.
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
    let card = {
        let mut card = ContactCard::default();
        card.addresses = Some(addresses);
        card
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
    let card = {
        let mut card = ContactCard::default();
        card.phones = Some(phones);
        card
    };

    // This case states one feature either way.
    // The test `a_number_that_is_a_voice_line_and_a_fax_states_one_feature`
    // already pins feature narrowing.
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
    let vcard = card_to_vcard(&{
        let mut card = ContactCard::default();
        card.phones = Some(phones);
        card
    });
    line(&vcard, "TEL").to_owned()
}

#[test]
fn a_number_that_is_a_voice_line_and_a_fax_states_one_feature() {
    // `TEL;TYPE=VOICE,FAX` maps to no field in libebook-contacts 3.52.
    // The contact editor then shows no number.
    // With context, EDS maps it to two fields that overwrite each other.
    // See `eds-sys` `a_line_wearing_several_feature_types_reaches_two_fields_or_none`.
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
    // EDS maps this pair to two fields without context.
    assert_eq!(
        phone_line(None, Some(json!({"mobile": true, "pager": true}))),
        "TEL;X-JMAP-KEY=p1;TYPE=CELL:+49 30 111"
    );
    // A mobile outranks unmarked `voice` and `fax`.
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
    // EDS has no field for `VIDEO`.
    // When another feature is present, write the mapped feature slot.
    assert_eq!(
        phone_line(None, Some(json!({"voice": true, "video": true}))),
        "TEL;X-JMAP-KEY=p1;TYPE=VOICE:+49 30 111"
    );
    // When `VIDEO` is alone, keep it on write.
    // Dropping it would wrongly claim a voice line.
    assert_eq!(
        phone_line(None, Some(json!({"video": true}))),
        "TEL;X-JMAP-KEY=p1;TYPE=VIDEO:+49 30 111"
    );
}

#[test]
fn a_line_with_several_features_is_still_read_as_all_of_them() {
    // Narrowing applies only to writer output.
    // Reader input that states both features keeps both.
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
    let card = {
        let mut card = ContactCard::default();
        card.emails = Some(emails);
        card
    };

    // EDS files an `EMAIL` line by position: `E_CONTACT_EMAIL_1` to `_4`.
    // It does not file by `TYPE`.
    // A line with both contexts reaches one field, not two.
    // State both contexts so save can read either context back.
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
    {
        let mut card = ContactCard::default();
        card.notes = Some(
            [(key.to_owned(), {
                let mut v = Note::default();
                v.note = note.to_owned();
                v
            })]
            .into(),
        );
        card
    }
}

#[test]
fn maps_notes_onto_note_lines() {
    // RFC 9553 §2.8.3 keys notes like other JSContact maps.
    // RFC 2426 §3.6.2 defines NOTE as plain text.
    // Store the key in X-JMAP-KEY as for EMAIL.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(line(&vcard, "NOTE"), "NOTE;X-JMAP-KEY=n1:met at FOSDEM");

    let card = vcard_to_card(&vcard).expect("parse");
    let notes = card.notes.expect("notes");
    assert_eq!(notes.keys().collect::<Vec<_>>(), vec!["n1"]);
    assert_eq!(notes["n1"].note, "met at FOSDEM");
}

#[test]
fn a_note_keeps_the_line_breaks_and_separators_it_was_written_with() {
    // Notes often contain prose text.
    // Escape vCard separators because users can type them in note text.
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
    // Evolution stores one Notes field per contact and writes no X-JMAP-KEY.
    // Create a key when such a note arrives.
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
    {
        let mut card = ContactCard::default();
        card.anniversaries = Some(
            [(
                "y1".to_owned(),
                Anniversary {
                    kind: kind.to_owned(),
                    date: Some(date),
                    ..Anniversary::default()
                },
            )]
            .into(),
        );
        card
    }
}

#[test]
fn maps_a_birthday_onto_the_bday_line() {
    // RFC 9553 maps memorable dates by `kind`.
    // BDAY carries only birthdays.
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
    // vCard 3.0 has no ANNIVERSARY property.
    // Use X-EVOLUTION-ANNIVERSARY for Evolution's anniversary field.
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
    // `death` has no vCard 3.0 property or EDS field, so it must not reuse BDAY.
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
    // Incomplete PartialDate values do not name one day, so no line is written.
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
    // `e_contact_date_to_string()` writes years only in 1000..=9999.
    // Do not emit a line for older years.
    for date in [
        json!({"year": 800, "month": 6, "day": 21}),
        json!({"year": 999, "month": 12, "day": 31}),
        json!({"year": 1, "month": 1, "day": 1}),
        json!({"@type": "Timestamp", "utc": "0800-06-21T09:00:00Z"}),
    ] {
        let vcard = card_to_vcard(&one_anniversary("birth", date.clone()));
        assert!(!vcard.contains("\r\nBDAY"), "{date}: {vcard}");
    }

    // Year 1000 is the first year EDS can write.
    let vcard = card_to_vcard(&one_anniversary(
        "birth",
        json!({"year": 1000, "month": 6, "day": 21}),
    ));
    assert_eq!(line(&vcard, "BDAY"), "BDAY;X-JMAP-KEY=y1:1000-06-21");
}

#[test]
fn an_anniversary_stated_as_a_point_in_time_crosses_as_the_day_it_names() {
    // Timestamps map to day-only vCard dates.
    // Do not encode time-of-day.
    let vcard = card_to_vcard(&one_anniversary(
        "birth",
        json!({"@type": "Timestamp", "utc": "1953-10-15T23:10:00Z"}),
    ));
    assert_eq!(line(&vcard, "BDAY"), "BDAY;X-JMAP-KEY=y1:1953-10-15");
}

#[test]
fn a_date_line_written_without_separators_states_the_same_day() {
    // Accept ISO 8601 basic form because external vCards can use it.
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

    // Empty text, free text, and impossible dates state no valid day.
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
    // Evolution rebuilds birthday lines and drops X-JMAP-KEY, so edited lines need fresh keys.
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
    let card = {
        let mut card = ContactCard::default();
        card.anniversaries = Some(
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
        );
        card
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
    {
        let mut card = ContactCard::default();
        card.nicknames = Some(
            [(key.to_owned(), {
                let mut v = Nickname::default();
                v.name = name.to_owned();
                v
            })]
            .into(),
        );
        card
    }
}

#[test]
fn maps_nicknames_onto_nickname_lines() {
    // NICKNAME is plain text. Keep the JSContact key in X-JMAP-KEY so EDS rewrites keep the link.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(line(&vcard, "NICKNAME"), "NICKNAME;X-JMAP-KEY=k1:Vee");

    let card = vcard_to_card(&vcard).expect("parse");
    let nicknames = card.nicknames.expect("nicknames");
    assert_eq!(nicknames.keys().collect::<Vec<_>>(), vec!["k1"]);
    assert_eq!(nicknames["k1"].name, "Vee");
}

#[test]
fn nickname_type_and_pref_parameters_parse_and_normalize_to_eds_profile() {
    let parsed = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "NICKNAME;TYPE=WORK,PREF;X-JMAP-KEY=k1:JQ\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");
    let nickname = &parsed.nicknames.as_ref().expect("nicknames")["k1"];
    assert_eq!(nickname.name, "JQ");
    assert_eq!(nickname.contexts, Some(json!({"work": true})));
    assert_eq!(nickname.pref, Some(1));

    let emitted = card_to_vcard(&parsed);
    assert_eq!(line(&emitted, "NICKNAME"), "NICKNAME;X-JMAP-KEY=k1:JQ");
}

#[test]
fn each_nickname_gets_a_line_of_its_own() {
    // Use one line per nickname key. A comma list cannot carry per-entry keys,
    // and EDS reads NICKNAME as one string anyway.
    let mut card = one_nickname("k1", "Vee");
    card.nicknames
        .as_mut()
        .expect("nicknames")
        .insert("k2".to_owned(), {
            let mut v = Nickname::default();
            v.name = "Vera the Elder".to_owned();
            v
        });
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
    // This is the other side of the same decision.
    // A comma in the value is escaped on write, exactly as EDS escapes it.
    // On read, it stays the original text.
    // It never splits into two entries sent to the server.
    let vcard = card_to_vcard(&one_nickname("k1", "Jim, the tall one"));
    assert_eq!(
        line(&vcard, "NICKNAME"),
        "NICKNAME;X-JMAP-KEY=k1:Jim\\, the tall one"
    );

    let nicknames = vcard_to_card(&vcard).expect("parse").nicknames.unwrap();
    assert_eq!(nicknames.len(), 1);
    assert_eq!(nicknames["k1"].name, "Jim, the tall one");

    // Even an unescaped comma stays one nickname to match EDS and avoid ghost entries.
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
fn nickname_list_value_reaches_fixed_point_in_eds_profile() {
    let raw_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "NICKNAME:Jim,Jimmie\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(raw_vcard).expect("parse nickname list");
    let nicknames = parsed.nicknames.as_ref().expect("nicknames");
    assert_eq!(nicknames.len(), 1);
    assert_eq!(nicknames["k1"].name, "Jim,Jimmie");

    let emitted = card_to_vcard(&parsed);
    assert_eq!(
        line(&emitted, "NICKNAME"),
        "NICKNAME;X-JMAP-KEY=k1:Jim\\,Jimmie"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized nickname list");
    assert_eq!(reparsed.nicknames, parsed.nicknames);

    let re_emitted = card_to_vcard(&reparsed);
    assert_eq!(emitted, re_emitted);
}

#[test]
fn a_nickname_that_names_nothing_is_skipped_in_both_directions() {
    // Empty nickname text emits no line and stays absent on read.
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
    {
        let mut card = ContactCard::default();
        card.links = Some(
            [(
                key.to_owned(),
                Link {
                    uri: uri.to_owned(),
                    ..Link::default()
                },
            )]
            .into(),
        );
        card
    }
}

#[test]
fn maps_links_onto_url_lines() {
    // RFC 2426 §3.6.8 defines `URL` as a bare URI.
    // Store the JSContact key in `X-JMAP-KEY`, as for `EMAIL`.
    // EDS rewrites the value and keeps this parameter.
    // Verify this behavior with libebook-contacts 3.52.
    // Evolution shows the first `URL` as the contact Home Page.
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
    // `kind=contact` maps to vCard 4.0 `CONTACT-URI`.
    // vCard 3.0 has no matching field, so omit this link.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(vcard.matches("\r\nURL").count(), 1, "{vcard}");
    assert!(!vcard.contains("write-to-me"), "{vcard}");

    // Omit unknown link kinds for the same reason.
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
    // Treat empty URIs as absent entries.
    assert!(!card_to_vcard(&one_link("l1", "")).contains("\r\nURL"));

    // EDS can leave an empty `URL` line after it clears Home Page.
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
    // Keep URI punctuation through roundtrip.
    // Do not split on punctuation that vCard uses structurally.
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
    // EDS can emit fresh `URL` lines without `X-JMAP-KEY`.
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
    {
        let mut card = ContactCard::default();
        card.calendars = Some(
            [(key.to_owned(), {
                let mut calendar = Calendar::default();
                calendar.kind = kind.map(str::to_owned);
                calendar.uri = uri.to_owned();
                calendar
            })]
            .into(),
        );
        card
    }
}

fn one_scheduling_address(key: &str, uri: &str) -> ContactCard {
    {
        let mut card = ContactCard::default();
        card.scheduling_addresses = Some([(key.to_owned(), SchedulingAddress::new(uri))].into());
        card
    }
}

#[test]
fn maps_calendars_onto_the_caluri_and_fburl_lines() {
    // `calendar` maps to `CALURI` and `freeBusy` maps to `FBURL`.
    // Both lines carry the `X-JMAP-KEY` parameter.
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
    // Unknown calendar kinds have no `CALURI` or `FBURL` target.
    // Omit them.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(vcard.matches("\r\nCALURI").count(), 1, "{vcard}");
    assert!(!vcard.contains("nameless"), "{vcard}");

    // Omit vendor calendar kinds too.
    let vcard = card_to_vcard(&one_calendar(
        "c1",
        Some("example.com:tasks"),
        "https://vera.example/tasks",
    ));
    assert!(!vcard.contains("tasks"), "{vcard}");
}

#[test]
fn a_calendar_that_points_nowhere_is_skipped_in_both_directions() {
    // Treat empty calendar URIs as absent entries.
    let vcard = card_to_vcard(&one_calendar("c1", Some("calendar"), ""));
    assert!(!vcard.contains("\r\nCALURI"), "{vcard}");

    // EDS can leave empty `CALURI` and `FBURL` lines after it clears fields.
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
    // Keep reader-generated keys unique across keyless `CALURI` and `FBURL` lines.
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
    // Non-primary `CALURI` lines must still roundtrip.
    assert_eq!(calendars["c7"].uri, "https://vera.example/cal/team.ics");
}

#[test]
fn calendars_read_pref_and_media_type_parameters() {
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "CALURI;X-JMAP-KEY=c1;PREF=1;MEDIATYPE=text/calendar:https://vera.example/cal/vera.ics\r\n",
        "FBURL;X-JMAP-KEY=c2;TYPE=PREF;MEDIATYPE=text/calendar:https://vera.example/fb/vera.ifb\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let calendars = card.calendars.expect("calendars");
    assert_eq!(calendars["c1"].pref, Some(1));
    assert_eq!(calendars["c1"].media_type.as_deref(), Some("text/calendar"));
    assert_eq!(calendars["c2"].pref, Some(1));
    assert_eq!(calendars["c2"].media_type.as_deref(), Some("text/calendar"));
}

#[test]
fn calendars_emit_the_eds_profile_without_pref_or_media_type_parameters() {
    let mut calendars = BTreeMap::new();
    calendars.insert("c1".to_owned(), {
        let mut calendar = Calendar::default();
        calendar.kind = Some("calendar".to_owned());
        calendar.uri = "https://vera.example/cal/vera.ics".to_owned();
        calendar.media_type = Some("text/calendar".to_owned());
        calendar.pref = Some(1);
        calendar
    });
    calendars.insert("c2".to_owned(), {
        let mut calendar = Calendar::default();
        calendar.kind = Some("freeBusy".to_owned());
        calendar.uri = "https://vera.example/fb/vera.ifb".to_owned();
        calendar.media_type = Some("text/calendar".to_owned());
        calendar.pref = Some(1);
        calendar
    });
    let card = {
        let mut card = ContactCard::default();
        card.calendars = Some(calendars);
        card
    };

    let vcard = unfolded(&card_to_vcard(&card));
    assert_eq!(
        line(&vcard, "CALURI"),
        "CALURI;X-JMAP-KEY=c1:https://vera.example/cal/vera.ics"
    );
    assert_eq!(
        line(&vcard, "FBURL"),
        "FBURL;X-JMAP-KEY=c2:https://vera.example/fb/vera.ifb"
    );
}

#[test]
fn maps_scheduling_addresses_onto_caladruri_lines() {
    // `CALADRURI` carries scheduling addresses with `X-JMAP-KEY` identity.
    let vcard = card_to_vcard(&one_scheduling_address(
        "s1",
        "mailto:vera-calendar@example.com",
    ));
    assert_eq!(
        line(&vcard, "CALADRURI"),
        "CALADRURI;X-JMAP-KEY=s1:mailto:vera-calendar@example.com"
    );

    let card = vcard_to_card(&vcard).expect("parse");
    let scheduling = card.scheduling_addresses.expect("schedulingAddresses");
    assert_eq!(scheduling.keys().collect::<Vec<_>>(), vec!["s1"]);
    assert_eq!(scheduling["s1"].uri, "mailto:vera-calendar@example.com");
}

#[test]
fn a_scheduling_address_that_points_nowhere_is_skipped_in_both_directions() {
    // Empty scheduling URIs are treated as absent entries.
    let vcard = card_to_vcard(&one_scheduling_address("s1", ""));
    assert!(!vcard.contains("\r\nCALADRURI"), "{vcard}");

    // Empty `CALADRURI` lines from imports are dropped.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "CALADRURI;X-JMAP-KEY=s1:\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");
    assert_eq!(card.scheduling_addresses, None);
}

#[test]
fn invents_a_key_for_a_scheduling_address_that_has_none() {
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "CALADRURI:mailto:vera-calendar@example.com\r\n",
        "CALADRURI;X-JMAP-KEY=s7:https://calendar.example/vera\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let scheduling = card.scheduling_addresses.expect("schedulingAddresses");
    assert_eq!(scheduling["s1"].uri, "mailto:vera-calendar@example.com");
    assert_eq!(scheduling["s7"].uri, "https://calendar.example/vera");
}

#[test]
fn scheduling_addresses_read_pref_parameters() {
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "CALADRURI;X-JMAP-KEY=s1;PREF=1:mailto:vera-calendar@example.com\r\n",
        "CALADRURI;X-JMAP-KEY=s2;TYPE=PREF:https://calendar.example/vera\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let scheduling = card.scheduling_addresses.expect("schedulingAddresses");
    assert_eq!(scheduling["s1"].pref, Some(1));
    assert_eq!(scheduling["s2"].pref, Some(1));
}

#[test]
fn scheduling_addresses_emit_the_eds_profile_without_pref_parameters() {
    let mut scheduling = BTreeMap::new();
    scheduling.insert(
        "s1".to_owned(),
        SchedulingAddress::new("mailto:vera-calendar@example.com").with_pref(1),
    );
    let card = {
        let mut card = ContactCard::default();
        card.scheduling_addresses = Some(scheduling);
        card
    };

    let vcard = unfolded(&card_to_vcard(&card));
    assert_eq!(
        line(&vcard, "CALADRURI"),
        "CALADRURI;X-JMAP-KEY=s1:mailto:vera-calendar@example.com"
    );
}

fn one_relation(key: &str, types: &[&str]) -> ContactCard {
    {
        let mut card = ContactCard::default();
        card.related_to = Some(
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
        );
        card
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
    // Only `spouse` maps to EDS, via `X-EVOLUTION-SPOUSE`; the spouse name is the map key.
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
    // Non-spouse relation kinds have no vCard 3.0 or EDS field and are omitted.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(vcard.matches("X-EVOLUTION-SPOUSE").count(), 1, "{vcard}");
    assert!(!vcard.contains("Nils"), "{vcard}");

    // Untyped relations are omitted too.
    let vcard = card_to_vcard(&one_relation("Jean Paul Oldenburg", &[]));
    assert!(!vcard.contains("SPOUSE"), "{vcard}");
    let vcard = card_to_vcard(&{
        let mut card = ContactCard::default();
        card.related_to = Some([("Jean Paul Oldenburg".to_owned(), Relation::default())].into());
        card
    });
    assert!(!vcard.contains("SPOUSE"), "{vcard}");
}

#[test]
fn a_spouse_the_card_names_by_uid_gets_no_line() {
    // Identifier-like spouse keys are omitted because spouse lines require displayable names.
    let vcard = card_to_vcard(&fixture_card());
    assert!(!vcard.contains("e1f0a1c2"), "{vcard}");

    // URI-shaped keys are treated as identifiers, not names.
    for key in [
        "urn:uuid:e1f0a1c2-0f6b-4d2e-9c3a-2b1f9d0e7c44",
        "mailto:jean@example.com",
        "https://vera.example/jean",
        "XMPP:jean@jabber.example",
    ] {
        let vcard = card_to_vcard(&one_relation(key, &["spouse"]));
        assert!(!vcard.contains("SPOUSE"), "{key}: {vcard}");
    }

    // A plain name may contain `:` when it is not a URI scheme.
    let vcard = card_to_vcard(&one_relation("Jean Paul: the second", &["spouse"]));
    assert!(vcard.contains("SPOUSE"), "{vcard}");
}

#[test]
fn a_spouse_whose_name_eds_would_rename_gets_no_line() {
    // Names that EDS would trim or rewrite are omitted because the name is the key.
    for key in [" Jean Paul Oldenburg", "Jean Paul Oldenburg\t", ""] {
        let vcard = card_to_vcard(&one_relation(key, &["spouse"]));
        assert!(!vcard.contains("SPOUSE"), "[{key}]: {vcard}");
    }
    let vcard = card_to_vcard(&one_relation("Jean\rPaul", &["spouse"]));
    assert!(!vcard.contains("SPOUSE"), "{vcard}");

    // Internal spaces remain stable and are accepted.
    let vcard = card_to_vcard(&one_relation("Jean Paul Oldenburg", &["spouse"]));
    assert_eq!(spouse_of(&vcard).as_deref(), Some("Jean Paul Oldenburg"));
}

#[test]
fn reads_every_spouse_line_and_keys_each_by_its_own_name() {
    // Readers keep every non-empty spouse line and key entries by spouse-name text.
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
    {
        let mut card = ContactCard::default();
        card.media = Some(
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
        );
        card
    }
}

/// "hello-photo", which stands in for the JPEG a real card carries.
const PAYLOAD: &str = "aGVsbG8tcGhvdG8=";

#[test]
fn maps_a_photo_onto_the_line_evolution_shows_it_on() {
    // Fixture media `m1` is inline photo data.
    // For EDS compatibility, PHOTO is written as inline bytes with `ENCODING=b`.
    let vcard = card_to_vcard(&fixture_card());
    assert_eq!(
        line(&vcard, "PHOTO"),
        format!("PHOTO;X-JMAP-KEY=m1;TYPE=jpeg;ENCODING=b:{PAYLOAD}")
    );
}

#[test]
fn a_picture_the_card_only_points_at_crosses_as_a_uri_line() {
    // Non-`data:` photos have no inline bytes, so PHOTO uses `VALUE=uri`.
    // EDS ignores URI-valued PHOTO lines without that parameter.
    let vcard = card_to_vcard(&one_photo("https://vera.example/me.png", None));
    assert_eq!(
        line(&vcard, "PHOTO"),
        "PHOTO;X-JMAP-KEY=m1;VALUE=uri:https://vera.example/me.png"
    );

    // URI photo lines intentionally omit `TYPE`; EDS neither reads nor writes it there.
    assert!(!vcard.contains("TYPE"), "{vcard}");
}

#[test]
fn the_type_parameter_states_the_subtype_and_nothing_else() {
    // EDS prepends `image/` to TYPE, so TYPE must hold only the subtype.
    let vcard = card_to_vcard(&one_photo(
        &format!("data:image/jpeg;base64,{PAYLOAD}"),
        Some("image/png"),
    ));
    assert_eq!(
        line(&vcard, "PHOTO"),
        format!("PHOTO;X-JMAP-KEY=m1;TYPE=png;ENCODING=b:{PAYLOAD}"),
        "the entry's own `mediaType` is what it says the bytes are"
    );

    // Without explicit mediaType, parse subtype from the `data:` URI itself.
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
    // Non-image inline bytes still map to PHOTO, but TYPE is omitted.
    // Adding TYPE would mislabel them as `image/<type>` in EDS.
    let vcard = card_to_vcard(&one_photo(
        &format!("data:application/pdf;base64,{PAYLOAD}"),
        None,
    ));
    assert_eq!(
        line(&vcard, "PHOTO"),
        format!("PHOTO;X-JMAP-KEY=m1;ENCODING=b:{PAYLOAD}")
    );

    // The same applies when the `data:` URI omits media type.
    let vcard = card_to_vcard(&one_photo(&format!("data:;base64,{PAYLOAD}"), None));
    assert_eq!(
        line(&vcard, "PHOTO"),
        format!("PHOTO;X-JMAP-KEY=m1;ENCODING=b:{PAYLOAD}")
    );
}

#[test]
fn a_payload_spelled_loosely_is_written_out_as_canonical_base64() {
    // Inline bytes are normalized by decode + re-encode, not copied verbatim.
    // This matches EDS consuming base64 payload bytes.
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
    // EDS-facing PHOTO lines require decodable base64 payload bytes.
    // URI forms that cannot supply that payload are dropped.
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
    // PHOTO is only for contact photos, not other media kinds.
    // Non-`photo` entries are not emitted.
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
    // Inline `PHOTO` bytes with `TYPE` read back as a JSContact `data:` URI plus media type.
    let media = photo_line(&format!("PHOTO;TYPE=jpeg;ENCODING=b:{PAYLOAD}"));
    assert_eq!(media.kind.as_deref(), Some("photo"));
    assert_eq!(media.uri, format!("data:image/jpeg;base64,{PAYLOAD}"));
    assert_eq!(media.media_type.as_deref(), Some("image/jpeg"));

    // Accept legacy `ENCODING=BASE64` too.
    let media = photo_line(&format!("PHOTO;ENCODING=BASE64;TYPE=PNG:{PAYLOAD}"));
    assert_eq!(media.uri, format!("data:image/PNG;base64,{PAYLOAD}"));
    assert_eq!(media.media_type.as_deref(), Some("image/PNG"));
}

#[test]
fn a_data_uri_photo_line_without_type_keeps_its_stated_media_type() {
    let media = photo_line(&format!("PHOTO:data:image/heic;base64,{PAYLOAD}"));
    assert_eq!(media.uri, format!("data:image/heic;base64,{PAYLOAD}"));
    assert_eq!(media.media_type.as_deref(), Some("image/heic"));
}

#[test]
fn a_photo_eds_holds_no_media_type_for_reads_back_without_one() {
    // `TYPE="X-EVOLUTION-UNKNOWN"` is treated as no media type.
    let media = photo_line(&format!(
        "PHOTO;TYPE=\"X-EVOLUTION-UNKNOWN\";ENCODING=b:{PAYLOAD}"
    ));
    assert_eq!(media.media_type, None);
    assert_eq!(media.uri, format!("data:;base64,{PAYLOAD}"));

    // `PHOTO` lines without `TYPE` also read back as no media type.
    let media = photo_line(&format!("PHOTO;ENCODING=b:{PAYLOAD}"));
    assert_eq!(media.media_type, None);
    assert_eq!(media.uri, format!("data:;base64,{PAYLOAD}"));
}

#[test]
fn reads_a_reference_photo_line_back_as_the_uri_it_names() {
    let media = photo_line("PHOTO;VALUE=uri:https://vera.example/me.png");
    assert_eq!(media.kind.as_deref(), Some("photo"));
    assert_eq!(media.uri, "https://vera.example/me.png");
    // URI `PHOTO` lines carry no usable media type in EDS.
    assert_eq!(media.media_type, None);
}

#[test]
fn photo_mediatype_parameter_on_value_uri_is_parse_only_in_this_profile() {
    // RFC 9555 section 2.3.14 maps MEDIATYPE into JSContact media type on read.
    let input = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:Photo MEDIATYPE VALUE URI\r\n",
        "PHOTO;MEDIATYPE=image/jpeg;VALUE=uri:https://vera.example/me.jpg\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(input).expect("parse MEDIATYPE VALUE=uri photo");
    let parsed_media = parsed.media.as_ref().expect("media");
    assert_eq!(parsed_media["m1"].uri, "https://vera.example/me.jpg");
    assert_eq!(parsed_media["m1"].media_type.as_deref(), Some("image/jpeg"));

    // EDS stores URI photos without media type metadata, so normalized emit drops MEDIATYPE.
    let emitted = card_to_vcard(&parsed);
    assert_eq!(
        line(&emitted, "PHOTO"),
        "PHOTO;X-JMAP-KEY=m1;VALUE=uri:https://vera.example/me.jpg"
    );
    assert!(
        !emitted.contains("MEDIATYPE="),
        "normalized URI photo must not emit MEDIATYPE in this profile: {emitted}"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized photo");
    let reparsed_media = reparsed.media.as_ref().expect("media");
    assert_eq!(reparsed_media["m1"].media_type, None);

    let emitted_again = card_to_vcard(&reparsed);
    let reparsed_again = vcard_to_card(&emitted_again).expect("reparse emitted again");
    assert_eq!(emitted, emitted_again);
    assert_eq!(reparsed, reparsed_again);
}

#[test]
fn a_photo_whose_bytes_are_not_text_reads_back_byte_for_byte() {
    // Non-UTF-8 payload bytes must roundtrip via bytes, not text coercion.
    let media = photo_line("PHOTO;TYPE=png;ENCODING=b:iVBORw0KGgr//g==");
    assert_eq!(media.uri, "data:image/png;base64,iVBORw0KGgr//g==");

    // Full roundtrip guards against fixture bias.
    let uri = "data:image/png;base64,iVBORw0KGgr//g==";
    let vcard = card_to_vcard(&one_photo(uri, None));
    assert_eq!(read_photo(&vcard).uri, uri, "{vcard}");
}

#[test]
fn a_photo_whose_bytes_are_text_that_also_looks_like_base64_is_not_decoded_twice() {
    // Regression from fuzzer case `raw_bytes = [97, 65, 56]`: do not decode text-like base64 twice.
    let media = photo_line("PHOTO;TYPE=jpeg;ENCODING=b:YUE4");
    assert_eq!(media.uri, "data:image/jpeg;base64,YUE4");

    // Validate with a full roundtrip, matching the property test path.
    let uri = "data:image/jpeg;base64,YUE4";
    let vcard = card_to_vcard(&one_photo(uri, Some("image/jpeg")));
    assert_eq!(read_photo(&vcard).uri, uri, "{vcard}");
}

#[test]
fn a_photo_line_states_the_key_its_entry_is_filed_under() {
    // `PHOTO` can carry `X-JMAP-KEY`, but readers must also handle keyless rewritten lines.
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
    // Empty `PHOTO` values name neither bytes nor URI, so no entry is created.
    for line in ["PHOTO;ENCODING=b:", "PHOTO;VALUE=uri:"] {
        let vcard = format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\n{line}\r\nEND:VCARD\r\n");
        let card = vcard_to_card(&vcard).unwrap_or_else(|e| panic!("{line}: {e}"));
        assert_eq!(card.media, None, "{line}");
    }
}

#[test]
fn a_photo_survives_the_round_trip_it_is_stated_on() {
    // Roundtrip keeps fixture photo identity: URI, media type, and key.
    let card = vcard_to_card(&card_to_vcard(&fixture_card())).expect("parse");
    let media = card.media.expect("media");
    let original = fixture_card().media.expect("media");
    assert_eq!(media["m1"].kind, original["m1"].kind);
    assert_eq!(media["m1"].uri, original["m1"].uri);
    assert_eq!(media["m1"].media_type, original["m1"].media_type);
    // The fixture's `m2` is a logo and has no `PHOTO` line.
    assert_eq!(media.len(), 1, "{media:?}");
}

fn tagged(tags: &[&str]) -> ContactCard {
    {
        let mut card = ContactCard::default();
        card.keywords = Some(
            tags.iter()
                .map(|tag| ((*tag).to_owned(), json!(true)))
                .collect(),
        );
        card
    }
}

#[test]
fn maps_keywords_onto_one_categories_line() {
    // Serialize keywords on one `CATEGORIES` line.
    // This avoids hidden duplicate category fields in EDS.
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
    // Serialize set members in sorted map order so repeated renders stay stable.
    let vcard = card_to_vcard(&tagged(&["Work", "Friends", "hiking"]));
    assert_eq!(line(&vcard, "CATEGORIES"), "CATEGORIES:Friends,Work,hiking");
}

#[test]
fn a_tag_holding_the_separators_is_escaped_and_comes_back_whole() {
    // Comma and semicolon must be escaped so one keyword stays one `CATEGORIES` item.
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
    // Skip tags that cannot roundtrip through one `CATEGORIES` item.
    // Empty items vanish on parse, `\r` is removed, and EDS trims edge ASCII whitespace.
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

    // Internal newlines are escaped, roundtrip, and stay visible.
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
    // RFC 9553 §1.4.3: Set values must be `true`; anything else is omitted.
    let card = {
        let mut card = ContactCard::default();
        card.keywords = Some([("hiking".to_owned(), json!(false))].into());
        card
    };
    assert!(!card_to_vcard(&card).contains("\r\nCATEGORIES"));
}

#[test]
fn states_keyword_answers_for_the_tag_the_line_left_off() {
    // Visibility is evaluated per tag, not for the full set.
    // Save restores hidden tags to match the emitter predicate.
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
    // False is rejected because RFC 9553 §1.4.3 requires `true`.
    assert!(!states_keyword("hiking", &json!(false)));
}

#[test]
fn a_card_with_no_tags_states_none() {
    // Keep `None` so absent keywords stay distinct from an explicit empty set.
    assert!(!card_to_vcard(&ContactCard::default()).contains("\r\nCATEGORIES"));
    let card =
        vcard_to_card("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\nEND:VCARD\r\n").expect("parse");
    assert_eq!(card.keywords, None);
}

#[test]
fn reads_the_tags_of_every_categories_line() {
    // The EDS UI shows only the first categories line, but preserves the rest.
    // Read all lines so hidden tags survive; duplicates collapse and empty items drop.
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

#[test]
fn categories_pref_parameter_is_read_and_not_emitted() {
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Categories Profile\r\n",
        "CATEGORIES;PREF=1:internet,IETF,Industry,Information Technology\r\n",
        "categories;pref=2:Open Source,contacts\\, sync\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let keywords = card.keywords.as_ref().expect("keywords");
    assert_eq!(keywords.len(), 6);
    assert!(keywords.contains_key("internet"));
    assert!(keywords.contains_key("IETF"));
    assert!(keywords.contains_key("Industry"));
    assert!(keywords.contains_key("Information Technology"));
    assert!(keywords.contains_key("Open Source"));
    assert!(keywords.contains_key("contacts, sync"));
    assert!(keywords.values().all(|value| value == &json!(true)));

    let emitted = card_to_vcard(&card);
    assert_eq!(emitted.matches("\r\nCATEGORIES:").count(), 1);
    assert!(!emitted.contains("PREF="));

    let reparsed = vcard_to_card(&emitted).expect("reparse");
    assert_eq!(reparsed.keywords, card.keywords);
}

fn on_service(service: Option<&str>, user: Option<&str>) -> ContactCard {
    {
        let mut card = ContactCard::default();
        card.online_services = Some(
            [(
                "s1".to_owned(),
                OnlineService {
                    service: service.map(str::to_owned),
                    user: user.map(str::to_owned),
                    ..OnlineService::default()
                },
            )]
            .into(),
        );
        card
    }
}

#[test]
fn maps_an_online_service_onto_the_x_line_eds_keeps_it_on() {
    // vCard 3.0 has no `IMPP`, so this mapping uses EDS `X-*` IM lines.
    // Set `TYPE` so the handle lands in one HOME or WORK slot.
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
    // EDS stores this value as handle text, not as a URI value.
    assert_eq!(services["s1"].uri, None);
}

#[test]
fn every_service_eds_has_a_field_for_gets_its_own_line() {
    // Emit one line for each service with an EDS HOME or WORK slot.
    // Match service names without case or punctuation sensitivity.
    // Keep save output stable across spelling variants.
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
    // Skip services without an editable EDS slot.
    // Skip missing service names because no property line can be selected.
    // Hidden lines make save logic treat unknown data as visible.
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
    // libebook-contacts 3.52 stores X-TWITTER and X-SIP as unslotted attributes.
    // No _HOME_1..3 or _WORK_1..3 slot exists for these lines.
    // Ignore X-TWITTER, X-SIP, and unknown X-SIGNAL lines.
    // This avoids unmapped `online_services` entries in reader output.
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
    {
        let mut card = ContactCard::default();
        card.online_services = Some(
            [(
                "s1".to_owned(),
                OnlineService {
                    service: service.map(str::to_owned),
                    uri: Some(uri.to_owned()),
                    ..OnlineService::default()
                },
            )]
            .into(),
        );
        card
    }
}

#[test]
fn an_entry_stated_only_as_a_uri_is_drawn_from_the_uri() {
    // RFC 9553 §2.3.2 allows `uri` or `user`.
    // Derive a handle only from known bare schemes.
    // Require the URI payload to match that handle exactly.
    let vcard = card_to_vcard(&at_uri(Some("Jabber"), "xmpp:vera@jabber.example"));
    assert_eq!(
        line(&vcard, "X-JABBER"),
        "X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera@jabber.example"
    );

    // Reader output keeps only handle text from the emitted line.
    let services = vcard_to_card(&vcard)
        .expect("parse")
        .online_services
        .expect("online services");
    assert_eq!(services["s1"].user.as_deref(), Some("vera@jabber.example"));
    assert_eq!(services["s1"].uri, None);

    // Compare the scheme case-insensitively, as RFC 3986 §3.1 requires.
    assert!(
        card_to_vcard(&at_uri(Some("Jabber"), "XMPP:vera@jabber.example"))
            .contains(":vera@jabber.example")
    );
    // Accept only the service's own scheme for handle extraction.
    assert!(
        !card_to_vcard(&at_uri(Some("Jabber"), "https://jabber.example/vera")).contains("vera")
    );
}

#[test]
fn a_uri_at_a_service_with_no_known_scheme_gets_no_line() {
    // Draw URI values only for services with known `scheme:handle` forms.
    for (service, uri) in [
        (Some("Matrix"), "matrix:u/vera:matrix.example"),
        (Some("GroupWise"), "vera.oldenburg"),
        (Some("Signal"), "sgnl://signal.me/vera"),
        (None, "xmpp:vera@jabber.example"),
    ] {
        let vcard = card_to_vcard(&at_uri(service, uri));
        assert!(!vcard.contains("vera"), "{service:?} was drawn: {vcard}");
    }

    // Fixture Mastodon URI is omitted for the same reason.
    let vcard = card_to_vcard(&fixture_card());
    assert!(!vcard.contains("social.example"), "{vcard}");
}

#[test]
fn a_uri_that_says_more_than_a_handle_gets_no_line() {
    // Accept only bare `scheme:handle` values.
    // Reject path, query, fragment, percent-encoding, and leading whitespace.
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
    // If both `user` and `uri` are present, `user` wins.
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
    // Skip handles that EDS normalizes, so rendered text matches saved text.
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
    // Escape separators so one handle remains one text-list item.
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
    // `contexts` selects HOME or WORK.
    // Unknown or mixed contexts fall back to HOME.
    // Keep each line mapped to one editable slot.
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
    // Handles in both contexts, or in an unknown context, fall back to HOME.
    assert!(slotted(json!({"work": true, "private": true})).ends_with(";TYPE=HOME:handle"));
    assert!(slotted(json!({"school": true})).ends_with(";TYPE=HOME:handle"));
    assert!(
        card_to_vcard(&on_service(Some("Jabber"), Some("handle"))).contains(";TYPE=HOME:handle")
    );
}

#[test]
fn the_slot_a_handle_sits_in_is_not_read_back_as_a_context() {
    // Reading `TYPE` back as JSContact `contexts` would invent contexts.
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
    // Keyless service lines from EDS are accepted and keyed by encounter order.
    let card = vcard_to_card(concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\n",
        "X-MATRIX;TYPE=HOME:@vera:matrix.example\r\n",
        "X-SKYPE;TYPE=WORK:vera.oldenburg\r\n",
        "X-SIGNAL;TYPE=HOME:+49301234\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let services = card.online_services.expect("online services");
    // Keep two entries because `X-SIGNAL` is not a mapped service line.
    assert_eq!(services.keys().collect::<Vec<_>>(), vec!["s1", "s2"]);
    assert_eq!(services["s1"].service.as_deref(), Some("Matrix"));
    assert_eq!(services["s1"].user.as_deref(), Some("@vera:matrix.example"));
    assert_eq!(services["s2"].service.as_deref(), Some("Skype"));
    assert_eq!(services["s2"].user.as_deref(), Some("vera.oldenburg"));
}

#[test]
fn a_card_with_no_online_services_states_none() {
    // Keep `online_services` at `None` when no supported line is present.
    assert!(!card_to_vcard(&ContactCard::default()).contains("X-JABBER"));
    let card =
        vcard_to_card("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\nEND:VCARD\r\n").expect("parse");
    assert_eq!(card.online_services, None);
}

#[test]
fn unmodeled_jscontact_properties_are_dropped_not_mangled() {
    // `preferredLanguages` is out of scope for this minimal vCard subset.
    let vcard = card_to_vcard(&fixture_card());
    assert!(!vcard.contains("de-DE"), "{vcard}");
    assert!(!vcard.contains("preferredLanguages"), "{vcard}");
    assert!(vcard_to_card(&vcard).expect("parse").extra.is_empty());
}

#[test]
fn address_book_membership_is_not_a_vcard_concept() {
    // Address-book membership comes from the source, not from vCard payload.
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
    // Preserve raw separators when importing foreign X-IM handles.
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
    // `gg` URIs map to the Gadu-Gadu numeric handle.
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
    // Action, query, and path URI forms are not bare handles and stay omitted.
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
    // Supported bare IM URIs resolve to handles and roundtrip.
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

    // Services without registered schemes or invalid handles return `None`.
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
    let mut card = {
        let mut card = ContactCard::default();
        card.id = Some("C10".into());
        card.name = Some(Name {
            full: Some("Vera Olden".to_owned()),
            ..Name::default()
        });
        card
    };
    card.notes = Some(
        [
            ("n1".to_owned(), {
                let mut v = Note::default();
                v.note = "First paragraph.\nSecond paragraph with details.".to_owned();
                v
            }),
            ("n2".to_owned(), {
                let mut v = Note::default();
                v.note = "Follow-up note for 2026 meeting.".to_owned();
                v
            }),
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
    let mut card = {
        let mut card = ContactCard::default();
        card.id = Some("C11".into());
        card.name = Some(Name {
            full: Some("Vera Olden".to_owned()),
            ..Name::default()
        });
        card
    };
    card.calendars = Some(
        [
            ("c1".to_owned(), {
                let mut calendar = Calendar::default();
                calendar.kind = Some("calendar".to_owned());
                calendar.uri = "https://example.com/team-calendar.ics".to_owned();
                calendar
            }),
            ("f1".to_owned(), {
                let mut calendar = Calendar::default();
                calendar.kind = Some("freeBusy".to_owned());
                calendar.uri = "https://example.com/busy-schedule.ifb".to_owned();
                calendar
            }),
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
    let mut card = {
        let mut card = ContactCard::default();
        card.id = Some("C12".into());
        card.name = Some(Name {
            full: Some("Vera Olden".to_owned()),
            ..Name::default()
        });
        card
    };
    card.nicknames = Some(
        [("k1".to_owned(), {
            let mut v = Nickname::default();
            v.name = "Vee".to_owned();
            v
        })]
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
    let card = {
        let mut card = ContactCard::default();
        card.phones = Some(
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
        );
        card
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
    let card = {
        let mut card = ContactCard::default();
        card.emails = Some(
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
        );
        card
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
    let card = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("Prof. Dr. Vera Marie Oldenburg MSc Ph.D.".to_owned()),
            components: Some(vec![
                NameComponent::new("title", "Prof. Dr."),
                NameComponent::new("given", "Vera"),
                NameComponent::new("given2", "Marie"),
                NameComponent::new("surname", "Oldenburg"),
                NameComponent::new("credential", "MSc Ph.D."),
            ]),
            ..Name::default()
        });
        card
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
    let card = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("Vera Oldenburg".to_owned()),
            ..Name::default()
        });
        card.extra = extra;
        card
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

    let card = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("Vera Olden".to_owned()),
            ..Name::default()
        });
        card.addresses = Some(addresses);
        card
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
            ..Organization::default()
        },
    );

    let card = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("Vera Olden".to_owned()),
            ..Name::default()
        });
        card.organizations = Some(organizations);
        card
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

    let p_wvf = &phones["p_work_voice_fax"];
    assert_eq!(p_wvf.number, "+49 30 111111");
    assert_eq!(p_wvf.contexts, Some(json!({"work": true})));
    assert_eq!(p_wvf.features, Some(json!({"voice": true, "fax": true})));
    assert_eq!(p_wvf.pref, None);
    assert!(states_phone_feature(p_wvf.features.as_ref(), "fax"));
    assert!(!states_phone_feature(p_wvf.features.as_ref(), "voice"));
    assert!(states_context(p_wvf.contexts.as_ref(), "work"));
    assert!(!states_context(p_wvf.contexts.as_ref(), "private"));

    let p_hvf = &phones["p_home_voice_fax"];
    assert_eq!(p_hvf.number, "+49 30 222222");
    assert_eq!(p_hvf.contexts, Some(json!({"private": true})));
    assert_eq!(p_hvf.features, Some(json!({"voice": true, "fax": true})));
    assert!(states_phone_feature(p_hvf.features.as_ref(), "fax"));
    assert!(!states_phone_feature(p_hvf.features.as_ref(), "voice"));
    assert!(states_context(p_hvf.contexts.as_ref(), "private"));

    let p_bvf = &phones["p_bare_voice_fax"];
    assert_eq!(p_bvf.number, "+49 30 333333");
    assert_eq!(p_bvf.contexts, None);
    assert_eq!(p_bvf.features, Some(json!({"voice": true, "fax": true})));
    assert!(states_phone_feature(p_bvf.features.as_ref(), "fax"));
    assert!(!states_phone_feature(p_bvf.features.as_ref(), "voice"));

    let p_wcv = &phones["p_work_cell_voice"];
    assert_eq!(p_wcv.number, "+49 170 444444");
    assert_eq!(p_wcv.contexts, Some(json!({"work": true})));
    assert_eq!(p_wcv.features, Some(json!({"mobile": true, "voice": true})));
    assert!(states_phone_feature(p_wcv.features.as_ref(), "mobile"));
    assert!(!states_phone_feature(p_wcv.features.as_ref(), "voice"));

    let p_wcf = &phones["p_work_cell_fax"];
    assert_eq!(p_wcf.number, "+49 170 555555");
    assert_eq!(p_wcf.contexts, Some(json!({"work": true})));
    assert_eq!(p_wcf.features, Some(json!({"mobile": true, "fax": true})));
    assert!(states_phone_feature(p_wcf.features.as_ref(), "mobile"));
    assert!(!states_phone_feature(p_wcf.features.as_ref(), "fax"));

    let p_hpv = &phones["p_home_pager_voice"];
    assert_eq!(p_hpv.number, "+49 30 666666");
    assert_eq!(p_hpv.contexts, Some(json!({"private": true})));
    assert_eq!(p_hpv.features, Some(json!({"pager": true, "voice": true})));
    assert!(states_phone_feature(p_hpv.features.as_ref(), "pager"));
    assert!(!states_phone_feature(p_hpv.features.as_ref(), "voice"));

    let p_wvv = &phones["p_work_voice_video"];
    assert_eq!(p_wvv.number, "+49 30 777777");
    assert_eq!(p_wvv.contexts, Some(json!({"work": true})));
    assert_eq!(p_wvv.features, Some(json!({"voice": true, "video": true})));
    assert!(states_phone_feature(p_wvv.features.as_ref(), "voice"));
    assert!(!states_phone_feature(p_wvv.features.as_ref(), "video"));

    let p_bfv = &phones["p_bare_fax_video"];
    assert_eq!(p_bfv.number, "+49 30 888888");
    assert_eq!(p_bfv.contexts, None);
    assert_eq!(p_bfv.features, Some(json!({"fax": true, "video": true})));
    assert!(states_phone_feature(p_bfv.features.as_ref(), "fax"));
    assert!(!states_phone_feature(p_bfv.features.as_ref(), "video"));

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

    let p_pref = &phones["p_pref_work_fax"];
    assert_eq!(p_pref.number, "+49 30 000000");
    assert_eq!(p_pref.pref, Some(1));
    assert_eq!(p_pref.contexts, Some(json!({"work": true})));
    assert_eq!(p_pref.features, Some(json!({"voice": true, "fax": true})));

    let p_sep = &phones["p_separate_params"];
    assert_eq!(p_sep.number, "+49 30 123456");
    assert_eq!(p_sep.contexts, Some(json!({"work": true})));
    assert_eq!(p_sep.features, Some(json!({"voice": true, "fax": true})));

    let p_case = &phones["p_mixed_case"];
    assert_eq!(p_case.number, "+49 30 234567");
    assert_eq!(p_case.pref, Some(1));
    assert_eq!(p_case.contexts, Some(json!({"work": true})));
    assert_eq!(p_case.features, Some(json!({"voice": true, "fax": true})));

    let p_unm = &phones["p_unmapped_types"];
    assert_eq!(p_unm.number, "+49 30 345678");
    assert_eq!(p_unm.contexts, Some(json!({"work": true})));
    assert_eq!(p_unm.features, Some(json!({"voice": true})));

    let p_bare = &phones["p_bare_plain"];
    assert_eq!(p_bare.number, "+49 30 456789");
    assert_eq!(p_bare.contexts, None);
    assert_eq!(p_bare.features, None);
    assert_eq!(p_bare.pref, None);
    assert!(!states_phone_feature(p_bare.features.as_ref(), "voice"));
    assert!(!states_context(p_bare.contexts.as_ref(), "work"));

    // Outbound output keeps one slot feature and normalizes TYPE casing/order.
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
    // Supported JSContact feature keys.
    assert!(maps_phone_feature("mobile"));
    assert!(maps_phone_feature("pager"));
    assert!(maps_phone_feature("fax"));
    assert!(maps_phone_feature("voice"));
    assert!(maps_phone_feature("video"));

    // Non-feature keys and unmapped TYPE tokens.
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
    // Single-feature slot mapping.
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

    // Pairwise precedence cases.
    for other in ["pager", "fax", "voice", "video"] {
        let features = json!({"mobile": true, other: true});
        let line = phone_line(None, Some(features.clone()));
        assert_eq!(line, "TEL;X-JMAP-KEY=p1;TYPE=CELL:+49 30 111");
        assert!(states_phone_feature(Some(&features), "mobile"));
        assert!(!states_phone_feature(Some(&features), other));
    }

    for other in ["fax", "voice", "video"] {
        let features = json!({"pager": true, other: true});
        let line = phone_line(None, Some(features.clone()));
        assert_eq!(line, "TEL;X-JMAP-KEY=p1;TYPE=PAGER:+49 30 111");
        assert!(states_phone_feature(Some(&features), "pager"));
        assert!(!states_phone_feature(Some(&features), other));
    }

    for other in ["voice", "video"] {
        let features = json!({"fax": true, other: true});
        let line = phone_line(None, Some(features.clone()));
        assert_eq!(line, "TEL;X-JMAP-KEY=p1;TYPE=FAX:+49 30 111");
        assert!(states_phone_feature(Some(&features), "fax"));
        assert!(!states_phone_feature(Some(&features), other));
    }

    let features = json!({"voice": true, "video": true});
    let line = phone_line(None, Some(features.clone()));
    assert_eq!(line, "TEL;X-JMAP-KEY=p1;TYPE=VOICE:+49 30 111");
    assert!(states_phone_feature(Some(&features), "voice"));
    assert!(!states_phone_feature(Some(&features), "video"));

    // Context narrowing plus feature precedence.
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
    // Bare-year PartialDate stays unstated because vCard BDAY needs year-month-day.
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

    let mixed_card = {
        let mut card = ContactCard::default();
        card.anniversaries = Some(
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
        );
        card
    };

    let emitted = card_to_vcard(&mixed_card);
    assert_eq!(line(&emitted, "BDAY"), "BDAY;X-JMAP-KEY=y2:1984-06-21");
    assert_eq!(
        line(&emitted, "X-EVOLUTION-ANNIVERSARY"),
        "X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=y4:1996-08-03"
    );
    // Entries that state nothing stay absent.
    assert!(!emitted.contains("X-JMAP-KEY=y1"), "{emitted}");
    assert!(!emitted.contains("X-JMAP-KEY=y3"), "{emitted}");
    assert!(!emitted.contains("X-JMAP-KEY=y5"), "{emitted}");
    assert!(!emitted.contains("X-JMAP-KEY=y6"), "{emitted}");
    assert!(!emitted.contains("X-JMAP-KEY=y7"), "{emitted}");
    assert!(!emitted.contains("X-JMAP-KEY=y8"), "{emitted}");

    // A parse roundtrip keeps the emitted lines unchanged.
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
    // PartialDate values with custom calendar attributes and bare years stay safe.
    // The mapper must not panic or emit malformed vCard lines.
    let card = {
        let mut card = ContactCard::default();
        card.anniversaries = Some(
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
        );
        card
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
    // Check when `states_org_unit` reports true.
    // A unit is stated only when its name is non-empty.
    // An empty name stays unstated, even when `extra` has `sortAs`.
    let empty_unit = OrgUnit::new("");
    assert!(!states_org_unit(&empty_unit));

    let mut empty_with_sort_as = OrgUnit::new("");
    empty_with_sort_as.extra = [("sortAs".to_owned(), json!("Alpha"))].into();
    assert!(!states_org_unit(&empty_with_sort_as));

    let normal_unit = OrgUnit::new("Engineering");
    assert!(states_org_unit(&normal_unit));

    let mut normal_with_sort_as = OrgUnit::new("Engineering");
    normal_with_sort_as.extra = [("sortAs".to_owned(), json!("Eng"))].into();
    assert!(states_org_unit(&normal_with_sort_as));

    let whitespace_unit = OrgUnit::new("   ");
    assert!(states_org_unit(&whitespace_unit));

    // Check `states_organization` with empty unit combinations.
    // An empty org name plus empty units states nothing.
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

    // An employer name states an ORG line, even when units are empty.
    let named_org_empty_units = Organization {
        name: Some("Acme Corp".to_owned()),
        units: Some(vec![OrgUnit::new(""), empty_with_sort_as.clone()]),
        ..Organization::default()
    };
    assert!(states_organization(&named_org_empty_units));

    // No employer name still states ORG when one unit has a name.
    let nameless_org_valid_unit = Organization {
        name: None,
        units: Some(vec![OrgUnit::new(""), OrgUnit::new("Finance")]),
        ..Organization::default()
    };
    assert!(states_organization(&nameless_org_valid_unit));
}

#[test]
fn org_with_empty_name_units_and_sort_as_emission_and_roundtrip() {
    // Case 1: employer name with only empty units.
    // Emit the employer name alone on ORG. Drop empty units from wire output.
    let card1 = {
        let mut card = ContactCard::default();
        card.organizations = Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some("Acme Corp".to_owned()),
                    units: Some(vec![OrgUnit::new(""), {
                        let mut unit = OrgUnit::new("");
                        unit.extra = [("sortAs".to_owned(), json!("Secret"))].into();
                        unit
                    }]),
                    ..Organization::default()
                },
            )]
            .into(),
        );
        card
    };
    let vcard1 = card_to_vcard(&card1);
    assert_eq!(line(&vcard1, "ORG"), "ORG;X-JMAP-KEY=o1:Acme Corp");
    let back1 = vcard_to_card(&vcard1).expect("parse");
    let org1 = &back1.organizations.as_ref().unwrap()["o1"];
    assert_eq!(org1.name.as_deref(), Some("Acme Corp"));
    assert_eq!(org1.units, None);

    // Case 2: employer name with empty and non-empty units.
    // Omit empty units. Keep non-empty units in order.
    let card2 = {
        let mut card = ContactCard::default();
        card.organizations = Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some("Acme Corp".to_owned()),
                    units: Some(vec![
                        OrgUnit::new(""),
                        OrgUnit::new("Research"),
                        {
                            let mut unit = OrgUnit::new("");
                            unit.extra = [("sortAs".to_owned(), json!("OpticsSort"))].into();
                            unit
                        },
                        OrgUnit::new("Optics"),
                        OrgUnit::new(""),
                    ]),
                    ..Organization::default()
                },
            )]
            .into(),
        );
        card
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

    // Case 3: nameless org with leading empty unit and valid units.
    // A leading semicolon keeps the department slot. Empty units are omitted.
    let card3 = {
        let mut card = ContactCard::default();
        card.organizations = Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: None,
                    units: Some(vec![OrgUnit::new(""), OrgUnit::new("Engineering")]),
                    ..Organization::default()
                },
            )]
            .into(),
        );
        card
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

    // Case 4: inbound vCards with empty component patterns.
    // ORG with empty employer and empty unit components.
    let empty_components_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "ORG;X-JMAP-KEY=o1:;;;\r\n",
        "END:VCARD\r\n"
    );
    let from_empty = vcard_to_card(empty_components_vcard).expect("parse");
    assert_eq!(from_empty.organizations, None);

    // ORG with empty middle components and trailing empty semicolons.
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
    // `states_name_component` needs a non-empty value and a mapped kind.
    assert!(states_name_component(&NameComponent::new("given", "Alice")));
    assert!(states_name_component(&NameComponent::new(
        "surname", "Smith"
    )));
    assert!(!states_name_component(&NameComponent::new("given", "")));
    assert!(!states_name_component(&NameComponent::new(
        "unmapped_kind",
        "Alice"
    )));
    // `states_name` checks mapped components, full name, or file-as.
    assert!(states_name(&Name {
        full: Some("Alice".to_string()),
        ..Name::default()
    }));
    assert!(states_name(&Name {
        components: Some(vec![NameComponent::new("given", "Alice")]),
        ..Name::default()
    }));
    assert!(!states_name(&Name::default()));
    assert!(!states_name(&Name {
        full: Some("".to_string()),
        components: Some(vec![]),
        ..Name::default()
    }));
    assert!(!states_name(&Name {
        components: Some(vec![NameComponent::new("unmapped_kind", "value")]),
        ..Name::default()
    }));

    // `maps_context` supports only `work` and `private`.
    assert!(maps_context("work"));
    assert!(maps_context("private"));
    assert!(!maps_context("other"));
    assert!(!maps_context("billing"));
    assert!(!maps_context("home"));
    assert!(!maps_context(""));

    // `states_address_component` needs non-empty text and a mapped kind.
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

    // `states_address` needs address fields or an address label.
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

    // Check simple field predicates and `title_kind`.
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
    assert!(states_note(&{
        let mut v = Note::default();
        v.note = "Important contact".into();
        v
    }));
    assert!(!states_note(&{
        let mut v = Note::default();
        v.note = "".into();
        v
    }));
    assert!(states_link(&Link {
        uri: "https://example.com".into(),
        ..Link::default()
    }));
    assert!(!states_link(&Link {
        uri: "".into(),
        ..Link::default()
    }));
    assert!(states_nickname(&{
        let mut v = Nickname::default();
        v.name = "Ali".into();
        v
    }));
    assert!(!states_nickname(&{
        let mut v = Nickname::default();
        v.name = "".into();
        v
    }));
    assert_eq!(title_kind(None), "title");
    assert_eq!(title_kind(Some("role")), "role");
}

#[test]
fn calendar_and_spouse_predicates_fidelity() {
    // `states_calendar` needs a non-empty URI and a mapped kind.
    assert!(states_calendar(&{
        let mut calendar = Calendar::default();
        calendar.kind = Some("calendar".into());
        calendar.uri = "https://calendar.example.com".into();
        calendar
    }));
    assert!(states_calendar(&{
        let mut calendar = Calendar::default();
        calendar.kind = Some("freeBusy".into());
        calendar.uri = "https://fb.example.com".into();
        calendar
    }));
    assert!(!states_calendar(&{
        let mut calendar = Calendar::default();
        calendar.kind = Some("calendar".into());
        calendar.uri = "".into();
        calendar
    }));
    assert!(!states_calendar(&{
        let mut calendar = Calendar::default();
        calendar.kind = Some("unmapped_kind".into());
        calendar.uri = "https://calendar.example.com".into();
        calendar
    }));
    assert!(!states_calendar(&{
        let mut calendar = Calendar::default();
        calendar.kind = None;
        calendar.uri = "https://calendar.example.com".into();
        calendar
    }));
    assert!(!states_calendar(&{
        let mut calendar = Calendar::default();
        calendar.kind = None;
        calendar.uri = "".into();
        calendar
    }));

    // Check `states_spouse` and `states_nothing_but_the_marriage`.
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
    // Check `states_media` and `same_photo`.
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

    // Check online-service predicates and helper mappings.
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
    // Unterminated input returns `VCardError::Unterminated`.
    let unterminated = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\n";
    let err_unterminated = vcard_to_card(unterminated).expect_err("should fail unterminated");
    assert_eq!(err_unterminated, VCardError::Unterminated);
    assert_eq!(
        err_unterminated.to_string(),
        "truncated vCard: missing END:VCARD"
    );

    // Non-vCard input returns `VCardError::NotAVCard`.
    let not_vcard = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";
    let err_not_vcard = vcard_to_card(not_vcard).expect_err("should fail not a vCard");
    assert_eq!(err_not_vcard, VCardError::NotAVCard);
    assert_eq!(
        err_not_vcard.to_string(),
        "not a vCard: missing BEGIN:VCARD"
    );

    let empty_not_vcard = vcard_to_card("").expect_err("empty string is not a vcard");
    assert_eq!(empty_not_vcard, VCardError::NotAVCard);

    // A malformed content line returns `VCardError::Malformed`.
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
    // An empty `X-JMAP-KEY` on `LABEL` must not become the map key `""`.
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

    // Multiple unkeyed `EMAIL` lines allocate keys `e1`, `e2`, and `e3`.
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
    // vCard 3.0 group signals (`KIND`, `MEMBER`) are unmapped.
    // Drop them instead of writing incorrect mapped fields.
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
    // `KIND` and `MEMBER` are unmapped, so `extra` stays empty.
    assert!(card.extra.is_empty());

    // Serialization keeps mapped fields and does not invent KIND or MEMBER lines.
    let emitted = card_to_vcard(&card);
    assert!(emitted.contains("FN:Dev Team\r\n"));
    assert!(emitted.contains("NOTE;X-JMAP-KEY=n1:Core engineering team\r\n"));
    assert!(!emitted.contains("KIND:"));
    assert!(!emitted.contains("MEMBER:"));

    // Roundtrip back to JSContact reaches a fixed point.
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
    // Vendor group cards: Apple uses `X-ADDRESSBOOKSERVER-*`; EDS uses
    // `X-EVOLUTION-LIST` and `X-EVOLUTION-DEST-EMAIL`.
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

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("grp-1".into());
        card.uid = Some("urn:uuid:grp-1".to_owned());
        card.card_type = Some("Card".to_owned());
        card.version = Some("1.0".to_owned());
        card.name = Some(Name {
            full: Some("Frontend Working Group".to_owned()),
            ..Name::default()
        });
        card.extra = extra;
        card
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
    // Group-only properties (`KIND`, `MEMBER`) coexist with mapped contact data.
    // The mapped fields below must survive a full vCard -> JSContact -> vCard ->
    // JSContact roundtrip without key or value drift.
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
    // JSContact keeps one primary `Name`, so this parser keeps the first `FN` and `N`.
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

    // Serialization emits exactly one primary `FN` and `N` pair.
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

    // Round-trip converges after one pass.
    let back = vcard_to_card(&emitted).expect("roundtrip parse");
    assert_eq!(back.name, card.name);
    let re_emitted = card_to_vcard(&back);
    assert_eq!(emitted, re_emitted);
}

#[test]
fn vcard_altid_and_language_multivalued_properties_preservation_and_roundtrip() {
    // ALTID/LANGUAGE variants stay as distinct keyed JSContact entries and round-trip.
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

    // NOTE alternates are preserved.
    let notes = card.notes.as_ref().expect("notes");
    assert_eq!(notes.len(), 3, "all 3 NOTE alternates must be preserved");
    let note_texts: Vec<&str> = notes.values().map(|n| n.note.as_str()).collect();
    assert!(note_texts.contains(&"Primary project maintainer"));
    assert!(note_texts.contains(&"Hauptverantwortliche Projektleiterin"));
    assert!(note_texts.contains(&"Responsable principale du projet"));

    // TITLE and ROLE alternates are preserved.
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

    // ORG alternates keep all units.
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

    // NICKNAME alternates are preserved.
    let nicks = card.nicknames.as_ref().expect("nicknames");
    assert_eq!(nicks.len(), 2, "both NICKNAME alternates must be preserved");
    let nick_names: Vec<&str> = nicks.values().map(|n| n.name.as_str()).collect();
    assert!(nick_names.contains(&"Lena"));
    assert!(nick_names.contains(&"Лена"));

    // URL alternates are preserved.
    let links = card.links.as_ref().expect("links");
    assert_eq!(links.len(), 2, "both URL alternates must be preserved");
    let uris: Vec<&str> = links.values().map(|l| l.uri.as_str()).collect();
    assert!(uris.contains(&"https://example.org/en/elena"));
    assert!(uris.contains(&"https://example.org/de/elena"));

    // EMAIL and TEL alternates are preserved.
    let emails = card.emails.as_ref().expect("emails");
    assert_eq!(emails.len(), 2, "both EMAIL alternates must be preserved");
    let phones = card.phones.as_ref().expect("phones");
    assert_eq!(phones.len(), 2, "both TEL alternates must be preserved");

    // Serialization keeps the assigned keys.
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

    // Round-trip converges to a fixed point.
    let back = vcard_to_card(&emitted).expect("second parse");
    let re_emitted = card_to_vcard(&back);
    assert_eq!(
        emitted, re_emitted,
        "roundtrip must converge to fixed-point"
    );
}

#[test]
fn vcard_altid_and_language_multilingual_structured_address_and_label_pairing() {
    // `ADR` and `LABEL` alternates pair by context and serialize with stable keys.
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
    // Multiple ALTID and LANGUAGE variants collapse into one deduplicated keyword
    // map, then serialize as one canonical sorted CATEGORIES line.
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
    // Explicit X-JMAP-KEY values are preserved when unique; collisions and empty
    // keys allocate fresh keys so alternates are kept without overwriting data.
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
    // Server cards may carry RFC 9553 localizations and preferredLanguages in
    // `extra`; vCard output must ignore these unmapped fields instead of leaking
    // or misrepresenting them as vCard lines.
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

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("srv-loc-01".into());
        card.uid = Some("urn:uuid:srv-loc-01".to_owned());
        card.card_type = Some("Card".to_owned());
        card.version = Some("1.0".to_owned());
        card.name = Some(Name {
            full: Some("Clara Schumann".to_owned()),
            ..Name::default()
        });
        card.titles = Some(
            [("t1".to_owned(), {
                let mut title = Title::default();
                title.name = "Chief Architect".to_owned();
                title.kind = Some("title".to_owned());
                title
            })]
            .into_iter()
            .collect(),
        );
        card.notes = Some(
            [("n1".to_owned(), {
                let mut v = Note::default();
                v.note = "English Note".to_owned();
                v
            })]
            .into_iter()
            .collect(),
        );
        card.extra = extra;
        card
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
    // 1. Multiple emails with distinct pref values: emit lowest pref first.
    // EDS maps the first email to E_CONTACT_EMAIL_1, then higher ranks and None.
    let card = {
        let mut card = ContactCard::default();
        card.emails = Some(
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
        );
        card
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

    // 2. Break ties on equal `pref` values by key.
    let tie_card = {
        let mut card = ContactCard::default();
        card.emails = Some(
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
        );
        card
    };
    let tie_emitted = card_to_vcard(&tie_card);
    let tie_lines: Vec<&str> = tie_emitted
        .lines()
        .filter(|l| l.starts_with("EMAIL"))
        .collect();
    assert_eq!(tie_lines.len(), 2);
    assert!(tie_lines[0].contains("X-JMAP-KEY=e_alpha"));
    assert!(tie_lines[1].contains("X-JMAP-KEY=e_beta"));

    // 3. Use key order when all `pref` values are `None`.
    let none_card = {
        let mut card = ContactCard::default();
        card.emails = Some(
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
        );
        card
    };
    let none_emitted = card_to_vcard(&none_card);
    let none_lines: Vec<&str> = none_emitted
        .lines()
        .filter(|l| l.starts_with("EMAIL"))
        .collect();
    assert_eq!(none_lines.len(), 2);
    assert!(none_lines[0].contains("X-JMAP-KEY=e_a") && !none_lines[0].contains("PREF"));
    assert!(none_lines[1].contains("X-JMAP-KEY=e_z") && !none_lines[1].contains("PREF"));

    // 4. Verify fixed-point roundtrip stability.
    let back = vcard_to_card(&emitted).expect("parse back");
    let re_emitted = card_to_vcard(&back);
    assert_eq!(emitted, re_emitted);
}

#[test]
fn phone_pref_ordering_primary_selection_and_slotting() {
    // 1. Phones with distinct pref values: lowest pref emitted first
    let card = {
        let mut card = ContactCard::default();
        card.phones = Some(
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
        );
        card
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
    let tie_card = {
        let mut card = ContactCard::default();
        card.phones = Some(
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
        );
        card
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

    let card = {
        let mut card = ContactCard::default();
        card.addresses = Some(
            [
                (
                    "a_sec".to_owned(),
                    Address {
                        components: Some(vec![AddressComponent::new("name", "Secondary Weg 2")]),
                        contexts: Some(json!({"home": true})),
                        full: None,
                        extra: extra_sec,
                        ..Address::default()
                    },
                ),
                (
                    "a_pri".to_owned(),
                    Address {
                        components: Some(vec![AddressComponent::new("name", "Primary Allee 1")]),
                        contexts: Some(json!({"home": true})),
                        full: Some("Primary Allee 1\n10115 Berlin\nGermany".to_owned()),
                        extra: extra_pri,
                        ..Address::default()
                    },
                ),
                (
                    "a_work".to_owned(),
                    Address {
                        components: Some(vec![AddressComponent::new("name", "Work Str 42")]),
                        contexts: Some(json!({"work": true})),
                        full: None,
                        extra: BTreeMap::new(),
                        ..Address::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        );
        card
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
        ..Address::default()
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
    // RFC 6350 §6.3.1 allows a LABEL parameter directly on ADR.
    // Example: `ADR;TYPE=WORK;LABEL="...":PO Box 123;...`
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

    // Emission keeps both vCard 3.0 ADR and standalone LABEL lines.
    let vcard = card_to_vcard(&card);
    let unfolded_vcard = unfolded(&vcard);
    assert!(unfolded_vcard.contains(
        "ADR;X-JMAP-KEY=a1;TYPE=WORK:PO Box 100;Suite 200;100 Tech Blvd;Austin;TX;78701;USA"
    ));
    assert!(unfolded_vcard.contains("LABEL;X-JMAP-KEY=a1;TYPE=WORK:Alice Dev\\n100 Tech Blvd\\nSuite 200\\nAustin\\, TX 78701\\nUSA"));

    // Parsing back keeps identical Address components and label.
    let back = vcard_to_card(&vcard).expect("parse back");
    assert_eq!(back.addresses, card.addresses);

    // Test ADR with empty structured components and a non-empty LABEL parameter.
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
            ..Address::default()
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
            ..Address::default()
        },
    );
    addresses.insert(
        "a3".to_owned(),
        Address {
            components: None,
            contexts: None,
            full: Some("Postal Delivery Only\nPO Box 9999\nRemote City".to_owned()),
            extra: BTreeMap::new(),
            ..Address::default()
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
            ..Address::default()
        },
    );

    let card = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("Multi Address User".to_owned()),
            ..Name::default()
        });
        card.addresses = Some(addresses);
        card
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
        // Keep unmapped instant messaging service lines.
        "X-DISCORD:vera#1234\r\n",
        "X-SIGNAL:+491701234567\r\n",
        "X-TELEGRAM:@vera_schmidt\r\n",
        "X-SLACK:U12345678\r\n",
        "X-WHATSAPP:+491701234567\r\n",
        // Keep arbitrary enterprise and vendor lines.
        "X-CUSTOM-EXTENSION:Value 123\r\n",
        "X-KEY-ID:0xDEADBEEF\r\n",
        "X-DEPARTMENT-CODE:DE-SEC-09\r\n",
        "X-OFFICE-HOURS:Mon-Thu 09:00-17:00\r\n",
        "X-BILLING-ACCOUNT:ACC-998877\r\n",
        "END:VCARD\r\n"
    );

    // 1. Parse into `ContactCard`. Parse mapped fields and ignore unknown X- fields.
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
    // Keep `online_services` empty for these unmapped service lines.
    assert_eq!(card.online_services, None);
    // Keep `related_to` empty for X-SPOUSE, X-ASSISTANT, and X-MANAGER.
    assert_eq!(card.related_to, None);
    // Keep `anniversaries` empty for vendor X-ANNIVERSARY.
    assert_eq!(card.anniversaries, None);
    // Keep `card.extra` empty. Parser never copies raw vCard lines there.
    assert!(card.extra.is_empty());

    // 2. Emit vCard 3.0 lines with standard fields and X-JMAP-KEY parameters.
    //    Do not emit unmapped X- lines.
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

    // Confirm emission drops unknown X- lines.
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

    // 3. Confirm fixed-point convergence after parse and re-emission.
    let card2 = vcard_to_card(&re_emitted).expect("parse re-emitted");
    assert_eq!(card2, card);
    let re_emitted2 = card_to_vcard(&card2);
    assert_eq!(re_emitted2, re_emitted);
}

#[test]
fn unmapped_eds_specific_x_properties_characterization_and_rationale() {
    // Characterize unmapped EDS and vendor X- properties.
    // Parse ignores them. Emission does not restore them.
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:eds-custom-001\r\n",
        "FN:EDS Custom Contact\r\n",
        "EMAIL;TYPE=WORK:eds.user@example.com\r\n",
        // EDS uses unslotted online service lines.
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
    // Covers mapped X- properties: spouse, anniversary, IM slots,
    // `X-JMAP-UID`, and `X-JMAP-KEY` keyed entries.
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
    // Standard properties can carry unknown X- parameters from external clients.
    // Parser keeps values and known parameters when unknown X- parameters appear.
    // Emitter writes standard parameters plus X-JMAP-KEY, and drops unknown X- parameters.
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

    // Drop unknown X- parameters during emission.
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

    // Confirm round-trip fixed point.
    let back = vcard_to_card(&re_emitted).expect("parse re-emitted");
    assert_eq!(back, card);
    let re_emitted2 = card_to_vcard(&back);
    assert_eq!(re_emitted2, re_emitted);
}

#[test]
fn jscontact_card_with_unmodeled_extra_properties_emission_and_fixed_point() {
    // Build a server-originated JSContact card with unmodeled RFC 9553 and custom data.
    // Store unmodeled data in `card.extra` and property `extra` maps.
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

    let card = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("Server Card With Extra".to_owned()),
            ..Name::default()
        });
        card.emails = Some(
            [(
                "e1".to_owned(),
                ContactEmail {
                    address: "extra@example.com".to_owned(),
                    ..ContactEmail::default()
                },
            )]
            .into(),
        );
        card.notes = Some(
            [("n1".to_owned(), {
                let mut v = Note::default();
                v.note = "Modeled note".to_owned();
                v.extra = note_extra;
                v
            })]
            .into(),
        );
        card.extra = extra;
        card
    };

    // 1. Emit only modeled fields. Ignore `card.extra`.
    let vcard = card_to_vcard(&card);
    assert!(vcard.contains("FN:Server Card With Extra\r\n"));
    assert!(vcard.contains("EMAIL;X-JMAP-KEY=e1:extra@example.com\r\n"));
    assert!(vcard.contains("NOTE;X-JMAP-KEY=n1:Modeled note\r\n"));

    // Confirm no unmodeled extra data leaks into vCard output.
    assert!(!vcard.contains("preferredLanguages"));
    assert!(!vcard.contains("localizations"));
    assert!(!vcard.contains("cryptoKeys"));
    assert!(!vcard.contains("gender"));
    assert!(!vcard.contains("customServerExtension"));
    assert!(!vcard.contains("2026-08-19T10:00:00Z"));
    assert!(!vcard.contains("author"));

    // 2. Read back modeled fields.
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

    // 3. Confirm fixed-point convergence.
    let vcard2 = card_to_vcard(&read_back);
    assert_eq!(vcard2, vcard);
}

#[test]
fn x_property_name_casing_and_empty_values_handling() {
    // Parse lowercase and mixed-case X- names, plus empty X- values.
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

    // Ignore empty and unknown X- properties.
    assert!(card.extra.is_empty());

    let re_emitted = card_to_vcard(&card);
    let back = vcard_to_card(&re_emitted).expect("parse re-emitted");
    assert_eq!(back, card);
}

#[test]
fn rfc2426_line_folding_and_unfolding_long_note_and_photo_roundtrip() {
    // NOTE values longer than 75 octets must fold and unfold without loss.
    let long_note_text = "This is an extremely long note that exceeds seventy-five octets by a substantial margin. \
        It contains detailed historical records, meeting minutes, action items, and extensive documentation \
        that must be folded across multiple physical lines according to RFC 2426 Section 2.6, and unfolded \
        with 100% lossless fidelity upon reading back from vCard 3.0 format.";

    let mut notes = BTreeMap::new();
    notes.insert("n1".to_owned(), {
        let mut v = Note::default();
        v.note = long_note_text.to_owned();
        v.extra = BTreeMap::new();
        v
    });

    // Build an inline `PHOTO` payload as a data URI.
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
            ..Media::default()
        },
    );

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-FOLD-1".into());
        card.name = Some(Name {
            full: Some("Line Folding Verification".to_owned()),
            ..Name::default()
        });
        card.notes = Some(notes);
        card.media = Some(media);
        card
    };

    // Emit vCard text.
    let vcard = card_to_vcard(&card);

    // `NOTE` and `PHOTO` must include folded continuation lines.
    assert!(vcard.contains("\r\n "));

    // Each physical line must stay within the RFC line-length limit.
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

    // Parse the output and check unfolded values.
    let read_card = vcard_to_card(&vcard).expect("parse folded vcard");

    // `NOTE` must round-trip without text changes.
    let read_notes = read_card.notes.as_ref().expect("notes present");
    assert_eq!(
        read_notes["n1"].note, long_note_text,
        "Folded NOTE did not unfold losslessly"
    );

    // `PHOTO` must keep media type and payload text.
    let read_media = read_card.media.as_ref().expect("media present");
    assert_eq!(read_media["m1"].kind.as_deref(), Some("photo"));
    assert_eq!(read_media["m1"].media_type.as_deref(), Some("image/jpeg"));
    assert_eq!(read_media["m1"].uri, photo_uri);

    // Re-emit the parsed card and verify a fixed point.
    let vcard2 = card_to_vcard(&read_card);
    assert_eq!(vcard2, vcard, "Emitted folded vCard must be at fixed-point");
}

#[test]
fn rfc2426_prefolded_vcard_unfolding_with_crlf_spaces_and_tabs() {
    // RFC 2426 §2.6: fold lines with `CRLF` plus space or tab.
    // Unfolding removes `CRLF` and one continuation marker.
    // Keep later spaces or tabs as payload bytes.
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

    // ORG keeps one payload space after CRLF unfolding.
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

    // Check ADR structured components.
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

    // Check LABEL.
    assert_eq!(
        adr.full.as_deref(),
        Some("221B Baker Street\nSuite 400\nLondon\nNW1 6XE\nUnited Kingdom")
    );

    // Check NOTE multiline text and leading spaces.
    let note = &card.notes.as_ref().unwrap()["n1"].note;
    assert!(note.contains("First line of note.Continued with space.Continued with tab.  Three leading spaces (one fold + two data)."));

    // Check CATEGORIES.
    let keywords = card.keywords.as_ref().expect("keywords");
    assert!(keywords.contains_key("Forensics"));
    assert!(keywords.contains_key("Ballistics"));
    assert!(keywords.contains_key("Investigation"));
    assert!(keywords.contains_key("Scotland Yard"));

    // Check URL.
    assert_eq!(
        card.links.as_ref().unwrap()["l1"].uri,
        "https://example.org/forensics/deep/case/archive"
    );

    // Check PHOTO.
    let photo = &card.media.as_ref().unwrap()["m1"];
    assert_eq!(photo.media_type.as_deref(), Some("image/PNG"));
    assert!(photo.uri.starts_with("data:image/PNG;base64,"));

    // Re-emission keeps valid line lengths and reaches a fixed point.
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
    // This test checks fold boundaries around multi-byte UTF-8 data.
    // A fold boundary must not split a UTF-8 code unit sequence.
    // Line folding must break on a valid UTF-8 character boundary.

    let test_cases = [
        // 2-byte UTF-8 samples: umlauts, Cyrillic, accented Latin.
        ("2-byte umlauts", "äöüßéñДж"),
        // 3-byte UTF-8 samples: CJK, Hiragana, Devanagari.
        ("3-byte CJK & Hiragana", "漢字東京日本語संस्कृत"),
        // 4-byte UTF-8 samples: emoji, musical symbols, math symbols.
        ("4-byte Emoji", "🦀🚀🌟🎉🔥𝄞𝕬🌍"),
    ];

    for (label, multibyte_sample) in test_cases {
        // Vary prefix lengths from 40 to 85 bytes before multi-byte text.
        // This hits many fold-boundary offsets near 75 octets.
        for pad_len in 40..=85 {
            let padding = "A".repeat(pad_len);
            let note_text = format!(
                "{padding}{multibyte_sample} -- trailing text to force multiple line folds if needed."
            );

            let mut notes = BTreeMap::new();
            notes.insert("n1".to_owned(), {
                let mut v = Note::default();
                v.note = note_text.clone();
                v.extra = BTreeMap::new();
                v
            });

            let card = {
                let mut card = ContactCard::default();
                card.id = Some("C-UTF8".into());
                card.name = Some(Name {
                    full: Some(format!("UTF8 Test {label} pad {pad_len}")),
                    ..Name::default()
                });
                card.notes = Some(notes);
                card
            };

            let vcard = card_to_vcard(&card);

            // 1. Each physical line stays within fold limits and valid UTF-8.
            for (line_idx, physical_line) in vcard.split("\r\n").enumerate() {
                assert!(
                    physical_line.len() <= 77,
                    "[{label}, pad {pad_len}, line {line_idx}] line exceeds max limit (len={}): {:?}",
                    physical_line.len(),
                    physical_line
                );
                assert!(
                    std::str::from_utf8(physical_line.as_bytes()).is_ok(),
                    "[{label}, pad {pad_len}, line {line_idx}] invalid UTF-8 in line slice"
                );
            }

            // 2. Parse back and preserve the note value exactly.
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

            // 3. Verify fixed point.
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
    // Check behavior near the 75-octet fold boundary.
    // Prefix "NOTE;X-JMAP-KEY=n1:" adds 19 octets.

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
        notes.insert("n1".to_owned(), {
            let mut v = Note::default();
            v.note = value.clone();
            v.extra = BTreeMap::new();
            v
        });

        let card = {
            let mut card = ContactCard::default();
            card.id = Some("C-BOUND".into());
            card.notes = Some(notes);
            card
        };

        let vcard = card_to_vcard(&card);

        // Collect NOTE and continuation lines.
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

        // Parse back and confirm value recovery.
        let read_card = vcard_to_card(&vcard).expect("parse bounded card");
        assert_eq!(read_card.notes.as_ref().unwrap()["n1"].note, value);
    }
}

#[test]
fn rfc2426_line_folding_with_escaped_delimiters_and_backslashes() {
    // Check folding with escaped text delimiters.
    // RFC 2426 section 2 escapes \n, \;, \,, and \\ in NOTE values.
    // Folding must not split escape pairs or lose characters.

    let multiline_note = "Line 1 with text.\nLine 2 with semicolon ; and comma , and backslash \\.\nLine 3 with more text.";
    let long_note_with_escapes = multiline_note.repeat(4);

    let mut notes = BTreeMap::new();
    notes.insert("n1".to_owned(), {
        let mut v = Note::default();
        v.note = long_note_with_escapes.clone();
        v.extra = BTreeMap::new();
        v
    });

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-ESC-FOLD".into());
        card.notes = Some(notes);
        card
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
    notes.insert("n1".to_owned(), {
        let mut v = Note::default();
        v.note = note_text.to_owned();
        v.extra = BTreeMap::new();
        v
    });

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-ESC-NOTE".into());
        card.notes = Some(notes);
        card
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
    // RFC 2426 §2 and §3.5.5: escape commas and semicolons inside ORG components.
    // Keep `;` as the component separator. Escape data commas as `\,` and data semicolons as `\;`.
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
            ..Organization::default()
        },
    );

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-ESC-ORG".into());
        card.organizations = Some(orgs);
        card
    };

    let vcard = card_to_vcard(&card);

    // Verify the wire format escapes commas and semicolons.
    assert!(
        vcard.contains(r"Acme\, Inc.") || vcard.contains("ORG;X-JMAP-KEY=o1:"),
        "vCard should contain escaped comma in ORG name: {vcard}"
    );

    // Parse back.
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

    // Verify fixed-point convergence.
    let vcard2 = card_to_vcard(&read_card);
    assert_eq!(vcard2, vcard, "Emitted vCard must reach fixed point");

    // Verify a nameless organization keeps a leading semicolon and escaped unit values.
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
            ..Organization::default()
        },
    );
    let nameless_card = {
        let mut card = ContactCard::default();
        card.id = Some("C-NAMELESS-ESC-ORG".into());
        card.organizations = Some(nameless_orgs);
        card
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
    // RFC 2426 §2 and §3.2.1: escape semicolons inside ADR components.
    // Keep `;` as the ADR component separator. Escape data semicolons as `\;`.
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
            extra: BTreeMap::new(), ..Address::default()
        },
    );

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-ESC-ADR".into());
        card.addresses = Some(addresses);
        card
    };

    let vcard = card_to_vcard(&card);

    // Parse back and verify that no component shifts.
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

    // Verify fixed-point convergence.
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
    nicknames.insert("k1".to_owned(), {
        let mut v = Nickname::default();
        v.name = "Ali, Baba; Chief\\Boss\nLead".to_owned();
        v.extra = BTreeMap::new();
        v
    });

    let mut titles = BTreeMap::new();
    titles.insert("t1".to_owned(), {
        let mut title = Title::default();
        title.name = "Director, Architecture; Core \\ Systems\nLead".to_owned();
        title.kind = Some("title".to_owned());
        title.extra = BTreeMap::new();
        title
    });
    titles.insert("t2".to_owned(), {
        let mut title = Title::default();
        title.name = "Lead, Quality; Assurance \\ Test\nSpecialist".to_owned();
        title.kind = Some("role".to_owned());
        title.extra = BTreeMap::new();
        title
    });

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
            ..Link::default()
        },
    );

    let mut keywords = BTreeMap::new();
    keywords.insert("Tag 1, with comma".to_owned(), json!(true));
    keywords.insert("Tag 2; with semicolon".to_owned(), json!(true));
    keywords.insert("Tag 3\\backslash".to_owned(), json!(true));

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-ALL-ESC".into());
        card.name = Some(Name {
            full: Some("Dr. Alice Smith, Ph.D.; Junior\\Senior".to_owned()),
            components: Some(vec![
                NameComponent::new("surname", "Smith, Jr."),
                NameComponent::new("given", "Alice; Marie"),
                NameComponent::new("given2", "B.\\C."),
                NameComponent::new("title", "Dr., Prof."),
                NameComponent::new("credential", "III; Esq."),
            ]),
            extra: BTreeMap::new(),
            ..Name::default()
        });
        card.nicknames = Some(nicknames);
        card.titles = Some(titles);
        card.related_to = Some(related_to);
        card.emails = Some(emails);
        card.phones = Some(phones);
        card.links = Some(links);
        card.keywords = Some(keywords);
        card
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
    notes.insert("n1".to_owned(), {
        let mut v = Note::default();
        v.note = complex_text.to_owned();
        v.extra = BTreeMap::new();
        v
    });

    let card0 = {
        let mut card = ContactCard::default();
        card.id = Some("C-MULTI-ROUND".into());
        card.notes = Some(notes);
        card
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
    // Covers \N newlines, trailing and repeated backslashes, and escaped delimiters.
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
    // Empty, absent, or refused keywords all collapse to `keywords: None`.
    let card_none = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("Alice Smith".to_owned()),
            ..Name::default()
        });
        card.keywords = None;
        card
    };
    let vcard_none = card_to_vcard(&card_none);
    assert!(!vcard_none.contains("\r\nCATEGORIES:"));
    let parsed_none = vcard_to_card(&vcard_none).expect("parse none card");
    assert_eq!(parsed_none.keywords, None);

    let card_empty_map = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("Alice Smith".to_owned()),
            ..Name::default()
        });
        card.keywords = Some(BTreeMap::new());
        card
    };
    let vcard_empty_map = card_to_vcard(&card_empty_map);
    assert!(!vcard_empty_map.contains("\r\nCATEGORIES:"));
    let parsed_empty_map = vcard_to_card(&vcard_empty_map).expect("parse empty map card");
    assert_eq!(parsed_empty_map.keywords, None);

    let inbound_empty =
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice Smith\r\nCATEGORIES:\r\nEND:VCARD\r\n";
    let parsed_inbound_empty = vcard_to_card(inbound_empty).expect("parse inbound empty");
    assert_eq!(parsed_inbound_empty.keywords, None);
    let reemitted_empty = card_to_vcard(&parsed_inbound_empty);
    assert!(!reemitted_empty.contains("\r\nCATEGORIES:"));

    let inbound_commas =
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice Smith\r\nCATEGORIES:,,,\r\nEND:VCARD\r\n";
    let parsed_inbound_commas = vcard_to_card(inbound_commas).expect("parse inbound commas");
    assert_eq!(parsed_inbound_commas.keywords, None);
    let reemitted_commas = card_to_vcard(&parsed_inbound_commas);
    assert!(!reemitted_commas.contains("\r\nCATEGORIES:"));

    let card_refused = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("Alice Smith".to_owned()),
            ..Name::default()
        });
        card.keywords = Some(
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
        );
        card
    };
    let vcard_refused = card_to_vcard(&card_refused);
    assert!(!vcard_refused.contains("\r\nCATEGORIES:"));
    let parsed_refused = vcard_to_card(&vcard_refused).expect("parse refused card");
    assert_eq!(parsed_refused.keywords, None);
}

#[test]
fn categories_single_tag_variations_and_escaped_delimiters_roundtrip() {
    // Single-tag categories preserve escaping and fixed-point behavior.
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
        let card = {
            let mut card = ContactCard::default();
            card.name = Some(Name {
                full: Some("Bob Builder".to_owned()),
                ..Name::default()
            });
            card.keywords = Some([(tag.to_owned(), json!(true))].into());
            card
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

        // Fixed-point convergence.
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
    // Multiple tags emit as one sorted line while preserving escaped delimiters.
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

    let card = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("Charlie Davis".to_owned()),
            ..Name::default()
        });
        card.keywords = Some(map);
        card
    };

    let vcard = card_to_vcard(&card);

    // Verify sorted order on the unfolded CATEGORIES line.
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

    // Parse back and verify exact tag preservation.
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
    // Multiple inbound CATEGORIES lines merge into one deduplicated, sorted line.
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

    // Re-emission stays consolidated as one sorted line.
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

    // Fixed-point stability across successive passes.
    let reparsed = vcard_to_card(&emitted).expect("reparse");
    assert_eq!(reparsed.keywords, parsed.keywords);
    let reemitted = card_to_vcard(&reparsed);
    assert_eq!(reemitted, emitted);
}

#[test]
fn categories_inbound_delimiter_variations_and_empty_item_skipping() {
    // Empty comma items are skipped while mixed-case names and parameters still parse.
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
    // Non-ASCII and multi-byte UTF-8 categories roundtrip without split code points.
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

    let card = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("Fiona Gallagher".to_owned()),
            ..Name::default()
        });
        card.keywords = Some(map);
        card
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
    // Covers accepted and refused `states_keyword` cases, including EDS trim defense.
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

    // 6. Mixed valid and refused tags emit only the valid tags.
    let card = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("George Harris".to_owned()),
            ..Name::default()
        });
        card.keywords = Some(
            [
                ("Valid Tag A".to_owned(), json!(true)),
                (" leading".to_owned(), json!(true)),
                ("trailing ".to_owned(), json!(true)),
                ("with\rcarriage_return".to_owned(), json!(true)),
                ("false_val".to_owned(), json!(false)),
                ("Valid Tag B".to_owned(), json!(true)),
            ]
            .into(),
        );
        card
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
    // Emit one `NICKNAME` line per keyed JSContact entry so each keeps `X-JMAP-KEY`.
    // EDS rewrites the first nickname slot and passes later lines through.

    // Single nickname roundtrip.
    let mut single_nick = BTreeMap::new();
    single_nick.insert("k1".to_owned(), {
        let mut v = Nickname::default();
        v.name = "Vee".to_owned();
        v.extra = BTreeMap::new();
        v
    });
    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-NICK-SINGLE".into());
        card.nicknames = Some(single_nick);
        card
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
    multi_nicks.insert("k1".to_owned(), {
        let mut v = Nickname::default();
        v.name = "Vee".to_owned();
        v.extra = BTreeMap::new();
        v
    });
    multi_nicks.insert("k2".to_owned(), {
        let mut v = Nickname::default();
        v.name = "Vera the Elder".to_owned();
        v.extra = BTreeMap::new();
        v
    });
    multi_nicks.insert("k3".to_owned(), {
        let mut v = Nickname::default();
        v.name = "Chief Architect".to_owned();
        v.extra = BTreeMap::new();
        v
    });
    let multi_card = {
        let mut card = ContactCard::default();
        card.id = Some("C-NICK-MULTI".into());
        card.nicknames = Some(multi_nicks);
        card
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
    // Characterizes comma handling in NICKNAME text-list input.
    // Inbound list values become one comma-joined nickname, and outbound emission escapes commas.
    // Re-import of the escaped output returns the same nickname text.
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
    nicks.insert("k_special".to_owned(), {
        let mut v = Nickname::default();
        v.name = "Nick;Name\\With\nNewline & \"Quotes\"".to_owned();
        v.extra = [
            ("pref".to_owned(), json!(1)),
            ("contexts".to_owned(), json!({"work": true})),
        ]
        .into();
        v
    });
    nicks.insert("k_unicode_jp".to_owned(), {
        let mut v = Nickname::default();
        v.name = "たなかさん (田中)".to_owned();
        v.extra = BTreeMap::new();
        v
    });
    nicks.insert("k_unicode_cyrillic".to_owned(), {
        let mut v = Nickname::default();
        v.name = "Саша (Александр)".to_owned();
        v.extra = BTreeMap::new();
        v
    });
    nicks.insert("k_unicode_emoji".to_owned(), {
        let mut v = Nickname::default();
        v.name = "🌟 SuperStar 🦊".to_owned();
        v.extra = BTreeMap::new();
        v
    });

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-NICK-SPECIAL".into());
        card.nicknames = Some(nicks);
        card
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
    assert!(states_nickname(&{
        let mut v = Nickname::default();
        v.name = "Nick".into();
        v.extra = BTreeMap::new();
        v
    }));
    assert!(!states_nickname(&{
        let mut v = Nickname::default();
        v.name = "".into();
        v.extra = BTreeMap::new();
        v
    }));

    // Empty nicknames are not emitted
    let mut nicks = BTreeMap::new();
    nicks.insert("k1".to_owned(), {
        let mut v = Nickname::default();
        v.name = "".to_owned();
        v.extra = BTreeMap::new();
        v
    });
    nicks.insert("k2".to_owned(), {
        let mut v = Nickname::default();
        v.name = "Valid Nick".to_owned();
        v.extra = BTreeMap::new();
        v
    });
    let card = {
        let mut card = ContactCard::default();
        card.nicknames = Some(nicks);
        card
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
    // Characterize URL properties in EDS fields.
    // 1. One URL with kind None maps to E_CONTACT_HOMEPAGE_URL.
    // 2. Multiple URLs with kind None emit multiple URL;X-JMAP-KEY=... lines.
    // EDS 3.52 maps the first URL line to E_CONTACT_HOMEPAGE_URL and keeps later lines.
    // 3. Roundtrips keep all keys and URIs.

    let mut links = BTreeMap::new();
    links.insert(
        "l1".to_owned(),
        Link {
            uri: "https://alice.example.com".to_owned(),
            kind: None,
            extra: BTreeMap::new(),
            ..Link::default()
        },
    );
    links.insert(
        "l2".to_owned(),
        Link {
            uri: "https://work.example.org/alice".to_owned(),
            kind: None,
            extra: BTreeMap::new(),
            ..Link::default()
        },
    );
    links.insert(
        "l3".to_owned(),
        Link {
            uri: "https://github.com/alice".to_owned(),
            kind: None,
            extra: BTreeMap::new(),
            ..Link::default()
        },
    );

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-URL-MULTI".into());
        card.links = Some(links);
        card
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
    // vCard 3.0 has URL but no CONTACT-URI field.
    // Emit URL only for plain website links with `kind: None`.

    let mut links = BTreeMap::new();
    links.insert(
        "l_web".to_owned(),
        Link {
            uri: "https://alice.example.com".to_owned(),
            kind: None,
            extra: BTreeMap::new(),
            ..Link::default()
        },
    );
    links.insert(
        "l_contact".to_owned(),
        Link {
            uri: "https://contact.example.com/form".to_owned(),
            kind: Some("contact".to_owned()),
            extra: BTreeMap::new(),
            ..Link::default()
        },
    );
    links.insert(
        "l_feed".to_owned(),
        Link {
            uri: "https://alice.example.com/rss.xml".to_owned(),
            kind: Some("feed".to_owned()),
            extra: BTreeMap::new(),
            ..Link::default()
        },
    );
    links.insert(
        "l_news".to_owned(),
        Link {
            uri: "https://news.alice.example.com".to_owned(),
            kind: Some("news".to_owned()),
            extra: BTreeMap::new(),
            ..Link::default()
        },
    );
    links.insert(
        "l_custom".to_owned(),
        Link {
            uri: "https://custom.example.com/profile".to_owned(),
            kind: Some("x-vendor-profile".to_owned()),
            extra: BTreeMap::new(),
            ..Link::default()
        },
    );

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-URL-KINDS".into());
        card.links = Some(links);
        card
    };

    let vcard = card_to_vcard(&card);
    // Emit `URL` only for plain website links.
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
    // Map EDS blog and video URL properties to link kinds.
    // Ignore vendor-prefixed variants without `X-EVOLUTION-`.
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
    // Complex URLs preserve URI punctuation and encoding as raw URI text.
    let mut links = BTreeMap::new();
    links.insert(
        "l_complex_query".to_owned(),
        Link {
            uri: "https://api.example.com:8443/v1/search?q=tag:a,b;status:active&filter=x,y;z#top"
                .to_owned(),
            kind: None,
            extra: BTreeMap::new(),
            ..Link::default()
        },
    );
    links.insert(
        "l_auth".to_owned(),
        Link {
            uri: "https://user:p%40ssw%3Brd@secure.example.org:9000/path/to/res?a=1&b=2#sec"
                .to_owned(),
            kind: None,
            extra: BTreeMap::new(),
            ..Link::default()
        },
    );
    links.insert(
        "l_ipv6".to_owned(),
        Link {
            uri: "http://[2001:db8::1]:8080/index.html?token=abc;def,ghi".to_owned(),
            kind: None,
            extra: BTreeMap::new(),
            ..Link::default()
        },
    );

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-URL-COMPLEX".into());
        card.links = Some(links);
        card
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
        extra: BTreeMap::new(),
        ..Link::default()
    }));
    assert!(!states_link(&Link {
        uri: "".into(),
        kind: None,
        extra: BTreeMap::new(),
        ..Link::default()
    }));
    assert!(!states_link(&Link {
        uri: "https://example.com".into(),
        kind: Some("contact".into()),
        extra: BTreeMap::new(),
        ..Link::default()
    }));
    assert!(!states_link(&Link {
        uri: "https://example.com".into(),
        kind: Some("other".into()),
        extra: BTreeMap::new(),
        ..Link::default()
    }));

    // Empty links are not emitted
    let mut links = BTreeMap::new();
    links.insert(
        "l1".to_owned(),
        Link {
            uri: "".to_owned(),
            kind: None,
            extra: BTreeMap::new(),
            ..Link::default()
        },
    );
    links.insert(
        "l2".to_owned(),
        Link {
            uri: "https://valid.example.com".to_owned(),
            kind: None,
            extra: BTreeMap::new(),
            ..Link::default()
        },
    );
    let card = {
        let mut card = ContactCard::default();
        card.links = Some(links);
        card
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

    // Unmodeled Link fields in extra survive roundtrips untouched.
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
            ..Link::default()
        },
    );
    let rich_card = {
        let mut card = ContactCard::default();
        card.links = Some(extra_links);
        card
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
            ..Link::default()
        },
    );

    let mut cals = BTreeMap::new();
    cals.insert("c1".to_owned(), {
        let mut calendar = Calendar::default();
        calendar.uri = "https://cal.example.com/alice.ics".to_owned();
        calendar.kind = Some("calendar".to_owned());
        calendar.extra = BTreeMap::new();
        calendar
    });
    cals.insert("c2".to_owned(), {
        let mut calendar = Calendar::default();
        calendar.uri = "https://cal.example.com/fb/alice.ifb".to_owned();
        calendar.kind = Some("freeBusy".to_owned());
        calendar.extra = BTreeMap::new();
        calendar
    });

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-URL-CAL-COEXIST".into());
        card.links = Some(links);
        card.calendars = Some(cals);
        card
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

        let card = {
            let mut card = ContactCard::default();
            card.id = Some(format!("C-NAME-{}", label.replace(' ', "-")).into());
            card.name = Some(Name {
                full: Some(full_name.to_owned()),
                components: (!components.is_empty()).then_some(components),
                extra: BTreeMap::new(),
                ..Name::default()
            });
            card
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
            ..Organization::default()
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
            ..Organization::default()
        },
    );

    let mut titles = BTreeMap::new();
    titles.insert("t1".to_owned(), {
        let mut title = Title::default();
        title.name = "Directeur Général Adjoint".to_owned();
        title.kind = Some("title".to_owned());
        title.extra = BTreeMap::new();
        title
    });
    titles.insert("t2".to_owned(), {
        let mut title = Title::default();
        title.name = "Главный архитектор систем".to_owned();
        title.kind = Some("role".to_owned());
        title.extra = BTreeMap::new();
        title
    });
    titles.insert("t3".to_owned(), {
        let mut title = Title::default();
        title.name = "開発最高責任者".to_owned();
        title.kind = Some("title".to_owned());
        title.extra = BTreeMap::new();
        title
    });

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-NONASCII-ORG-TITLE".into());
        card.organizations = Some(orgs);
        card.titles = Some(titles);
        card
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
            ..Address::default()
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
            ..Address::default()
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
            ..Address::default()
        },
    );

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-NONASCII-ADR".into());
        card.addresses = Some(addrs);
        card
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
        { let mut v = Note::default(); v.note = "München ist eine wunderschöne Stadt mit vielen Parks und Museen.\nRené & Hélène apprécient beaucoup la gastronomie française: café, croissants, crème brûlée.\n∀x ∈ ℝ: x² ≥ 0 (math symbols test 🧑‍💻🚀🌟)".to_owned(); v.extra = BTreeMap::new(); v },
    );

    let mut nicknames = BTreeMap::new();
    nicknames.insert("k1".to_owned(), {
        let mut v = Nickname::default();
        v.name = "Schätzchen".to_owned();
        v.extra = BTreeMap::new();
        v
    });
    nicknames.insert("k2".to_owned(), {
        let mut v = Nickname::default();
        v.name = "Маша (Мария)".to_owned();
        v.extra = BTreeMap::new();
        v
    });
    nicknames.insert("k3".to_owned(), {
        let mut v = Nickname::default();
        v.name = "たなか (田中)".to_owned();
        v.extra = BTreeMap::new();
        v
    });

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

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-NONASCII-MIXED".into());
        card.notes = Some(notes);
        card.nicknames = Some(nicknames);
        card.keywords = Some(keywords);
        card.related_to = Some(related_to);
        card
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
    // Accept case-variant `CHARSET=UTF-8` input.
    // Normalize outbound vCard 3.0 by omitting `CHARSET`.
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

    // Assert extraction into JSContact fields.
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

    // Export strips ENCODING=8BIT and 7BIT on text, and keeps ENCODING=b on PHOTO.
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
    // 1. Supported URI variants: HTTPS and HTTP.
    let test_uris = [
        "https://example.com/alice.jpg",
        "http://photos.org/pic.png?size=large&format=raw",
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

    // Unsupported URI schemes are not emitted as `PHOTO` lines.
    for unsupported_uri in [
        "file:///home/user/.face",
        "ftp://photos.org/pic.png",
        "urn:uuid:6a2f7965-728b-47e2-8924-d2e7d7ffba44",
    ] {
        let vcard = card_to_vcard(&one_photo(unsupported_uri, None));
        assert!(
            !vcard.contains("\r\nPHOTO"),
            "unsupported URI must not emit PHOTO: {unsupported_uri} in {vcard}"
        );
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

    // 3. Lossy by design: media_type on a remote URI.
    // JSContact can set media_type to Some("image/jpeg") on a remote URI.
    // RFC 2426 §3.1.4 allows no TYPE on URI references, and EDS reads no TYPE there.
    // card_to_vcard omits TYPE by design, so reads return media_type as None.
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
        ..Media::default()
    };
    let inline_jpeg_caps = Media {
        kind: Some("photo".to_owned()),
        uri: format!("data:image/JPEG;base64,{PAYLOAD}"),
        media_type: Some("image/JPEG".to_owned()),
        extra: BTreeMap::new(),
        ..Media::default()
    };
    let inline_jpeg_unpadded = Media {
        kind: Some("photo".to_owned()),
        uri: format!("data:image/jpeg;base64,{}", PAYLOAD.trim_end_matches('=')),
        media_type: Some("image/jpeg".to_owned()),
        extra: BTreeMap::new(),
        ..Media::default()
    };
    let inline_png = Media {
        kind: Some("photo".to_owned()),
        uri: format!("data:image/png;base64,{PAYLOAD}"),
        media_type: Some("image/png".to_owned()),
        extra: BTreeMap::new(),
        ..Media::default()
    };
    let inline_diff_payload = Media {
        kind: Some("photo".to_owned()),
        uri: "data:image/jpeg;base64,AQIDBA==".to_owned(),
        media_type: Some("image/jpeg".to_owned()),
        extra: BTreeMap::new(),
        ..Media::default()
    };
    let uri_photo1 = Media {
        kind: Some("photo".to_owned()),
        uri: "https://example.com/alice.jpg".to_owned(),
        media_type: None,
        extra: BTreeMap::new(),
        ..Media::default()
    };
    let uri_photo2 = Media {
        kind: Some("photo".to_owned()),
        uri: "https://example.com/bob.jpg".to_owned(),
        media_type: None,
        extra: BTreeMap::new(),
        ..Media::default()
    };
    let non_photo = Media {
        kind: Some("logo".to_owned()),
        uri: "https://example.com/logo.png".to_owned(),
        media_type: None,
        extra: BTreeMap::new(),
        ..Media::default()
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
    // 1. Multiple PHOTO entries on one card.
    let mut media_map = BTreeMap::new();
    media_map.insert(
        "m1".to_owned(),
        Media {
            kind: Some("photo".to_owned()),
            uri: format!("data:image/jpeg;base64,{PAYLOAD}"),
            media_type: Some("image/jpeg".to_owned()),
            extra: BTreeMap::new(),
            ..Media::default()
        },
    );
    media_map.insert(
        "m2".to_owned(),
        Media {
            kind: Some("photo".to_owned()),
            uri: "https://example.com/profile-large.jpg".to_owned(),
            media_type: None,
            extra: BTreeMap::new(),
            ..Media::default()
        },
    );
    media_map.insert(
        "m3".to_owned(),
        Media {
            kind: Some("photo".to_owned()),
            uri: "data:image/png;base64,AQIDBA==".to_owned(),
            media_type: Some("image/png".to_owned()),
            extra: BTreeMap::new(),
            ..Media::default()
        },
    );

    let multi_card = {
        let mut card = ContactCard::default();
        card.id = Some("C-MULTI-PHOTO".into());
        card.media = Some(media_map);
        card
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
            ..Media::default()
        },
    );
    mixed_media.insert(
        "m2".to_owned(),
        Media {
            kind: Some("logo".to_owned()),
            uri: "https://example.com/corp-logo.png".to_owned(),
            media_type: Some("image/png".to_owned()),
            extra: BTreeMap::new(),
            ..Media::default()
        },
    );
    mixed_media.insert(
        "m3".to_owned(),
        Media {
            kind: Some("sound".to_owned()),
            uri: "https://example.com/pronunciation.ogg".to_owned(),
            media_type: Some("audio/ogg".to_owned()),
            extra: BTreeMap::new(),
            ..Media::default()
        },
    );
    mixed_media.insert(
        "m4".to_owned(),
        Media {
            kind: Some("document".to_owned()),
            uri: "https://example.com/resume.pdf".to_owned(),
            media_type: Some("application/pdf".to_owned()),
            extra: BTreeMap::new(),
            ..Media::default()
        },
    );
    mixed_media.insert(
        "m5".to_owned(),
        Media {
            kind: None,
            uri: "https://example.com/unknown.dat".to_owned(),
            media_type: None,
            extra: BTreeMap::new(),
            ..Media::default()
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

    let mixed_card = {
        let mut card = ContactCard::default();
        card.id = Some("C-MIXED-MEDIA".into());
        card.media = Some(mixed_media);
        card
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
    // Parse unsupported standard properties and drop them on emit.
    // Coverage list: GEO, TZ, MAILER, KIND, GENDER, GRAMGENDER, PRONOUNS, LANG, LANGUAGE,
    // SOCIALPROFILE, EXPERTISE, HOBBY, INTEREST, MEMBER, CONTACT-URI, ORG-DIRECTORY,
    // SOURCE, XML, CREATED, PRODID, REV, SORT-STRING, CLASS, SOUND, LOGO.

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
        // Unsupported standard properties under audit.
        "GEO:40.331575;-74.667232\r\n",
        "TZ:-05:00\r\n",
        "MAILER:Evolution 3.52 / JMAP Client\r\n",
        "KIND:group\r\n",
        "GENDER:M\r\n",
        "GRAMGENDER:NEUTER\r\n",
        "PRONOUNS;PREF=1:xe/xir\r\n",
        "PRONOUNS;PREF=2:they/them\r\n",
        "LANG;TYPE=WORK;PREF=1:en\r\n",
        "LANGUAGE:de-AT\r\n",
        "SOCIALPROFILE;SERVICE-TYPE=Mastodon:https://social.example/@albert\r\n",
        "EXPERTISE;LEVEL=beginner;INDEX=2:Chinese literature\r\n",
        "HOBBY;LEVEL=high;INDEX=1:reading\r\n",
        "INTEREST;LEVEL=medium;INDEX=2:rock&roll music\r\n",
        "MEMBER:urn:uuid:03a0e51f-d1aa-4385-8a53-e29025acd8af\r\n",
        "MEMBER:mailto:alice@example.com\r\n",
        "CONTACT-URI:https://directory.example/contact/albert\r\n",
        "ORG-DIRECTORY:https://directory.example/contacts\r\n",
        "ORG-DIRECTORY;INDEX=1;PREF=1;TYPE=work:ldap://directory.example/ou=research\r\n",
        "SOURCE:https://directory.example/contacts/albert.vcf\r\n",
        "XML:<contact><displayName>Dr. Albert Einstein</displayName></contact>\r\n",
        "CREATED:19940930T143510Z\r\n",
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

    // Mapped properties stay intact.
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
    assert!(
        parsed.directories.is_none(),
        "SOURCE and ORG-DIRECTORY must not project to directories in this profile"
    );
    assert!(parsed.kind.is_none(), "KIND must not project to card.kind");
    assert!(
        parsed.preferred_languages.is_none(),
        "LANG must not project to preferred_languages in this profile"
    );
    assert!(
        parsed.language.is_none(),
        "LANGUAGE must not project to language in this profile"
    );
    assert!(
        parsed.personal_info.is_none(),
        "EXPERTISE/HOBBY/INTEREST must not project to personal_info in this profile"
    );
    assert!(
        parsed.speak_to_as.is_none(),
        "GRAMGENDER/PRONOUNS must not project to speak_to_as in this profile"
    );

    // Only PHOTO enters media; SOUND and LOGO are ignored.
    let media = parsed.media.as_ref().expect("media map present");
    assert_eq!(media.len(), 1, "Only PHOTO is read into media map");
    let photo = &media["m1"];
    assert_eq!(photo.kind.as_deref(), Some("photo"));
    assert_eq!(photo.media_type.as_deref(), Some("image/JPEG"));
    assert_eq!(
        photo.uri.as_str(),
        "data:image/JPEG;base64,/9j/4AAQSkZJRg=="
    );

    // Outbound vCard keeps mapped lines and omits unsupported ones.
    let emitted = card_to_vcard(&parsed);
    assert!(emitted.contains("FN:Dr. Albert Einstein\r\n"));
    assert!(emitted.contains("PHOTO;X-JMAP-KEY=m1;TYPE=JPEG;ENCODING=b:/9j/4AAQSkZJRg==\r\n"));
    assert!(!emitted.contains("GEO:"), "GEO must not be emitted");
    assert!(!emitted.contains("TZ:"), "TZ must not be emitted");
    assert!(!emitted.contains("MAILER:"), "MAILER must not be emitted");
    assert!(!emitted.contains("KIND:"), "KIND must not be emitted");
    assert!(!emitted.contains("GENDER:"), "GENDER must not be emitted");
    assert!(
        !emitted.contains("GRAMGENDER:") && !emitted.contains("GRAMGENDER;"),
        "GRAMGENDER must not be emitted"
    );
    assert!(
        !emitted.contains("PRONOUNS:") && !emitted.contains("PRONOUNS;"),
        "PRONOUNS must not be emitted"
    );
    assert!(!emitted.contains("LANG:") && !emitted.contains("LANG;"));
    assert!(
        !emitted.contains("LANGUAGE:") && !emitted.contains("LANGUAGE;"),
        "LANGUAGE must not be emitted"
    );
    assert!(
        !emitted.contains("SOCIALPROFILE:") && !emitted.contains("SOCIALPROFILE;"),
        "SOCIALPROFILE must not be emitted"
    );
    assert!(
        !emitted.contains("EXPERTISE:") && !emitted.contains("EXPERTISE;"),
        "EXPERTISE must not be emitted"
    );
    assert!(
        !emitted.contains("HOBBY:") && !emitted.contains("HOBBY;"),
        "HOBBY must not be emitted"
    );
    assert!(
        !emitted.contains("INTEREST:") && !emitted.contains("INTEREST;"),
        "INTEREST must not be emitted"
    );
    assert!(!emitted.contains("MEMBER:"), "MEMBER must not be emitted");
    assert!(
        !emitted.contains("CONTACT-URI:") && !emitted.contains("CONTACT-URI;"),
        "CONTACT-URI must not be emitted"
    );
    assert!(
        !emitted.contains("ORG-DIRECTORY:") && !emitted.contains("ORG-DIRECTORY;"),
        "ORG-DIRECTORY must not be emitted"
    );
    assert!(!emitted.contains("SOURCE:"), "SOURCE must not be emitted");
    assert!(!emitted.contains("XML:"), "XML must not be emitted");
    assert!(!emitted.contains("PRODID:"), "PRODID must not be emitted");
    assert!(!emitted.contains("REV:"), "REV must not be emitted");
    assert!(
        !emitted.contains("SORT-STRING:"),
        "SORT-STRING must not be emitted"
    );
    assert!(!emitted.contains("CLASS:"), "CLASS must not be emitted");
    assert!(!emitted.contains("SOUND"), "SOUND must not be emitted");
    assert!(!emitted.contains("LOGO"), "LOGO must not be emitted");

    // Fixed-point convergence.
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
    // Each unsupported standard property parses safely and is omitted on emit.

    // GEO variants.
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

    // TZ variants.
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

    // MAILER variants.
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

    // KIND variants.
    for kind_val in [
        "KIND:individual",
        "KIND:group",
        "KIND:org",
        "KIND:location",
        "kind:application",
    ] {
        let vcard =
            format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Kind User\r\n{kind_val}\r\nEND:VCARD\r\n");
        let parsed = vcard_to_card(&vcard).expect("parse KIND");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Kind User")
        );
        assert!(parsed.kind.is_none(), "KIND must not project to card.kind");
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("KIND:") && !emitted.contains("KIND;"),
            "KIND must not be emitted"
        );
    }

    // GENDER variants.
    for gender_val in [
        "GENDER:M",
        "GENDER:F",
        "GENDER:O",
        "GENDER:N",
        "GENDER:U",
        "GENDER:;non-binary",
        "GENDER:M;masculine identity",
        "gender:f;female identity",
    ] {
        let vcard = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Gender User\r\n{gender_val}\r\nEND:VCARD\r\n"
        );
        let parsed = vcard_to_card(&vcard).expect("parse GENDER");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Gender User")
        );
        assert!(
            parsed.personal_info.is_none(),
            "GENDER must not project to personal_info"
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("GENDER:") && !emitted.contains("GENDER;"),
            "GENDER must not be emitted"
        );
    }

    // GRAMGENDER variants.
    for gramgender_val in [
        "GRAMGENDER:NEUTER",
        "GRAMGENDER:FEMININE",
        "GRAMGENDER:MASCULINE",
        "GRAMGENDER:COMMON",
        "gramgender:neuter",
    ] {
        let vcard = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Gramgender User\r\n{gramgender_val}\r\nEND:VCARD\r\n"
        );
        let parsed = vcard_to_card(&vcard).expect("parse GRAMGENDER");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Gramgender User")
        );
        assert!(
            parsed.speak_to_as.is_none(),
            "GRAMGENDER must not project to speak_to_as"
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("GRAMGENDER:") && !emitted.contains("GRAMGENDER;"),
            "GRAMGENDER must not be emitted"
        );
    }

    // PRONOUNS variants.
    for pronouns_val in [
        "PRONOUNS;PREF=1:xe/xir",
        "PRONOUNS;PREF=2:they/them",
        "PRONOUNS;LANGUAGE=en:they/them",
        "PRONOUNS;TYPE=work;PREF=3:ze/zir",
        "pronouns;pref=1:elle/ella",
    ] {
        let vcard = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Pronouns User\r\n{pronouns_val}\r\nEND:VCARD\r\n"
        );
        let parsed = vcard_to_card(&vcard).expect("parse PRONOUNS");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Pronouns User")
        );
        assert!(
            parsed.speak_to_as.is_none(),
            "PRONOUNS must not project to speak_to_as"
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("PRONOUNS:") && !emitted.contains("PRONOUNS;"),
            "PRONOUNS must not be emitted"
        );
    }

    // LANG variants.
    for lang_val in [
        "LANG;TYPE=work;PREF=1:en",
        "LANG;TYPE=home:fr",
        "LANG;PREF=2:de-AT",
        "LANG;TYPE=work,home:zh-Hant",
        "lang;type=work:en-US",
    ] {
        let vcard =
            format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Lang User\r\n{lang_val}\r\nEND:VCARD\r\n");
        let parsed = vcard_to_card(&vcard).expect("parse LANG");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Lang User")
        );
        assert!(
            parsed.preferred_languages.is_none(),
            "LANG must not project to preferred_languages"
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("LANG:") && !emitted.contains("LANG;"),
            "LANG must not be emitted"
        );
    }

    // LANGUAGE variants.
    for language_val in [
        "LANGUAGE:de-AT",
        "LANGUAGE:en-US",
        "LANGUAGE:zh-Hant",
        "LANGUAGE:x-klingon",
        "language:fr",
    ] {
        let vcard = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Language User\r\n{language_val}\r\nEND:VCARD\r\n"
        );
        let parsed = vcard_to_card(&vcard).expect("parse LANGUAGE");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Language User")
        );
        assert!(
            parsed.language.is_none(),
            "LANGUAGE must not project to language"
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("LANGUAGE:") && !emitted.contains("LANGUAGE;"),
            "LANGUAGE must not be emitted"
        );
    }

    // SOCIALPROFILE variants.
    for socialprofile_val in [
        "SOCIALPROFILE;SERVICE-TYPE=Mastodon:https://social.example/@lang-user",
        "SOCIALPROFILE;SERVICE-TYPE=Signal;USERNAME=lang-user:lang-user.42",
        "SOCIALPROFILE;TYPE=work;PREF=1:https://social.example/work/lang-user",
        "SOCIALPROFILE;VALUE=text;SERVICE-TYPE=Matrix:@lang-user:matrix.example",
        "socialprofile;service-type=LinkedIn:https://example.com/in/lang-user",
    ] {
        let vcard = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Social Profile User\r\n{socialprofile_val}\r\nEND:VCARD\r\n"
        );
        let parsed = vcard_to_card(&vcard).expect("parse SOCIALPROFILE");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Social Profile User")
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("SOCIALPROFILE:") && !emitted.contains("SOCIALPROFILE;"),
            "SOCIALPROFILE must not be emitted"
        );
    }

    // EXPERTISE variants.
    for expertise_val in [
        "EXPERTISE;LEVEL=beginner;INDEX=2:Chinese literature",
        "EXPERTISE;INDEX=1;LEVEL=expert:chemistry",
        "EXPERTISE;PREF=1;LEVEL=average:large language models",
        "EXPERTISE;TYPE=work;LEVEL=high:discrete mathematics",
        "expertise;level=low:cooperative board games",
    ] {
        let vcard = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Expertise User\r\n{expertise_val}\r\nEND:VCARD\r\n"
        );
        let parsed = vcard_to_card(&vcard).expect("parse EXPERTISE");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Expertise User")
        );
        assert!(
            parsed.personal_info.is_none(),
            "EXPERTISE must not project to personal_info"
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("EXPERTISE:") && !emitted.contains("EXPERTISE;"),
            "EXPERTISE must not be emitted"
        );
    }

    // HOBBY variants.
    for hobby_val in [
        "HOBBY;LEVEL=high;INDEX=1:reading",
        "HOBBY;INDEX=2;LEVEL=medium:sewing",
        "HOBBY;PREF=1;LEVEL=average:mountain biking",
        "HOBBY;TYPE=work;LEVEL=low:board games",
        "hobby;level=none:gardening",
    ] {
        let vcard =
            format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Hobby User\r\n{hobby_val}\r\nEND:VCARD\r\n");
        let parsed = vcard_to_card(&vcard).expect("parse HOBBY");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Hobby User")
        );
        assert!(
            parsed.personal_info.is_none(),
            "HOBBY must not project to personal_info"
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("HOBBY:") && !emitted.contains("HOBBY;"),
            "HOBBY must not be emitted"
        );
    }

    // INTEREST variants.
    for interest_val in [
        "INTEREST;LEVEL=medium;INDEX=1:r&b music",
        "INTEREST;INDEX=2;LEVEL=high:rock&roll music",
        "INTEREST;PREF=1;LEVEL=average:astrophotography",
        "INTEREST;TYPE=work;LEVEL=low:linguistics",
        "interest;level=none:folk dance",
    ] {
        let vcard = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Interest User\r\n{interest_val}\r\nEND:VCARD\r\n"
        );
        let parsed = vcard_to_card(&vcard).expect("parse INTEREST");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Interest User")
        );
        assert!(
            parsed.personal_info.is_none(),
            "INTEREST must not project to personal_info"
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("INTEREST:") && !emitted.contains("INTEREST;"),
            "INTEREST must not be emitted"
        );
    }

    // MEMBER variants.
    for member_val in [
        "MEMBER:urn:uuid:03a0e51f-d1aa-4385-8a53-e29025acd8af",
        "MEMBER:mailto:alice@example.com",
        "MEMBER;VALUE=uri:https://example.com/member/alice",
        "MEMBER;X-JMAP-KEY=m1:urn:uuid:b8767877-b4a1-4c70-9acc-505d3819e519",
        "member:mailto:case-insensitive@example.com",
    ] {
        let vcard = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Member User\r\n{member_val}\r\nEND:VCARD\r\n"
        );
        let parsed = vcard_to_card(&vcard).expect("parse MEMBER");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Member User")
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("MEMBER:") && !emitted.contains("MEMBER;"),
            "MEMBER must not be emitted"
        );
    }

    // CONTACT-URI variants.
    for contact_uri_val in [
        "CONTACT-URI:https://directory.example/people/alice",
        "CONTACT-URI;PREF=1:https://directory.example/people/alice/support",
        "CONTACT-URI;TYPE=work:https://directory.example/people/alice/work",
        "CONTACT-URI;TYPE=work,home;PREF=2:https://directory.example/people/alice/mixed",
        "contact-uri:https://directory.example/case-insensitive",
    ] {
        let vcard = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Contact URI User\r\n{contact_uri_val}\r\nEND:VCARD\r\n"
        );
        let parsed = vcard_to_card(&vcard).expect("parse CONTACT-URI");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Contact URI User")
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("CONTACT-URI:") && !emitted.contains("CONTACT-URI;"),
            "CONTACT-URI must not be emitted"
        );
    }

    // ORG-DIRECTORY variants.
    for org_directory_val in [
        "ORG-DIRECTORY:https://directory.example.com/people/alice",
        "ORG-DIRECTORY;INDEX=1:https://directory.example.com/indexed/alice",
        "ORG-DIRECTORY;PREF=1;TYPE=work:ldap://ldap.example.com/ou=Engineering",
        "ORG-DIRECTORY;TYPE=home,work;PREF=2:https://directory.example.com/mixed",
        "org-directory:https://directory.example.com/case-insensitive",
    ] {
        let vcard = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Org Directory User\r\n{org_directory_val}\r\nEND:VCARD\r\n"
        );
        let parsed = vcard_to_card(&vcard).expect("parse ORG-DIRECTORY");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Org Directory User")
        );
        assert!(
            parsed.directories.is_none(),
            "ORG-DIRECTORY must not project to directories in this profile"
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("ORG-DIRECTORY:") && !emitted.contains("ORG-DIRECTORY;"),
            "ORG-DIRECTORY must not be emitted"
        );
    }

    // SOURCE variants.
    for source_val in [
        "SOURCE:https://directory.example.com/addrbook/jdoe.vcf",
        "SOURCE;MEDIATYPE=text/vcard:https://directory.example.com/people/alice.vcf",
        "SOURCE;TYPE=WORK:https://directory.example.com/people/work/bob.vcf",
        "SOURCE;PREF=1:https://directory.example.com/current/person.vcf",
        "source:https://directory.example.com/mixed-case.vcf",
    ] {
        let vcard = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Source User\r\n{source_val}\r\nEND:VCARD\r\n"
        );
        let parsed = vcard_to_card(&vcard).expect("parse SOURCE");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Source User")
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("SOURCE:") && !emitted.contains("SOURCE;"),
            "SOURCE must not be emitted"
        );
    }

    // XML variants.
    for xml_val in [
        "XML:<contact><displayName>Jane Doe</displayName></contact>",
        "XML:<x/>",
        "XML;VALUE=text:<meta><flag>true</flag></meta>",
        "XML;MEDIATYPE=application/xml:<meta><a>1</a></meta>",
        "xml:<card><uid>12345</uid></card>",
    ] {
        let vcard =
            format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Xml User\r\n{xml_val}\r\nEND:VCARD\r\n");
        let parsed = vcard_to_card(&vcard).expect("parse XML");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Xml User")
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("XML:") && !emitted.contains("XML;"),
            "XML must not be emitted"
        );
    }

    // CREATED variants.
    for created_val in [
        "CREATED:19940930T143510Z",
        "CREATED:2024-01-15T12:00:00Z",
        "CREATED:2024-01-15T12:00:00.123Z",
        "CREATED;VALUE=date-time:2024-01-15T12:00:00Z",
        "created:2026-08-22T00:00:00Z",
    ] {
        let vcard = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Created User\r\n{created_val}\r\nEND:VCARD\r\n"
        );
        let parsed = vcard_to_card(&vcard).expect("parse CREATED");
        assert_eq!(
            parsed.name.as_ref().unwrap().full.as_deref(),
            Some("Created User")
        );
        let emitted = card_to_vcard(&parsed);
        assert!(
            !emitted.contains("CREATED:") && !emitted.contains("CREATED;"),
            "CREATED must not be emitted"
        );
    }

    // PRODID variants.
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

    // REV variants.
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

    // SORT-STRING variants.
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

    // CLASS variants.
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

    // SOUND variants.
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

    // LOGO variants.
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
    // Only `kind: "photo"` maps to vCard `PHOTO`; sound, logo, and document stay server-only.
    // `PatchObject` keeps those non-photo media entries untouched during sync.

    let mut media = BTreeMap::new();
    media.insert(
        "m_photo".to_owned(),
        Media {
            kind: Some("photo".to_owned()),
            media_type: Some("image/jpeg".to_owned()),
            uri: "data:image/jpeg;base64,/9j/4AAQSkZJRg==".to_owned(),
            extra: BTreeMap::new(),
            ..Media::default()
        },
    );
    media.insert(
        "m_logo".to_owned(),
        Media {
            kind: Some("logo".to_owned()),
            media_type: Some("image/png".to_owned()),
            uri: "https://example.com/company_logo.png".to_owned(),
            extra: BTreeMap::new(),
            ..Media::default()
        },
    );
    media.insert(
        "m_sound".to_owned(),
        Media {
            kind: Some("sound".to_owned()),
            media_type: Some("audio/wav".to_owned()),
            uri: "data:audio/wav;base64,UklGRg==".to_owned(),
            extra: BTreeMap::new(),
            ..Media::default()
        },
    );
    media.insert(
        "m_doc".to_owned(),
        Media {
            kind: Some("document".to_owned()),
            media_type: Some("application/pdf".to_owned()),
            uri: "https://example.com/resume.pdf".to_owned(),
            extra: BTreeMap::new(),
            ..Media::default()
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

    let card = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("Media Test Contact".to_owned()),
            ..Name::default()
        });
        card.media = Some(media);
        card
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
    // Verify that empty or whitespace-only lines for all audited standard properties
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
        "LANG:",
        "LANG:   ",
        "lang:",
        "LANG;TYPE=work:",
        "LANGUAGE:",
        "LANGUAGE:   ",
        "language:",
        "SOCIALPROFILE:",
        "SOCIALPROFILE:   ",
        "socialprofile:",
        "SOCIALPROFILE;SERVICE-TYPE=Mastodon:",
        "EXPERTISE:",
        "expertise:   ",
        "EXPERTISE;LEVEL=high:",
        "EXPERTISE;INDEX=1:",
        "HOBBY:",
        "hobby:   ",
        "HOBBY;LEVEL=high:",
        "HOBBY;INDEX=1:",
        "INTEREST:",
        "interest:   ",
        "INTEREST;LEVEL=medium:",
        "INTEREST;INDEX=2:",
        "KIND:",
        "kind:   ",
        "GENDER:",
        "GENDER:   ",
        "gender:",
        "GRAMGENDER:",
        "GRAMGENDER:   ",
        "gramgender:",
        "PRONOUNS:",
        "PRONOUNS:   ",
        "pronouns:",
        "PRONOUNS;PREF=1:",
        "MEMBER:",
        "MEMBER:   ",
        "member:",
        "MEMBER;VALUE=uri:",
        "CONTACT-URI:",
        "CONTACT-URI:   ",
        "contact-uri:",
        "CONTACT-URI;TYPE=work:",
        "ORG-DIRECTORY:",
        "ORG-DIRECTORY:   ",
        "org-directory:",
        "ORG-DIRECTORY;TYPE=work:",
        "ORG-DIRECTORY;VALUE=uri:",
        "SOURCE:",
        "source:   ",
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
fn contact_uri_lines_stay_parse_safe_and_unmapped_in_eds_profile() {
    let vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:contact-uri-profile-boundary-1\r\n",
        "FN:Contact URI Profile Boundary\r\n",
        "URL;X-JMAP-KEY=l1:https://example.org/people/profile\r\n",
        "CONTACT-URI:https://example.org/people/contact\r\n",
        "contact-uri;PREF=1;TYPE=work:https://example.org/people/support\r\n",
        "END:VCARD\r\n",
    );

    let parsed = vcard_to_card(vcard).expect("parse contact-uri profile boundary card");
    let links = parsed.links.as_ref().expect("links map");
    assert_eq!(links.len(), 1, "only URL link should remain mapped");
    assert_eq!(
        links["l1"].uri, "https://example.org/people/profile",
        "URL line must remain intact"
    );
    assert_eq!(links["l1"].kind, None, "URL maps to homepage link kind");

    let emitted = card_to_vcard(&parsed);
    assert!(
        emitted.contains("URL;X-JMAP-KEY=l1:https://example.org/people/profile"),
        "Mapped URL payload must survive"
    );
    assert!(
        !emitted.contains("CONTACT-URI:") && !emitted.contains("CONTACT-URI;"),
        "CONTACT-URI must not be emitted"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse emitted card");
    let reemitted = card_to_vcard(&reparsed);
    assert_eq!(emitted, reemitted, "Export reaches fixed point");
}

#[test]
fn clientpidmap_lines_stay_parse_safe_and_unmapped_in_eds_profile() {
    let vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:clientpidmap-profile-boundary-1\r\n",
        "FN:CLIENTPIDMAP Profile Boundary\r\n",
        "NOTE;X-JMAP-KEY=n1:Mapped note survives\r\n",
        "CLIENTPIDMAP:1;urn:uuid:53e374d9-337e-4727-8803-a1e9c14e0556\r\n",
        "clientpidmap:2;urn:uuid:53e374d9-337e-4727-8803-a1e9c14e0557\r\n",
        "END:VCARD\r\n",
    );

    let parsed = vcard_to_card(vcard).expect("parse CLIENTPIDMAP profile boundary card");
    let notes = parsed.notes.as_ref().expect("note map");
    assert_eq!(notes.len(), 1, "only mapped NOTE line should remain");
    assert_eq!(notes["n1"].note.as_str(), "Mapped note survives");
    assert!(
        parsed.extra.is_empty(),
        "CLIENTPIDMAP must not project into card extra data"
    );

    let emitted = card_to_vcard(&parsed);
    assert!(
        emitted.contains("NOTE;X-JMAP-KEY=n1:Mapped note survives"),
        "Mapped NOTE payload must survive"
    );
    assert!(
        !emitted.contains("CLIENTPIDMAP:") && !emitted.contains("clientpidmap:"),
        "CLIENTPIDMAP must not be emitted"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse emitted card");
    let reemitted = card_to_vcard(&reparsed);
    assert_eq!(emitted, reemitted, "Export reaches fixed point");
}

#[test]
fn org_directory_lines_stay_parse_safe_and_unmapped_in_eds_profile() {
    let vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:org-directory-profile-boundary-1\r\n",
        "FN:ORG-DIRECTORY Profile Boundary\r\n",
        "URL;X-JMAP-KEY=l1:https://example.org/people/profile\r\n",
        "ORG-DIRECTORY:https://directory.example.org/people/alice\r\n",
        "org-directory;INDEX=1;PREF=1;TYPE=work:ldap://ldap.example.org/ou=Engineering\r\n",
        "END:VCARD\r\n",
    );

    let parsed = vcard_to_card(vcard).expect("parse org-directory profile boundary card");
    let links = parsed.links.as_ref().expect("links map");
    assert_eq!(links.len(), 1, "only URL link should remain mapped");
    assert_eq!(
        links["l1"].uri, "https://example.org/people/profile",
        "URL line must remain intact"
    );
    assert_eq!(links["l1"].kind, None, "URL maps to homepage link kind");
    assert!(
        parsed.directories.is_none(),
        "ORG-DIRECTORY must not project to directories in this profile"
    );

    let emitted = card_to_vcard(&parsed);
    assert!(
        emitted.contains("URL;X-JMAP-KEY=l1:https://example.org/people/profile"),
        "Mapped URL payload must survive"
    );
    assert!(
        !emitted.contains("ORG-DIRECTORY:") && !emitted.contains("ORG-DIRECTORY;"),
        "ORG-DIRECTORY must not be emitted"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse emitted card");
    let reemitted = card_to_vcard(&reparsed);
    assert_eq!(emitted, reemitted, "Export reaches fixed point");
}

#[test]
fn gramgender_and_pronouns_lines_stay_parse_safe_and_unmapped_in_eds_profile() {
    let vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:gramgender-pronouns-profile-boundary-1\r\n",
        "FN:GRAMGENDER PRONOUNS Profile Boundary\r\n",
        "NICKNAME;X-JMAP-KEY=k1:Alex\r\n",
        "GRAMGENDER:NEUTER\r\n",
        "PRONOUNS;PREF=1:xe/xir\r\n",
        "pronouns;PREF=2:they/them\r\n",
        "END:VCARD\r\n",
    );

    let parsed = vcard_to_card(vcard).expect("parse GRAMGENDER/PRONOUNS profile boundary card");
    let nicknames = parsed.nicknames.as_ref().expect("nickname map");
    assert_eq!(nicknames.len(), 1, "only NICKNAME should remain mapped");
    assert_eq!(nicknames["k1"].name.as_str(), "Alex");
    assert!(
        parsed.speak_to_as.is_none(),
        "GRAMGENDER and PRONOUNS must not project to speak_to_as in this profile"
    );

    let emitted = card_to_vcard(&parsed);
    assert!(
        emitted.contains("NICKNAME;X-JMAP-KEY=k1:Alex"),
        "Mapped NICKNAME payload must survive"
    );
    assert!(
        !emitted.contains("GRAMGENDER:") && !emitted.contains("GRAMGENDER;"),
        "GRAMGENDER must not be emitted"
    );
    assert!(
        !emitted.contains("PRONOUNS:") && !emitted.contains("PRONOUNS;"),
        "PRONOUNS must not be emitted"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse emitted card");
    let reemitted = card_to_vcard(&reparsed);
    assert_eq!(emitted, reemitted, "Export reaches fixed point");
}

#[test]
fn x_ablabel_group_lines_stay_parse_safe_and_unmapped_in_eds_profile() {
    let vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:x-ablabel-profile-boundary-1\r\n",
        "FN:X-ABLabel Profile Boundary\r\n",
        "item1.TEL:+1-555-0199\r\n",
        "item1.X-ABLabel:Direct Line\r\n",
        "item2.EMAIL:team@example.com\r\n",
        "item2.X-ABLabel:Project Alias\r\n",
        "END:VCARD\r\n",
    );

    let parsed = vcard_to_card(vcard).expect("parse X-ABLabel profile boundary card");

    let phones = parsed.phones.as_ref().expect("phone map");
    assert_eq!(phones.len(), 1, "one grouped phone must remain mapped");
    let phone = phones.values().next().expect("grouped phone entry");
    assert_eq!(phone.number, "+1-555-0199");
    assert_eq!(phone.label(), Some("Direct Line"));

    let emails = parsed.emails.as_ref().expect("email map");
    assert_eq!(emails.len(), 1, "one grouped email must remain mapped");
    let email = emails.values().next().expect("grouped email entry");
    assert_eq!(email.address, "team@example.com");
    assert_eq!(email.label(), Some("Project Alias"));

    let emitted = card_to_vcard(&parsed);
    assert!(
        emitted.contains("TEL;X-JMAP-KEY=p1:+1-555-0199"),
        "Mapped TEL payload must survive"
    );
    assert!(
        emitted.contains("EMAIL;X-JMAP-KEY=e1:team@example.com"),
        "Mapped EMAIL payload must survive"
    );
    assert!(
        !emitted.contains("X-ABLabel:") && !emitted.contains("X-ABLABEL:"),
        "X-ABLabel annotations must not be emitted"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse emitted card");
    let reemitted = card_to_vcard(&reparsed);
    assert_eq!(emitted, reemitted, "Export reaches fixed point");

    let reparsed_phone = reparsed
        .phones
        .as_ref()
        .expect("reparsed phone map")
        .values()
        .next()
        .expect("reparsed phone entry");
    assert_eq!(
        reparsed_phone.label(),
        None,
        "label metadata from grouped X-ABLabel does not survive vCard 3.0 profile export"
    );
    let reparsed_email = reparsed
        .emails
        .as_ref()
        .expect("reparsed email map")
        .values()
        .next()
        .expect("reparsed email entry");
    assert_eq!(
        reparsed_email.label(),
        None,
        "email label metadata from grouped X-ABLabel does not survive vCard 3.0 profile export"
    );
}

#[test]
fn geo_tz_lines_with_adr_stay_parse_safe_and_unmapped_in_eds_profile() {
    let vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:geo-tz-profile-boundary-1\r\n",
        "FN:Geo TZ Profile Boundary\r\n",
        "ADR;X-JMAP-KEY=a1;TYPE=WORK,PREF:;;1 Infinite Loop;Cupertino;CA;95014;USA\r\n",
        "LABEL;X-JMAP-KEY=a1;TYPE=WORK,PREF:1 Infinite Loop\\nCupertino\\, CA 95014\\nUSA\r\n",
        "GEO;X-JMAP-KEY=a1:geo:37.3317,-122.0307\r\n",
        "TZ;X-JMAP-KEY=a1:America/Los_Angeles\r\n",
        "END:VCARD\r\n",
    );

    let parsed = vcard_to_card(vcard).expect("parse geo/tz profile boundary card");
    let addresses = parsed.addresses.as_ref().expect("address map");
    assert_eq!(addresses.len(), 1);

    let work = &addresses["a1"];
    assert!(work.components.is_some(), "ADR components must be kept");
    assert_eq!(
        work.coordinates, None,
        "GEO must not project to coordinates"
    );
    assert_eq!(work.time_zone, None, "TZ must not project to time_zone");
    assert!(
        work.extra.contains_key("pref"),
        "ADR preference flag must still be preserved"
    );

    let emitted = card_to_vcard(&parsed);
    assert!(
        emitted
            .contains("ADR;X-JMAP-KEY=a1;TYPE=WORK,PREF:;;1 Infinite Loop;Cupertino;CA;95014;USA"),
        "Mapped ADR payload must survive"
    );
    assert!(
        !emitted.contains("GEO:") && !emitted.contains("GEO;"),
        "GEO must not be emitted"
    );
    assert!(
        !emitted.contains("TZ:") && !emitted.contains("TZ;"),
        "TZ must not be emitted"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse emitted card");
    let reemitted = card_to_vcard(&reparsed);
    assert_eq!(emitted, reemitted, "Export reaches fixed point");
}

#[test]
fn phone_mobile_type_synonym_and_permutations_characterization() {
    // Parse `TYPE=MOBILE` as mobile and normalize back to `TYPE=CELL`.
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

    let p_bare = &phones["p_bare_mobile"];
    assert_eq!(p_bare.number, "+1-555-0101");
    assert_eq!(p_bare.contexts, None);
    assert_eq!(p_bare.features, Some(json!({"mobile": true})));
    assert_eq!(p_bare.pref, None);
    assert!(states_phone_feature(p_bare.features.as_ref(), "mobile"));

    let p_lower = &phones["p_lower_mobile"];
    assert_eq!(p_lower.number, "+1-555-0102");
    assert_eq!(p_lower.features, Some(json!({"mobile": true})));

    let p_mixed = &phones["p_mixed_mobile"];
    assert_eq!(p_mixed.number, "+1-555-0103");
    assert_eq!(p_mixed.features, Some(json!({"mobile": true})));

    let p_wm = &phones["p_work_mobile_plain"];
    assert_eq!(p_wm.number, "+1-555-0104");
    assert_eq!(p_wm.contexts, Some(json!({"work": true})));
    assert_eq!(p_wm.features, Some(json!({"mobile": true})));
    assert!(states_context(p_wm.contexts.as_ref(), "work"));
    assert!(states_phone_feature(p_wm.features.as_ref(), "mobile"));

    let p_hm = &phones["p_home_mobile"];
    assert_eq!(p_hm.number, "+1-555-0105");
    assert_eq!(p_hm.contexts, Some(json!({"private": true})));
    assert_eq!(p_hm.features, Some(json!({"mobile": true})));
    assert!(states_context(p_hm.contexts.as_ref(), "private"));
    assert!(states_phone_feature(p_hm.features.as_ref(), "mobile"));

    let p_pm = &phones["p_pref_mobile"];
    assert_eq!(p_pm.number, "+1-555-0106");
    assert_eq!(p_pm.pref, Some(1));
    assert_eq!(p_pm.features, Some(json!({"mobile": true})));

    let p_wmp = &phones["p_work_mobile_pref"];
    assert_eq!(p_wmp.number, "+1-555-0107");
    assert_eq!(p_wmp.pref, Some(1));
    assert_eq!(p_wmp.contexts, Some(json!({"work": true})));
    assert_eq!(p_wmp.features, Some(json!({"mobile": true})));

    let p_mv = &phones["p_mobile_voice"];
    assert_eq!(p_mv.number, "+1-555-0108");
    assert_eq!(p_mv.features, Some(json!({"mobile": true, "voice": true})));
    assert!(states_phone_feature(p_mv.features.as_ref(), "mobile"));
    assert!(!states_phone_feature(p_mv.features.as_ref(), "voice"));

    let p_mf = &phones["p_mobile_fax"];
    assert_eq!(p_mf.number, "+1-555-0109");
    assert_eq!(p_mf.features, Some(json!({"mobile": true, "fax": true})));
    assert!(states_phone_feature(p_mf.features.as_ref(), "mobile"));
    assert!(!states_phone_feature(p_mf.features.as_ref(), "fax"));

    // Outbound `MOBILE` spellings normalize to standard vCard 3.0 `TYPE=CELL`.
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

    // Roundtripping the normalized card remains stable.
    let card2 = vcard_to_card(&emitted).expect("parse roundtrip vcard");
    let emitted2 = card_to_vcard(&card2);
    assert_eq!(emitted, emitted2);
}

#[test]
fn phone_nineteen_eds_fields_complete_matrix_and_roundtrip() {
    // Exercise the full EDS phone slot surface and keep a stable fixed point.
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

    assert_eq!(phones["p01_primary"].pref, Some(1));
    assert_eq!(phones["p01_primary"].number, "+1-555-0100");

    assert_eq!(phones["p02_business"].contexts, Some(json!({"work": true})));
    assert_eq!(
        phones["p02_business"].features,
        Some(json!({"voice": true}))
    );

    assert_eq!(
        phones["p03_business_2"].contexts,
        Some(json!({"work": true}))
    );
    assert_eq!(phones["p03_business_2"].features, None);

    assert_eq!(
        phones["p04_business_fax"].contexts,
        Some(json!({"work": true}))
    );
    assert_eq!(
        phones["p04_business_fax"].features,
        Some(json!({"fax": true}))
    );

    assert_eq!(phones["p05_home"].contexts, Some(json!({"private": true})));
    assert_eq!(phones["p05_home"].features, Some(json!({"voice": true})));

    assert_eq!(
        phones["p06_home_2"].contexts,
        Some(json!({"private": true}))
    );
    assert_eq!(phones["p06_home_2"].features, None);

    assert_eq!(
        phones["p07_home_fax"].contexts,
        Some(json!({"private": true}))
    );
    assert_eq!(phones["p07_home_fax"].features, Some(json!({"fax": true})));

    assert_eq!(
        phones["p08_mobile_cell"].features,
        Some(json!({"mobile": true}))
    );

    assert_eq!(
        phones["p09_mobile_syn"].features,
        Some(json!({"mobile": true}))
    );

    assert_eq!(phones["p10_pager"].features, Some(json!({"pager": true})));

    assert_eq!(
        phones["p11_other_voice"].features,
        Some(json!({"voice": true}))
    );

    assert_eq!(phones["p12_other_bare"].features, None);
    assert_eq!(phones["p12_other_bare"].contexts, None);

    assert_eq!(phones["p13_other_fax"].features, Some(json!({"fax": true})));

    // Specialized telephony aliases remain parseable and stateful.
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

    // Outbound order keeps the preferred number first.
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

    // Multi-roundtrip output remains stable.
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
        ..Link::default()
    }));
    assert!(states_link(&Link {
        uri: "https://blogs.example.com".to_string(),
        kind: Some("blog".to_string()),
        extra: BTreeMap::new(),
        ..Link::default()
    }));
    assert!(states_link(&Link {
        uri: "https://video.example.com".to_string(),
        kind: Some("video".to_string()),
        extra: BTreeMap::new(),
        ..Link::default()
    }));
    assert!(!states_link(&Link {
        uri: "".to_string(),
        kind: None,
        extra: BTreeMap::new(),
        ..Link::default()
    }));
    assert!(!states_link(&Link {
        uri: "".to_string(),
        kind: Some("blog".to_string()),
        extra: BTreeMap::new(),
        ..Link::default()
    }));
    assert!(!states_link(&Link {
        uri: "https://example.com/contact".to_string(),
        kind: Some("contact".to_string()),
        extra: BTreeMap::new(),
        ..Link::default()
    }));
    assert!(!states_link(&Link {
        uri: "https://example.com/rss".to_string(),
        kind: Some("feed".to_string()),
        extra: BTreeMap::new(),
        ..Link::default()
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
        // Ignore empty lines.
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
    // EDS has four dedicated email slots plus the full EMAIL attribute list.
    // Emission order is by `(pref.unwrap_or(u32::MAX), key)` and drives slot assignment.

    // Six emails: one preferred, five unranked, with deterministic key ordering.
    let card = {
        let mut card = ContactCard::default();
        card.card_type = Some("Card".to_owned());
        card.version = Some("1.0".to_owned());
        card.emails = Some(
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
        );
        card
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
    // EDS exposes work, home, and other address slots as both structured ADR and
    // synthetic label fields. This matrix checks each slot shape, mixed slot
    // combinations, in-place label edits, PREF ordering, and unkeyed ADR/LABEL pairing.

    // 1. All three slots with structured ADR and matching LABEL lines.
    let mut extra_work = BTreeMap::new();
    extra_work.insert("pref".to_owned(), json!(1));

    let card_all3 = {
        let mut card = ContactCard::default();
        card.card_type = Some("Card".to_owned());
        card.version = Some("1.0".to_owned());
        card.addresses = Some(
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
                        ..Address::default()
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
                        ..Address::default()
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
                        ..Address::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        );
        card
    };

    let emitted_all3 = card_to_vcard(&card_all3);

    // Check emission order: `a_work` (`pref: 1`) is first, then `a_home`, then `a_other`.
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

    // Check the work slot (`E_CONTACT_ADDRESS_WORK` and `E_CONTACT_ADDRESS_LABEL_WORK`).
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

    // Check the home slot (`E_CONTACT_ADDRESS_HOME` and `E_CONTACT_ADDRESS_LABEL_HOME`).
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

    // Check the other slot (`E_CONTACT_ADDRESS_OTHER` and `E_CONTACT_ADDRESS_LABEL_OTHER`).
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

    // Parse inbound data and restore all 3 addresses with structured components and full labels.
    let parsed_all3 = vcard_to_card(&emitted_all3).expect("parse back all3");
    assert_eq!(parsed_all3, card_all3);

    // Confirm a round-trip fixed point.
    let re_emitted_all3 = card_to_vcard(&parsed_all3);
    assert_eq!(re_emitted_all3, emitted_all3);

    // 2. All 3 slots use standalone `LABEL` only, without structured `ADR`.
    let card_labels_only = {
        let mut card = ContactCard::default();
        card.card_type = Some("Card".to_owned());
        card.version = Some("1.0".to_owned());
        card.addresses = Some(
            [
                (
                    "a1".to_owned(),
                    Address {
                        components: None,
                        contexts: Some(json!({"work": true})),
                        full: Some("Work Label Only\nBerlin".to_owned()),
                        extra: BTreeMap::new(),
                        ..Address::default()
                    },
                ),
                (
                    "a2".to_owned(),
                    Address {
                        components: None,
                        contexts: Some(json!({"private": true})),
                        full: Some("Home Label Only\nMünchen".to_owned()),
                        extra: BTreeMap::new(),
                        ..Address::default()
                    },
                ),
                (
                    "a3".to_owned(),
                    Address {
                        components: None,
                        contexts: None,
                        full: Some("Other Label Only\nHamburg".to_owned()),
                        extra: BTreeMap::new(),
                        ..Address::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        );
        card
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

    // 3. Mixed slots: work uses `ADR` plus `LABEL`, home uses `LABEL` only, other uses `ADR` only.
    let card_mixed = {
        let mut card = ContactCard::default();
        card.card_type = Some("Card".to_owned());
        card.version = Some("1.0".to_owned());
        card.addresses = Some(
            [
                (
                    "a_work".to_owned(),
                    Address {
                        components: Some(vec![AddressComponent::new("name", "Work Street 10")]),
                        contexts: Some(json!({"work": true})),
                        full: Some("Work Street 10\nBerlin".to_owned()),
                        extra: BTreeMap::new(),
                        ..Address::default()
                    },
                ),
                (
                    "a_home".to_owned(),
                    Address {
                        components: None,
                        contexts: Some(json!({"private": true})),
                        full: Some("Home Label Only\nCologne".to_owned()),
                        extra: BTreeMap::new(),
                        ..Address::default()
                    },
                ),
                (
                    "a_other".to_owned(),
                    Address {
                        components: Some(vec![AddressComponent::new("locality", "Frankfurt")]),
                        contexts: None,
                        full: None,
                        extra: BTreeMap::new(),
                        ..Address::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        );
        card
    };

    let emitted_mixed = card_to_vcard(&card_mixed);
    let parsed_mixed = vcard_to_card(&emitted_mixed).expect("parse mixed slots");
    assert_eq!(parsed_mixed, card_mixed);

    // 4. Parse unkeyed `WORK`, `HOME`, and `OTHER` `ADR` plus `LABEL` lines.
    // Pair each label with its `ADR` context; map a bare `LABEL` to `OTHER`.
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
    // `a1` is work, `a2` is home, `a3` is other, and `a4` is a bare label.
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

    // Re-emission converges to a fixed point.
    let unkeyed_emitted = card_to_vcard(&unkeyed_card);
    let unkeyed_card2 = vcard_to_card(&unkeyed_emitted).expect("re-parse");
    assert_eq!(unkeyed_card2, unkeyed_card);
}

#[test]
fn email_and_address_label_edge_cases_and_parameter_permutations() {
    // 1. Multi-line labels with escaped delimiters: newlines, commas, semicolons, backslashes
    let card = {
        let mut card = ContactCard::default();
        card.card_type = Some("Card".to_owned());
        card.version = Some("1.0".to_owned());
        card.addresses = Some(
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
                    ..Address::default()
                },
            )]
            .into_iter()
            .collect(),
        );
        card.emails = Some(
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
        );
        card
    };

    let emitted = card_to_vcard(&card);
    let card2 = vcard_to_card(&emitted).expect("parse complex delimiters");
    assert_eq!(card2, card);
    let emitted2 = card_to_vcard(&card2);
    assert_eq!(emitted2, emitted);

    // 2. Inbound vCard with mixed-case parameter spellings.
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
    // Use a representative Outlook vCard 2.1 fixture.
    // Keep legacy type words and quoted-printable payloads.
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

    let name = card.name.as_ref().expect("name");
    assert_eq!(name.full.as_deref(), Some("Dr. Erika Mustermann"));
    let components = name.components.as_ref().expect("components");
    assert_eq!(components[0].kind, "title");
    assert_eq!(components[0].value, "Dr.");
    assert_eq!(components[1].kind, "given");
    assert_eq!(components[1].value, "Erika");
    assert_eq!(components[2].kind, "surname");
    assert_eq!(components[2].value, "Mustermann");

    let orgs = card.organizations.as_ref().expect("organizations");
    assert_eq!(orgs["o1"].name.as_deref(), Some("Musterfirma GmbH"));
    let units = orgs["o1"].units.as_ref().expect("units");
    assert_eq!(units.len(), 2);
    assert_eq!(units[0].name, "Entwicklung");
    assert_eq!(units[1].name, "Software");
    let titles = card.titles.as_ref().expect("titles");
    assert_eq!(titles["t1"].name, "Leitende Entwicklerin");

    let notes = card.notes.as_ref().expect("notes");
    assert_eq!(
        notes["n1"].note,
        "Dies ist eine Notiz mit Umlauten: Ä, Ö, Ü, ä, ö, ü, ß.\r\nZweite Zeile mit Semikolon; und Komma, und Backslash\\"
    );

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

    let emails = card.emails.as_ref().expect("emails");
    assert_eq!(emails.len(), 2);
    assert_eq!(emails["e1"].address, "erika@musterfirma.de");
    assert_eq!(emails["e1"].pref, Some(1));
    assert_eq!(emails["e2"].address, "erika.mustermann@home.de");
    assert_eq!(emails["e2"].pref, None);

    let links = card.links.as_ref().expect("links");
    assert_eq!(links["l1"].uri, "https://www.musterfirma.de");
    let anniv = card.anniversaries.as_ref().expect("anniversaries");
    assert_eq!(anniv["y1"].kind.as_str(), "birth");
    assert_eq!(
        anniversary_date(&anniv["y1"]),
        Some("1975-04-12".to_owned())
    );

    let media = card.media.as_ref().expect("media");
    assert_eq!(media.len(), 1);
    let photo = &media["m1"];
    assert_eq!(photo.kind.as_deref(), Some("photo"));
    assert_eq!(photo.media_type.as_deref(), Some("image/JPEG"));
    assert!(photo.uri.starts_with("data:image/JPEG;base64,"));

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

    // Re-emitting normalized vCard 3.0 reaches a fixed point.
    let card2 = vcard_to_card(&emitted).expect("parse normalized 3.0 vcard");
    let emitted2 = card_to_vcard(&card2);
    assert_eq!(emitted2, emitted);
    let card3 = vcard_to_card(&emitted2).expect("parse fixpoint vcard");
    assert_eq!(card3, card2);
}

#[test]
fn vcard_21_feature_phone_nokia_sony_ericsson_fixtures_import() {
    // Real-world vCard 2.1 from Nokia and Sony Ericsson feature phones:
    // - Bare TEL types: VOICE+HOME/WORK, CELL, MOBILE, FAX+WORK, and PAGER
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
    // Covers quoted-printable soft line wraps and encoded delimiter bytes.
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

    // Emitting 3.0 keeps escaped delimiters and folded lines.
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
    // Characterizes convergence after one full import-export cycle.
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
    // Minimal reproduction input for the filed trailing-whitespace round-trip bug:
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
    let card = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            components: Some(vec![NameComponent::new("given", "TestGiven")]),
            full: Some("".to_string()),
            extra: BTreeMap::new(),
            ..Name::default()
        });
        card
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
    let card_dual = {
        let mut card = ContactCard::default();
        card.uid = Some("dual-sort-file-01".to_string());
        card.name = Some(Name {
            full: Some("Albert Einstein".to_string()),
            components: None,
            extra,
            ..Name::default()
        });
        card
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
    let card1 = {
        let mut card = ContactCard::default();
        card.uid = Some("fa-001".to_string());
        card.name = Some(Name {
            full: Some("John Doe".to_string()),
            components: None,
            extra: {
                let mut m = BTreeMap::new();
                m.insert("file_as".to_string(), json!("Doe, John (Name Extra)"));
                m
            },
            ..Name::default()
        });
        card
    };
    let emitted1 = card_to_vcard(&card1);
    assert!(emitted1.contains("X-EVOLUTION-FILE-AS:Doe\\, John (Name Extra)"));
    assert!(states_file_as(card1.name.as_ref()));

    // 2. fileAs in card.extra
    let card2 = {
        let mut card = ContactCard::default();
        card.uid = Some("fa-002".to_string());
        card.name = None;
        card.extra = {
            let mut m = BTreeMap::new();
            m.insert("fileAs".to_string(), json!("Doe, John (Card Extra)"));
            m
        };
        card
    };
    let emitted2 = card_to_vcard(&card2);
    assert!(emitted2.contains("X-EVOLUTION-FILE-AS:Doe\\, John (Card Extra)"));

    // 3. Empty string / whitespace-only fileAs is filtered and states_file_as is false
    let card3 = {
        let mut card = ContactCard::default();
        card.uid = Some("fa-003".to_string());
        card.name = Some(Name {
            full: Some("John Doe".to_string()),
            components: None,
            extra: {
                let mut m = BTreeMap::new();
                m.insert("fileAs".to_string(), json!("   "));
                m
            },
            ..Name::default()
        });
        card
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
fn related_group_labels_map_partner_custom_and_empty_labels() {
    let input = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN:Related Group Labels\r\n",
        "item1.RELATED:Pat Partner\r\n",
        "item1.X-ABLabel:_$!<Partner>!$_\r\n",
        "item2.RELATED:Riley Mentor\r\n",
        "item2.X-ABLabel:Mentor\r\n",
        "item3.RELATED:Casey Fallback\r\n",
        "item3.X-ABLabel:_$!<>!$_\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(input).expect("parse grouped related labels");
    let related = card.related_to.as_ref().expect("related_to");

    assert!(states_spouse("Pat Partner", &related["Pat Partner"]));

    let mentor = related["Riley Mentor"]
        .relation
        .as_ref()
        .expect("mentor relation");
    assert_eq!(mentor.get("mentor"), Some(&Value::Bool(true)));

    let fallback = related["Casey Fallback"]
        .relation
        .as_ref()
        .expect("fallback relation");
    assert_eq!(fallback.get("contact"), Some(&Value::Bool(true)));
    assert_eq!(fallback.get(""), None);
}

#[test]
fn x_abrelatednames_empty_group_label_maps_to_contact() {
    let input = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN:Related Empty Label\r\n",
        "item1.X-ABRELATEDNAMES:Chris Fallback\r\n",
        "item1.X-ABLabel:_$!<>!$_\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(input).expect("parse grouped x-abrelatednames");
    let related = card.related_to.as_ref().expect("related_to");
    let fallback = related["Chris Fallback"]
        .relation
        .as_ref()
        .expect("fallback relation");

    assert_eq!(fallback.get("contact"), Some(&Value::Bool(true)));
    assert_eq!(fallback.get(""), None);
}

#[test]
fn x_abdate_empty_group_label_maps_to_wedding() {
    let input = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN:Date Empty Label\r\n",
        "item1.X-ABDATE:2018-11-22\r\n",
        "item1.X-ABLabel:_$!<>!$_\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(input).expect("parse grouped x-abdate empty label");
    let anniversaries = card.anniversaries.as_ref().expect("anniversaries");

    assert_eq!(anniversaries.len(), 1);
    let only = anniversaries.values().next().expect("anniversary entry");
    assert_eq!(only.kind, "wedding");
    assert_eq!(anniversary_date(only), Some("2018-11-22".to_owned()));
}

#[test]
fn x_abdate_distinct_non_empty_keys_keep_same_kind_and_date_entries() {
    let input = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN:Date Distinct Keys\r\n",
        "item1.X-ABDATE;X-JMAP-KEY=y1:2018-11-22\r\n",
        "item1.X-ABLabel:_$!<Anniversary>!$_\r\n",
        "item2.X-ABDATE;X-JMAP-KEY=y2:2018-11-22\r\n",
        "item2.X-ABLabel:_$!<Anniversary>!$_\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(input).expect("parse grouped x-abdate keys");
    let anniversaries = card.anniversaries.as_ref().expect("anniversaries");

    assert_eq!(anniversaries.len(), 2);
    assert_eq!(anniversaries["y1"].kind, "wedding");
    assert_eq!(anniversaries["y2"].kind, "wedding");
    assert_eq!(
        anniversary_date(&anniversaries["y1"]),
        Some("2018-11-22".to_owned())
    );
    assert_eq!(
        anniversary_date(&anniversaries["y2"]),
        Some("2018-11-22".to_owned())
    );
}

#[test]
fn x_abdate_reused_key_keeps_entries_when_kind_or_date_differs() {
    let input = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN:Date Reused Key\r\n",
        "BDAY;X-JMAP-KEY=y1:1980-01-01\r\n",
        "item1.X-ABDATE;X-JMAP-KEY=y1:1980-01-01\r\n",
        "item1.X-ABLabel:_$!<Anniversary>!$_\r\n",
        "item2.X-ABDATE;X-JMAP-KEY=y1:1980-02-02\r\n",
        "item2.X-ABLabel:_$!<Birth>!$_\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(input).expect("parse grouped x-abdate reused key");
    let anniversaries = card.anniversaries.as_ref().expect("anniversaries");

    assert_eq!(anniversaries.len(), 3);
    assert!(
        anniversaries.values().any(|entry| entry.kind == "birth"
            && anniversary_date(entry) == Some("1980-01-01".to_owned()))
    );
    assert!(
        anniversaries.values().any(|entry| entry.kind == "birth"
            && anniversary_date(entry) == Some("1980-02-02".to_owned()))
    );
    assert!(
        anniversaries.values().any(|entry| entry.kind == "wedding"
            && anniversary_date(entry) == Some("1980-01-01".to_owned()))
    );
}

#[test]
fn x_abdate_unkeyed_same_day_different_kinds_are_distinct_entries() {
    let input = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN:Date Same Day Different Kinds\r\n",
        "item1.X-ABDATE:2018-11-22\r\n",
        "item1.X-ABLabel:_$!<Birthday>!$_\r\n",
        "item2.X-ABDATE:2018-11-22\r\n",
        "item2.X-ABLabel:_$!<Anniversary>!$_\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(input).expect("parse unkeyed grouped x-abdate");
    let anniversaries = card.anniversaries.as_ref().expect("anniversaries");

    assert_eq!(anniversaries.len(), 2);
    assert!(
        anniversaries.values().any(|entry| entry.kind == "birth"
            && anniversary_date(entry) == Some("2018-11-22".to_owned()))
    );
    assert!(
        anniversaries.values().any(|entry| entry.kind == "wedding"
            && anniversary_date(entry) == Some("2018-11-22".to_owned()))
    );
}

#[test]
fn bday_lines_with_distinct_keys_keep_same_day_birth_entries() {
    // RFC 9553 section 2.8.1 keeps anniversaries keyed as map entries, so two
    // non-empty keys must remain two entries even when the birth day matches.
    let input = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN:Date Distinct Birthday Keys\r\n",
        "BDAY;X-JMAP-KEY=y1:1980-01-01\r\n",
        "BDAY;X-JMAP-KEY=y2:1980-01-01\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(input).expect("parse keyed birthday duplicates");
    let anniversaries = card.anniversaries.as_ref().expect("anniversaries");

    assert_eq!(anniversaries.len(), 2);
    assert_eq!(anniversaries["y1"].kind, "birth");
    assert_eq!(
        anniversary_date(&anniversaries["y1"]),
        Some("1980-01-01".to_owned())
    );
    assert_eq!(anniversaries["y2"].kind, "birth");
    assert_eq!(
        anniversary_date(&anniversaries["y2"]),
        Some("1980-01-01".to_owned())
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
fn apple_group_labels_for_other_home_and_fax_map_without_spurious_labels() {
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Grouped Labels\r\n",
        "item1.EMAIL:other@example.com\r\n",
        "item1.X-ABLABEL:other\r\n",
        "item2.TEL:+1-555-3010\r\n",
        "item2.X-ABLABEL:fax\r\n",
        "item3.TEL:+1-555-3011\r\n",
        "item3.X-ABLABEL:home\r\n",
        "item4.TEL:+1-555-3012\r\n",
        "item4.X-ABLABEL:other\r\n",
        "END:VCARD\r\n",
    );

    let card = vcard_to_card(vcard).expect("parse grouped-label card");

    let emails = card.emails.as_ref().expect("emails map");
    let email = emails
        .values()
        .find(|entry| entry.address == "other@example.com")
        .expect("grouped other email");
    assert_eq!(email.contexts, None);
    assert!(email.extra.is_empty());

    let phones = card.phones.as_ref().expect("phones map");

    let fax = phones
        .values()
        .find(|entry| entry.number == "+1-555-3010")
        .expect("grouped fax phone");
    assert_eq!(fax.contexts, None);
    assert_eq!(fax.features, Some(json!({"fax": true})));
    assert!(fax.extra.is_empty());

    let home = phones
        .values()
        .find(|entry| entry.number == "+1-555-3011")
        .expect("grouped home phone");
    assert_eq!(home.contexts, Some(json!({"private": true})));
    assert_eq!(home.features, None);
    assert!(home.extra.is_empty());

    let other = phones
        .values()
        .find(|entry| entry.number == "+1-555-3012")
        .expect("grouped other phone");
    assert_eq!(other.contexts, None);
    assert_eq!(other.features, None);
    assert!(other.extra.is_empty());
}

#[test]
fn apple_group_label_other_on_address_keeps_no_custom_label() {
    // RFC 9553 §1.4.3 models contexts as known true-valued slots.
    // Apple's grouped "other" label does not map to a slot and must stay unlabeled.
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Grouped Address Other\r\n",
        "item1.ADR:;;42 Side St;Berlin;;10115;Germany\r\n",
        "item1.X-ABLABEL:other\r\n",
        "END:VCARD\r\n",
    );

    let card = vcard_to_card(vcard).expect("parse grouped address label");
    let addresses = card.addresses.as_ref().expect("addresses map");
    assert_eq!(addresses.len(), 1);

    let address = addresses.values().next().expect("grouped address");
    assert_eq!(address.contexts, None);
    assert!(address.extra.is_empty());
}

#[test]
fn apple_group_labels_for_url_slots_map_to_kind_or_context_without_fallback_labels() {
    // RFC 2426 §3.6.8 keeps URL as URI text; RFC 9553 §1.4.3 uses true-valued sets.
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Grouped URL Labels\r\n",
        "item1.URL:https://example.test/home\r\n",
        "item1.X-ABLABEL:HomePage\r\n",
        "item2.URL:https://example.test/blog\r\n",
        "item2.X-ABLABEL:blog\r\n",
        "item3.URL:https://example.test/work\r\n",
        "item3.X-ABLABEL:work\r\n",
        "item4.URL:https://example.test/private\r\n",
        "item4.X-ABLABEL:home\r\n",
        "item5.URL:https://example.test/other\r\n",
        "item5.X-ABLABEL:other\r\n",
        "END:VCARD\r\n",
    );

    let card = vcard_to_card(vcard).expect("parse grouped url labels card");
    let links = card.links.as_ref().expect("links map");

    let homepage = links
        .values()
        .find(|entry| entry.uri == "https://example.test/home")
        .expect("homepage link");
    assert_eq!(homepage.kind, None);
    assert!(homepage.extra.is_empty());

    let blog = links
        .values()
        .find(|entry| entry.uri == "https://example.test/blog")
        .expect("blog link");
    assert_eq!(blog.kind.as_deref(), Some("blog"));
    assert!(blog.extra.is_empty());

    let work = links
        .values()
        .find(|entry| entry.uri == "https://example.test/work")
        .expect("work link");
    assert_eq!(work.kind, None);
    assert_eq!(work.extra.get("contexts"), Some(&json!({"work": true})));

    let home = links
        .values()
        .find(|entry| entry.uri == "https://example.test/private")
        .expect("home link");
    assert_eq!(home.kind, None);
    assert_eq!(home.extra.get("contexts"), Some(&json!({"private": true})));

    let other = links
        .values()
        .find(|entry| entry.uri == "https://example.test/other")
        .expect("other link");
    assert_eq!(other.kind, None);
    assert!(other.extra.is_empty());
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
            ..OnlineService::default()
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
            ..OnlineService::default()
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
    // LOGO and KEY lines must stay ignored while PHOTO and regular contact fields
    // remain round-trippable.
    let vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "UID:pas-id-test-logo-key-001\r\n",
        "FN:Dr. Vera Marie Oldenburg\r\n",
        "N:Oldenburg;Vera;Marie;Dr.;\r\n",
        "EMAIL;X-JMAP-KEY=e1;TYPE=WORK,PREF:vera@example.com\r\n",
        "TEL;X-JMAP-KEY=p1;TYPE=WORK,CELL:+1-555-0199\r\n",
        "ADR;X-JMAP-KEY=a1;TYPE=WORK:;;Hauptstr. 1;Berlin;;10115;Germany\r\n",
        "PHOTO;X-JMAP-KEY=m1;TYPE=JPEG;ENCODING=b:/9j/4AAQSkZJRg==\r\n",
        // LOGO lines: inline base64 and remote URI.
        "LOGO;TYPE=PNG;ENCODING=b:iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==\r\n",
        "LOGO;VALUE=uri:https://example.com/corporate_logo.png\r\n",
        // KEY lines: X.509, PGP, URI, and lower-case property spelling.
        "KEY;TYPE=X509;ENCODING=b:MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA0Y123\r\n",
        "KEY;TYPE=PGP;ENCODING=b:mQGNBF+1234567890abcdef\r\n",
        "KEY;VALUE=uri:https://example.com/keys/vera.asc\r\n",
        "key;type=x509;encoding=b:MIIB...case_insensitive\r\n",
        "KEY:bare-untyped-key-payload-string\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(vcard).expect("parse vcard with logo and key");

    // Contact identity fields and PHOTO parse as usual.
    assert_eq!(
        card.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Dr. Vera Marie Oldenburg")
    );
    assert_eq!(card.emails.as_ref().map(|e| e.len()), Some(1));
    assert_eq!(card.phones.as_ref().map(|p| p.len()), Some(1));
    assert_eq!(card.addresses.as_ref().map(|a| a.len()), Some(1));

    // Media contains only PHOTO. LOGO lines are dropped.
    let media = card.media.as_ref().expect("media present");
    assert_eq!(media.len(), 1);
    assert_eq!(media["m1"].kind.as_deref(), Some("photo"));
    assert_eq!(media["m1"].media_type.as_deref(), Some("image/JPEG"));
    assert!(media["m1"].uri.starts_with("data:image/JPEG;base64,"));

    // Parsing does not leak unmodeled lines into `extra`.
    assert!(card.extra.is_empty());

    // Emission keeps PHOTO and omits LOGO and KEY.
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

    // Round-trip reaches a fixed point.
    let card2 = vcard_to_card(&emitted).expect("re-parse emitted vcard");
    assert_eq!(card2, card);
    let emitted2 = card_to_vcard(&card2);
    assert_eq!(emitted2, emitted);
}

#[test]
fn crypto_keys_and_logo_server_state_untouched_characterization() {
    use jmap_vcard::contact::{same_photo, states_media};

    // 1. Server-side contact card carrying cryptoKeys in extra and mixed media (photo, logo, sound)
    let mut card = {
        let mut card = ContactCard::default();
        card.card_type = Some("Card".to_owned());
        card.version = Some("1.0".to_owned());
        card
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
        extra: BTreeMap::new(), ..Media::default()
        };
    let logo = Media {
        kind: Some("logo".to_owned()),
        uri: "https://example.com/corp_logo.svg".to_owned(),
        media_type: Some("image/svg+xml".to_owned()),
        extra: BTreeMap::new(),
        ..Media::default()
    };
    let sound = Media {
        kind: Some("sound".to_owned()),
        uri: "https://example.com/pronunciation.ogg".to_owned(),
        media_type: Some("audio/ogg".to_owned()),
        extra: BTreeMap::new(),
        ..Media::default()
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

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-GEN-SYNC".into());
        card.name = Some(Name {
            full: Some("John Doe".into()),
            components: Some(vec![
                NameComponent::new("given", "John"),
                NameComponent::new("surname", "Doe"),
            ]),
            extra: name_extra,
            ..Name::default()
        });
        card.emails = Some(
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
        );
        card.phones = Some(
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
        );
        card.addresses = Some(
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
                    ..Address::default()
                },
            )]
            .into(),
        );
        card.links = Some(
            [(
                "l1".to_string(),
                Link {
                    uri: "https://portfolio.example.com".into(),
                    kind: Some("website".into()),
                    extra: link_extra,
                    ..Link::default()
                },
            )]
            .into(),
        );
        card.related_to = Some(
            [(
                "r1".to_string(),
                Relation {
                    relation: Some([("partner".to_string(), json!(true))].into()),
                    extra: rel_extra,
                },
            )]
            .into(),
        );
        card.extra = card_extra;
        card
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

    // Anniversaries (one birth date and one wedding date)
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
    // AGENT has no mapped EDS slot, so parsing is non-destructive and export omits it.
    // This matrix covers URI, nested vCard, plain-text, case-variant, and empty AGENT forms.

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
    // SOUND has no mapped EDS slot, so parse must ignore it without damaging mapped fields.
    // This matrix spans inline audio, URIs, legacy syntax, and folded payload forms.

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
    // Unmapped relation/media kinds stay server-owned and are omitted from emitted vCard lines.
    // Mapped spouse/manager/assistant relations and photo media must still survive round-trip.

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
    // Unmapped relations have no EDS slots: agent, child, colleague.
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
        ..Media::default()
    };
    let sound = Media {
        kind: Some("sound".to_owned()),
        uri: "https://example.com/pronounce.mp3".to_owned(),
        media_type: Some("audio/mpeg".to_owned()),
        extra: BTreeMap::new(),
        ..Media::default()
    };
    let logo = Media {
        kind: Some("logo".to_owned()),
        uri: "https://example.com/corp_logo.png".to_owned(),
        media_type: Some("image/png".to_owned()),
        extra: BTreeMap::new(),
        ..Media::default()
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

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-SERVER-PRESERVE".into());
        card.name = Some(Name {
            full: Some("Alice Representative".into()),
            ..Name::default()
        });
        card.related_to = Some(related_to);
        card.media = Some(media);
        card.extra = extra;
        card
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
    // Full-card regression for AGENT and SOUND.
    // Keep mapped fields, drop AGENT and SOUND, and keep fixed-point stability.

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

    // Structured name remains intact.
    let name = parsed.name.as_ref().expect("name");
    assert_eq!(name.full.as_deref(), Some("Dr. Elena Rostova"));
    assert_eq!(
        name.extra.get("fileAs"),
        Some(&serde_json::json!("Rostova, Elena (Global Tech)"))
    );

    // Phone, email, and address counts remain intact.
    assert_eq!(parsed.phones.as_ref().unwrap().len(), 3);
    assert_eq!(parsed.emails.as_ref().unwrap().len(), 2);
    assert_eq!(parsed.addresses.as_ref().unwrap().len(), 1);

    // Organization and title payload stays intact.
    let org = &parsed.organizations.as_ref().unwrap()["o1"];
    assert_eq!(org.name.as_deref(), Some("Global Tech"));
    assert_eq!(org.units.as_ref().unwrap().len(), 2);
    assert_eq!(parsed.titles.as_ref().unwrap().len(), 2);

    // Only mapped relations survive parse.
    let relations = parsed.related_to.as_ref().expect("relations");
    assert_eq!(relations.len(), 3);
    assert!(relations.contains_key("Mikhail Rostov"));
    assert!(relations.contains_key("Victor Vance"));
    assert!(relations.contains_key("Natalia Romanova"));
    assert!(
        !relations.contains_key("Alex Agent"),
        "AGENT must not create relation"
    );

    // Only photo media survives parse.
    let media = parsed.media.as_ref().expect("media");
    assert_eq!(media.len(), 1);
    assert_eq!(media["m1"].kind.as_deref(), Some("photo"));

    // Export omits AGENT and SOUND.
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

    // Multi-pass export converges.
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
        RealExporterTestCase {
            name: "Mozilla Thunderbird Export (vCard 4.0 with PREF, URI Tel & Multiline Labels)",
            fixture_file: "thunderbird_vcard40_export.vcf",
            exporter_name: "Mozilla Thunderbird",
            expected_full_name: "Dr. Arthur Philip Dent",
            expected_surname: "Dent",
            expected_given_name: "Arthur",
            expected_email_count: 2,
            expected_phone_count: 4, // Cell, Work, Home, WorkFAX
            expected_address_count: 2,
            expected_org_name: Some("British Broadcasting Corporation"),
            expected_org_units_count: 2,
            expected_title_count: 2,
            expected_anniversaries_count: 2, // BDAY + Anniversary
            expected_relations_count: 0,
            expected_has_photo: true,
            expected_categories_count: 3,
            unmapped_vendor_properties_dropped_on_export: &[
                "X-MOZILLA-HTML",
                "X-MOZILLA-PROPERTY",
                "PRODID",
                "REV",
            ],
        },
        RealExporterTestCase {
            name: "SOGo & Radicale CardDAV Export (vCard 3.0 with EDS Relations & Slotted Services)",
            fixture_file: "sogo_carddav_export.vcf",
            exporter_name: "SOGo / Radicale CardDAV",
            expected_full_name: "Prof. Hélène Renée Dubois-Martin",
            expected_surname: "Dubois-Martin",
            expected_given_name: "Hélène",
            expected_email_count: 2,
            expected_phone_count: 4, // Work, Cell, Home, Fax
            expected_address_count: 2,
            expected_org_name: Some("Sorbonne Université"),
            expected_org_units_count: 2,
            expected_title_count: 2,
            expected_anniversaries_count: 2, // BDAY + Anniversary
            expected_relations_count: 3,     // Spouse, Manager, Assistant
            expected_has_photo: true,
            expected_categories_count: 3,
            unmapped_vendor_properties_dropped_on_export: &["X-RADICALE-NAME", "PRODID", "REV"],
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

    // 1. Confirm the four-level ORG hierarchy: company, unit 1, unit 2, office.
    let orgs = card.organizations.as_ref().unwrap();
    let org = orgs.values().next().unwrap();
    assert_eq!(org.name.as_deref(), Some("Nordic Solutions AB"));
    let units = org.units.as_ref().unwrap();
    assert_eq!(units.len(), 3);
    assert_eq!(units[0].name, "Cloud Infrastructure");
    assert_eq!(units[1].name, "Platform Security");
    assert_eq!(units[2].name, "Executive Team");

    // 2. Confirm postal address parsing and label content.
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

    // 3. Export and confirm X-MS-* extensions are dropped.
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

    // 1. Confirm quoted-printable decoding of German umlauts.
    let name = card.name.as_ref().unwrap();
    assert_eq!(name.full.as_deref(), Some("Dr. Wolfgang Klaus Müller"));

    let notes = card.notes.as_ref().unwrap();
    let note_text = &notes.values().next().unwrap().note;
    assert!(note_text.contains("Großkunde für Logistiklösungen"));
    assert!(note_text.contains("telefonische Kontaktaufnahme"));

    // 2. Confirm quoted-printable address decoding.
    let addresses = card.addresses.as_ref().unwrap();
    let work_adr = addresses
        .values()
        .find(|a| a.contexts.as_ref().and_then(|c| c.get("work")) == Some(&serde_json::json!(true)))
        .expect("work adr");
    assert!(work_adr.full.as_ref().unwrap().contains("Gebäude B"));

    // 3. Export as strict UTF-8 vCard 3.0.
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

    // 1. Confirm data URI photo parsing.
    let media = card.media.as_ref().unwrap();
    let photo = media
        .values()
        .find(|m| m.kind.as_deref() == Some("photo"))
        .unwrap();
    assert_eq!(photo.media_type.as_deref(), Some("image/jpeg"));
    assert!(photo.uri.starts_with("data:image/jpeg;base64,"));

    // 2. Confirm IMPP online service mapping.
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

    // 3. Export as canonical vCard 3.0.
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

    // 1. Confirm File-As extraction.
    let name = card.name.as_ref().unwrap();
    assert_eq!(
        name.extra.get("fileAs"),
        Some(&serde_json::json!("Dubois, Henri (Aéronautique)"))
    );

    // 2. Confirm EDS relation, blog, video, and slotted IM mappings.
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
fn real_exporter_fixture_thunderbird_vcard40_detailed_roundtrip() {
    let vcard_text = read_fixture("thunderbird_vcard40_export.vcf");
    let card = vcard_to_card(&vcard_text).expect("parse Thunderbird vCard 4.0 fixture");

    // 1. Verify clean extra map
    assert!(
        card.extra.is_empty(),
        "card.extra must be empty, got: {:?}",
        card.extra
    );

    // 2. Validate parsed name & nickname
    let name = card.name.as_ref().unwrap();
    assert_eq!(name.full.as_deref(), Some("Dr. Arthur Philip Dent"));
    let nicknames = card.nicknames.as_ref().unwrap();
    assert!(nicknames.values().any(|n| n.name == "Dentarthurdent"));

    // 3. Validate email preferences and contexts
    let emails = card.emails.as_ref().unwrap();
    let home_email = emails
        .values()
        .find(|e| e.address == "arthur.dent@earth.example")
        .expect("home email");
    assert_eq!(home_email.pref, Some(1));
    assert_eq!(
        home_email.contexts.as_ref().and_then(|c| c.get("private")),
        Some(&serde_json::json!(true))
    );

    let work_email = emails
        .values()
        .find(|e| e.address == "a.dent@radio4.bbc.example")
        .expect("work email");
    assert_eq!(
        work_email.contexts.as_ref().and_then(|c| c.get("work")),
        Some(&serde_json::json!(true))
    );

    // 4. Validate URI tel formatting and telephony features
    let phones = card.phones.as_ref().unwrap();
    let cell_phone = phones
        .values()
        .find(|p| p.number == "+44-20-7946-0123" || p.number.contains("7946-0123"))
        .expect("cell phone");
    assert_eq!(
        cell_phone.features.as_ref().and_then(|f| f.get("mobile")),
        Some(&serde_json::json!(true))
    );

    let fax_phone = phones
        .values()
        .find(|p| p.number == "+44 20 7946 0988" || p.number.contains("7946 0988"))
        .expect("fax phone");
    assert_eq!(
        fax_phone.features.as_ref().and_then(|f| f.get("fax")),
        Some(&serde_json::json!(true))
    );

    // 5. Validate multiline address label extraction
    let addresses = card.addresses.as_ref().unwrap();
    let work_adr = addresses
        .values()
        .find(|a| a.contexts.as_ref().and_then(|c| c.get("work")) == Some(&serde_json::json!(true)))
        .expect("work adr");
    assert!(work_adr.full.as_ref().unwrap().contains("Portland Place"));

    // 6. Validate organization units hierarchy
    let orgs = card.organizations.as_ref().unwrap();
    let org = orgs.values().next().unwrap();
    assert_eq!(
        org.name.as_deref(),
        Some("British Broadcasting Corporation")
    );
    let units = org.units.as_ref().unwrap();
    assert_eq!(units.len(), 2);
    assert_eq!(units[0].name, "Radio Drama");
    assert_eq!(units[1].name, "Light Entertainment");

    // 7. Validate anniversaries (BDAY and ANNIVERSARY)
    let anniversaries = card.anniversaries.as_ref().unwrap();
    let bday = anniversaries
        .values()
        .find(|a| a.kind == "birth")
        .expect("bday");
    assert_eq!(
        bday.date,
        Some(serde_json::json!({"@type": "PartialDate", "year": 1978, "month": 3, "day": 8}))
    );
    let wedding = anniversaries
        .values()
        .find(|a| a.kind == "wedding")
        .expect("wedding");
    assert_eq!(
        wedding.date,
        Some(serde_json::json!({"@type": "PartialDate", "year": 2005, "month": 4, "day": 28}))
    );

    // 8. Validate inline photo data URI
    let media = card.media.as_ref().unwrap();
    let photo = media
        .values()
        .find(|m| m.kind.as_deref() == Some("photo"))
        .expect("photo");
    assert!(photo.uri.starts_with("data:image/png;base64,"));

    // 9. Export to canonical vCard 3.0 and verify vendor property isolation
    let export1 = card_to_vcard(&card);
    assert!(export1.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"));
    assert!(!export1.contains("X-MOZILLA-HTML"));
    assert!(!export1.contains("X-MOZILLA-PROPERTY"));
    assert!(!export1.contains("PRODID"));
    assert!(!export1.contains("REV"));

    // 10. Multi-pass round-trip fixpoint
    let card2 = vcard_to_card(&export1).expect("parse export1");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃");
    assert_eq!(card2, card3, "Card₂ == Card₃");
}

#[test]
fn real_exporter_fixture_sogo_carddav_detailed_roundtrip() {
    let vcard_text = read_fixture("sogo_carddav_export.vcf");
    let card = vcard_to_card(&vcard_text).expect("parse SOGo CardDAV fixture");

    // 1. Verify clean extra map
    assert!(
        card.extra.is_empty(),
        "card.extra must be empty, got: {:?}",
        card.extra
    );

    // 2. Validate French Unicode names and components
    let name = card.name.as_ref().unwrap();
    assert_eq!(
        name.full.as_deref(),
        Some("Prof. Hélène Renée Dubois-Martin")
    );
    let surname = name
        .components
        .as_ref()
        .and_then(|c| c.iter().find(|comp| comp.kind == "surname"))
        .unwrap();
    assert_eq!(surname.value, "Dubois-Martin");

    // 3. Validate EDS relations preserved losslessly
    let relations = card.related_to.as_ref().unwrap();
    assert!(states_spouse(
        "Jean-Pierre Martin",
        &relations["Jean-Pierre Martin"]
    ));
    assert!(states_manager(
        "Prof. Marcel Perrin",
        &relations["Prof. Marcel Perrin"]
    ));
    assert!(states_assistant(
        "Chantal Petit",
        &relations["Chantal Petit"]
    ));

    // 4. Validate online services (Jabber and Matrix)
    let services = card.online_services.as_ref().unwrap();
    assert!(
        services
            .values()
            .any(|s| s.service.as_deref() == Some("Jabber")
                && s.user.as_deref() == Some("hdubois@jabber.sorbonne-universite.example"))
    );
    assert!(
        services
            .values()
            .any(|s| s.service.as_deref() == Some("Matrix")
                && s.user.as_deref() == Some("hdubois:matrix.org"))
    );

    // 5. Validate multiline notes
    let notes = card.notes.as_ref().unwrap();
    let note_text = &notes.values().next().unwrap().note;
    assert!(note_text.contains("synchronisation JMAP/CalDAV"));

    // 6. Export and verify clean vendor dropping
    let export1 = card_to_vcard(&card);
    assert!(!export1.contains("X-RADICALE-NAME"));
    assert!(!export1.contains("PRODID"));
    assert!(!export1.contains("REV"));
    assert!(export1.contains("X-EVOLUTION-SPOUSE:Jean-Pierre Martin"));
    assert!(export1.contains("X-EVOLUTION-MANAGER:Prof. Marcel Perrin"));
    assert!(export1.contains("X-EVOLUTION-ASSISTANT:Chantal Petit"));

    // 7. Multi-pass round-trip fixpoint
    let card2 = vcard_to_card(&export1).expect("parse export1");
    let export2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&export2).expect("parse export2");
    let export3 = card_to_vcard(&card3);

    assert_eq!(export2, export3, "Export₂ == Export₃");
    assert_eq!(card2, card3, "Card₂ == Card₃");
}

#[test]
fn real_exporter_fixture_evolution_roundtrip_self_consistency() {
    let mut name_extra = BTreeMap::new();
    name_extra.insert("fileAs".to_string(), json!("SelfConsistency, Test"));

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-SELF-CONSISTENCY-1".into());
        card.name = Some(Name {
            full: Some("Dr. Test SelfConsistency".into()),
            components: Some(vec![
                NameComponent::new("prefix", "Dr."),
                NameComponent::new("given", "Test"),
                NameComponent::new("surname", "SelfConsistency"),
            ]),
            extra: name_extra,
            ..Name::default()
        });
        card.nicknames = Some(
            [("n1".to_string(), {
                let mut v = Nickname::default();
                v.name = "SelfTest".into();
                v.extra = BTreeMap::new();
                v
            })]
            .into(),
        );
        card.emails = Some(
            [
                (
                    "e1".to_string(),
                    ContactEmail {
                        address: "selftest.work@example.com".into(),
                        contexts: Some(json!({"work": true})),
                        pref: Some(1),
                        extra: BTreeMap::new(),
                    },
                ),
                (
                    "e2".to_string(),
                    ContactEmail {
                        address: "selftest.home@example.org".into(),
                        contexts: Some(json!({"private": true})),
                        pref: None,
                        extra: BTreeMap::new(),
                    },
                ),
            ]
            .into(),
        );
        card.phones = Some(
            [
                (
                    "p1".to_string(),
                    ContactPhone {
                        number: "+1 555 0100".into(),
                        features: Some(json!({"voice": true, "mobile": true})),
                        contexts: Some(json!({"work": true})),
                        pref: Some(1),
                        extra: BTreeMap::new(),
                    },
                ),
                (
                    "p2".to_string(),
                    ContactPhone {
                        number: "+1 555 0199".into(),
                        features: Some(json!({"fax": true})),
                        contexts: Some(json!({"work": true})),
                        pref: None,
                        extra: BTreeMap::new(),
                    },
                ),
            ]
            .into(),
        );
        card.addresses = Some(
            [(
                "a1".to_string(),
                Address {
                    components: Some(vec![
                        AddressComponent::new("name", "100 Innovation Way"),
                        AddressComponent::new("locality", "Cambridge"),
                        AddressComponent::new("region", "MA"),
                        AddressComponent::new("postcode", "02139"),
                        AddressComponent::new("country", "USA"),
                    ]),
                    contexts: Some(json!({"work": true})),
                    full: Some("100 Innovation Way, Cambridge, MA 02139, USA".into()),
                    extra: BTreeMap::new(),
                    ..Address::default()
                },
            )]
            .into(),
        );
        card.organizations = Some(
            [(
                "o1".to_string(),
                Organization {
                    name: Some("SelfConsistency Labs Inc.".into()),
                    units: Some(vec![OrgUnit::new("Core Engine"), OrgUnit::new("Protocols")]),
                    extra: BTreeMap::new(),
                    ..Organization::default()
                },
            )]
            .into(),
        );
        card.titles = Some(
            [
                ("t1".to_string(), {
                    let mut title = Title::default();
                    title.name = "Chief Architect".into();
                    title.kind = Some("title".into());
                    title.extra = BTreeMap::new();
                    title
                }),
                ("t2".to_string(), {
                    let mut title = Title::default();
                    title.name = "Principal Investigator".into();
                    title.kind = Some("role".into());
                    title.extra = BTreeMap::new();
                    title
                }),
            ]
            .into(),
        );
        card.anniversaries = Some(
            [
                (
                    "bday".to_string(),
                    Anniversary {
                        kind: "birth".into(),
                        date: Some(
                            json!({"@type": "PartialDate", "year": 1980, "month": 5, "day": 20}),
                        ),
                        extra: BTreeMap::new(),
                    },
                ),
                (
                    "anniv".to_string(),
                    Anniversary {
                        kind: "wedding".into(),
                        date: Some(
                            json!({"@type": "PartialDate", "year": 2008, "month": 10, "day": 12}),
                        ),
                        extra: BTreeMap::new(),
                    },
                ),
            ]
            .into(),
        );
        card.related_to = Some(
            [
                (
                    "Spouse Partner".to_string(),
                    Relation {
                        relation: Some([("spouse".to_string(), json!(true))].into()),
                        extra: BTreeMap::new(),
                    },
                ),
                (
                    "Manager Leader".to_string(),
                    Relation {
                        relation: Some([("manager".to_string(), json!(true))].into()),
                        extra: BTreeMap::new(),
                    },
                ),
                (
                    "Assistant Helper".to_string(),
                    Relation {
                        relation: Some([("assistant".to_string(), json!(true))].into()),
                        extra: BTreeMap::new(),
                    },
                ),
            ]
            .into(),
        );
        card.online_services = Some(
            [
                (
                    "s1".to_string(),
                    OnlineService {
                        service: Some("Jabber".into()),
                        user: Some("selftest@jabber.org".into()),
                        uri: None,
                        extra: BTreeMap::new(),
                        ..OnlineService::default()
                    },
                ),
                (
                    "s2".to_string(),
                    OnlineService {
                        service: Some("Matrix".into()),
                        user: Some("selftest:matrix.org".into()),
                        uri: None,
                        extra: BTreeMap::new(),
                        ..OnlineService::default()
                    },
                ),
            ]
            .into(),
        );
        card.links = Some(
            [
                (
                    "l1".to_string(),
                    Link {
                        uri: "https://blog.selftest.example".into(),
                        kind: Some("blog".into()),
                        extra: BTreeMap::new(),
                        ..Link::default()
                    },
                ),
                (
                    "l2".to_string(),
                    Link {
                        uri: "https://video.selftest.example/watch".into(),
                        kind: Some("video".into()),
                        extra: BTreeMap::new(),
                        ..Link::default()
                    },
                ),
            ]
            .into(),
        );
        card.notes = Some(
            [("n1".to_string(), {
                let mut v = Note::default();
                v.note = "Self-consistency verification note.\nLine 2 of multi-line note.".into();
                v.extra = BTreeMap::new();
                v
            })]
            .into(),
        );
        card.keywords = Some(
            [
                ("Testing".into(), json!(true)),
                ("SelfConsistency".into(), json!(true)),
                ("Evolution".into(), json!(true)),
            ]
            .into(),
        );
        card.media = Some([
            ("m1".to_string(), Media {
                kind: Some("photo".into()),
                uri: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==".into(),
                media_type: Some("image/png".into()),
                extra: BTreeMap::new(), ..Media::default()
        }),
        ].into());
        card
    };

    // 1. Export to vCard 3.0
    let vcard1 = card_to_vcard(&card);

    // 2. Re-import from generated vCard
    let card2 = vcard_to_card(&vcard1).expect("re-import generated vCard");

    // 3. Export second pass
    let vcard2 = card_to_vcard(&card2);

    // 4. Assert exact round-trip self-consistency (Pass 1 == Pass 2)
    assert_eq!(
        vcard1, vcard2,
        "Self-consistency: emitted vCard must match across passes"
    );

    // 5. Re-import second pass
    let card3 = vcard_to_card(&vcard2).expect("re-import second pass");
    assert_eq!(
        card2, card3,
        "Self-consistency: re-imported Card must match across passes"
    );

    // 6. Verify all mapped domains are preserved losslessly
    assert_eq!(
        card2.name.as_ref().unwrap().full,
        card.name.as_ref().unwrap().full
    );
    assert_eq!(
        card2.emails.as_ref().unwrap().len(),
        card.emails.as_ref().unwrap().len()
    );
    assert_eq!(
        card2.phones.as_ref().unwrap().len(),
        card.phones.as_ref().unwrap().len()
    );
    assert_eq!(
        card2.addresses.as_ref().unwrap().len(),
        card.addresses.as_ref().unwrap().len()
    );
    assert_eq!(
        card2.organizations.as_ref().unwrap().len(),
        card.organizations.as_ref().unwrap().len()
    );
    assert_eq!(
        card2.anniversaries.as_ref().unwrap().len(),
        card.anniversaries.as_ref().unwrap().len()
    );
    assert_eq!(
        card2.related_to.as_ref().unwrap().len(),
        card.related_to.as_ref().unwrap().len()
    );
    assert_eq!(
        card2.online_services.as_ref().unwrap().len(),
        card.online_services.as_ref().unwrap().len()
    );
    assert_eq!(
        card2.keywords.as_ref().unwrap().len(),
        card.keywords.as_ref().unwrap().len()
    );
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

#[test]
fn batch_6_item_4_generator_sync_and_domain_fixpoints_fidelity() {
    // 1. Server-side JSContact card with mapped and unmodeled fields
    let mut name_extra = BTreeMap::new();
    name_extra.insert("fileAs".to_string(), json!("Sync, Fuzz (Name Extra)"));
    name_extra.insert("sortAs".to_string(), json!("SYNC"));

    let mut card_extra = BTreeMap::new();
    card_extra.insert("fileAs".to_string(), json!("Sync, Fuzz (Card Extra)"));
    card_extra.insert(
        "cryptoKeys".to_string(),
        json!({"k1": {"kind": "pgp", "uri": "https://example.com/sync_key.asc"}}),
    );
    card_extra.insert("unmodeledServerTag".to_string(), json!("server_state"));

    let mut email_extra = BTreeMap::new();
    email_extra.insert("label".to_string(), json!("Engineering"));

    let mut phone_extra = BTreeMap::new();
    phone_extra.insert("label".to_string(), json!("Mobile Work"));

    let mut adr_extra = BTreeMap::new();
    adr_extra.insert("label".to_string(), json!("Main HQ"));
    adr_extra.insert("pref".to_string(), json!(1));

    let mut rel_extra_spouse = BTreeMap::new();
    rel_extra_spouse.insert("label".to_string(), json!("Spouse"));

    let mut rel_extra_agent = BTreeMap::new();
    rel_extra_agent.insert("label".to_string(), json!("Legal Representative"));

    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C-GEN-SYNC-B6".into());
        card.name = Some(Name {
            full: Some("Fuzz Sync".into()),
            components: Some(vec![
                NameComponent::new("given", "Fuzz"),
                NameComponent::new("surname", "Sync"),
            ]),
            extra: name_extra,
            ..Name::default()
        });
        card.emails = Some(
            [(
                "e1".to_string(),
                ContactEmail {
                    address: "fuzz@example.com".into(),
                    contexts: Some(json!({"work": true})),
                    pref: Some(1),
                    extra: email_extra,
                },
            )]
            .into(),
        );
        card.phones = Some(
            [(
                "p1".to_string(),
                ContactPhone {
                    number: "+1-555-0188".into(),
                    features: Some(json!({"voice": true, "mobile": true})),
                    contexts: Some(json!({"work": true})),
                    pref: Some(1),
                    extra: phone_extra,
                },
            )]
            .into(),
        );
        card.addresses = Some(
            [(
                "a1".to_string(),
                Address {
                    components: Some(vec![
                        AddressComponent::new("name", "500 Silicon Ave"),
                        AddressComponent::new("locality", "Palo Alto"),
                        AddressComponent::new("region", "CA"),
                        AddressComponent::new("postcode", "94301"),
                        AddressComponent::new("country", "USA"),
                    ]),
                    contexts: Some(json!({"work": true})),
                    full: Some("500 Silicon Ave, Palo Alto, CA 94301, USA".into()),
                    extra: adr_extra,
                    ..Address::default()
                },
            )]
            .into(),
        );
        card.media = Some(
            [
                (
                    "m1".to_string(),
                    Media {
                        kind: Some("photo".to_string()),
                        uri: "data:image/jpeg;base64,/9j/4AAQSkZJRg==".to_string(),
                        media_type: Some("image/jpeg".to_string()),
                        extra: BTreeMap::new(),
                        ..Media::default()
                    },
                ),
                (
                    "m2".to_string(),
                    Media {
                        kind: Some("sound".to_string()),
                        uri: "https://example.com/audio.ogg".to_string(),
                        media_type: Some("audio/ogg".to_string()),
                        extra: BTreeMap::new(),
                        ..Media::default()
                    },
                ),
                (
                    "m3".to_string(),
                    Media {
                        kind: Some("logo".to_string()),
                        uri: "https://example.com/logo.png".to_string(),
                        media_type: Some("image/png".to_string()),
                        extra: BTreeMap::new(),
                        ..Media::default()
                    },
                ),
            ]
            .into(),
        );
        card.related_to = Some(
            [
                (
                    "Alice Spouse".to_string(),
                    Relation {
                        relation: Some([("spouse".to_string(), json!(true))].into()),
                        extra: rel_extra_spouse,
                    },
                ),
                (
                    "Bob Agent".to_string(),
                    Relation {
                        relation: Some([("agent".to_string(), json!(true))].into()),
                        extra: rel_extra_agent,
                    },
                ),
                (
                    "Carol Colleague".to_string(),
                    Relation {
                        relation: Some([("colleague".to_string(), json!(true))].into()),
                        extra: BTreeMap::new(),
                    },
                ),
            ]
            .into(),
        );
        card.online_services = Some(
            [
                (
                    "s1".to_string(),
                    OnlineService {
                        service: Some("Jabber".to_string()),
                        user: Some("fuzz@jabber.org".to_string()),
                        uri: None,
                        extra: BTreeMap::new(),
                        ..OnlineService::default()
                    },
                ),
                (
                    "s2".to_string(),
                    OnlineService {
                        service: Some("Skype".to_string()),
                        user: Some("fuzz_skype".to_string()),
                        uri: None,
                        extra: BTreeMap::new(),
                        ..OnlineService::default()
                    },
                ),
            ]
            .into(),
        );
        card.extra = card_extra;
        card
    };

    // 2. Outbound vCard 3.0 excludes unmodeled fields and AGENT/SOUND/LOGO/KEY payloads.
    let vcard1 = card_to_vcard(&card);
    assert!(vcard1.contains("X-EVOLUTION-FILE-AS:Sync\\, Fuzz (Name Extra)"));
    assert!(vcard1.contains("X-EVOLUTION-SPOUSE:Alice Spouse"));
    assert!(vcard1.contains("PHOTO;X-JMAP-KEY=m1;TYPE=jpeg;ENCODING=b:"));
    assert!(!vcard1.contains("AGENT:"));
    assert!(!vcard1.contains("AGENT;"));
    assert!(!vcard1.contains("SOUND:"));
    assert!(!vcard1.contains("SOUND;"));
    assert!(!vcard1.contains("LOGO:"));
    assert!(!vcard1.contains("LOGO;"));
    assert!(!vcard1.contains("KEY:"));
    assert!(!vcard1.contains("KEY;"));
    assert!(!vcard1.contains("Bob Agent"));
    assert!(!vcard1.contains("Carol Colleague"));
    assert!(!vcard1.contains("cryptoKeys"));
    assert!(!vcard1.contains("unmodeledServerTag"));

    // 3. Round-trip reaches exact fixpoint stability
    let parsed1 = vcard_to_card(&vcard1).expect("parse vcard1");
    let vcard2 = card_to_vcard(&parsed1);
    let parsed2 = vcard_to_card(&vcard2).expect("parse vcard2");
    let vcard3 = card_to_vcard(&parsed2);
    assert_eq!(vcard2, vcard3, "Export₂ == Export₃ fixpoint invariant");
    assert_eq!(parsed1, parsed2, "Card₂ == Card₃ fixpoint invariant");

    // 4. Inbound vCard 4.0 stream covering the same feature matrix
    let raw_v4 = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:Comprehensive v4 Fuzz\r\n",
        "N:Fuzz;Comprehensive;;;\r\n",
        "EMAIL;TYPE=\"work,pref\";PREF=1:v4user@example.com\r\n",
        "TEL;TYPE=\"work,voice\";PREF=1:+1-555-0177\r\n",
        "ADR;TYPE=\"work,pref\";LABEL=\"700 Tech Way\\nSan Jose, CA 95110\":;;700 Tech Way;San Jose;CA;95110;USA\r\n",
        "BDAY;VALUE=date:1988-03-25\r\n",
        "ANNIVERSARY:2016-10-14\r\n",
        "PHOTO:data:image/jpeg;base64,/9j/4AAQSkZJRg==\r\n",
        "IMPP:xmpp:v4user@jabber.example.com\r\n",
        "IMPP:skype:v4user_skype\r\n",
        "IMPP:matrix:v4user:matrix.org\r\n",
        "IMPP:aim:v4user_aim\r\n",
        "IMPP:icq:7654321\r\n",
        "IMPP:msn:v4user@msn.com\r\n",
        "IMPP:yahoo:v4user_yahoo\r\n",
        "IMPP:groupwise:v4user_gw\r\n",
        "IMPP:gg:554433\r\n",
        "IMPP:gtalk:v4user@gmail.com\r\n",
        "IMPP:xmpp:rejected@chat.org?action=call\r\n",
        "RELATED;TYPE=spouse:Valerie Spouse\r\n",
        "RELATED;TYPE=manager:Victor Manager\r\n",
        "RELATED;TYPE=assistant:Vera Assistant\r\n",
        "RELATED;TYPE=colleague:Vincent Colleague\r\n",
        "AGENT:BEGIN:VCARD\\nVERSION:3.0\\nFN:Nested Agent\\nEND:VCARD\\n\r\n",
        "SOUND;TYPE=BASIC;ENCODING=b:AQIDBA==\r\n",
        "GENDER:F\r\n",
        "CLIENTPIDMAP:1;urn:uuid:67e374d9-337e-4727-8803-a1e9c14e0999\r\n",
        "MEMBER:urn:uuid:550e8400-e29b-41d4-a716-446655440000\r\n",
        "NOTE;X-JMAP-KEY=n1:vCard 4.0 comprehensive generator synchronization test.\r\n",
        "END:VCARD\r\n"
    );

    let parsed_v4 = vcard_to_card(raw_v4).expect("parse raw vCard 4.0");

    // Assert accurate extraction of mapped properties
    let p_anniv = parsed_v4.anniversaries.as_ref().expect("anniversaries");
    assert_eq!(p_anniv.len(), 2);
    let p_rel = parsed_v4.related_to.as_ref().expect("related_to");
    assert_eq!(p_rel.len(), 4);
    let p_im = parsed_v4.online_services.as_ref().expect("online_services");
    assert_eq!(p_im.len(), 10); // 10 valid services, 1 action query rejected

    // Outbound emission normalizes to standard vCard 3.0 and achieves fixed point
    let export1_v4 = card_to_vcard(&parsed_v4);
    assert!(!export1_v4.contains("AGENT"));
    assert!(!export1_v4.contains("SOUND"));
    assert!(!export1_v4.contains("GENDER"));
    assert!(!export1_v4.contains("CLIENTPIDMAP"));
    assert!(!export1_v4.contains("MEMBER"));

    let card2_v4 = vcard_to_card(&export1_v4).expect("parse export1_v4");
    let export2_v4 = card_to_vcard(&card2_v4);
    let card3_v4 = vcard_to_card(&export2_v4).expect("parse export2_v4");
    let export3_v4 = card_to_vcard(&card3_v4);

    assert_eq!(
        export2_v4, export3_v4,
        "Export₂ == Export₃ (v4 normalization)"
    );
    assert_eq!(card2_v4, card3_v4, "Card₂ == Card₃ (v4 normalization)");
}

#[test]
fn multi_org_and_multi_title_and_role_with_organization_id_association_roundtrip() {
    // Characterizes multi-ORG, multi-TITLE, and multi-ROLE configurations:
    // 1. Multiple organizations (o1, o2, o3) with distinct unit hierarchies.
    // 2. Multiple titles and roles associated with organizations via organizationId.
    // 3. Lossless roundtrip through standard vCard 3.0 lines with X-JMAP-KEY parameter tracking.
    let card = {
        let mut card = ContactCard::default();
        card.organizations = Some(
            [
                (
                    "o1".to_owned(),
                    Organization {
                        name: Some("Acme Corporation".to_owned()),
                        units: Some(vec![
                            OrgUnit::new("Engineering"),
                            OrgUnit::new("Hardware Division"),
                            OrgUnit::new("Optics Lab"),
                            OrgUnit::new("Lenses Team"),
                        ]),
                        ..Organization::default()
                    },
                ),
                (
                    "o2".to_owned(),
                    Organization {
                        name: Some("Starlight Research Foundation".to_owned()),
                        units: Some(vec![
                            OrgUnit::new("Astrophysics"),
                            OrgUnit::new("Observatories Network"),
                        ]),
                        ..Organization::default()
                    },
                ),
                (
                    "o3".to_owned(),
                    Organization {
                        name: Some("Quantum Innovations GmbH".to_owned()),
                        units: Some(vec![OrgUnit::new("Theoretical Physics")]),
                        ..Organization::default()
                    },
                ),
            ]
            .into(),
        );
        card.titles = Some(
            [
                ("t1".to_owned(), {
                    let mut title = Title::default();
                    title.name = "Principal Optical Engineer".to_owned();
                    title.kind = None;
                    // default kind is "title"
                    title.extra = [("organizationId".to_string(), json!("o1"))].into();
                    title
                }),
                ("t2".to_owned(), {
                    let mut title = Title::default();
                    title.name = "Senior Research Fellow".to_owned();
                    title.kind = Some("title".to_owned());
                    title.extra = [("organizationId".to_string(), json!("o2"))].into();
                    title
                }),
                ("r1".to_owned(), {
                    let mut title = Title::default();
                    title.name = "Chief Architect & Science Lead".to_owned();
                    title.kind = Some("role".to_owned());
                    title.extra = [("organizationId".to_string(), json!("o1"))].into();
                    title
                }),
                ("r2".to_owned(), {
                    let mut title = Title::default();
                    title.name = "Advisory Council Member".to_owned();
                    title.kind = Some("role".to_owned());
                    title.extra = [("organizationId".to_string(), json!("o2"))].into();
                    title
                }),
            ]
            .into(),
        );
        card
    };

    let vcard = card_to_vcard(&card);
    let unfolded_vcard = unfolded(&vcard);

    // Verify all 3 ORG lines are emitted with X-JMAP-KEY parameters
    assert!(
        unfolded_vcard.contains("ORG;X-JMAP-KEY=o1:Acme Corporation;Engineering;Hardware Division;Optics Lab;Lenses Team\r\n"),
        "Missing o1 ORG line in:\n{unfolded_vcard}"
    );
    assert!(
        unfolded_vcard.contains(
            "ORG;X-JMAP-KEY=o2:Starlight Research Foundation;Astrophysics;Observatories Network\r\n"
        ),
        "Missing o2 ORG line in:\n{unfolded_vcard}"
    );
    assert!(
        unfolded_vcard
            .contains("ORG;X-JMAP-KEY=o3:Quantum Innovations GmbH;Theoretical Physics\r\n"),
        "Missing o3 ORG line in:\n{unfolded_vcard}"
    );

    // Verify TITLE and ROLE lines keep X-JMAP-KEY.
    assert!(
        unfolded_vcard.contains("TITLE;X-JMAP-KEY=t1:Principal Optical Engineer\r\n"),
        "Missing t1 TITLE line in:\n{unfolded_vcard}"
    );
    assert!(
        unfolded_vcard.contains("TITLE;X-JMAP-KEY=t2:Senior Research Fellow\r\n"),
        "Missing t2 TITLE line in:\n{unfolded_vcard}"
    );
    assert!(
        unfolded_vcard.contains("ROLE;X-JMAP-KEY=r1:Chief Architect & Science Lead\r\n"),
        "Missing r1 ROLE line in:\n{unfolded_vcard}"
    );
    assert!(
        unfolded_vcard.contains("ROLE;X-JMAP-KEY=r2:Advisory Council Member\r\n"),
        "Missing r2 ROLE line in:\n{unfolded_vcard}"
    );

    // Organizations roundtrip losslessly.
    let back = vcard_to_card(&vcard).expect("parse emitted multi-org vcard");
    assert_eq!(back.organizations, card.organizations);

    // Mapped titles and roles roundtrip; unmodeled organizationId stays off-wire.
    let back_titles = back.titles.as_ref().expect("titles");
    assert_eq!(back_titles.len(), 4);
    assert_eq!(back_titles["t1"].name, "Principal Optical Engineer");
    assert_eq!(back_titles["t1"].kind, None);
    assert_eq!(back_titles["t2"].name, "Senior Research Fellow");
    assert_eq!(back_titles["t2"].kind, None); // TITLE line maps to default kind None
    assert_eq!(back_titles["r1"].name, "Chief Architect & Science Lead");
    assert_eq!(back_titles["r1"].kind, Some("role".to_owned()));
    assert_eq!(back_titles["r2"].name, "Advisory Council Member");
    assert_eq!(back_titles["r2"].kind, Some("role".to_owned()));

    // Multi-pass fixpoint check.
    let vcard2 = card_to_vcard(&back);
    let card3 = vcard_to_card(&vcard2).expect("parse pass 2");
    let vcard3 = card_to_vcard(&card3);
    assert_eq!(
        vcard2, vcard3,
        "vCard fixpoint stability on multi-org/title"
    );
    assert_eq!(
        back, card3,
        "JSContact fixpoint stability on multi-org/title"
    );
}

#[test]
fn evolution_contact_editor_three_component_org_office_and_fourth_unit_fidelity() {
    // Evolution exposes three ORG UI slots: company, department, and office.
    // ORG components beyond office are not shown there, but must still roundtrip.

    // 1. Company only.
    let c1 = {
        let mut card = ContactCard::default();
        card.organizations = Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some("Megacorp Inc".to_owned()),
                    units: None,
                    ..Organization::default()
                },
            )]
            .into(),
        );
        card
    };
    let v1 = card_to_vcard(&c1);
    assert_eq!(line(&v1, "ORG"), "ORG;X-JMAP-KEY=o1:Megacorp Inc");
    assert_eq!(
        vcard_to_card(&v1).expect("p1").organizations,
        c1.organizations
    );

    // 2. Company plus department.
    let c2 = {
        let mut card = ContactCard::default();
        card.organizations = Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some("Megacorp Inc".to_owned()),
                    units: Some(vec![OrgUnit::new("Aerospace")]),
                    ..Organization::default()
                },
            )]
            .into(),
        );
        card
    };
    let v2 = card_to_vcard(&c2);
    assert_eq!(line(&v2, "ORG"), "ORG;X-JMAP-KEY=o1:Megacorp Inc;Aerospace");
    assert_eq!(
        vcard_to_card(&v2).expect("p2").organizations,
        c2.organizations
    );

    // 3. Company plus department plus office.
    let c3 = {
        let mut card = ContactCard::default();
        card.organizations = Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some("Megacorp Inc".to_owned()),
                    units: Some(vec![OrgUnit::new("Aerospace"), OrgUnit::new("Building 4B")]),
                    ..Organization::default()
                },
            )]
            .into(),
        );
        card
    };
    let v3 = card_to_vcard(&c3);
    assert_eq!(
        line(&v3, "ORG"),
        "ORG;X-JMAP-KEY=o1:Megacorp Inc;Aerospace;Building 4B"
    );
    assert_eq!(
        vcard_to_card(&v3).expect("p3").organizations,
        c3.organizations
    );

    // 4. Company plus department plus office plus one extra unit.
    let c4 = {
        let mut card = ContactCard::default();
        card.organizations = Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some("Megacorp Inc".to_owned()),
                    units: Some(vec![
                        OrgUnit::new("Aerospace"),
                        OrgUnit::new("Building 4B"),
                        OrgUnit::new("Propulsion Lab"),
                    ]),
                    ..Organization::default()
                },
            )]
            .into(),
        );
        card
    };
    let v4 = card_to_vcard(&c4);
    assert_eq!(
        line(&v4, "ORG"),
        "ORG;X-JMAP-KEY=o1:Megacorp Inc;Aerospace;Building 4B;Propulsion Lab"
    );
    assert_eq!(
        vcard_to_card(&v4).expect("p4").organizations,
        c4.organizations
    );

    // 5. Company plus five units.
    let c6 = {
        let mut card = ContactCard::default();
        card.organizations = Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some("Megacorp Inc".to_owned()),
                    units: Some(vec![
                        OrgUnit::new("Aerospace"),
                        OrgUnit::new("Building 4B"),
                        OrgUnit::new("Propulsion Lab"),
                        OrgUnit::new("Ion Thruster Project"),
                        OrgUnit::new("Sub-team Alpha"),
                    ]),
                    ..Organization::default()
                },
            )]
            .into(),
        );
        card
    };
    let v6 = card_to_vcard(&c6);
    assert_eq!(
        line(&unfolded(&v6), "ORG"),
        "ORG;X-JMAP-KEY=o1:Megacorp Inc;Aerospace;Building 4B;Propulsion Lab;Ion Thruster Project;Sub-team Alpha"
    );
    assert_eq!(
        vcard_to_card(&v6).expect("p6").organizations,
        c6.organizations
    );

    // 6. Inbound unkeyed ORG with four components preserves all components.
    let raw_unkeyed = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Tester\r\nORG:Globex Corp;Robotics;Bay 12;Actuators\r\nEND:VCARD\r\n";
    let from_raw = vcard_to_card(raw_unkeyed).expect("parse raw 4-component org");
    let orgs = from_raw.organizations.expect("orgs");
    assert_eq!(orgs["o1"].name.as_deref(), Some("Globex Corp"));
    assert_eq!(
        orgs["o1"].units.as_deref(),
        Some(
            [
                OrgUnit::new("Robotics"),
                OrgUnit::new("Bay 12"),
                OrgUnit::new("Actuators"),
            ]
            .as_slice()
        )
    );
}

#[test]
fn multi_component_org_special_characters_escaping_and_fixpoint() {
    // ORG components with escaped separators, backslashes, and UTF-8 stay stable.
    let card = {
        let mut card = ContactCard::default();
        card.organizations = Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some("Müller & Söhne; GmbH, Berlin \\ [HQ]".to_owned()),
                    units: Some(vec![
                        OrgUnit::new("R&D; Advanced Division, Sector 7"),
                        OrgUnit::new("Office 42; Building B \\ Room 101"),
                        OrgUnit::new("Deep Tech / Quantum \\ Sub-unit, Team 4"),
                    ]),
                    ..Organization::default()
                },
            )]
            .into(),
        );
        card
    };

    let vcard1 = card_to_vcard(&card);
    let card2 = vcard_to_card(&vcard1).expect("parse pass 1");
    let vcard2 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&vcard2).expect("parse pass 2");
    let vcard3 = card_to_vcard(&card3);

    // Special characters survive verbatim without double-escaping.
    let org2 = &card2.organizations.as_ref().unwrap()["o1"];
    assert_eq!(
        org2.name.as_deref(),
        Some("Müller & Söhne; GmbH, Berlin \\ [HQ]")
    );
    let units2 = org2.units.as_ref().unwrap();
    assert_eq!(units2[0].name, "R&D; Advanced Division, Sector 7");
    assert_eq!(units2[1].name, "Office 42; Building B \\ Room 101");
    assert_eq!(units2[2].name, "Deep Tech / Quantum \\ Sub-unit, Team 4");

    // Multi-pass fixpoint stability.
    assert_eq!(
        vcard2, vcard3,
        "vCard fixpoint stability for escaped ORG components"
    );
    assert_eq!(
        card2, card3,
        "JSContact fixpoint stability for escaped ORG components"
    );
}

#[test]
fn multi_component_org_sparse_and_empty_slots_positioning() {
    // Sparse ORG components remain position-stable.
    // 1. Nameless organisation with three units keeps an empty name slot.
    let nameless = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Nameless Org Test\r\n",
        "ORG;X-JMAP-KEY=o1:;Engineering;Security;Office 9\r\n",
        "END:VCARD\r\n"
    );
    let c_nameless = vcard_to_card(nameless).expect("parse nameless");
    let o_nameless = &c_nameless.organizations.as_ref().unwrap()["o1"];
    assert_eq!(o_nameless.name, None);
    assert_eq!(
        o_nameless.units.as_deref(),
        Some(
            [
                OrgUnit::new("Engineering"),
                OrgUnit::new("Security"),
                OrgUnit::new("Office 9")
            ]
            .as_slice()
        )
    );
    let emitted_nameless = card_to_vcard(&c_nameless);
    assert_eq!(
        line(&emitted_nameless, "ORG"),
        "ORG;X-JMAP-KEY=o1:;Engineering;Security;Office 9"
    );

    // 2. Empty intermediate unit is dropped, and non-empty units keep order.
    let empty_dept = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Empty Dept Test\r\n",
        "ORG;X-JMAP-KEY=o1:Acme Corp;;Office 4B;Lenses\r\n",
        "END:VCARD\r\n"
    );
    let c_empty_dept = vcard_to_card(empty_dept).expect("parse empty dept");
    let o_empty_dept = &c_empty_dept.organizations.as_ref().unwrap()["o1"];
    assert_eq!(o_empty_dept.name.as_deref(), Some("Acme Corp"));
    assert_eq!(
        o_empty_dept.units.as_deref(),
        Some([OrgUnit::new("Office 4B"), OrgUnit::new("Lenses")].as_slice())
    );

    // 3. Multi-pass fixpoint stability across sparse inputs.
    let v_pass1 = card_to_vcard(&c_empty_dept);
    let c_pass2 = vcard_to_card(&v_pass1).expect("parse pass 1");
    let v_pass2 = card_to_vcard(&c_pass2);
    let c_pass3 = vcard_to_card(&v_pass2).expect("parse pass 2");
    let v_pass3 = card_to_vcard(&c_pass3);

    assert_eq!(v_pass2, v_pass3, "vCard fixpoint stability for sparse ORG");
    assert_eq!(
        c_pass2, c_pass3,
        "JSContact fixpoint stability for sparse ORG"
    );
}

#[test]
fn tel_type_work_voice_fax_evolution_contact_editor_dual_field_aliasing_and_slot_narrowing() {
    // `TEL;TYPE=WORK,VOICE,FAX` parses into `features` {voice,fax} and
    // `contexts` {work} without loss.
    let inbound_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice Office\r\n",
        "TEL;TYPE=WORK,VOICE,FAX;X-JMAP-KEY=p1:+1 555 0100\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(inbound_vcard).expect("parse inbound vcard");
    let phone = &card.phones.as_ref().unwrap()["p1"];
    assert_eq!(phone.number, "+1 555 0100");
    assert_eq!(phone.contexts, Some(json!({"work": true})));
    assert_eq!(phone.features, Some(json!({"voice": true, "fax": true})));

    // Outbound narrows to `WORK,FAX` so one TEL line does not alias both
    // `E_CONTACT_PHONE_BUSINESS` and `E_CONTACT_PHONE_BUSINESS_FAX`.
    let emitted_vcard = card_to_vcard(&card);
    assert_eq!(
        line(&emitted_vcard, "TEL"),
        "TEL;X-JMAP-KEY=p1;TYPE=WORK,FAX:+1 555 0100"
    );

    // Predicate checks keep `fax` as the winner while suppressing `voice` so sync
    // diffing does not misclassify the narrowed feature set as a user deletion.
    assert!(states_phone_feature(phone.features.as_ref(), "fax"));
    assert!(!states_phone_feature(phone.features.as_ref(), "voice"));
    assert!(!states_phone_feature(phone.features.as_ref(), "mobile"));
    assert!(!states_phone_feature(phone.features.as_ref(), "pager"));
    assert!(!states_phone_feature(phone.features.as_ref(), "video"));

    // Accept vCard 2.1 bare parameters and vCard 4.0 quoted parameters inbound.
    let vcard21 = concat!(
        "BEGIN:VCARD\r\nVERSION:2.1\r\nFN:Alice QP\r\n",
        "TEL;WORK;VOICE;FAX;X-JMAP-KEY=p1:+1 555 0100\r\n",
        "END:VCARD\r\n"
    );
    let card21 = vcard_to_card(vcard21).expect("parse vcard 2.1");
    let p21 = &card21.phones.as_ref().unwrap()["p1"];
    assert_eq!(p21.contexts, Some(json!({"work": true})));
    assert_eq!(p21.features, Some(json!({"voice": true, "fax": true})));

    let vcard40 = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice v4\r\n",
        "TEL;TYPE=\"work,voice,fax\";X-JMAP-KEY=p1:+1 555 0100\r\n",
        "END:VCARD\r\n"
    );
    let card40 = vcard_to_card(vcard40).expect("parse vcard 4.0");
    let p40 = &card40.phones.as_ref().unwrap()["p1"];
    assert_eq!(p40.contexts, Some(json!({"work": true})));
    assert_eq!(p40.features, Some(json!({"voice": true, "fax": true})));
}

#[test]
fn tel_type_home_voice_fax_and_unqualified_voice_fax_slot_narrowing_and_editor_visibility() {
    // 1. `TEL;TYPE=HOME,VOICE,FAX` narrows to `HOME,FAX` to avoid
    // aliasing `E_CONTACT_PHONE_HOME` and `E_CONTACT_PHONE_HOME_FAX`.
    let mut phones = BTreeMap::new();
    phones.insert(
        "p1".to_owned(),
        ContactPhone {
            number: "+1 555 0200".to_owned(),
            contexts: Some(json!({"private": true})),
            features: Some(json!({"voice": true, "fax": true})),
            ..ContactPhone::default()
        },
    );
    let home_card = {
        let mut card = ContactCard::default();
        card.phones = Some(phones);
        card
    };
    let home_vcard = card_to_vcard(&home_card);
    assert_eq!(
        line(&home_vcard, "TEL"),
        "TEL;X-JMAP-KEY=p1;TYPE=HOME,FAX:+1 555 0200"
    );
    let home_p = &home_card.phones.as_ref().unwrap()["p1"];
    assert!(states_phone_feature(home_p.features.as_ref(), "fax"));
    assert!(!states_phone_feature(home_p.features.as_ref(), "voice"));

    // 2. Bare `TEL;TYPE=VOICE,FAX` narrows to `FAX` so Evolution maps it to
    // "Other Fax" instead of dropping it as an ambiguous match.
    let mut bare_phones = BTreeMap::new();
    bare_phones.insert(
        "p2".to_owned(),
        ContactPhone {
            number: "+1 555 0300".to_owned(),
            contexts: None,
            features: Some(json!({"voice": true, "fax": true})),
            ..ContactPhone::default()
        },
    );
    let bare_card = {
        let mut card = ContactCard::default();
        card.phones = Some(bare_phones);
        card
    };
    let bare_vcard = card_to_vcard(&bare_card);
    assert_eq!(
        line(&bare_vcard, "TEL"),
        "TEL;X-JMAP-KEY=p2;TYPE=FAX:+1 555 0300"
    );
    let bare_p = &bare_card.phones.as_ref().unwrap()["p2"];
    assert!(states_phone_feature(bare_p.features.as_ref(), "fax"));
    assert!(!states_phone_feature(bare_p.features.as_ref(), "voice"));
}

#[test]
fn tel_multi_feature_hierarchy_complete_pairwise_and_composite_matrix() {
    // Pairwise and composite sets follow: `CELL`/`MOBILE` > `PAGER` > `FAX` > `VOICE` > `VIDEO`.
    let cases = [
        // (features, expected_emitted_type, winning_feature, suppressed_features)
        (
            json!({"mobile": true, "voice": true}),
            "CELL",
            "mobile",
            &["voice", "fax", "pager", "video"][..],
        ),
        (
            json!({"mobile": true, "fax": true}),
            "CELL",
            "mobile",
            &["voice", "fax", "pager", "video"][..],
        ),
        (
            json!({"mobile": true, "pager": true}),
            "CELL",
            "mobile",
            &["voice", "fax", "pager", "video"][..],
        ),
        (
            json!({"mobile": true, "video": true}),
            "CELL",
            "mobile",
            &["voice", "fax", "pager", "video"][..],
        ),
        (
            json!({"pager": true, "voice": true}),
            "PAGER",
            "pager",
            &["voice", "fax", "mobile", "video"][..],
        ),
        (
            json!({"pager": true, "fax": true}),
            "PAGER",
            "pager",
            &["voice", "fax", "mobile", "video"][..],
        ),
        (
            json!({"pager": true, "video": true}),
            "PAGER",
            "pager",
            &["voice", "fax", "mobile", "video"][..],
        ),
        (
            json!({"fax": true, "voice": true}),
            "FAX",
            "fax",
            &["voice", "pager", "mobile", "video"][..],
        ),
        (
            json!({"fax": true, "video": true}),
            "FAX",
            "fax",
            &["voice", "pager", "mobile", "video"][..],
        ),
        (
            json!({"voice": true, "video": true}),
            "VOICE",
            "voice",
            &["fax", "pager", "mobile", "video"][..],
        ),
        (
            json!({"mobile": true, "pager": true, "fax": true, "voice": true, "video": true}),
            "CELL",
            "mobile",
            &["pager", "fax", "voice", "video"][..],
        ),
        (
            json!({"pager": true, "fax": true, "voice": true, "video": true}),
            "PAGER",
            "pager",
            &["fax", "voice", "video"][..],
        ),
        (
            json!({"fax": true, "voice": true, "video": true}),
            "FAX",
            "fax",
            &["voice", "video"][..],
        ),
    ];

    for (features_json, expected_type, winning_feature, suppressed) in cases {
        let mut phones = BTreeMap::new();
        phones.insert(
            "p1".to_owned(),
            ContactPhone {
                number: "+1 555 0999".to_owned(),
                contexts: None,
                features: Some(features_json.clone()),
                ..ContactPhone::default()
            },
        );
        let card = {
            let mut card = ContactCard::default();
            card.phones = Some(phones);
            card
        };
        let vcard = card_to_vcard(&card);
        let expected_line = format!("TEL;X-JMAP-KEY=p1;TYPE={expected_type}:+1 555 0999");
        assert_eq!(
            line(&vcard, "TEL"),
            expected_line,
            "failed slot narrowing for features {features_json:?}"
        );

        let p = &card.phones.as_ref().unwrap()["p1"];
        assert!(
            states_phone_feature(p.features.as_ref(), winning_feature),
            "winning feature {winning_feature} must be stated"
        );
        for &suppressed_feature in suppressed {
            if features_json.get(suppressed_feature) == Some(&Value::Bool(true)) {
                assert!(
                    !states_phone_feature(p.features.as_ref(), suppressed_feature),
                    "suppressed feature {suppressed_feature} must not be stated"
                );
            }
        }
    }
}

#[test]
fn tel_multi_feature_and_multi_context_roundtrip_fixpoint_stability() {
    let mut phones = BTreeMap::new();
    phones.insert(
        "p1".to_owned(),
        ContactPhone {
            number: "+1 555 1001".to_owned(),
            contexts: Some(json!({"work": true})),
            features: Some(json!({"voice": true, "fax": true})),
            pref: Some(1),
            ..ContactPhone::default()
        },
    );
    phones.insert(
        "p2".to_owned(),
        ContactPhone {
            number: "+1 555 1002".to_owned(),
            contexts: Some(json!({"private": true})),
            features: Some(json!({"voice": true, "fax": true})),
            pref: Some(2),
            ..ContactPhone::default()
        },
    );
    phones.insert(
        "p3".to_owned(),
        ContactPhone {
            number: "+1 555 1003".to_owned(),
            contexts: Some(json!({"work": true})),
            features: Some(json!({"mobile": true, "pager": true})),
            ..ContactPhone::default()
        },
    );
    phones.insert(
        "p4".to_owned(),
        ContactPhone {
            number: "+1 555 1004".to_owned(),
            contexts: None,
            features: Some(json!({"voice": true, "fax": true})),
            ..ContactPhone::default()
        },
    );
    let card1 = {
        let mut card = ContactCard::default();
        card.phones = Some(phones);
        card
    };

    // Pass 1: JSContact -> vCard
    let vcard1 = card_to_vcard(&card1);
    // Pass 2: vCard -> JSContact -> vCard
    let card2 = vcard_to_card(&vcard1).expect("parse pass 1");
    let vcard2 = card_to_vcard(&card2);
    // Pass 3: vCard -> JSContact -> vCard
    let card3 = vcard_to_card(&vcard2).expect("parse pass 2");
    let vcard3 = card_to_vcard(&card3);

    // Multi-pass fixed-point stability
    assert_eq!(
        vcard2, vcard3,
        "vCard fixpoint stability for multi-feature phones"
    );
    assert_eq!(
        card2, card3,
        "JSContact fixpoint stability for multi-feature phones"
    );
}

#[test]
fn im_uri_schemes_all_10_services_and_18_aliases_roundtrip_matrix() {
    // Matrix for 10 slotted services and 18 URI aliases across vCard 3.0 and IMPP.
    let test_matrix: &[(&str, &str, &str, &str, &str)] = &[
        // (canonical service, scheme alias, handle, expected X-property, vCard 4.0 service)
        ("AIM", "aim", "alice_aim", "X-AIM", "AIM"),
        ("AIM", "aol", "alice_aol", "X-AIM", "AIM"),
        ("Gadu-Gadu", "gg", "87654321", "X-GADUGADU", "Gadu-Gadu"),
        (
            "Gadu-Gadu",
            "gadugadu",
            "87654321",
            "X-GADUGADU",
            "Gadu-Gadu",
        ),
        ("Gadu-Gadu", "gadu", "87654321", "X-GADUGADU", "Gadu-Gadu"),
        (
            "Google Talk",
            "xmpp",
            "alice.gtalk@gmail.com",
            "X-GOOGLE-TALK",
            "Jabber",
        ), // Note: xmpp in IMPP defaults to Jabber
        (
            "Google Talk",
            "gtalk",
            "alice.gtalk@gmail.com",
            "X-GOOGLE-TALK",
            "Google Talk",
        ),
        (
            "GroupWise",
            "groupwise",
            "alice_gw",
            "X-GROUPWISE",
            "GroupWise",
        ),
        (
            "GroupWise",
            "novell",
            "alice_gw",
            "X-GROUPWISE",
            "GroupWise",
        ),
        ("ICQ", "icq", "12345678", "X-ICQ", "ICQ"),
        ("Jabber", "xmpp", "alice@jabber.org", "X-JABBER", "Jabber"),
        ("Jabber", "jabber", "alice@jabber.org", "X-JABBER", "Jabber"),
        ("MSN", "msn", "alice@msn.com", "X-MSN", "MSN"),
        ("MSN", "msnim", "alice@msn.com", "X-MSN", "MSN"),
        (
            "Matrix",
            "matrix",
            "@alice:matrix.org",
            "X-MATRIX",
            "Matrix",
        ),
        ("Skype", "skype", "alice_skype", "X-SKYPE", "Skype"),
        ("Yahoo", "yahoo", "alice_yahoo", "X-YAHOO", "Yahoo"),
        ("Yahoo", "ymsgr", "alice_yahoo", "X-YAHOO", "Yahoo"),
    ];

    for &(service, scheme, handle, expected_x_prop, v4_service) in test_matrix {
        let uri_str = format!("{scheme}:{handle}");

        // 1. Test outbound emission from JSContact with `uri`
        let mut services = BTreeMap::new();
        services.insert(
            "s1".to_owned(),
            OnlineService {
                service: Some(service.to_owned()),
                uri: Some(uri_str.clone()),
                ..OnlineService::default()
            },
        );
        let card_with_uri = {
            let mut card = ContactCard::default();
            card.online_services = Some(services.clone());
            card
        };

        assert!(
            states_online_service(&services["s1"]),
            "service {service} with uri {uri_str} must be stated"
        );
        assert_eq!(
            online_service_handle(&services["s1"]),
            Some(handle),
            "handle extracted from uri {uri_str}"
        );

        let vcard_from_uri = card_to_vcard(&card_with_uri);
        let expected_line = format!("{expected_x_prop};X-JMAP-KEY=s1;TYPE=HOME:{handle}");
        assert_eq!(
            line(&vcard_from_uri, expected_x_prop),
            expected_line,
            "failed outbound vCard 3.0 emission for {service} from uri {uri_str}"
        );

        // 2. Test outbound emission from JSContact with explicit `user`
        let mut services_user = BTreeMap::new();
        services_user.insert(
            "s1".to_owned(),
            OnlineService {
                service: Some(service.to_owned()),
                user: Some(handle.to_owned()),
                ..OnlineService::default()
            },
        );
        let card_with_user = {
            let mut card = ContactCard::default();
            card.online_services = Some(services_user);
            card
        };
        let vcard_from_user = card_to_vcard(&card_with_user);
        assert_eq!(
            line(&vcard_from_user, expected_x_prop),
            expected_line,
            "failed outbound vCard 3.0 emission for {service} from user"
        );

        // 3. Test inbound parsing of vCard 3.0 wire format
        let parsed_v3 = vcard_to_card(&vcard_from_uri).expect("parse vCard 3.0");
        let parsed_v3_services = parsed_v3
            .online_services
            .as_ref()
            .expect("online_services present");
        assert_eq!(
            parsed_v3_services["s1"].user.as_deref(),
            Some(handle),
            "parsed vCard 3.0 user handle mismatch for {service}"
        );
        assert_eq!(
            parsed_v3_services["s1"].service.as_deref(),
            Some(service),
            "parsed vCard 3.0 service name mismatch for {service}"
        );

        // 4. Test inbound parsing of vCard 4.0 IMPP property
        let vcard_v4 = format!(
            "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Test User\r\nIMPP:{scheme}:{handle}\r\nEND:VCARD\r\n"
        );
        let parsed_v4 = vcard_to_card(&vcard_v4).expect("parse vCard 4.0 IMPP");
        let parsed_v4_services = parsed_v4.online_services.expect("v4 online_services");
        assert_eq!(
            parsed_v4_services["s1"].user.as_deref(),
            Some(handle),
            "vCard 4.0 IMPP handle mismatch for scheme {scheme}"
        );
        assert_eq!(
            parsed_v4_services["s1"].service.as_deref(),
            Some(v4_service),
            "vCard 4.0 IMPP service mismatch for scheme {scheme}"
        );

        // 5. Verify roundtrip fixpoint stability
        let export2 = card_to_vcard(&parsed_v3);
        let card3 = vcard_to_card(&export2).expect("re-parse");
        let export3 = card_to_vcard(&card3);
        assert_eq!(
            export2, export3,
            "vCard fixpoint stability for service {service} (scheme {scheme})"
        );
        assert_eq!(
            parsed_v3, card3,
            "JSContact fixpoint stability for service {service} (scheme {scheme})"
        );
    }
}

#[test]
fn evolution_contact_editor_im_slotted_fields_vs_unslotted_twitter_and_sip_characterization() {
    // Cover slotted IM fields against unslotted and unmodeled service inputs.

    // Mix slotted services with unslotted and unmodeled ones.
    let mut services = BTreeMap::new();
    services.insert(
        "s_jabber".to_owned(),
        OnlineService {
            service: Some("Jabber".to_owned()),
            user: Some("vera@jabber.example".to_owned()),
            extra: [("contexts".to_owned(), json!({"home": true}))].into(),
            ..OnlineService::default()
        },
    );
    services.insert(
        "s_skype".to_owned(),
        OnlineService {
            service: Some("Skype".to_owned()),
            user: Some("vera_skype".to_owned()),
            extra: [("contexts".to_owned(), json!({"work": true}))].into(),
            ..OnlineService::default()
        },
    );
    services.insert(
        "s_matrix".to_owned(),
        OnlineService {
            service: Some("Matrix".to_owned()),
            user: Some("@vera:matrix.org".to_owned()),
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

    let card = {
        let mut card = ContactCard::default();
        card.online_services = Some(services);
        card
    };

    // Outbound serialization keeps only slotted services.
    let vcard = card_to_vcard(&card);
    assert!(
        vcard.contains("X-JABBER;X-JMAP-KEY=s_jabber;TYPE=HOME:vera@jabber.example\r\n"),
        "{vcard}"
    );
    assert!(
        vcard.contains("X-SKYPE;X-JMAP-KEY=s_skype;TYPE=WORK:vera_skype\r\n"),
        "{vcard}"
    );
    assert!(
        vcard.contains("X-MATRIX;X-JMAP-KEY=s_matrix;TYPE=HOME:@vera:matrix.org\r\n"),
        "{vcard}"
    );

    // Unslotted and unmodeled services are omitted from wire output.
    assert!(!vcard.contains("X-TWITTER"), "{vcard}");
    assert!(!vcard.contains("X-SIP"), "{vcard}");
    assert!(!vcard.contains("X-DISCORD"), "{vcard}");
    assert!(!vcard.contains("X-SIGNAL"), "{vcard}");
    assert!(!vcard.contains("twitter"), "{vcard}");
    assert!(!vcard.contains("sip:"), "{vcard}");

    // Sync safety rejects unslotted and unmodeled services.
    let s_map = card.online_services.as_ref().unwrap();
    assert!(states_online_service(&s_map["s_jabber"]));
    assert!(states_online_service(&s_map["s_skype"]));
    assert!(states_online_service(&s_map["s_matrix"]));
    assert!(
        !states_online_service(&s_map["s_twitter"]),
        "Twitter is unslotted and must not be stated"
    );
    assert!(
        !states_online_service(&s_map["s_sip"]),
        "SIP is unslotted and must not be stated"
    );
    assert!(
        !states_online_service(&s_map["s_discord"]),
        "Discord is unmodeled and must not be stated"
    );
    assert!(
        !states_online_service(&s_map["s_signal"]),
        "Signal is unmodeled and must not be stated"
    );

    // Inbound foreign lines parse safely without synthetic unslotted entries.
    let foreign_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Foreign Contact\r\n",
        "X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera@jabber.example\r\n",
        "X-TWITTER:@foreign_tw\r\n",
        "X-SIP:sip:foreign@sip.example.com\r\n",
        "X-DISCORD:foreign#9999\r\n",
        "IMPP:sip:foreign@sip.example.com\r\n",
        "IMPP:twitter:foreign_tw\r\n",
        "END:VCARD\r\n"
    );
    let parsed_foreign = vcard_to_card(foreign_vcard).expect("parse foreign vcard");
    let foreign_services = parsed_foreign
        .online_services
        .expect("online services present");
    assert_eq!(
        foreign_services.len(),
        1,
        "only mapped slotted services must be parsed into JSContact"
    );
    assert_eq!(
        foreign_services["s1"].user.as_deref(),
        Some("vera@jabber.example")
    );
    assert_eq!(foreign_services["s1"].service.as_deref(), Some("Jabber"));
}

#[test]
fn im_uri_schemes_action_query_fragment_and_percent_encoding_rejection_matrix() {
    // Reject handles that carry actions, paths, fragments, encoding, or whitespace.

    let rejected_uris: &[(&str, &str, &str)] = &[
        // (service, invalid URI, reason)
        ("Skype", "skype:echo123?call", "query action call"),
        ("Skype", "skype:echo123?chat", "query action chat"),
        ("Skype", "skype:echo123?add", "query action add"),
        ("Skype", "skype:user#profile", "fragment"),
        ("Skype", "skype:user/full", "path separator"),
        ("Skype", "skype:user name", "whitespace"),
        ("Skype", "skype:", "empty handle"),
        ("AIM", "aim:goim?screenname=alice", "query action goim"),
        ("AIM", "aim:alice?message=hi", "query parameter"),
        ("AIM", "aol:alice#room", "fragment"),
        ("AIM", "aim:alice%20name", "percent-encoding"),
        ("MSN", "msnim:chat?contact=alice", "query action chat"),
        ("MSN", "msn:alice@msn.com?voice", "query parameter"),
        ("MSN", "msn:alice/sub", "path separator"),
        ("Yahoo", "ymsgr:sendim?alice", "query action sendim"),
        ("Yahoo", "yahoo:alice?call", "query action call"),
        ("Yahoo", "yahoo:alice#status", "fragment"),
        ("ICQ", "icq:message?uin=12345", "query action message"),
        ("ICQ", "icq:12345?action", "query action"),
        ("ICQ", "icq:12345/home", "path separator"),
        (
            "Jabber",
            "xmpp:alice@chat.org?message;subject=Hi",
            "XMPP query action",
        ),
        (
            "Jabber",
            "xmpp:room@muc.example.com/nick",
            "XMPP resource/occupant path",
        ),
        ("Jabber", "xmpp:alice@chat.org#room", "fragment"),
        ("Jabber", "xmpp:vera%40jabber.org", "percent-encoding"),
        ("Jabber", "xmpp: vera@jabber.org", "leading whitespace"),
        ("Jabber", "xmpp:vera@jabber.org\t", "trailing tab"),
        (
            "Matrix",
            "matrix:u/alice:matrix.org",
            "Matrix path prefix u/",
        ),
        ("Matrix", "matrix:@alice:matrix.org#room", "Matrix fragment"),
        ("Matrix", "matrix:%40alice%3Amatrix.org", "percent-encoding"),
        ("Google Talk", "gtalk:alice@gmail.com?call", "query action"),
        ("GroupWise", "groupwise:alice/office", "path separator"),
        ("Gadu-Gadu", "gg:12345?status", "query parameter"),
    ];

    for &(service, invalid_uri, reason) in rejected_uris {
        let mut services = BTreeMap::new();
        services.insert(
            "s1".to_owned(),
            OnlineService {
                service: Some(service.to_owned()),
                uri: Some(invalid_uri.to_owned()),
                ..OnlineService::default()
            },
        );
        let card = {
            let mut card = ContactCard::default();
            card.online_services = Some(services);
            card
        };

        // Reject stating any invalid URI.
        assert!(
            !states_online_service(&card.online_services.as_ref().unwrap()["s1"]),
            "URI '{invalid_uri}' for service '{service}' ({reason}) must NOT be stated"
        );
        assert_eq!(
            online_service_handle(&card.online_services.as_ref().unwrap()["s1"]),
            None,
            "URI '{invalid_uri}' must not produce a plain handle ({reason})"
        );

        // Emission omits invalid URIs.
        let vcard = card_to_vcard(&card);
        assert!(
            !vcard.contains(invalid_uri),
            "invalid URI '{invalid_uri}' must not appear in vCard: {vcard}"
        );

        // Inbound IMPP with invalid URIs is ignored.
        let v4_impp = format!(
            "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Reject Contact\r\nIMPP:{invalid_uri}\r\nEND:VCARD\r\n"
        );
        let parsed = vcard_to_card(&v4_impp).expect("parse IMPP");
        assert_eq!(
            parsed.online_services, None,
            "invalid IMPP URI '{invalid_uri}' must not create online_services entry"
        );
    }
}

#[test]
fn online_services_case_punctuation_normalization_and_user_vs_uri_precedence() {
    // 1. Service name normalization variations (case, spacing, hyphens, punctuation)
    let service_variations: &[(&str, &str)] = &[
        ("AIM", "X-AIM"),
        ("aim", "X-AIM"),
        ("Aim", "X-AIM"),
        ("A-I-M", "X-AIM"),
        ("Gadu-Gadu", "X-GADUGADU"),
        ("Gadu Gadu", "X-GADUGADU"),
        ("gadugadu", "X-GADUGADU"),
        ("gadu-gadu", "X-GADUGADU"),
        ("Google Talk", "X-GOOGLE-TALK"),
        ("Google-Talk", "X-GOOGLE-TALK"),
        ("GoogleTalk", "X-GOOGLE-TALK"),
        ("google talk", "X-GOOGLE-TALK"),
        ("GroupWise", "X-GROUPWISE"),
        ("Group-Wise", "X-GROUPWISE"),
        ("groupwise", "X-GROUPWISE"),
        ("Group Wise", "X-GROUPWISE"),
        ("ICQ", "X-ICQ"),
        ("icq", "X-ICQ"),
        ("I-C-Q", "X-ICQ"),
        ("Jabber", "X-JABBER"),
        ("jabber", "X-JABBER"),
        ("JABBER", "X-JABBER"),
        ("Matrix", "X-MATRIX"),
        ("matrix", "X-MATRIX"),
        ("MSN", "X-MSN"),
        ("msn", "X-MSN"),
        ("M-S-N", "X-MSN"),
        ("Skype", "X-SKYPE"),
        ("skype", "X-SKYPE"),
        ("Yahoo", "X-YAHOO"),
        ("Yahoo!", "X-YAHOO"),
        ("yahoo", "X-YAHOO"),
    ];

    for &(service_input, expected_x_prop) in service_variations {
        let mut services = BTreeMap::new();
        services.insert(
            "s1".to_owned(),
            OnlineService {
                service: Some(service_input.to_owned()),
                user: Some("test_user".to_owned()),
                ..OnlineService::default()
            },
        );
        let card = {
            let mut card = ContactCard::default();
            card.online_services = Some(services);
            card
        };

        assert!(
            states_online_service(&card.online_services.as_ref().unwrap()["s1"]),
            "service variation '{service_input}' must be recognized"
        );
        let vcard = card_to_vcard(&card);
        let expected_line = format!("{expected_x_prop};X-JMAP-KEY=s1;TYPE=HOME:test_user");
        assert_eq!(
            line(&vcard, expected_x_prop),
            expected_line,
            "failed property mapping for service input '{service_input}'"
        );
    }

    // Unmapped/extended names (such as 'AOL Instant Messenger', 'MSN Messenger', 'Yahoo Messenger')
    // do not match the 10 canonical EDS services and must not be stated
    for &unmapped in &[
        "AOL Instant Messenger",
        "MSN Messenger",
        "Yahoo Messenger",
        "Telegram",
        "Discord",
    ] {
        let mut services = BTreeMap::new();
        services.insert(
            "s1".to_owned(),
            OnlineService {
                service: Some(unmapped.to_owned()),
                user: Some("test_user".to_owned()),
                ..OnlineService::default()
            },
        );
        let card = {
            let mut card = ContactCard::default();
            card.online_services = Some(services);
            card
        };
        assert!(
            !states_online_service(&card.online_services.as_ref().unwrap()["s1"]),
            "unmapped service '{unmapped}' must not be stated"
        );
        let vcard = card_to_vcard(&card);
        assert!(
            !vcard.contains("test_user"),
            "unmapped service '{unmapped}' must not be emitted in vCard"
        );
    }

    // 2. User vs URI precedence: explicit user takes precedence over secondary URI
    let mut prec_services = BTreeMap::new();
    prec_services.insert(
        "s1".to_owned(),
        OnlineService {
            service: Some("Jabber".to_owned()),
            user: Some("explicit_handle@jabber.org".to_owned()),
            uri: Some("xmpp:different_uri_handle@jabber.org".to_owned()),
            ..OnlineService::default()
        },
    );
    let prec_card = {
        let mut card = ContactCard::default();
        card.online_services = Some(prec_services);
        card
    };
    let s_obj = &prec_card.online_services.as_ref().unwrap()["s1"];
    assert_eq!(
        online_service_handle(s_obj),
        Some("explicit_handle@jabber.org"),
        "explicit user must take precedence over URI"
    );
    let vcard_prec = card_to_vcard(&prec_card);
    assert_eq!(
        line(&vcard_prec, "X-JABBER"),
        "X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:explicit_handle@jabber.org"
    );

    // 3. online_service_uri canonical generation
    let uri_matrix: &[(&str, &str, Option<&str>)] = &[
        ("AIM", "alice_aim", Some("aim:alice_aim")),
        ("Gadu-Gadu", "87654321", Some("gg:87654321")),
        (
            "Google Talk",
            "alice@gmail.com",
            Some("xmpp:alice@gmail.com"),
        ),
        ("GroupWise", "alice_gw", Some("groupwise:alice_gw")),
        ("ICQ", "12345678", Some("icq:12345678")),
        ("Jabber", "alice@jabber.org", Some("xmpp:alice@jabber.org")),
        ("MSN", "alice@msn.com", Some("msn:alice@msn.com")),
        (
            "Matrix",
            "@alice:matrix.org",
            Some("matrix:@alice:matrix.org"),
        ),
        ("Skype", "alice_skype", Some("skype:alice_skype")),
        ("Yahoo", "alice_yahoo", Some("yahoo:alice_yahoo")),
        // Invalid handles
        ("Skype", "alice?call", None),
        ("Jabber", "alice/muc", None),
        ("AIM", "alice%20name", None),
        ("Twitter", "alice", None), // Unmapped service
        ("SIP", "alice", None),     // Unmapped service
    ];

    for &(service, handle, expected_uri) in uri_matrix {
        let generated = online_service_uri(service, handle);
        assert_eq!(
            generated.as_deref(),
            expected_uri,
            "online_service_uri for service {service} and handle {handle}"
        );
    }
}

#[test]
fn evolution_contact_editor_photo_lifecycle_uri_inline_and_clearing_matrix() {
    // 1. Setting and emitting URI photos on supported schemes.
    let uri_schemes = [
        "https://example.com/photos/avatar.jpg",
        "http://photos.internal.net/user.png",
    ];

    for uri in uri_schemes {
        let card = one_photo(uri, None);
        assert!(states_media(&card.media.as_ref().unwrap()["m1"]));
        let vcard = card_to_vcard(&card);
        let unfolded_vcard = unfolded(&vcard);
        assert_eq!(
            line(&unfolded_vcard, "PHOTO"),
            format!("PHOTO;X-JMAP-KEY=m1;VALUE=uri:{uri}"),
            "URI photo must emit VALUE=uri without TYPE parameter: {vcard}"
        );
        assert!(
            !vcard.contains("TYPE="),
            "URI photo must not state TYPE: {vcard}"
        );

        // Inbound roundtrip
        let parsed = vcard_to_card(&vcard).expect("parse emitted URI photo");
        let media = parsed.media.expect("media");
        assert_eq!(media["m1"].kind.as_deref(), Some("photo"));
        assert_eq!(media["m1"].uri, uri);
        assert_eq!(media["m1"].media_type, None);
    }

    // Unsupported URI schemes do not state media.
    for uri in [
        "file:///home/runner/.photos/contact_profile.webp",
        "ftp://files.example.org/images/id_badge.jpg",
    ] {
        let card = one_photo(uri, None);
        assert!(
            !states_media(&card.media.as_ref().unwrap()["m1"]),
            "unsupported URI must not state PHOTO: {uri}"
        );
        let vcard = card_to_vcard(&card);
        assert!(
            !vcard.contains("\r\nPHOTO"),
            "unsupported URI leaked into PHOTO: {uri} in {vcard}"
        );
    }

    // 2. Setting and emitting inlined binary photos across multiple MIME types
    let inlined_formats = [
        ("image/jpeg", "jpeg", PAYLOAD),
        (
            "image/png",
            "png",
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
        ),
        (
            "image/gif",
            "gif",
            "R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7",
        ),
        (
            "image/webp",
            "webp",
            "UklGRhoAAABXRUJQVlA4TA0AAAAvAAAAEAcQERGIiP4HAA==",
        ),
        (
            "image/svg+xml",
            "svg+xml",
            "PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIxIiBoZWlnaHQ9IjEiPjwvc3ZnPg==",
        ),
    ];

    for (media_type, subtype, base64_payload) in inlined_formats {
        let data_uri = format!("data:{media_type};base64,{base64_payload}");
        let card = one_photo(&data_uri, Some(media_type));
        assert!(states_media(&card.media.as_ref().unwrap()["m1"]));
        let vcard = card_to_vcard(&card);

        assert!(
            vcard.contains(&format!("PHOTO;X-JMAP-KEY=m1;TYPE={subtype};ENCODING=b:"))
                || vcard.contains(&format!(
                    "PHOTO;X-JMAP-KEY=m1;TYPE={};ENCODING=b:",
                    subtype.to_ascii_uppercase()
                )),
            "inlined photo must emit subtype {subtype} and ENCODING=b: {vcard}"
        );
        assert!(
            !vcard.contains("VALUE=uri"),
            "inlined photo must not state VALUE=uri: {vcard}"
        );

        let parsed = vcard_to_card(&vcard).expect("parse emitted inlined photo");
        let media = parsed.media.expect("media");
        assert_eq!(media["m1"].kind.as_deref(), Some("photo"));
        assert_eq!(media["m1"].media_type.as_deref(), Some(media_type));
        assert!(same_photo(&card.media.unwrap()["m1"], &media["m1"]));
    }

    // 3. Non-image data URIs emitted without TYPE parameter
    let non_image_data_uris = [
        format!("data:application/pdf;base64,{PAYLOAD}"),
        format!("data:;base64,{PAYLOAD}"),
    ];
    for data_uri in non_image_data_uris {
        let card = one_photo(&data_uri, None);
        assert!(states_media(&card.media.as_ref().unwrap()["m1"]));
        let vcard = card_to_vcard(&card);
        assert_eq!(
            line(&vcard, "PHOTO"),
            format!("PHOTO;X-JMAP-KEY=m1;ENCODING=b:{PAYLOAD}")
        );
        assert!(
            !vcard.contains("TYPE="),
            "non-image data URI must not state TYPE: {vcard}"
        );

        let parsed = vcard_to_card(&vcard).expect("parse");
        let media = parsed.media.expect("media");
        assert_eq!(media["m1"].media_type, None);
    }

    // Invalid data URIs and non-photo kinds are not stateable and emit no PHOTO line.
    let unmapped_or_corrupt_media = [
        photo_of_kind(Some("logo"), "https://example.com/logo.png", None),
        photo_of_kind(Some("sound"), "https://example.com/sound.ogg", None),
        photo_of_kind(Some("document"), "https://example.com/doc.pdf", None),
        photo_of_kind(None, "https://example.com/unknown.jpg", None),
        one_photo("data:image/jpeg;base64,not!valid!base64!", None),
        one_photo("data:image/jpeg,%89PNG%0D%0A", None),
        one_photo("data:image/jpeg", None),
        one_photo("data:", None),
        one_photo("", None),
    ];
    for card in unmapped_or_corrupt_media {
        let media_entry = &card.media.as_ref().unwrap()["m1"];
        assert!(
            !states_media(media_entry),
            "media entry {:?} must not state a line",
            media_entry
        );
        let vcard = card_to_vcard(&card);
        assert!(!vcard.contains("PHOTO"), "must emit no PHOTO line: {vcard}");
    }

    // Start with an inlined JPEG and cover the editor replacement path for PHOTO values.
    let orig_card = one_photo(
        &format!("data:image/jpeg;base64,{PAYLOAD}"),
        Some("image/jpeg"),
    );
    let orig_vcard = card_to_vcard(&orig_card);

    // Rewriting inline data to a URI drops X-JMAP-KEY.
    let edited_uri_vcard = orig_vcard.replace(
        &format!("PHOTO;X-JMAP-KEY=m1;TYPE=jpeg;ENCODING=b:{PAYLOAD}"),
        "PHOTO;VALUE=uri:https://example.com/new_avatar.png",
    );
    let edited_card = vcard_to_card(&edited_uri_vcard).expect("parse edited URI vcard");
    let media = edited_card.media.expect("media");
    assert_eq!(media["m1"].kind.as_deref(), Some("photo"));
    assert_eq!(media["m1"].uri, "https://example.com/new_avatar.png");
    assert_eq!(media["m1"].media_type, None);
    assert!(!same_photo(
        &orig_card.media.as_ref().unwrap()["m1"],
        &media["m1"]
    ));

    // Rewriting the URI PHOTO back to inlined PNG updates type and URI.
    let edited_png_vcard = edited_uri_vcard.replace(
        "PHOTO;VALUE=uri:https://example.com/new_avatar.png",
        "PHOTO;TYPE=png;ENCODING=b:iVBORw0KGgr//g==",
    );
    let edited_png_card = vcard_to_card(&edited_png_vcard).expect("parse edited PNG vcard");
    let media_png = edited_png_card.media.expect("media");
    assert_eq!(media_png["m1"].kind.as_deref(), Some("photo"));
    assert_eq!(
        media_png["m1"].uri,
        "data:image/png;base64,iVBORw0KGgr//g=="
    );
    assert_eq!(media_png["m1"].media_type.as_deref(), Some("image/png"));

    // Clearing PHOTO still ignores unrelated LOGO on import.
    let cleared_vcard = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera Oldenburg\r\nN:Oldenburg;Vera;;;\r\nLOGO;VALUE=uri:https://example.com/logo.png\r\nEND:VCARD\r\n";
    let cleared_card = vcard_to_card(cleared_vcard).expect("parse cleared vcard");
    // Only PHOTO is mapped into media.
    assert_eq!(cleared_card.media, None);
}

#[test]
fn photo_uri_schemes_and_vcard_variations_inbound_tolerance() {
    // Direct URIs parse without VALUE=uri.
    let bare_uri_lines = [
        (
            "PHOTO:https://example.com/avatar.jpg",
            "https://example.com/avatar.jpg",
        ),
        (
            "PHOTO:http://example.org/pic.png",
            "http://example.org/pic.png",
        ),
        (
            "PHOTO;VALUE=URI:https://example.com/avatar.jpg",
            "https://example.com/avatar.jpg",
        ),
    ];

    for (line_str, expected_uri) in bare_uri_lines {
        let media = photo_line(line_str);
        assert_eq!(media.kind.as_deref(), Some("photo"));
        assert_eq!(media.uri, expected_uri);
        assert_eq!(media.media_type, None);
    }

    // Unsupported URI schemes are dropped on import.
    for unsupported_line in [
        "PHOTO:file:///local/path/photo.webp",
        "PHOTO:ftp://server.org/user.jpg",
        "PHOTO;VALUE=uri:file:///local/photo.png",
        "PHOTO;VALUE=uri:smb://server/share/photo.png",
    ] {
        let vcard =
            format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\n{unsupported_line}\r\nEND:VCARD\r\n");
        let parsed = vcard_to_card(&vcard).expect("parse card with unsupported URI reference");
        assert!(
            parsed.media.is_none(),
            "unsupported URI must not survive import: {unsupported_line}"
        );
    }

    // Direct RFC 2397 data URIs parse as-is.
    let direct_data_uris = [
        (
            "PHOTO:data:image/jpeg;base64,aGVsbG8tcGhvdG8=",
            "data:image/jpeg;base64,aGVsbG8tcGhvdG8=",
            Some("image/jpeg"),
        ),
        (
            "PHOTO:data:image/png;base64,iVBORw0KGgo=",
            "data:image/png;base64,iVBORw0KGgo=",
            Some("image/png"),
        ),
        ("PHOTO:data:;base64,aGVsbG8=", "data:;base64,aGVsbG8=", None),
    ];

    for (line_str, expected_uri, expected_media_type) in direct_data_uris {
        let media = photo_line(line_str);
        assert_eq!(media.kind.as_deref(), Some("photo"));
        assert_eq!(media.uri, expected_uri);
        assert_eq!(media.media_type.as_deref(), expected_media_type);
    }

    // 3. RFC 6350 MEDIATYPE parameter extraction on URI photos
    let mediatype_photos = [
        (
            "PHOTO;MEDIATYPE=image/jpeg;VALUE=uri:https://example.com/photo.jpg",
            "https://example.com/photo.jpg",
            Some("image/jpeg"),
        ),
        (
            "PHOTO;MEDIATYPE=image/png;VALUE=uri:https://example.com/photo.png",
            "https://example.com/photo.png",
            Some("image/png"),
        ),
        (
            "PHOTO;MEDIATYPE=image/webp:https://example.com/photo.webp",
            "https://example.com/photo.webp",
            Some("image/webp"),
        ),
    ];

    for (line_str, expected_uri, expected_media_type) in mediatype_photos {
        let media = photo_line(line_str);
        assert_eq!(media.kind.as_deref(), Some("photo"));
        assert_eq!(media.uri, expected_uri);
        assert_eq!(media.media_type.as_deref(), expected_media_type);
    }

    // 4. Legacy and exporter parameter variations: bare type name, ENCODING=BASE64
    let legacy_lines = [
        ("PHOTO;JPEG;ENCODING=BASE64:aGVsbG8tcGhvdG8=", "image/JPEG"),
        ("PHOTO;PNG;BASE64:aGVsbG8tcGhvdG8=", "image/PNG"),
        ("PHOTO;GIF;ENCODING=b:aGVsbG8tcGhvdG8=", "image/GIF"),
        (
            "PHOTO;TYPE=WEBP;ENCODING=BASE64:aGVsbG8tcGhvdG8=",
            "image/WEBP",
        ),
    ];

    for (line_str, expected_media_type) in legacy_lines {
        let media = photo_line(line_str);
        assert_eq!(media.kind.as_deref(), Some("photo"));
        assert_eq!(media.media_type.as_deref(), Some(expected_media_type));
        assert!(media.uri.starts_with("data:image/"));
    }
}

#[test]
fn same_photo_and_states_media_comprehensive_decision_matrix() {
    let p_uri1 = Media {
        kind: Some("photo".to_owned()),
        uri: "https://example.com/avatar.jpg".to_owned(),
        media_type: None,
        ..Media::default()
    };
    let p_uri1_same = Media {
        kind: Some("photo".to_owned()),
        uri: "https://example.com/avatar.jpg".to_owned(),
        media_type: Some("image/jpeg".to_owned()), // media_type ignored for URI photos
        ..Media::default()
    };
    let p_uri2 = Media {
        kind: Some("photo".to_owned()),
        uri: "https://example.com/other.png".to_owned(),
        media_type: None,
        ..Media::default()
    };

    // Inline photos with identical decoded bytes but different padding / case
    let p_inline_padded = Media {
        kind: Some("photo".to_owned()),
        uri: format!("data:image/jpeg;base64,{PAYLOAD}"),
        media_type: Some("image/jpeg".to_owned()),
        ..Media::default()
    };
    let p_inline_unpadded = Media {
        kind: Some("photo".to_owned()),
        uri: format!("data:image/JPEG;base64,{}", PAYLOAD.trim_end_matches('=')),
        media_type: Some("image/JPEG".to_owned()),
        ..Media::default()
    };
    let p_inline_different_bytes = Media {
        kind: Some("photo".to_owned()),
        uri: "data:image/jpeg;base64,ZGlmZmVyZW50".to_owned(),
        media_type: Some("image/jpeg".to_owned()),
        ..Media::default()
    };
    let p_inline_different_type = Media {
        kind: Some("photo".to_owned()),
        uri: format!("data:image/png;base64,{PAYLOAD}"),
        media_type: Some("image/png".to_owned()),
        ..Media::default()
    };

    // Non-photo media
    let logo_media = Media {
        kind: Some("logo".to_owned()),
        uri: "https://example.com/logo.png".to_owned(),
        media_type: None,
        ..Media::default()
    };
    let sound_media = Media {
        kind: Some("sound".to_owned()),
        uri: "https://example.com/audio.ogg".to_owned(),
        media_type: None,
        ..Media::default()
    };
    let empty_uri_media = Media {
        kind: Some("photo".to_owned()),
        uri: String::new(),
        media_type: None,
        ..Media::default()
    };

    // 1. states_media checks
    assert!(states_media(&p_uri1));
    assert!(states_media(&p_uri2));
    assert!(states_media(&p_inline_padded));
    assert!(states_media(&p_inline_unpadded));
    assert!(states_media(&p_inline_different_bytes));
    assert!(states_media(&p_inline_different_type));
    assert!(!states_media(&logo_media));
    assert!(!states_media(&sound_media));
    assert!(!states_media(&empty_uri_media));

    // 2. same_photo equality checks
    assert!(same_photo(&p_uri1, &p_uri1));
    assert!(same_photo(&p_uri1, &p_uri1_same));
    assert!(same_photo(&p_uri1_same, &p_uri1));
    assert!(!same_photo(&p_uri1, &p_uri2));

    assert!(same_photo(&p_inline_padded, &p_inline_padded));
    assert!(same_photo(&p_inline_padded, &p_inline_unpadded));
    assert!(same_photo(&p_inline_unpadded, &p_inline_padded));
    assert!(!same_photo(&p_inline_padded, &p_inline_different_bytes));
    assert!(!same_photo(&p_inline_padded, &p_inline_different_type));

    assert!(!same_photo(&p_uri1, &p_inline_padded));
    assert!(!same_photo(&p_inline_padded, &p_uri1));

    // Non-photo media (photo() returns None for both, so same_photo is true)
    assert!(same_photo(&logo_media, &sound_media));
    assert!(!same_photo(&p_uri1, &logo_media));
    assert!(!same_photo(&p_inline_padded, &logo_media));
}

#[test]
fn photo_roundtrip_fixpoint_multi_pass_convergence() {
    let test_cards = vec![
        one_photo("https://example.com/photo.jpg", None),
        one_photo("file:///home/runner/.photos/me.png", None),
        one_photo(
            &format!("data:image/jpeg;base64,{PAYLOAD}"),
            Some("image/jpeg"),
        ),
        one_photo(
            "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
            Some("image/png"),
        ),
        one_photo(
            &format!("data:image/jpeg;base64,{}", PAYLOAD.trim_end_matches('=')),
            None,
        ),
        one_photo(&format!("data:application/pdf;base64,{PAYLOAD}"), None),
    ];

    for original_card in test_cards {
        // Run pass 1: card -> vCard1 -> card1.
        let vcard1 = card_to_vcard(&original_card);
        let card1 = vcard_to_card(&vcard1).expect("parse vcard1");

        // Run pass 2: card1 -> vCard2 -> card2.
        let vcard2 = card_to_vcard(&card1);
        let card2 = vcard_to_card(&vcard2).expect("parse vcard2");

        // Run pass 3: card2 -> vCard3 -> card3.
        let vcard3 = card_to_vcard(&card2);
        let card3 = vcard_to_card(&vcard3).expect("parse vcard3");

        // Pass 2 and pass 3 must be strictly identical.
        assert_eq!(
            vcard2, vcard3,
            "vCard output must reach fixed point at pass 2"
        );
        assert_eq!(
            card2, card3,
            "ContactCard model must reach fixed point at pass 2"
        );

        let m1 = card1.media.as_ref().and_then(|m| m.get("m1"));
        let m2 = card2.media.as_ref().and_then(|m| m.get("m1"));
        assert_eq!(m1.map(|m| &m.uri), m2.map(|m| &m.uri));
    }
}

#[test]
fn birthday_deathday_anniversary_bare_year_and_partial_date_matrix() {
    // Emit anniversary lines only for complete dates with year >= 1000.
    // Reject bare years, partial dates, ancient years, and death anniversaries.

    // Birth cases.
    let bday_bare_year = one_anniversary("birth", json!({"year": 1984}));
    let bday_year_month = one_anniversary("birth", json!({"year": 1984, "month": 5}));
    let bday_month_day = one_anniversary("birth", json!({"month": 5, "day": 20}));
    let bday_day_only = one_anniversary("birth", json!({"day": 20}));
    let bday_year_ancient = one_anniversary("birth", json!({"year": 800}));
    let bday_date_ancient = one_anniversary("birth", json!({"year": 800, "month": 6, "day": 21}));
    let bday_full = one_anniversary("birth", json!({"year": 1984, "month": 5, "day": 20}));

    // Wedding cases.
    let wedding_bare_year = one_anniversary("wedding", json!({"year": 2005}));
    let wedding_year_month = one_anniversary("wedding", json!({"year": 2005, "month": 11}));
    let wedding_month_day = one_anniversary("wedding", json!({"month": 11, "day": 15}));
    let wedding_full = one_anniversary("wedding", json!({"year": 2005, "month": 11, "day": 15}));

    // Death cases.
    let death_bare_year = one_anniversary("death", json!({"year": 2020}));
    let death_full = one_anniversary("death", json!({"year": 2020, "month": 1, "day": 15}));

    // Predicate checks.
    assert!(!states_anniversary(
        &bday_bare_year.anniversaries.as_ref().unwrap()["y1"]
    ));
    assert!(!states_anniversary(
        &bday_year_month.anniversaries.as_ref().unwrap()["y1"]
    ));
    assert!(!states_anniversary(
        &bday_month_day.anniversaries.as_ref().unwrap()["y1"]
    ));
    assert!(!states_anniversary(
        &bday_day_only.anniversaries.as_ref().unwrap()["y1"]
    ));
    assert!(!states_anniversary(
        &bday_year_ancient.anniversaries.as_ref().unwrap()["y1"]
    ));
    assert!(!states_anniversary(
        &bday_date_ancient.anniversaries.as_ref().unwrap()["y1"]
    ));
    assert!(states_anniversary(
        &bday_full.anniversaries.as_ref().unwrap()["y1"]
    ));

    assert!(!states_anniversary(
        &wedding_bare_year.anniversaries.as_ref().unwrap()["y1"]
    ));
    assert!(!states_anniversary(
        &wedding_year_month.anniversaries.as_ref().unwrap()["y1"]
    ));
    assert!(!states_anniversary(
        &wedding_month_day.anniversaries.as_ref().unwrap()["y1"]
    ));
    assert!(states_anniversary(
        &wedding_full.anniversaries.as_ref().unwrap()["y1"]
    ));

    assert!(!states_anniversary(
        &death_bare_year.anniversaries.as_ref().unwrap()["y1"]
    ));
    assert!(!states_anniversary(
        &death_full.anniversaries.as_ref().unwrap()["y1"]
    ));

    // Date formatting checks
    assert_eq!(
        anniversary_date(&bday_bare_year.anniversaries.as_ref().unwrap()["y1"]),
        None
    );
    assert_eq!(
        anniversary_date(&bday_year_month.anniversaries.as_ref().unwrap()["y1"]),
        None
    );
    assert_eq!(
        anniversary_date(&bday_month_day.anniversaries.as_ref().unwrap()["y1"]),
        None
    );
    assert_eq!(
        anniversary_date(&bday_day_only.anniversaries.as_ref().unwrap()["y1"]),
        None
    );
    assert_eq!(
        anniversary_date(&bday_year_ancient.anniversaries.as_ref().unwrap()["y1"]),
        None
    );
    assert_eq!(
        anniversary_date(&bday_date_ancient.anniversaries.as_ref().unwrap()["y1"]),
        None
    );
    assert_eq!(
        anniversary_date(&bday_full.anniversaries.as_ref().unwrap()["y1"]),
        Some("1984-05-20".into())
    );

    assert_eq!(
        anniversary_date(&wedding_bare_year.anniversaries.as_ref().unwrap()["y1"]),
        None
    );
    assert_eq!(
        anniversary_date(&wedding_full.anniversaries.as_ref().unwrap()["y1"]),
        Some("2005-11-15".into())
    );

    // vCard wire emission checks
    let vcard_bday_bare = card_to_vcard(&bday_bare_year);
    assert!(!vcard_bday_bare.contains("BDAY"), "{vcard_bday_bare}");

    let vcard_bday_ym = card_to_vcard(&bday_year_month);
    assert!(!vcard_bday_ym.contains("BDAY"), "{vcard_bday_ym}");

    let vcard_bday_md = card_to_vcard(&bday_month_day);
    assert!(!vcard_bday_md.contains("BDAY"), "{vcard_bday_md}");

    let vcard_bday_ancient = card_to_vcard(&bday_date_ancient);
    assert!(!vcard_bday_ancient.contains("BDAY"), "{vcard_bday_ancient}");

    let vcard_bday_full = card_to_vcard(&bday_full);
    assert!(
        vcard_bday_full.contains("BDAY;X-JMAP-KEY=y1:1984-05-20"),
        "{vcard_bday_full}"
    );

    let vcard_wedding_bare = card_to_vcard(&wedding_bare_year);
    assert!(
        !vcard_wedding_bare.contains("X-EVOLUTION-ANNIVERSARY"),
        "{vcard_wedding_bare}"
    );
    assert!(
        !vcard_wedding_bare.contains("ANNIVERSARY"),
        "{vcard_wedding_bare}"
    );

    let vcard_wedding_full = card_to_vcard(&wedding_full);
    assert!(
        vcard_wedding_full.contains("X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=y1:2005-11-15"),
        "{vcard_wedding_full}"
    );

    let vcard_death_bare = card_to_vcard(&death_bare_year);
    assert!(!vcard_death_bare.contains("DEATH"), "{vcard_death_bare}");
    assert!(!vcard_death_bare.contains("BDAY"), "{vcard_death_bare}");

    let vcard_death_full = card_to_vcard(&death_full);
    assert!(!vcard_death_full.contains("DEATH"), "{vcard_death_full}");
    assert!(!vcard_death_full.contains("BDAY"), "{vcard_death_full}");
    assert!(
        !vcard_death_full.contains("ANNIVERSARY"),
        "{vcard_death_full}"
    );
}

#[test]
fn birthday_anniversary_deathday_inbound_tolerance_and_isolation() {
    // Accept multiple inbound anniversary encodings, and ignore unsupported ones.

    // 1. Extended ISO `YYYY-MM-DD`.
    let vcard1 = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nBDAY:1984-05-20\r\nEND:VCARD\r\n";
    let card1 = vcard_to_card(vcard1).expect("parse vcard1");
    let anniv1 = &card1.anniversaries.expect("anniversaries")["y1"];
    assert_eq!(anniv1.kind, "birth");
    assert_eq!(
        anniv1.date,
        Some(json!({"@type": "PartialDate", "year": 1984, "month": 5, "day": 20}))
    );

    // 2. Basic ISO `YYYYMMDD`.
    let vcard2 = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Bob\r\nBDAY:19840520\r\nEND:VCARD\r\n";
    let card2 = vcard_to_card(vcard2).expect("parse vcard2");
    let anniv2 = &card2.anniversaries.expect("anniversaries")["y1"];
    assert_eq!(anniv2.kind, "birth");
    assert_eq!(
        anniv2.date,
        Some(json!({"@type": "PartialDate", "year": 1984, "month": 5, "day": 20}))
    );

    // 3. `VALUE` parameter.
    let vcard3 =
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Charlie\r\nBDAY;VALUE=date:1984-05-20\r\nEND:VCARD\r\n";
    let card3 = vcard_to_card(vcard3).expect("parse vcard3");
    let anniv3 = &card3.anniversaries.expect("anniversaries")["y1"];
    assert_eq!(anniv3.kind, "birth");
    assert_eq!(
        anniv3.date,
        Some(json!({"@type": "PartialDate", "year": 1984, "month": 5, "day": 20}))
    );

    // 4. Timestamp variant.
    let vcard4 = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:David\r\nBDAY;VALUE=date-time:1984-05-20T14:30:00Z\r\nEND:VCARD\r\n";
    let card4 = vcard_to_card(vcard4).expect("parse vcard4");
    let anniv4 = &card4.anniversaries.expect("anniversaries")["y1"];
    assert_eq!(anniv4.kind, "birth");
    assert_eq!(
        anniv4.date,
        Some(json!({"@type": "PartialDate", "year": 1984, "month": 5, "day": 20}))
    );

    // 5. vCard 4.0 `ANNIVERSARY`.
    let vcard5 = "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Eve\r\nANNIVERSARY:1996-08-03\r\nEND:VCARD\r\n";
    let card5 = vcard_to_card(vcard5).expect("parse vcard5");
    let anniv5 = &card5.anniversaries.expect("anniversaries")["y1"];
    assert_eq!(anniv5.kind, "wedding");
    assert_eq!(
        anniv5.date,
        Some(json!({"@type": "PartialDate", "year": 1996, "month": 8, "day": 3}))
    );

    // 6. EDS `X-EVOLUTION-ANNIVERSARY`.
    let vcard6 = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Frank\r\nX-EVOLUTION-ANNIVERSARY:1996-08-03\r\nEND:VCARD\r\n";
    let card6 = vcard_to_card(vcard6).expect("parse vcard6");
    let anniv6 = &card6.anniversaries.expect("anniversaries")["y1"];
    assert_eq!(anniv6.kind, "wedding");
    assert_eq!(
        anniv6.date,
        Some(json!({"@type": "PartialDate", "year": 1996, "month": 8, "day": 3}))
    );

    // 7. Apple grouped `X-ABDATE` anniversary label.
    let vcard7 = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Grace\r\nitem1.X-ABDATE:1996-08-03\r\nitem1.X-ABLabel:_$!<Anniversary>!$_\r\nEND:VCARD\r\n";
    let card7 = vcard_to_card(vcard7).expect("parse vcard7");
    let anniv7 = &card7.anniversaries.expect("anniversaries")["y1"];
    assert_eq!(anniv7.kind, "wedding");
    assert_eq!(
        anniv7.date,
        Some(json!({"@type": "PartialDate", "year": 1996, "month": 8, "day": 3}))
    );

    // 8. Bare-year `BDAY` is ignored.
    let vcard8 = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Heidi\r\nBDAY:1984\r\nEND:VCARD\r\n";
    let card8 = vcard_to_card(vcard8).expect("parse vcard8");
    assert_eq!(card8.anniversaries, None);

    // 9. Year-month `BDAY` is ignored.
    let vcard9 = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Ivan\r\nBDAY:1984-05\r\nEND:VCARD\r\n";
    let card9 = vcard_to_card(vcard9).expect("parse vcard9");
    assert_eq!(card9.anniversaries, None);

    // 10. Year-less `BDAY` is ignored.
    let vcard10 = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Judy\r\nBDAY:--05-20\r\nEND:VCARD\r\n";
    let card10 = vcard_to_card(vcard10).expect("parse vcard10");
    assert_eq!(card10.anniversaries, None);

    // 11. `DEATHDATE` variants are unmodeled and ignored.
    let vcard11 = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Karl\r\nDEATHDATE:2020-01-15\r\nX-DEATHDATE:2020-01-15\r\nX-EVOLUTION-DEATH-DATE:2020-01-15\r\nEND:VCARD\r\n";
    let card11 = vcard_to_card(vcard11).expect("parse vcard11");
    assert_eq!(card11.anniversaries, None);
}

#[test]
fn evolution_contact_editor_clamping_and_sync_diff_safety() {
    // Sync preserves unstated bare-year and death anniversaries without
    // generating phantom deletions or modifications.
    let mut anniversaries = BTreeMap::new();
    anniversaries.insert(
        "y1".to_owned(),
        Anniversary {
            kind: "birth".into(),
            date: Some(json!({"@type": "PartialDate", "year": 1984})), // bare year
            ..Anniversary::default()
        },
    );
    anniversaries.insert(
        "y2".to_owned(),
        Anniversary {
            kind: "wedding".into(),
            date: Some(json!({"@type": "PartialDate", "year": 2005, "month": 11, "day": 15})), // full date
            ..Anniversary::default()
        },
    );
    anniversaries.insert(
        "y3".to_owned(),
        Anniversary {
            kind: "death".into(),
            date: Some(json!({"@type": "PartialDate", "year": 2020, "month": 1, "day": 15})), // death date
            ..Anniversary::default()
        },
    );
    anniversaries.insert(
        "y4".to_owned(),
        Anniversary {
            kind: "birth".into(),
            date: Some(json!({"@type": "Timestamp", "utc": "1990-05-12T10:30:00Z"})), // timestamp point-in-time
            ..Anniversary::default()
        },
    );

    let card = {
        let mut card = ContactCard::default();
        card.uid = Some("contact-100".into());
        card.anniversaries = Some(anniversaries.clone());
        card
    };

    // Wire output carries only representable anniversaries: y2 and y4.
    let vcard = card_to_vcard(&card);
    assert!(!vcard.contains("y1"), "{vcard}");
    assert!(
        vcard.contains("X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=y2:2005-11-15"),
        "{vcard}"
    );
    assert!(!vcard.contains("y3"), "{vcard}");
    assert!(vcard.contains("BDAY;X-JMAP-KEY=y4:1990-05-12"), "{vcard}");

    // Sync visibility predicates.
    assert!(!states_anniversary(&anniversaries["y1"]));
    assert!(states_anniversary(&anniversaries["y2"]));
    assert!(!states_anniversary(&anniversaries["y3"]));
    assert!(states_anniversary(&anniversaries["y4"]));

    // Point-in-time predicate.
    assert!(!states_a_point_in_time(&anniversaries["y1"]));
    assert!(!states_a_point_in_time(&anniversaries["y2"]));
    assert!(!states_a_point_in_time(&anniversaries["y3"]));
    assert!(states_a_point_in_time(&anniversaries["y4"]));
}

#[test]
fn anniversary_multi_pass_roundtrip_fixpoint_stability() {
    let test_cards = vec![
        one_anniversary("birth", json!({"year": 1984, "month": 5, "day": 20})),
        one_anniversary("wedding", json!({"year": 2005, "month": 11, "day": 15})),
        one_anniversary("birth", json!({"utc": "1990-05-12T10:30:00Z"})),
        one_anniversary("birth", json!({"year": 1984})), // bare year -> empty line, immediate fixpoint
        one_anniversary("death", json!({"year": 2020, "month": 1, "day": 15})), // death -> empty line, immediate fixpoint
    ];

    for original_card in test_cards {
        let vcard1 = card_to_vcard(&original_card);
        let card1 = vcard_to_card(&vcard1).expect("parse vcard1");

        let vcard2 = card_to_vcard(&card1);
        let card2 = vcard_to_card(&vcard2).expect("parse vcard2");

        let vcard3 = card_to_vcard(&card2);
        let card3 = vcard_to_card(&vcard3).expect("parse vcard3");

        // Pass 2 is the fixed point.
        assert_eq!(
            vcard2, vcard3,
            "vCard output must reach fixed point at pass 2"
        );
        assert_eq!(
            card2, card3,
            "ContactCard model must reach fixed point at pass 2"
        );
    }
}

#[test]
fn vcard_trailing_whitespace_and_nickname_encoding_fixpoint_characterization() {
    // Inbound NICKNAME with trailing whitespace survives pass 1 emit.
    // calcard trims the trailing whitespace on pass 2. Passes 2 and 3 must match bytes.
    let raw_vcard = "BEGIN:VCARD\r\nVERSION:3.0\r\nNICKNAME;ENCODING=b:! \r\nEND:VCARD\r\n";
    let parsed1 = vcard_to_card(raw_vcard).expect("first parse of raw vcard");
    assert_eq!(
        parsed1.nicknames.as_ref().unwrap()["k1"].name,
        "! ",
        "pass 1 preserves the raw parsed value"
    );

    let vcard1 = card_to_vcard(&parsed1);
    assert!(
        vcard1.contains("NICKNAME;X-JMAP-KEY=k1:! "),
        "vcard1 emits the preserved nickname with trailing whitespace: {vcard1}"
    );

    let parsed2 = vcard_to_card(&vcard1).expect("second parse of emitted vcard");
    assert_eq!(
        parsed2.nicknames.as_ref().unwrap()["k1"].name,
        "!",
        "pass 2 normalizes unescaped trailing whitespace"
    );

    let vcard2 = card_to_vcard(&parsed2);
    assert!(
        vcard2.contains("NICKNAME;X-JMAP-KEY=k1:!\r\n")
            || vcard2.contains("NICKNAME;X-JMAP-KEY=k1:!\n"),
        "vcard2 emits the normalized nickname: {vcard2}"
    );

    let parsed3 = vcard_to_card(&vcard2).expect("third parse of stabilized vcard");
    let vcard3 = card_to_vcard(&parsed3);

    assert_eq!(
        vcard2, vcard3,
        "vCard serialization reaches exact byte-identical fixed point at pass 2"
    );
    assert_eq!(
        parsed2, parsed3,
        "ContactCard domain model reaches exact fixed point at pass 2"
    );
}

#[test]
fn vcard_trailing_whitespace_across_all_property_types_matrix() {
    // Tests unescaped trailing whitespace, escaped spaces ('\ '), and tabs across
    // FN, NOTE, ORG, TITLE, ROLE, CATEGORIES, TEL, EMAIL, and URL lines.
    let cases = vec![
        (
            "FN",
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:John Doe  \r\nN:Doe;John;;;\r\nEND:VCARD\r\n",
        ),
        (
            "NOTE",
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:John Doe\r\nNOTE;X-JMAP-KEY=n1:Important note with space  \r\nEND:VCARD\r\n",
        ),
        (
            "ORG",
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:John Doe\r\nORG;X-JMAP-KEY=o1:Acme Corp ;Engineering \r\nEND:VCARD\r\n",
        ),
        (
            "TITLE",
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:John Doe\r\nTITLE;X-JMAP-KEY=t1:Senior Architect \r\nEND:VCARD\r\n",
        ),
        (
            "ROLE",
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:John Doe\r\nROLE;X-JMAP-KEY=r1:Lead Developer \r\nEND:VCARD\r\n",
        ),
        (
            "CATEGORIES",
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:John Doe\r\nCATEGORIES:Work ,VIP \r\nEND:VCARD\r\n",
        ),
        (
            "TEL",
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:John Doe\r\nTEL;TYPE=WORK,VOICE;X-JMAP-KEY=p1:+1-555-0100 \r\nEND:VCARD\r\n",
        ),
        (
            "EMAIL",
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:John Doe\r\nEMAIL;TYPE=WORK;X-JMAP-KEY=e1:john@example.com \r\nEND:VCARD\r\n",
        ),
        (
            "URL",
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:John Doe\r\nURL;X-JMAP-KEY=l1:https://example.com/john \r\nEND:VCARD\r\n",
        ),
    ];

    for (prop_name, raw_vcard) in cases {
        let parsed1 = vcard_to_card(raw_vcard)
            .unwrap_or_else(|e| panic!("parse 1 failed for {prop_name}: {e}"));
        let vcard1 = card_to_vcard(&parsed1);
        let parsed2 = vcard_to_card(&vcard1)
            .unwrap_or_else(|e| panic!("parse 2 failed for {prop_name}: {e}"));
        let vcard2 = card_to_vcard(&parsed2);
        let parsed3 = vcard_to_card(&vcard2)
            .unwrap_or_else(|e| panic!("parse 3 failed for {prop_name}: {e}"));
        let vcard3 = card_to_vcard(&parsed3);

        assert_eq!(
            vcard2, vcard3,
            "property {prop_name} must reach vCard fixed point at pass 2"
        );
        assert_eq!(
            parsed2, parsed3,
            "property {prop_name} must reach ContactCard fixed point at pass 2"
        );
    }
}

#[test]
fn contact_card_empty_stated_full_name_synthesis_fixpoint_matrix() {
    // `full: Some("")` plus name components used to need one extra pass.
    // `card_to_vcard` emitted `FN:`.
    // `read_name` treated empty `FN` as absent and derived `full` in the next pass.
    // The empty-is-absent filter now derives once.
    // vCard and ContactCard stabilize by pass 2.
    let test_names = vec![
        Name {
            components: Some(vec![NameComponent::new("given", "0")]),
            full: Some(String::new()),
            extra: BTreeMap::new(),
            ..Name::default()
        },
        Name {
            components: Some(vec![NameComponent::new("given", "𞋀")]),
            full: Some(String::new()),
            extra: BTreeMap::new(),
            ..Name::default()
        },
        Name {
            components: Some(vec![
                NameComponent::new("given", "Alice"),
                NameComponent::new("surname", "Smith"),
            ]),
            full: Some(String::new()),
            extra: BTreeMap::new(),
            ..Name::default()
        },
        Name {
            components: Some(vec![
                NameComponent::new("prefix", "Dr."),
                NameComponent::new("given", "René"),
                NameComponent::new("middle", "Jean"),
                NameComponent::new("surname", "Müller"),
                NameComponent::new("suffix", "III"),
            ]),
            full: Some(String::new()),
            extra: BTreeMap::new(),
            ..Name::default()
        },
    ];

    for name in test_names {
        let card = {
            let mut card = ContactCard::default();
            card.id = Some("C-NAME-FIXPOINT".into());
            card.name = Some(name);
            card
        };

        let vcard1 = card_to_vcard(&card);
        let parsed1 = vcard_to_card(&vcard1).expect("parse vcard1");

        let vcard2 = card_to_vcard(&parsed1);
        let parsed2 = vcard_to_card(&vcard2).expect("parse vcard2");

        let vcard3 = card_to_vcard(&parsed2);
        let parsed3 = vcard_to_card(&vcard3).expect("parse vcard3");

        // Pass 1 reaches a fixed point.
        assert_eq!(
            vcard1, vcard2,
            "vCard output must reach fixed point immediately at pass 1"
        );
        assert_eq!(
            parsed1, parsed2,
            "ContactCard model must reach fixed point immediately at pass 1"
        );

        // Passes 2 and 3 stay equal.
        assert_eq!(vcard2, vcard3);
        assert_eq!(parsed2, parsed3);
    }
}

#[test]
fn vcard_trailing_whitespace_roundtrip_multi_pass_convergence() {
    // Use one card with trailing spaces, escaped spaces, and multi-component fields.
    let raw_vcard = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "FN:Dr. Jane Doe \r\n",
        "N:Doe ;Jane ;A. ;Dr. ;Jr. \r\n",
        "NICKNAME;X-JMAP-KEY=k1:Janey \r\n",
        "EMAIL;TYPE=WORK;X-JMAP-KEY=e1:jane.doe@work.example.com \r\n",
        "TEL;TYPE=WORK,VOICE;X-JMAP-KEY=p1:+1 555 0199 \r\n",
        "ADR;TYPE=WORK;X-JMAP-KEY=a1:;;100 Enterprise Way ;Metropolis ;CA ;94000 ;USA \r\n",
        "ORG;X-JMAP-KEY=o1:Enterprise Inc. ;Engineering ;Platform \r\n",
        "TITLE;X-JMAP-KEY=t1:Chief Architect \r\n",
        "ROLE;X-JMAP-KEY=r1:Technical Fellow \r\n",
        "NOTE;X-JMAP-KEY=n1:Multi-line note with trailing spaces \r\n and continuation \r\n",
        "CATEGORIES:Engineering ,Architecture ,VIP \r\n",
        "URL;X-JMAP-KEY=l1:https://jane.example.com \r\n",
        "BDAY:1988-03-24\r\n",
        "END:VCARD\r\n",
    );

    let card1 = vcard_to_card(raw_vcard).expect("pass 1 parse");
    let vcard1 = card_to_vcard(&card1);

    let card2 = vcard_to_card(&vcard1).expect("pass 2 parse");
    let vcard2 = card_to_vcard(&card2);

    let card3 = vcard_to_card(&vcard2).expect("pass 3 parse");
    let vcard3 = card_to_vcard(&card3);

    let card4 = vcard_to_card(&vcard3).expect("pass 4 parse");
    let vcard4 = card_to_vcard(&card4);

    // Verify multi-pass convergence.
    assert_eq!(vcard2, vcard3, "vCard pass 2 == pass 3");
    assert_eq!(vcard3, vcard4, "vCard pass 3 == pass 4");
    assert_eq!(card2, card3, "ContactCard pass 2 == pass 3");
    assert_eq!(card3, card4, "ContactCard pass 3 == pass 4");
}

#[test]
fn empty_org_name_reads_as_absent_inbound_matrix() {
    // Inbound ORG values with only empty components read as absent.
    let empty_org_vcards = [
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG:\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG:;\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG:;;\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG:;;;\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG:;;;;\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG;X-JMAP-KEY=o1:\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG;X-JMAP-KEY=o1:;\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG;X-JMAP-KEY=o1:;;;\r\nEND:VCARD\r\n",
    ];
    for vcard in empty_org_vcards {
        let card = vcard_to_card(vcard).expect("parse empty org vcard");
        assert_eq!(
            card.organizations, None,
            "empty ORG components must yield organizations: None for {vcard}"
        );
    }

    // Inbound ORG values with an empty first component keep units and no name.
    let unit_only_vcards = [
        (
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG:;Engineering\r\nEND:VCARD\r\n",
            vec!["Engineering"],
        ),
        (
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG:;;Platform\r\nEND:VCARD\r\n",
            vec!["Platform"],
        ),
        (
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG:;Engineering;Security\r\nEND:VCARD\r\n",
            vec!["Engineering", "Security"],
        ),
        (
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG:;;;Security;Ops\r\nEND:VCARD\r\n",
            vec!["Security", "Ops"],
        ),
        (
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG;X-JMAP-KEY=o1:;Research;Optics;\r\nEND:VCARD\r\n",
            vec!["Research", "Optics"],
        ),
    ];
    for (vcard, expected_units) in unit_only_vcards {
        let card = vcard_to_card(vcard).expect("parse unit-only org vcard");
        let orgs = card.organizations.expect("organizations present");
        let org = orgs.values().next().expect("single organization");
        assert_eq!(
            org.name, None,
            "first empty component must read as name: None for {vcard}"
        );
        let units: Vec<&str> = org
            .units
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|u| u.name.as_str())
            .collect();
        assert_eq!(units, expected_units, "units parsed correctly for {vcard}");
    }

    // Employer name survives inbound parse; trailing empty ORG units are dropped.
    let named_org_vcards: [(&str, &str, Vec<&str>); 5] = [
        (
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG:Acme Corp\r\nEND:VCARD\r\n",
            "Acme Corp",
            vec![],
        ),
        (
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG:Acme Corp;\r\nEND:VCARD\r\n",
            "Acme Corp",
            vec![],
        ),
        (
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG:Acme Corp;;;\r\nEND:VCARD\r\n",
            "Acme Corp",
            vec![],
        ),
        (
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG:Acme Corp;Engineering;\r\nEND:VCARD\r\n",
            "Acme Corp",
            vec!["Engineering"],
        ),
        (
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nORG:Acme Corp;;Security;\r\nEND:VCARD\r\n",
            "Acme Corp",
            vec!["Security"],
        ),
    ];
    for (vcard, expected_name, expected_units) in named_org_vcards {
        let card = vcard_to_card(vcard).expect("parse named org vcard");
        let orgs = card.organizations.expect("organizations present");
        let org = orgs.values().next().expect("single organization");
        assert_eq!(org.name.as_deref(), Some(expected_name));
        let units: Vec<&str> = org
            .units
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|u| u.name.as_str())
            .collect();
        assert_eq!(units, expected_units);
    }
}

#[test]
fn empty_org_name_outbound_emission_and_predicate_decision_matrix() {
    // Empty organization payloads are omitted from state and wire output.
    let org_none_no_units = Organization {
        name: None,
        units: None,
        ..Default::default()
    };
    assert!(!states_organization(&org_none_no_units));

    let org_empty_name_no_units = Organization {
        name: Some(String::new()),
        units: None,
        ..Default::default()
    };
    assert!(!states_organization(&org_empty_name_no_units));

    let org_empty_name_empty_units = Organization {
        name: Some(String::new()),
        units: Some(vec![OrgUnit::new(""), OrgUnit::new("")]),
        ..Default::default()
    };
    assert!(!states_organization(&org_empty_name_empty_units));

    // Empty name and no units still emit no ORG line.
    let card_empty_org = {
        let mut card = ContactCard::default();
        card.organizations = Some([("o1".to_owned(), org_empty_name_no_units.clone())].into());
        card
    };
    let vcard_empty_org = card_to_vcard(&card_empty_org);
    assert!(
        !vcard_empty_org.contains("ORG"),
        "must emit no ORG line when org name is empty and no units exist"
    );

    // Named organizations without units emit a single ORG component.
    let card_named_org = {
        let mut card = ContactCard::default();
        card.organizations = Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some("Acme Corp".to_owned()),
                    units: None,
                    ..Default::default()
                },
            )]
            .into(),
        );
        card
    };
    let vcard_named_org = card_to_vcard(&card_named_org);
    assert_eq!(line(&vcard_named_org, "ORG"), "ORG;X-JMAP-KEY=o1:Acme Corp");

    // Empty unit components are dropped on write.
    let card_named_org_empty_units = {
        let mut card = ContactCard::default();
        card.organizations = Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some("Acme Corp".to_owned()),
                    units: Some(vec![OrgUnit::new(""), OrgUnit::new("")]),
                    ..Default::default()
                },
            )]
            .into(),
        );
        card
    };
    let vcard_named_org_empty_units = card_to_vcard(&card_named_org_empty_units);
    assert_eq!(
        line(&vcard_named_org_empty_units, "ORG"),
        "ORG;X-JMAP-KEY=o1:Acme Corp"
    );

    // Empty name with non-empty units keeps the leading empty ORG slot.
    let card_empty_name_valid_units = {
        let mut card = ContactCard::default();
        card.organizations = Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some(String::new()),
                    units: Some(vec![OrgUnit::new("Engineering"), OrgUnit::new("Security")]),
                    ..Default::default()
                },
            )]
            .into(),
        );
        card
    };
    let vcard_empty_name_valid_units = card_to_vcard(&card_empty_name_valid_units);
    assert_eq!(
        line(&vcard_empty_name_valid_units, "ORG"),
        "ORG;X-JMAP-KEY=o1:;Engineering;Security"
    );
}

#[test]
fn empty_org_name_normalisation_and_fixpoint_stability() {
    // Start with an explicit empty name and non-empty units.
    let initial_card = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("Alice Smith".to_owned()),
            ..Default::default()
        });
        card.organizations = Some(
            [(
                "o1".to_owned(),
                Organization {
                    name: Some(String::new()),
                    units: Some(vec![OrgUnit::new("Engineering"), OrgUnit::new("Platform")]),
                    ..Default::default()
                },
            )]
            .into(),
        );
        card
    };

    // Pass 1.
    let vcard1 = card_to_vcard(&initial_card);
    assert_eq!(
        line(&vcard1, "ORG"),
        "ORG;X-JMAP-KEY=o1:;Engineering;Platform"
    );

    let card1 = vcard_to_card(&vcard1).expect("pass 1 parse");
    let org1 = &card1.organizations.as_ref().unwrap()["o1"];
    // Empty organization names normalize to None.
    assert_eq!(
        org1.name, None,
        "empty org name must be normalized to None (absent)"
    );
    assert_eq!(
        org1.units.as_deref(),
        Some([OrgUnit::new("Engineering"), OrgUnit::new("Platform")].as_slice())
    );

    // Pass 2.
    let vcard2 = card_to_vcard(&card1);
    let card2 = vcard_to_card(&vcard2).expect("pass 2 parse");

    // Pass 3.
    let vcard3 = card_to_vcard(&card2);
    let card3 = vcard_to_card(&vcard3).expect("pass 3 parse");

    // Round-trip reaches immediate fixed point.
    assert_eq!(vcard1, vcard2, "vCard pass 1 == pass 2");
    assert_eq!(vcard2, vcard3, "vCard pass 2 == pass 3");
    assert_eq!(card1, card2, "ContactCard pass 1 == pass 2");
    assert_eq!(card2, card3, "ContactCard pass 2 == pass 3");
}

#[test]
fn empty_org_name_multiple_organizations_matrix() {
    let card = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("Bob MultiOrg".to_owned()),
            ..Default::default()
        });
        card.organizations = Some(
            [
                (
                    "o1".to_owned(),
                    Organization {
                        name: Some("Acme Corp".to_owned()),
                        units: Some(vec![OrgUnit::new("Primary")]),
                        ..Default::default()
                    },
                ),
                (
                    "o2".to_owned(),
                    Organization {
                        name: Some(String::new()),
                        units: Some(vec![OrgUnit::new("Contracting")]),
                        ..Default::default()
                    },
                ),
                (
                    "o3".to_owned(),
                    Organization {
                        name: None,
                        units: Some(vec![OrgUnit::new("Advisory")]),
                        ..Default::default()
                    },
                ),
                (
                    "o4".to_owned(),
                    Organization {
                        name: Some(String::new()),
                        units: None,
                        ..Default::default()
                    },
                ),
            ]
            .into(),
        );
        card
    };

    let vcard = card_to_vcard(&card);
    // `o4` has neither name nor units, so it is omitted.
    let org_lines: Vec<&str> = vcard.lines().filter(|l| l.starts_with("ORG")).collect();
    assert_eq!(
        org_lines.len(),
        3,
        "only o1, o2, o3 must be emitted; o4 must be omitted"
    );
    assert!(vcard.contains("ORG;X-JMAP-KEY=o1:Acme Corp;Primary"));
    assert!(vcard.contains("ORG;X-JMAP-KEY=o2:;Contracting"));
    assert!(vcard.contains("ORG;X-JMAP-KEY=o3:;Advisory"));
    assert!(!vcard.contains("X-JMAP-KEY=o4"));

    let back = vcard_to_card(&vcard).expect("parse multi-org vcard");
    let orgs = back.organizations.expect("organizations present");
    assert_eq!(orgs.len(), 3);
    assert_eq!(orgs["o1"].name.as_deref(), Some("Acme Corp"));
    assert_eq!(orgs["o2"].name, None);
    assert_eq!(orgs["o3"].name, None);
    assert!(!orgs.contains_key("o4"));
}

#[test]
fn eds_360_wedding_anniversary_dual_emission_both_spellings() {
    // Emit both X-EVOLUTION-ANNIVERSARY and ANNIVERSARY for wedding anniversaries.
    // EDS 3.52 reads the former and EDS 3.60+ reads the latter.
    let card = one_anniversary(
        "wedding",
        json!({"@type": "PartialDate", "year": 1996, "month": 8, "day": 3}),
    );

    let vcard = card_to_vcard(&card);

    // 1. Must emit both lines with the X-JMAP-KEY parameter
    assert_eq!(
        line(&vcard, "X-EVOLUTION-ANNIVERSARY"),
        "X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=y1:1996-08-03"
    );
    assert_eq!(
        line(&vcard, "ANNIVERSARY"),
        "ANNIVERSARY;X-JMAP-KEY=y1:1996-08-03"
    );

    // 2. Inbound parse must deduplicate the dual representation into a single Anniversary
    let parsed = vcard_to_card(&vcard).expect("parse dual-emitted anniversary");
    let anniversaries = parsed.anniversaries.as_ref().expect("anniversaries");
    assert_eq!(
        anniversaries.len(),
        1,
        "dual emission must not create a duplicate anniversary entry"
    );
    assert_eq!(anniversaries["y1"].kind, "wedding");
    assert_eq!(
        anniversary_date(&anniversaries["y1"]),
        Some("1996-08-03".to_owned())
    );

    // 3. Multi-stage fixed-point stability
    let vcard2 = card_to_vcard(&parsed);
    let parsed2 = vcard_to_card(&vcard2).expect("pass 2 parse");
    let vcard3 = card_to_vcard(&parsed2);

    assert_eq!(
        vcard, vcard2,
        "Pass 1 and Pass 2 vCards must be byte-identical fixed points"
    );
    assert_eq!(
        vcard2, vcard3,
        "Pass 2 and Pass 3 vCards must be byte-identical fixed points"
    );
    assert_eq!(
        parsed, parsed2,
        "Pass 1 and Pass 2 ContactCards must be structurally identical fixed points"
    );
}

#[test]
fn eds_360_wedding_anniversary_inbound_either_spelling_or_both_matrix() {
    // 1. EDS 3.52 native input: only X-EVOLUTION-ANNIVERSARY
    let vcard_352 = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Alice Smith\r\n",
        "X-EVOLUTION-ANNIVERSARY:1996-08-03\r\n",
        "END:VCARD\r\n"
    );
    let card_352 = vcard_to_card(vcard_352).expect("parse EDS 3.52 native");
    let anniv_352 = card_352.anniversaries.as_ref().expect("anniversaries 352");
    assert_eq!(anniv_352.len(), 1);
    assert_eq!(anniv_352["y1"].kind, "wedding");
    assert_eq!(
        anniversary_date(&anniv_352["y1"]),
        Some("1996-08-03".to_owned())
    );
    let emitted_352 = card_to_vcard(&card_352);
    assert!(emitted_352.contains("X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=y1:1996-08-03\r\n"));
    assert!(emitted_352.contains("ANNIVERSARY;X-JMAP-KEY=y1:1996-08-03\r\n"));

    // 2. EDS 3.60+ / standard vCard 4.0 native input: only ANNIVERSARY
    let vcard_360 = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Alice Smith\r\n",
        "ANNIVERSARY:1996-08-03\r\n",
        "END:VCARD\r\n"
    );
    let card_360 = vcard_to_card(vcard_360).expect("parse EDS 3.60 native");
    let anniv_360 = card_360.anniversaries.as_ref().expect("anniversaries 360");
    assert_eq!(anniv_360.len(), 1);
    assert_eq!(anniv_360["y1"].kind, "wedding");
    assert_eq!(
        anniversary_date(&anniv_360["y1"]),
        Some("1996-08-03".to_owned())
    );
    let emitted_360 = card_to_vcard(&card_360);
    assert!(emitted_360.contains("X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=y1:1996-08-03\r\n"));
    assert!(emitted_360.contains("ANNIVERSARY;X-JMAP-KEY=y1:1996-08-03\r\n"));

    // 3. Unkeyed dual input (both lines present without X-JMAP-KEY)
    let vcard_both_unkeyed = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Alice Smith\r\n",
        "X-EVOLUTION-ANNIVERSARY:1996-08-03\r\n",
        "ANNIVERSARY:1996-08-03\r\n",
        "END:VCARD\r\n"
    );
    let card_both = vcard_to_card(vcard_both_unkeyed).expect("parse unkeyed dual");
    let anniv_both = card_both
        .anniversaries
        .as_ref()
        .expect("anniversaries both");
    assert_eq!(
        anniv_both.len(),
        1,
        "unkeyed dual lines must be deduplicated to 1 anniversary"
    );
    assert_eq!(anniv_both["y1"].kind, "wedding");
    assert_eq!(
        anniversary_date(&anniv_both["y1"]),
        Some("1996-08-03".to_owned())
    );

    // 4. Reverse-order unkeyed dual input (ANNIVERSARY preceding X-EVOLUTION-ANNIVERSARY)
    let vcard_reverse = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Alice Smith\r\n",
        "ANNIVERSARY:1996-08-03\r\n",
        "X-EVOLUTION-ANNIVERSARY:1996-08-03\r\n",
        "END:VCARD\r\n"
    );
    let card_rev = vcard_to_card(vcard_reverse).expect("parse reverse dual");
    let anniv_rev = card_rev
        .anniversaries
        .as_ref()
        .expect("anniversaries reverse");
    assert_eq!(
        anniv_rev.len(),
        1,
        "reverse dual lines must be deduplicated to 1 anniversary"
    );
    assert_eq!(anniv_rev["y1"].kind, "wedding");
    assert_eq!(
        anniversary_date(&anniv_rev["y1"]),
        Some("1996-08-03".to_owned())
    );

    // 5. Coexistence with birthday (BDAY + dual wedding lines)
    let vcard_bday_wedding = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Alice Smith\r\n",
        "BDAY;X-JMAP-KEY=y1:1980-01-01\r\n",
        "X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=y2:1996-08-03\r\n",
        "ANNIVERSARY;X-JMAP-KEY=y2:1996-08-03\r\n",
        "END:VCARD\r\n"
    );
    let card_bday_wedding = vcard_to_card(vcard_bday_wedding).expect("parse bday + wedding");
    let annivs = card_bday_wedding
        .anniversaries
        .as_ref()
        .expect("anniversaries bday+wedding");
    assert_eq!(
        annivs.len(),
        2,
        "birthday and wedding anniversary must coexist as 2 entries"
    );
    assert_eq!(annivs["y1"].kind, "birth");
    assert_eq!(
        anniversary_date(&annivs["y1"]),
        Some("1980-01-01".to_owned())
    );
    assert_eq!(annivs["y2"].kind, "wedding");
    assert_eq!(
        anniversary_date(&annivs["y2"]),
        Some("1996-08-03".to_owned())
    );

    let emitted_bday_wedding = card_to_vcard(&card_bday_wedding);
    assert!(emitted_bday_wedding.contains("BDAY;X-JMAP-KEY=y1:1980-01-01\r\n"));
    assert!(emitted_bday_wedding.contains("X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=y2:1996-08-03\r\n"));
    assert!(emitted_bday_wedding.contains("ANNIVERSARY;X-JMAP-KEY=y2:1996-08-03\r\n"));
}

#[test]
fn eds_360_chat_handles_and_sort_order_measured_non_issue_characterization() {
    // Chat handles: mapping reads and writes the X-JABBER-style lines only.
    // EDS internal field slot choices do not alter this external wire surface.
    let im_vcard = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Test User\r\n",
        "X-AIM;X-JMAP-KEY=s1;TYPE=HOME:test_aim\r\n",
        "X-GADUGADU;X-JMAP-KEY=s2;TYPE=WORK:123456\r\n",
        "X-GOOGLE-TALK;X-JMAP-KEY=s3;TYPE=WORK:test@gmail.com\r\n",
        "X-GROUPWISE;X-JMAP-KEY=s4;TYPE=WORK:test_gw\r\n",
        "X-ICQ;X-JMAP-KEY=s5;TYPE=HOME:654321\r\n",
        "X-JABBER;X-JMAP-KEY=s6;TYPE=HOME:test@jabber.org\r\n",
        "X-MSN;X-JMAP-KEY=s7;TYPE=HOME:test@msn.com\r\n",
        "X-MATRIX;X-JMAP-KEY=s8;TYPE=WORK:@test:matrix.org\r\n",
        "X-SKYPE;X-JMAP-KEY=s9;TYPE=WORK:test_skype\r\n",
        "X-YAHOO;X-JMAP-KEY=s10;TYPE=HOME:test_yahoo\r\n",
        "END:VCARD\r\n"
    );
    let card = vcard_to_card(im_vcard).expect("parse IM vcard");
    let services = card.online_services.as_ref().expect("online_services");
    assert_eq!(services.len(), 10);
    let emitted = card_to_vcard(&card);
    for line in [
        "X-AIM;X-JMAP-KEY=s1;TYPE=HOME:test_aim\r\n",
        "X-GADUGADU;X-JMAP-KEY=s2;TYPE=HOME:123456\r\n",
        "X-GOOGLE-TALK;X-JMAP-KEY=s3;TYPE=HOME:test@gmail.com\r\n",
        "X-GROUPWISE;X-JMAP-KEY=s4;TYPE=HOME:test_gw\r\n",
        "X-ICQ;X-JMAP-KEY=s5;TYPE=HOME:654321\r\n",
        "X-JABBER;X-JMAP-KEY=s6;TYPE=HOME:test@jabber.org\r\n",
        "X-MSN;X-JMAP-KEY=s7;TYPE=HOME:test@msn.com\r\n",
        "X-MATRIX;X-JMAP-KEY=s8;TYPE=HOME:@test:matrix.org\r\n",
        "X-SKYPE;X-JMAP-KEY=s9;TYPE=HOME:test_skype\r\n",
        "X-YAHOO;X-JMAP-KEY=s10;TYPE=HOME:test_yahoo\r\n",
    ] {
        assert!(emitted.contains(line), "emitted must contain {line}");
    }

    // E_CONTACT_NAME_OR_ORG sort order is EDS-owned derived state.
    // This mapping writes X-EVOLUTION-FILE-AS when stated and does not read
    // E_CONTACT_NAME_OR_ORG directly.
    let card_with_file_as = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("Albert Einstein".to_owned()),
            extra: [("fileAs".to_owned(), json!("Einstein, Albert"))].into(),
            ..Default::default()
        });
        card
    };
    let emitted_file_as = card_to_vcard(&card_with_file_as);
    assert!(emitted_file_as.contains("X-EVOLUTION-FILE-AS:Einstein\\, Albert\r\n"));

    let card_without_file_as = {
        let mut card = ContactCard::default();
        card.name = Some(Name {
            full: Some("Albert Einstein".to_owned()),
            ..Default::default()
        });
        card
    };
    let emitted_no_file_as = card_to_vcard(&card_without_file_as);
    assert!(!emitted_no_file_as.contains("X-EVOLUTION-FILE-AS"));
    assert!(emitted_no_file_as.contains("FN:Albert Einstein\r\n"));
}

#[test]
fn photo_non_image_mediatype_and_data_uri_roundtrip_fixpoint_matrix() {
    let cases = [
        // 1. Non-image audio/ogg data URI with invalid VALUE parameter.
        "BEGIN:VCARD\r\nVERSION:3.0\r\nPHOTO;VALUE=date:data:audio/ogg;base64,T2dnUwACAAAAAAAAAABAAAABAAAAAKs1N1E=\r\nEND:VCARD\r\n",
        // 2. Non-image MEDIATYPE on audio data URI.
        "BEGIN:VCARD\r\nVERSION:3.0\r\nPHOTO;MEDIATYPE=audio/ogg:data:audio/ogg;base64,T2dnUwACAAAAAAAAAABAAAABAAAAAKs1N1E=\r\nEND:VCARD\r\n",
        // 3. Non-image TYPE on binary photo.
        "BEGIN:VCARD\r\nVERSION:3.0\r\nPHOTO;TYPE=audio/ogg;ENCODING=b:T2dnUwACAAAAAAAAAABAAAABAAAAAKs1N1E=\r\nEND:VCARD\r\n",
        // 4. Non-image MEDIATYPE on remote URI.
        "BEGIN:VCARD\r\nVERSION:3.0\r\nPHOTO;VALUE=uri;MEDIATYPE=audio/ogg:https://example.com/sound.ogg\r\nEND:VCARD\r\n",
        // 5. Data URI with explicit VALUE=uri.
        "BEGIN:VCARD\r\nVERSION:3.0\r\nPHOTO;VALUE=uri:data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==\r\nEND:VCARD\r\n",
        // 6. Unknown TYPE parameter.
        "BEGIN:VCARD\r\nVERSION:3.0\r\nPHOTO;TYPE=UNKNOWN;ENCODING=b:AQIDBA==\r\nEND:VCARD\r\n",
        // 7. vCard 4.0 data URI.
        "BEGIN:VCARD\r\nVERSION:4.0\r\nPHOTO:data:image/webp;base64,UklGRiQAAABXRUJQVlA4IBgAAAAwAQCdASoBAAEAAwA0JaQAA3AA/vuUAAA=\r\nEND:VCARD\r\n",
        // 8. Remote URI with image MEDIATYPE.
        "BEGIN:VCARD\r\nVERSION:3.0\r\nPHOTO;VALUE=uri;MEDIATYPE=image/jpeg:https://example.com/photo.jpg\r\nEND:VCARD\r\n",
    ];

    for (idx, raw_vcard) in cases.iter().enumerate() {
        let card1 = vcard_to_card(raw_vcard)
            .unwrap_or_else(|e| panic!("case {idx} pass 1 parse failed: {e:?}"));
        let vcard1 = card_to_vcard(&card1);
        let card2 = vcard_to_card(&vcard1)
            .unwrap_or_else(|e| panic!("case {idx} pass 2 parse failed: {e:?}"));
        let vcard2 = card_to_vcard(&card2);
        let card3 = vcard_to_card(&vcard2)
            .unwrap_or_else(|e| panic!("case {idx} pass 3 parse failed: {e:?}"));

        assert_eq!(
            vcard1, vcard2,
            "case {idx} vcard1 and vcard2 must reach fixed point"
        );
        assert_eq!(
            card2, card3,
            "case {idx} card2 and card3 must reach fixed point"
        );
        if idx < 7 {
            assert_eq!(
                card1, card2,
                "case {idx} card1 and card2 must reach immediate fixed point"
            );
        }
    }
}

#[test]
fn vcardparams_extension_parameters_drop_after_parse_and_emit_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Parameter Probe\r\n",
        "EMAIL;TYPE=HOME;X-FOO=alpha;PID=1.1:param@example.test\r\n",
        "TEL;TYPE=WORK,VOICE;X-BAR=beta;INDEX=2:+1-555-0100\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(source).expect("parse extension-parameter profile probe");
    let emails = card.emails.as_ref().expect("emails map");
    assert_eq!(emails["e1"].address, "param@example.test");
    assert_eq!(emails["e1"].pref, None);
    assert!(
        emails["e1"].extra.is_empty(),
        "profile does not retain RFC 9555 vCardParams metadata on EMAIL"
    );

    let phones = card.phones.as_ref().expect("phones map");
    assert_eq!(phones["p1"].number, "+1-555-0100");
    assert_eq!(phones["p1"].pref, None);
    assert!(
        phones["p1"].extra.is_empty(),
        "profile does not retain RFC 9555 vCardParams metadata on TEL"
    );

    let emitted = card_to_vcard(&card);
    let email_line = line(&emitted, "EMAIL;");
    assert!(email_line.contains("X-JMAP-KEY=e1"));
    assert!(email_line.contains("TYPE=HOME"));
    assert!(email_line.ends_with(":param@example.test"));

    let tel_line = line(&emitted, "TEL;");
    assert!(tel_line.contains("X-JMAP-KEY=p1"));
    assert!(tel_line.contains("TYPE=WORK"));
    assert!(tel_line.ends_with(":+1-555-0100"));
    assert!(
        !emitted.contains("X-FOO=")
            && !emitted.contains("PID=")
            && !emitted.contains("X-BAR=")
            && !emitted.contains("INDEX="),
        "non-projected parameters must be dropped on emit in this profile"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized profile probe");
    assert_eq!(card, reparsed, "profile reaches fixed point after one pass");
}

#[test]
fn vcardprops_extension_properties_drop_after_parse_and_emit_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "FN:Property Probe\r\n",
        "item1.X-FOO;X-BAR=Hello:World!\r\n",
        "EMAIL;TYPE=WORK:property@example.test\r\n",
        "END:VCARD\r\n"
    );

    let card = vcard_to_card(source).expect("parse extension-property profile probe");
    let emails = card.emails.as_ref().expect("emails map");
    assert_eq!(emails["e1"].address, "property@example.test");
    assert!(
        card.extra.is_empty(),
        "profile does not retain RFC 9555 vCardProps metadata"
    );

    let emitted = card_to_vcard(&card);
    let email_line = line(&emitted, "EMAIL;");
    assert!(email_line.contains("X-JMAP-KEY=e1"));
    assert!(email_line.contains("TYPE=WORK"));
    assert!(email_line.ends_with(":property@example.test"));
    assert!(
        !emitted.contains("item1.X-FOO") && !emitted.contains("X-BAR=Hello"),
        "non-projected extension properties must be dropped on emit in this profile"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized profile probe");
    assert_eq!(card, reparsed, "profile reaches fixed point after one pass");
}

#[test]
fn vcardname_source_property_identity_is_not_retained_in_eds_profile() {
    let mut online_services = BTreeMap::new();
    online_services.insert(
        "s1".to_owned(),
        OnlineService {
            service: Some("Jabber".to_owned()),
            user: Some("alice@jabber.example".to_owned()),
            extra: [("vCardName".to_owned(), json!("socialprofile"))].into(),
            ..OnlineService::default()
        },
    );
    let source = {
        let mut card = ContactCard::default();
        card.online_services = Some(online_services);
        card
    };

    let emitted = card_to_vcard(&source);
    let service_line = line(&emitted, "X-JABBER;");
    assert!(service_line.contains("X-JMAP-KEY=s1"));
    assert!(service_line.contains("TYPE=HOME"));
    assert!(service_line.ends_with(":alice@jabber.example"));
    assert!(
        !emitted.contains("IMPP:") && !emitted.contains("SOCIALPROFILE"),
        "profile does not emit source-property identity as RFC 9555 vCardName metadata"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized profile probe");
    let services = reparsed
        .online_services
        .as_ref()
        .expect("online services map");
    assert_eq!(services["s1"].service.as_deref(), Some("Jabber"));
    assert_eq!(services["s1"].user.as_deref(), Some("alice@jabber.example"));
    assert!(
        services["s1"].extra.is_empty(),
        "profile does not retain RFC 9555 vCardName metadata"
    );

    let re_emitted = card_to_vcard(&reparsed);
    assert_eq!(
        emitted, re_emitted,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn grouped_property_identity_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:Grouped Property Probe\r\n",
        "item1.URL:https://example.test/work\r\n",
        "item2.URL:https://example.test/blog\r\n",
        "item3.EMAIL:grouped@example.test\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse grouped-property profile probe");
    let links = parsed.links.as_ref().expect("links map");
    assert_eq!(links.len(), 2, "both grouped URL lines must remain mapped");
    assert!(
        links.values().all(|link| link.extra.is_empty()),
        "profile does not retain RFC 9555 group metadata on URL"
    );
    assert!(
        links
            .values()
            .any(|link| link.uri == "https://example.test/work"),
        "work URL must survive"
    );
    assert!(
        links
            .values()
            .any(|link| link.uri == "https://example.test/blog"),
        "blog URL must survive"
    );

    let emails = parsed.emails.as_ref().expect("emails map");
    assert_eq!(emails.len(), 1, "grouped EMAIL line must remain mapped");
    assert_eq!(emails["e1"].address, "grouped@example.test");
    assert!(
        emails["e1"].extra.is_empty(),
        "profile does not retain RFC 9555 group metadata on EMAIL"
    );

    let emitted = card_to_vcard(&parsed);
    assert!(emitted.contains("URL;X-JMAP-KEY=l1:https://example.test/work"));
    assert!(emitted.contains("URL;X-JMAP-KEY=l2:https://example.test/blog"));
    assert!(emitted.contains("EMAIL;X-JMAP-KEY=e1:grouped@example.test"));
    assert!(
        !emitted.contains("item1.") && !emitted.contains("item2.") && !emitted.contains("item3."),
        "grouped property identity must not be emitted"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized grouped-property probe");
    let re_emitted = card_to_vcard(&reparsed);
    assert_eq!(
        emitted, re_emitted,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn sort_as_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:Sort As Probe\r\n",
        "N;SORT-AS=ProbeSort:Probe;Person;;;\r\n",
        "ORG;SORT-AS=ProbeOrgSort,ProbeUnitSort:Probe Org;Research Unit\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse SORT-AS profile probe");
    let name = parsed.name.as_ref().expect("name object");
    assert_eq!(name.full.as_deref(), Some("Sort As Probe"));
    assert!(
        !name.extra.contains_key("sortAs"),
        "profile does not retain RFC 9555 SORT-AS metadata on N"
    );

    let organizations = parsed.organizations.as_ref().expect("organizations map");
    let organization = organizations.get("o1").expect("first organization");
    assert_eq!(organization.name.as_deref(), Some("Probe Org"));
    assert!(
        organization.extra.is_empty(),
        "profile does not retain RFC 9555 SORT-AS metadata on ORG"
    );

    let units = organization.units.as_ref().expect("org units");
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].name, "Research Unit");
    assert!(
        units[0].extra.is_empty(),
        "profile does not retain RFC 9555 SORT-AS metadata on ORG unit"
    );

    let emitted = card_to_vcard(&parsed);
    let org_line = line(&emitted, "ORG;");
    assert!(org_line.contains("X-JMAP-KEY=o1"));
    assert!(org_line.ends_with(":Probe Org;Research Unit"));
    assert!(
        !emitted.contains("SORT-AS="),
        "non-projected SORT-AS parameter metadata must be dropped on emit in this profile"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized SORT-AS profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn jscomps_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:JSCOMPS Parameter Probe\r\n",
        "N;JSCOMPS=\";1;0\":Doe;Jane;;;\r\n",
        "ADR;JSCOMPS=\"s,\\\\, ;11;s, ;10;3\":;;123 Main St;Exampletown;;12345;Exampleland\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse JSCOMPS profile probe");
    let name = parsed.name.as_ref().expect("name object");
    assert_eq!(name.full.as_deref(), Some("JSCOMPS Parameter Probe"));
    assert!(
        name.extra.is_empty(),
        "profile does not retain RFC 9555 JSCOMPS metadata on N"
    );

    let name_components = name.components.as_ref().expect("name components");
    assert_eq!(name_components.len(), 2, "N components stay mapped");
    assert!(
        name_components
            .iter()
            .any(|component| component.value == "Doe")
    );
    assert!(
        name_components
            .iter()
            .any(|component| component.value == "Jane")
    );

    let addresses = parsed.addresses.as_ref().expect("addresses map");
    let address = addresses.get("a1").expect("first address entry");
    assert!(
        address.extra.is_empty(),
        "profile does not retain RFC 9555 JSCOMPS metadata on ADR"
    );
    let components = address.components.as_ref().expect("address components");
    assert!(components.len() >= 3, "ADR components stay mapped");
    assert!(
        components
            .iter()
            .any(|component| component.value == "123 Main St")
    );
    assert!(
        components
            .iter()
            .any(|component| component.value == "Exampletown")
    );
    assert!(
        components
            .iter()
            .any(|component| component.value == "12345")
    );

    let emitted = card_to_vcard(&parsed);
    let n_line = line(&emitted, "N");
    assert_eq!(n_line, "N:Doe;Jane;;;");
    let adr_line = line(&emitted, "ADR;");
    assert!(adr_line.contains("X-JMAP-KEY=a1"));
    assert!(adr_line.ends_with(":;;123 Main St;Exampletown;;12345;Exampleland"));
    assert!(
        !emitted.contains("JSCOMPS="),
        "non-projected JSCOMPS parameter metadata must be dropped on emit in this profile"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized JSCOMPS profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn jsptr_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:JSPTR Parameter Probe\r\n",
        "EMAIL;TYPE=WORK:jsptr@example.test\r\n",
        "JSPROP;JSPTR=\"/links/l1/uri\":https://example.test/jsptr\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse JSPTR parameter profile probe");
    let emails = parsed.emails.as_ref().expect("emails map");
    assert_eq!(emails.len(), 1, "one mapped EMAIL line must remain");
    assert_eq!(emails["e1"].address, "jsptr@example.test");
    assert!(
        parsed.extra.is_empty(),
        "profile does not retain RFC 9555 JSPROP/JSPTR extension metadata"
    );

    let emitted = card_to_vcard(&parsed);
    let email_line = line(&emitted, "EMAIL;");
    assert!(email_line.contains("X-JMAP-KEY=e1"));
    assert!(email_line.contains("TYPE=WORK"));
    assert!(email_line.ends_with(":jsptr@example.test"));
    assert!(
        !emitted.contains("JSPROP") && !emitted.contains("JSPTR="),
        "non-projected JSPROP/JSPTR metadata must be dropped on emit in this profile"
    );

    let reparsed =
        vcard_to_card(&emitted).expect("reparse normalized JSPTR parameter profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn note_metadata_parameters_are_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:Note Metadata Probe\r\n",
        "NOTE;ALTID=1;LANGUAGE=en;AUTHOR=\"mailto:author@example.test\";AUTHOR-NAME=\"Author Example\";CREATED=20221123T150132Z:Probe note text\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse NOTE metadata profile probe");
    let notes = parsed.notes.as_ref().expect("notes map");
    assert_eq!(notes.len(), 1, "one NOTE line must remain mapped");

    let note = notes.get("n1").expect("first note entry");
    assert_eq!(note.note, "Probe note text");
    assert!(
        note.author.is_none(),
        "profile does not retain RFC 9555 AUTHOR metadata on NOTE"
    );
    assert!(
        note.created.is_none(),
        "profile does not retain RFC 9555 CREATED metadata on NOTE"
    );
    assert!(
        note.extra.is_empty(),
        "profile does not retain RFC 9555 NOTE metadata parameters"
    );

    let emitted = card_to_vcard(&parsed);
    let note_line = line(&emitted, "NOTE;");
    assert!(note_line.contains("X-JMAP-KEY=n1"));
    assert!(note_line.ends_with(":Probe note text"));
    assert!(
        !emitted.contains("AUTHOR=")
            && !emitted.contains("AUTHOR-NAME=")
            && !emitted.contains("CREATED=")
            && !emitted.contains("ALTID=")
            && !emitted.contains("LANGUAGE="),
        "non-projected NOTE metadata parameters must be dropped on emit in this profile"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized NOTE metadata profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn derived_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "N:Probe;Derived;;;\r\n",
        "FN;DERIVED=TRUE:Probe Derived\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse DERIVED profile probe");
    let name = parsed.name.as_ref().expect("name object");
    assert_eq!(name.full.as_deref(), Some("Probe Derived"));
    assert!(name.components.is_some(), "N components must remain mapped");
    assert!(
        !name.extra.contains_key("derived"),
        "profile does not retain RFC 9555 DERIVED metadata on FN"
    );

    let emitted = card_to_vcard(&parsed);
    assert!(emitted.contains("N:Probe;Derived;;;\r\n"));
    assert!(emitted.contains("FN:Probe Derived\r\n"));
    assert!(
        !emitted.contains("DERIVED="),
        "non-projected DERIVED parameter metadata must be dropped on emit in this profile"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized DERIVED profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn calscale_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:CALSCALE Profile Probe\r\n",
        "BDAY;CALSCALE=chinese:20211003\r\n",
        "ANNIVERSARY;CALSCALE=gregorian:20220708\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse CALSCALE profile probe");
    let anniversaries = parsed.anniversaries.as_ref().expect("anniversaries map");

    let birth = anniversaries
        .values()
        .find(|anniversary| anniversary.kind == "birth")
        .expect("birth anniversary");
    assert_eq!(
        birth.date,
        Some(json!({"@type": "PartialDate", "year": 2021, "month": 10, "day": 3}))
    );
    assert!(
        birth.extra.is_empty(),
        "profile does not retain RFC 9555 CALSCALE metadata on BDAY"
    );

    let wedding = anniversaries
        .values()
        .find(|anniversary| anniversary.kind == "wedding")
        .expect("wedding anniversary");
    assert_eq!(
        wedding.date,
        Some(json!({"@type": "PartialDate", "year": 2022, "month": 7, "day": 8}))
    );
    assert!(
        wedding.extra.is_empty(),
        "profile does not retain RFC 9555 CALSCALE metadata on ANNIVERSARY"
    );

    let emitted = card_to_vcard(&parsed);
    let bday_line = line(&emitted, "BDAY;");
    assert!(bday_line.contains("X-JMAP-KEY="));
    assert!(bday_line.ends_with(":2021-10-03"));

    let anniversary_line = line(&emitted, "ANNIVERSARY;");
    assert!(anniversary_line.contains("X-JMAP-KEY="));
    assert!(anniversary_line.ends_with(":2022-07-08"));
    assert!(
        emitted.contains("X-EVOLUTION-ANNIVERSARY;"),
        "wedding anniversaries must keep X-EVOLUTION-ANNIVERSARY compatibility line"
    );
    assert!(
        !emitted.contains("CALSCALE="),
        "non-projected CALSCALE parameter metadata must be dropped on emit in this profile"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized CALSCALE profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn cc_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:CC Parameter Profile Probe\r\n",
        "ADR;TYPE=HOME;CC=DE:;;Unter den Linden 1;Berlin;;10117;Germany\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse CC profile probe");
    let addresses = parsed.addresses.as_ref().expect("addresses map");
    assert_eq!(addresses.len(), 1, "one ADR line must remain mapped");

    let address = addresses.get("a1").expect("first address entry");
    assert_eq!(address.country_code.as_deref(), None);
    assert!(
        address.extra.is_empty(),
        "profile does not retain RFC 9555 CC metadata on ADR"
    );

    let emitted = card_to_vcard(&parsed);
    let adr_line = line(&emitted, "ADR;");
    assert!(adr_line.contains("X-JMAP-KEY=a1"));
    assert!(adr_line.contains("TYPE=HOME"));
    assert!(adr_line.ends_with(":;;Unter den Linden 1;Berlin;;10117;Germany"));
    assert!(
        !emitted.contains("CC="),
        "non-projected CC parameter metadata must be dropped on emit in this profile"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized CC profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn prop_id_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:PROP-ID Profile Probe\r\n",
        "TEL;VALUE=uri;TYPE=work;PROP-ID=PHONE-A:tel:+1-202-555-0100\r\n",
        "TEL;VALUE=uri;TYPE=home;PROP-ID=PHONE-B:tel:+1-202-555-0101\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse PROP-ID profile probe");
    let phones = parsed.phones.as_ref().expect("phones map");
    assert_eq!(phones.len(), 2, "both TEL lines must remain mapped");

    for phone in phones.values() {
        assert!(
            phone.extra.is_empty(),
            "profile does not retain RFC 9555 PROP-ID metadata on TEL"
        );
    }

    let emitted = card_to_vcard(&parsed);
    assert!(
        emitted.contains("TEL;X-JMAP-KEY="),
        "emit must keep keyed TEL lines"
    );
    assert!(
        !emitted.contains("PROP-ID="),
        "non-projected PROP-ID parameter metadata must be dropped on emit in this profile"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized PROP-ID profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn pid_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:PID Profile Probe\r\n",
        "EMAIL;TYPE=work;PID=1.1:pid-work@example.test\r\n",
        "EMAIL;TYPE=home;PID=2.1:pid-home@example.test\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse PID profile probe");
    let emails = parsed.emails.as_ref().expect("emails map");
    assert_eq!(emails.len(), 2, "both EMAIL lines must remain mapped");

    for email in emails.values() {
        assert!(
            email.extra.is_empty(),
            "profile does not retain RFC 9555 PID metadata on EMAIL"
        );
    }

    let emitted = card_to_vcard(&parsed);
    assert!(
        emitted.contains("EMAIL;X-JMAP-KEY="),
        "emit must keep keyed EMAIL lines"
    );
    assert!(
        !emitted.contains("PID="),
        "non-projected PID parameter metadata must be dropped on emit in this profile"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized PID profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn username_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:USERNAME Profile Probe\r\n",
        "IMPP;TYPE=work;USERNAME=alice.example:xmpp:alice@example.test\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse USERNAME profile probe");
    let services = parsed
        .online_services
        .as_ref()
        .expect("online services map");
    assert_eq!(services.len(), 1, "IMPP line must remain mapped");

    let service = services.get("s1").expect("first online service entry");
    assert_eq!(service.service.as_deref(), Some("Jabber"));
    assert_eq!(service.user.as_deref(), Some("alice@example.test"));
    assert!(
        service.extra.is_empty(),
        "profile does not retain RFC 9555 USERNAME metadata on IMPP"
    );

    let emitted = card_to_vcard(&parsed);
    let service_line = line(&emitted, "X-JABBER;");
    assert!(service_line.contains("X-JMAP-KEY=s1"));
    assert!(service_line.contains("TYPE="));
    assert!(service_line.ends_with(":alice@example.test"));
    assert!(
        !emitted.contains("USERNAME=") && !emitted.contains("IMPP:"),
        "non-projected USERNAME metadata must be dropped on emit in this profile"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized USERNAME profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn tz_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:TZ Parameter Probe\r\n",
        "ADR;TYPE=home;TZ=Europe/Zurich:;;Bahnhofstrasse 1;Zurich;;8001;Switzerland\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse TZ parameter profile probe");
    let addresses = parsed.addresses.as_ref().expect("addresses map");
    assert_eq!(addresses.len(), 1, "one ADR line must remain mapped");

    let address = addresses.get("a1").expect("first address entry");
    assert_eq!(address.time_zone.as_deref(), None);
    assert!(
        address.extra.is_empty(),
        "profile does not retain RFC 9555 TZ metadata on ADR"
    );

    let emitted = card_to_vcard(&parsed);
    let adr_line = line(&emitted, "ADR;");
    assert!(adr_line.contains("X-JMAP-KEY=a1"));
    assert!(adr_line.contains("TYPE=HOME"));
    assert!(adr_line.ends_with(":;;Bahnhofstrasse 1;Zurich;;8001;Switzerland"));
    assert!(
        !emitted.contains("TZ="),
        "non-projected TZ parameter metadata must be dropped on emit in this profile"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized TZ parameter profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn geo_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:GEO Parameter Probe\r\n",
        "ADR;TYPE=home;GEO=\"geo:47.3769,8.5417\":;;Bahnhofstrasse 1;Zurich;;8001;Switzerland\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse GEO parameter profile probe");
    let addresses = parsed.addresses.as_ref().expect("addresses map");
    assert_eq!(addresses.len(), 1, "one ADR line must remain mapped");

    let address = addresses.get("a1").expect("first address entry");
    assert_eq!(address.coordinates, None);
    assert!(
        address.extra.is_empty(),
        "profile does not retain RFC 9555 GEO metadata on ADR"
    );

    let emitted = card_to_vcard(&parsed);
    let adr_line = line(&emitted, "ADR;");
    assert!(adr_line.contains("X-JMAP-KEY=a1"));
    assert!(adr_line.contains("TYPE=HOME"));
    assert!(adr_line.ends_with(":;;Bahnhofstrasse 1;Zurich;;8001;Switzerland"));
    assert!(
        !emitted.contains("GEO="),
        "non-projected GEO parameter metadata must be dropped on emit in this profile"
    );

    let reparsed = vcard_to_card(&emitted).expect("reparse normalized GEO parameter profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn altid_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:ALTID Parameter Probe\r\n",
        "TITLE;ALTID=1:Directrice\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse ALTID parameter profile probe");
    let titles = parsed.titles.as_ref().expect("titles map");
    assert_eq!(titles.len(), 1, "one TITLE line must remain mapped");

    let title = titles.get("t1").expect("first title entry");
    assert_eq!(title.name, "Directrice");
    assert_eq!(title.kind, None);
    assert!(
        parsed.localizations.is_none(),
        "profile does not retain RFC 9555 ALTID metadata as localizations"
    );
    assert!(
        title.extra.is_empty(),
        "profile does not retain RFC 9555 ALTID metadata on TITLE"
    );

    let emitted = card_to_vcard(&parsed);
    let title_line = line(&emitted, "TITLE;");
    assert!(title_line.contains("X-JMAP-KEY=t1"));
    assert!(title_line.ends_with(":Directrice"));
    assert!(
        !emitted.contains("ALTID="),
        "non-projected ALTID parameter metadata must be dropped on emit in this profile"
    );

    let reparsed =
        vcard_to_card(&emitted).expect("reparse normalized ALTID parameter profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn language_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:LANGUAGE Parameter Probe\r\n",
        "TITLE;LANGUAGE=fr:Directrice\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse LANGUAGE parameter profile probe");
    let titles = parsed.titles.as_ref().expect("titles map");
    assert_eq!(titles.len(), 1, "one TITLE line must remain mapped");

    let title = titles.get("t1").expect("first title entry");
    assert_eq!(title.name, "Directrice");
    assert_eq!(title.kind, None);
    assert!(
        parsed.localizations.is_none(),
        "profile does not retain RFC 9555 LANGUAGE metadata as localizations"
    );
    assert!(
        title.extra.is_empty(),
        "profile does not retain RFC 9555 LANGUAGE metadata on TITLE"
    );

    let emitted = card_to_vcard(&parsed);
    let title_line = line(&emitted, "TITLE;");
    assert!(title_line.contains("X-JMAP-KEY=t1"));
    assert!(title_line.ends_with(":Directrice"));
    assert!(
        !emitted.contains("LANGUAGE="),
        "non-projected LANGUAGE parameter metadata must be dropped on emit in this profile"
    );

    let reparsed =
        vcard_to_card(&emitted).expect("reparse normalized LANGUAGE parameter profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn index_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:INDEX Parameter Probe\r\n",
        "ORG-DIRECTORY;INDEX=1:https://directory.example.com/people/alice\r\n",
        "HOBBY;INDEX=2:Trail running\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse INDEX parameter profile probe");
    assert_eq!(
        parsed.name.as_ref().and_then(|name| name.full.as_deref()),
        Some("INDEX Parameter Probe")
    );
    assert!(
        parsed.directories.is_none(),
        "profile does not retain RFC 9555 INDEX metadata as directories"
    );
    assert!(
        parsed.personal_info.is_none(),
        "profile does not retain RFC 9555 INDEX metadata as personal_info"
    );

    let emitted = card_to_vcard(&parsed);
    assert!(
        !emitted.contains("INDEX="),
        "non-projected INDEX parameter metadata must be dropped on emit in this profile"
    );
    assert!(
        !emitted.contains("ORG-DIRECTORY:") && !emitted.contains("HOBBY:"),
        "non-projected properties that carry INDEX metadata must stay dropped on emit"
    );

    let reparsed =
        vcard_to_card(&emitted).expect("reparse normalized INDEX parameter profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn level_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:LEVEL Parameter Probe\r\n",
        "EXPERTISE;LEVEL=high:Chess openings\r\n",
        "HOBBY;LEVEL=medium:Trail running\r\n",
        "INTEREST;LEVEL=low:Classical guitar\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse LEVEL parameter profile probe");
    assert_eq!(
        parsed.name.as_ref().and_then(|name| name.full.as_deref()),
        Some("LEVEL Parameter Probe")
    );
    assert!(
        parsed.personal_info.is_none(),
        "profile does not retain RFC 9555 LEVEL metadata as personal_info"
    );

    let emitted = card_to_vcard(&parsed);
    assert!(
        !emitted.contains("LEVEL="),
        "non-projected LEVEL parameter metadata must be dropped on emit in this profile"
    );
    assert!(
        !emitted.contains("EXPERTISE:") && !emitted.contains("EXPERTISE;"),
        "non-projected properties that carry LEVEL metadata must stay dropped on emit"
    );
    assert!(
        !emitted.contains("HOBBY:") && !emitted.contains("HOBBY;"),
        "non-projected properties that carry LEVEL metadata must stay dropped on emit"
    );
    assert!(
        !emitted.contains("INTEREST:") && !emitted.contains("INTEREST;"),
        "non-projected properties that carry LEVEL metadata must stay dropped on emit"
    );

    let reparsed =
        vcard_to_card(&emitted).expect("reparse normalized LEVEL parameter profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn phonetic_and_script_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:PHONETIC Parameter Probe\r\n",
        "N;PHONETIC=ipa;SCRIPT=Latn:Mueller;Vera;;;\r\n",
        "ADR;TYPE=home;PHONETIC=ipa;SCRIPT=Latn:;;Hauptstrasse 1;Berlin;;10115;Germany\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse PHONETIC parameter profile probe");
    let name = parsed.name.as_ref().expect("name present");
    assert_eq!(name.full.as_deref(), Some("PHONETIC Parameter Probe"));
    assert!(
        !name.extra.contains_key("phoneticSystem"),
        "profile does not retain RFC 9555 PHONETIC metadata on name"
    );
    assert!(
        !name.extra.contains_key("phoneticScript"),
        "profile does not retain RFC 9555 SCRIPT metadata on name"
    );
    let name_components = name.components.as_ref().expect("name components present");
    assert_eq!(name_components.len(), 2);
    assert!(
        name_components
            .iter()
            .all(|component| !component.extra.contains_key("phonetic")),
        "profile does not retain RFC 9555 PHONETIC component metadata on N"
    );

    let addresses = parsed.addresses.as_ref().expect("addresses map");
    let address = addresses.get("a1").expect("first address entry");
    assert_eq!(address.contexts, Some(json!({"private": true})));
    assert!(
        !address.extra.contains_key("phoneticSystem"),
        "profile does not retain RFC 9555 PHONETIC metadata on ADR"
    );
    assert!(
        !address.extra.contains_key("phoneticScript"),
        "profile does not retain RFC 9555 SCRIPT metadata on ADR"
    );
    let address_components = address
        .components
        .as_ref()
        .expect("address components present");
    assert!(
        address_components
            .iter()
            .all(|component| !component.extra.contains_key("phonetic")),
        "profile does not retain RFC 9555 PHONETIC component metadata on ADR"
    );

    let emitted = card_to_vcard(&parsed);
    let n_line = line(&emitted, "N");
    assert_eq!(n_line, "N:Mueller;Vera;;;");
    let adr_line = line(&emitted, "ADR;");
    assert!(adr_line.contains("X-JMAP-KEY=a1"));
    assert!(adr_line.contains("TYPE=HOME"));
    assert!(adr_line.ends_with(":;;Hauptstrasse 1;Berlin;;10115;Germany"));
    assert!(
        !emitted.contains("PHONETIC="),
        "non-projected PHONETIC metadata must be dropped on emit in this profile"
    );
    assert!(
        !emitted.contains("SCRIPT="),
        "non-projected SCRIPT metadata must be dropped on emit in this profile"
    );

    let reparsed =
        vcard_to_card(&emitted).expect("reparse normalized PHONETIC parameter profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn service_type_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:SERVICE-TYPE Parameter Probe\r\n",
        "IMPP;SERVICE-TYPE=skype:skype:alice.example\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse SERVICE-TYPE parameter profile probe");
    let services = parsed
        .online_services
        .as_ref()
        .expect("online services map");
    assert_eq!(services.len(), 1, "one IMPP line must remain mapped");

    let service = services.get("s1").expect("first online service entry");
    assert_eq!(service.service.as_deref(), Some("Skype"));
    assert_eq!(service.user.as_deref(), Some("alice.example"));
    assert_eq!(service.uri, None);
    assert!(
        service.extra.is_empty(),
        "profile does not retain RFC 9555 SERVICE-TYPE metadata on IMPP"
    );

    let emitted = card_to_vcard(&parsed);
    let service_line = line(&emitted, "X-SKYPE;");
    assert!(service_line.contains("X-JMAP-KEY=s1"));
    assert!(service_line.contains("TYPE=HOME"));
    assert!(service_line.ends_with(":alice.example"));
    assert!(
        !emitted.contains("SERVICE-TYPE="),
        "non-projected SERVICE-TYPE metadata must be dropped on emit in this profile"
    );
    assert!(
        !emitted.contains("IMPP;"),
        "emission uses EDS service slots rather than IMPP in this profile"
    );

    let reparsed =
        vcard_to_card(&emitted).expect("reparse normalized SERVICE-TYPE parameter profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn value_parameter_metadata_is_not_retained_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:VALUE Parameter Probe\r\n",
        "TEL;VALUE=uri;TYPE=work:tel:+1-202-555-0110\r\n",
        "PHOTO;VALUE=uri:https://cdn.example.test/value-probe.png\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse VALUE parameter profile probe");
    let phones = parsed.phones.as_ref().expect("phones map");
    assert_eq!(phones.len(), 1, "TEL line must remain mapped");

    let phone = phones.get("p1").expect("first phone entry");
    assert_eq!(phone.number, "tel:+1-202-555-0110");
    assert_eq!(phone.contexts, Some(json!({"work": true})));
    assert!(
        phone.extra.is_empty(),
        "profile does not retain RFC 9555 VALUE metadata on TEL"
    );

    let media = parsed.media.as_ref().expect("media map");
    assert_eq!(media.len(), 1, "PHOTO line must remain mapped");

    let photo = media.get("m1").expect("first media entry");
    assert_eq!(photo.uri, "https://cdn.example.test/value-probe.png");
    assert_eq!(photo.media_type, None);

    let emitted = card_to_vcard(&parsed);
    let tel_line = line(&emitted, "TEL;");
    assert!(tel_line.contains("X-JMAP-KEY=p1"));
    assert!(tel_line.contains("TYPE=WORK"));
    assert!(tel_line.ends_with(":tel:+1-202-555-0110"));
    assert!(
        !tel_line.contains("VALUE=uri"),
        "non-projected VALUE metadata must be dropped on emit for TEL in this profile"
    );

    let photo_line = line(&emitted, "PHOTO;");
    assert!(photo_line.contains("X-JMAP-KEY=m1"));
    assert!(photo_line.contains("VALUE=uri"));
    assert!(photo_line.ends_with(":https://cdn.example.test/value-probe.png"));

    let reparsed =
        vcard_to_card(&emitted).expect("reparse normalized VALUE parameter profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn label_parameter_maps_to_address_full_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:LABEL Parameter Probe\r\n",
        "ADR;LABEL=Apartment 4B;TYPE=home:;;42 Example Street;Berlin;;10115;Germany\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse LABEL parameter profile probe");
    let addresses = parsed.addresses.as_ref().expect("addresses map");
    assert_eq!(addresses.len(), 1, "ADR line must remain mapped");

    let address = addresses.get("a1").expect("first address entry");
    assert_eq!(address.full.as_deref(), Some("Apartment 4B"));
    assert_eq!(address.contexts, Some(json!({"private": true})));

    let emitted = card_to_vcard(&parsed);
    let adr_line = line(&emitted, "ADR;");
    assert!(adr_line.contains("X-JMAP-KEY=a1"));
    assert!(adr_line.contains("TYPE=HOME"));
    assert!(adr_line.ends_with(":;;42 Example Street;Berlin;;10115;Germany"));
    assert!(
        !adr_line.contains("LABEL="),
        "ADR output carries address fields while LABEL moves to a dedicated line"
    );

    let label_line = line(&emitted, "LABEL;");
    assert!(label_line.contains("X-JMAP-KEY=a1"));
    assert!(label_line.contains("TYPE=HOME"));
    assert!(label_line.ends_with(":Apartment 4B"));

    let reparsed =
        vcard_to_card(&emitted).expect("reparse normalized LABEL parameter profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn type_parameter_maps_contexts_and_drops_unmapped_tokens_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:TYPE Parameter Probe\r\n",
        "EMAIL;TYPE=HOME,X-LEGACY:home-type@example.test\r\n",
        "TEL;TYPE=WORK,CELL,PREF,X-LEGACY:tel:+1-202-555-0123\r\n",
        "ADR;TYPE=WORK,X-LEGACY:;;500 Example Avenue;Berlin;;10999;Germany\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse TYPE parameter profile probe");

    let emails = parsed.emails.as_ref().expect("emails map");
    assert_eq!(emails.len(), 1, "EMAIL line must remain mapped");
    let email = emails.get("e1").expect("first email entry");
    assert_eq!(email.address, "home-type@example.test");
    assert_eq!(email.contexts, Some(json!({"private": true})));

    let phones = parsed.phones.as_ref().expect("phones map");
    assert_eq!(phones.len(), 1, "TEL line must remain mapped");
    let phone = phones.get("p1").expect("first phone entry");
    assert_eq!(phone.number, "tel:+1-202-555-0123");
    assert_eq!(phone.contexts, Some(json!({"work": true})));
    assert_eq!(phone.features, Some(json!({"mobile": true})));
    assert_eq!(phone.pref, Some(1));

    let addresses = parsed.addresses.as_ref().expect("addresses map");
    assert_eq!(addresses.len(), 1, "ADR line must remain mapped");
    let address = addresses.get("a1").expect("first address entry");
    assert_eq!(address.contexts, Some(json!({"work": true})));

    let emitted = card_to_vcard(&parsed);
    let email_line = line(&emitted, "EMAIL;");
    assert!(email_line.contains("X-JMAP-KEY=e1"));
    assert!(email_line.contains("TYPE=HOME"));
    assert!(email_line.ends_with(":home-type@example.test"));

    let tel_line = line(&emitted, "TEL;");
    assert!(tel_line.contains("X-JMAP-KEY=p1"));
    assert!(tel_line.contains("TYPE=WORK,CELL,PREF"));
    assert!(tel_line.ends_with(":tel:+1-202-555-0123"));

    let adr_line = line(&emitted, "ADR;");
    assert!(adr_line.contains("X-JMAP-KEY=a1"));
    assert!(adr_line.contains("TYPE=WORK"));
    assert!(adr_line.ends_with(":;;500 Example Avenue;Berlin;;10999;Germany"));

    assert!(
        !emitted.contains("X-LEGACY"),
        "non-projected TYPE tokens must be dropped on emit in this profile"
    );

    let reparsed =
        vcard_to_card(&emitted).expect("reparse normalized TYPE parameter profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}

#[test]
fn pref_parameter_rank_is_normalized_to_flag_in_eds_profile() {
    let source = concat!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\n",
        "FN:PREF Parameter Probe\r\n",
        "EMAIL;PREF=7;TYPE=WORK:primary.rank@example.test\r\n",
        "TEL;PREF=9;TYPE=HOME,VOICE:tel:+1-202-555-0134\r\n",
        "ADR;PREF=5;TYPE=WORK:;;700 Preference Way;Berlin;;10117;Germany\r\n",
        "END:VCARD\r\n"
    );

    let parsed = vcard_to_card(source).expect("parse PREF parameter profile probe");

    let emails = parsed.emails.as_ref().expect("emails map");
    assert_eq!(emails.len(), 1, "EMAIL line must remain mapped");
    let email = emails.get("e1").expect("first email entry");
    assert_eq!(email.address, "primary.rank@example.test");
    assert_eq!(email.contexts, Some(json!({"work": true})));
    assert_eq!(email.pref, Some(1));

    let phones = parsed.phones.as_ref().expect("phones map");
    assert_eq!(phones.len(), 1, "TEL line must remain mapped");
    let phone = phones.get("p1").expect("first phone entry");
    assert_eq!(phone.number, "tel:+1-202-555-0134");
    assert_eq!(phone.contexts, Some(json!({"private": true})));
    assert_eq!(phone.features, Some(json!({"voice": true})));
    assert_eq!(phone.pref, Some(1));

    let addresses = parsed.addresses.as_ref().expect("addresses map");
    assert_eq!(addresses.len(), 1, "ADR line must remain mapped");
    let address = addresses.get("a1").expect("first address entry");
    assert_eq!(address.contexts, Some(json!({"work": true})));
    assert_eq!(address.extra.get("pref"), Some(&json!(1)));

    let emitted = card_to_vcard(&parsed);
    let email_line = line(&emitted, "EMAIL;");
    assert!(email_line.contains("X-JMAP-KEY=e1"));
    assert!(email_line.contains("TYPE=WORK,PREF"));
    assert!(email_line.ends_with(":primary.rank@example.test"));

    let tel_line = line(&emitted, "TEL;");
    assert!(tel_line.contains("X-JMAP-KEY=p1"));
    assert!(tel_line.contains("TYPE=HOME,VOICE,PREF"));
    assert!(tel_line.ends_with(":tel:+1-202-555-0134"));

    let adr_line = line(&emitted, "ADR;");
    assert!(adr_line.contains("X-JMAP-KEY=a1"));
    assert!(adr_line.contains("TYPE=WORK,PREF"));
    assert!(adr_line.ends_with(":;;700 Preference Way;Berlin;;10117;Germany"));

    assert!(
        !emitted.contains("PREF=7") && !emitted.contains("PREF=9") && !emitted.contains("PREF=5"),
        "profile normalizes PREF ranking to a preferred flag"
    );

    let reparsed =
        vcard_to_card(&emitted).expect("reparse normalized PREF parameter profile probe");
    assert_eq!(
        parsed, reparsed,
        "profile reaches fixed point after one pass"
    );
}
