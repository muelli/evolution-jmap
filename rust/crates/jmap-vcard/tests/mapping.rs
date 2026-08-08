// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSContact `ContactCard` ↔ vCard 3.0, the minimal property set the
//! address book backend needs: UID, FN, N, EMAIL, TEL.

use jmap_proto::contacts::{ContactCard, ContactEmail, ContactPhone, Name, NameComponent};
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
fn unmodeled_jscontact_properties_are_dropped_not_mangled() {
    // `organizations` has no place in the minimal vCard set. Dropping it is
    // safe only because saving goes through a PatchObject that touches the
    // mapped properties alone — this test pins that expectation down.
    let vcard = card_to_vcard(&fixture_card());
    assert!(!vcard.contains("Example GmbH"), "{vcard}");
    assert!(!vcard.contains("organizations"), "{vcard}");
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
