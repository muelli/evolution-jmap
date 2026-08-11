// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The write side. The theme throughout is that saving a vCard must not
//! destroy what the vCard could not carry: the mapping drops most of a
//! JSContact card, so a save that replaced properties wholesale would delete
//! data the user never touched and cannot even see.

mod common;

use common::Fixture;
use serde_json::json;

/// The vCard Evolution hands to `save_contact_sync` for a brand new contact:
/// the `UID` is a name the local cache invented, not a server identifier.
const NEW_CONTACT: &str = "BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
UID:pas-id-68A2F1C400000000\r\n\
FN:Vera Oldenburg\r\n\
N:Oldenburg;Vera;;;\r\n\
EMAIL;TYPE=WORK:vera@example.com\r\n\
END:VCARD\r\n";

#[test]
fn saving_a_new_contact_files_it_in_this_book_under_a_server_identifier() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let saved = sync.save_contact(NEW_CONTACT, None).unwrap();

    assert_ne!(
        saved.uid, "pas-id-68A2F1C400000000",
        "the locally invented UID must not be sent as the JMAP id"
    );
    let stored = fixture.card(&saved.uid.as_str().into());
    assert_eq!(
        stored.name.as_ref().unwrap().full.as_deref(),
        Some("Vera Oldenburg")
    );
    assert!(
        stored
            .address_book_ids
            .as_ref()
            .unwrap()
            .contains_key(&fixture.ours),
        "filed in the book being synced"
    );
    // The listing agrees with what save reported.
    let (_, contacts) = sync.list_existing().unwrap();
    assert_eq!(contacts, vec![saved]);
}

#[test]
fn editing_a_contact_leaves_unmapped_properties_alone() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // Properties no vCard we produce can carry.
    fixture.patch(
        &id,
        json!({
            "nicknames": {"k1": {"name": "Vee"}},
            "notes": {"n1": {"note": "met at FOSDEM"}},
            "emails/e0/label": "day job",
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace("vera@example.com", "vera@example.org");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.card(&id);
    assert_eq!(
        stored.extra.get("nicknames"),
        Some(&json!({"k1": {"name": "Vee"}})),
        "an unmapped property was overwritten"
    );
    assert_eq!(
        stored.extra.get("notes"),
        Some(&json!({"n1": {"note": "met at FOSDEM"}}))
    );
    let emails = stored.emails.as_ref().unwrap();
    assert_eq!(emails.len(), 1, "patched in place, not re-added");
    assert_eq!(emails["e0"].address, "vera@example.org");
    assert_eq!(
        emails["e0"].extra.get("label"),
        Some(&json!("day job")),
        "an unmapped property of a mapped entry was overwritten"
    );
}

#[test]
fn editing_preserves_contexts_the_vcard_cannot_express() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // "school" has no vCard TYPE, so it survives the round trip only if the
    // patch merges rather than replaces.
    fixture.patch(
        &id,
        json!({"emails/e0/contexts": {"work": true, "school": true}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(vcard.contains("TYPE=WORK"), "{vcard}");
    // The user reclassifies the address as private.
    let edited = vcard.replace("TYPE=WORK", "TYPE=HOME");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let contexts = fixture.card(&id).emails.as_ref().unwrap()["e0"]
        .contexts
        .clone()
        .unwrap();
    assert_eq!(contexts, json!({"private": true, "school": true}));
}

#[test]
fn editing_preserves_a_preference_ranking_the_vcard_flattens() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // vCard 3.0 has only a PREF flag, so a rank of 30 comes back as "PREF".
    fixture.patch(&id, json!({"emails/e0/pref": 30}));
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(vcard.contains("PREF"), "{vcard}");
    let edited = vcard.replace("vera@example.com", "vera@example.org");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.card(&id).emails.as_ref().unwrap()["e0"].pref,
        Some(30),
        "the rank must not be flattened to 1 by a save that did not touch it"
    );
}

#[test]
fn clearing_a_preference_in_the_vcard_clears_it_on_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(&id, json!({"emails/e0/pref": 30}));
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace(";TYPE=PREF", "").replace("TYPE=PREF:", "");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.card(&id).emails.as_ref().unwrap()["e0"].pref, None);
}

#[test]
fn removing_and_adding_entries_survives_the_round_trip() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"phones": {"p0": {"number": "+49 30 111", "features": {"voice": true}}}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    // Drop the phone, add a second address.
    let edited = vcard
        .lines()
        .filter(|line| !line.starts_with("TEL"))
        .map(|line| {
            if line.starts_with("END:VCARD") {
                "EMAIL:vera@example.org\r\nEND:VCARD".to_owned()
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\r\n");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.card(&id);
    assert!(stored.phones.is_none(), "{:?}", stored.phones);
    let addresses: Vec<&str> = stored
        .emails
        .as_ref()
        .unwrap()
        .values()
        .map(|email| email.address.as_str())
        .collect();
    assert_eq!(addresses, vec!["vera@example.com", "vera@example.org"]);
}

#[test]
fn editing_the_structured_name_replaces_only_the_mapped_components() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"name/components": [
            {"kind": "surname", "value": "Oldenburg"},
            {"kind": "given", "value": "Vera"},
            {"kind": "generation", "value": "III"},
        ]}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(vcard.contains("N:Oldenburg;Vera;;;"), "{vcard}");
    let edited = vcard.replace("N:Oldenburg;Vera;;;", "N:Oldenburg-Meier;Vera;;Dr.;");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let components = fixture.card(&id).name.unwrap().components.unwrap();
    let by_kind: Vec<(&str, &str)> = components
        .iter()
        .map(|c| (c.kind.as_str(), c.value.as_str()))
        .collect();
    assert!(
        by_kind.contains(&("generation", "III")),
        "a component kind the vCard cannot carry was dropped: {by_kind:?}"
    );
    // Carrying the unmapped kinds across must not also carry the mapped ones
    // it is meant to be replacing, or the card ends up with two surnames.
    assert_eq!(
        by_kind,
        vec![
            ("title", "Dr."),
            ("given", "Vera"),
            ("surname", "Oldenburg-Meier"),
            ("generation", "III"),
        ]
    );
}

#[test]
fn renaming_an_employer_keeps_what_the_org_line_cannot_carry() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // `sortAs` and `contexts` have no ORG component and no ORG parameter, so
    // they survive only if the patch reaches into the entry.
    fixture.patch(
        &id,
        json!({"organizations": {"o1": {
            "name": "Acme",
            "sortAs": "Acme",
            "contexts": {"work": true},
            "units": [
                {"@type": "OrgUnit", "name": "Research", "sortAs": "Res"},
                {"@type": "OrgUnit", "name": "Optics"},
            ],
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("ORG;X-JMAP-KEY=o1:Acme;Research;Optics"),
        "{vcard}"
    );
    // The user renames the employer and dissolves the second department.
    let edited = vcard.replace("Acme;Research;Optics", "Acme Ltd;Research");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.card(&id);
    let organizations = stored.organizations.as_ref().expect("organizations");
    assert_eq!(organizations.len(), 1, "patched in place, not re-added");
    let organization = &organizations["o1"];
    assert_eq!(organization.name.as_deref(), Some("Acme Ltd"));
    assert_eq!(
        organization.extra.get("sortAs"),
        Some(&json!("Acme")),
        "a member the ORG line cannot carry was overwritten"
    );
    assert_eq!(
        organization.extra.get("contexts"),
        Some(&json!({"work": true}))
    );
    let units = organization.units.as_ref().expect("units");
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].name, "Research");
    assert_eq!(
        units[0].extra.get("sortAs"),
        Some(&json!("Res")),
        "a unit that kept its name kept nothing else"
    );
}

#[test]
fn removing_the_org_line_removes_the_organization() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(&id, json!({"organizations": {"o1": {"name": "Acme"}}}));
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited: String = vcard
        .lines()
        .filter(|line| !line.starts_with("ORG"))
        .map(|line| format!("{line}\r\n"))
        .collect();
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.card(&id).organizations, None);
}

#[test]
fn changing_a_job_title_keeps_which_organisation_it_is_held_at() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // A TITLE line is plain text: it has no room for the organisation the
    // title is held at, so that survives only if the patch reaches in.
    fixture.patch(
        &id,
        json!({"titles": {"t1": {
            "@type": "Title",
            "name": "Research Scientist",
            "organizationId": "o1",
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("TITLE;X-JMAP-KEY=t1:Research Scientist"),
        "{vcard}"
    );
    let edited = vcard.replace("Research Scientist", "Principal Scientist");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let titles = fixture.card(&id).titles.expect("titles");
    assert_eq!(titles.len(), 1, "patched in place, not re-added");
    assert_eq!(titles["t1"].name, "Principal Scientist");
    assert_eq!(
        titles["t1"].extra.get("organizationId"),
        Some(&json!("o1")),
        "a member the TITLE line cannot carry was overwritten"
    );
}

#[test]
fn a_title_of_a_vendor_kind_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // vCard 3.0 has TITLE and ROLE and nothing else, so an entry of a kind
    // outside RFC 9553 §2.2.4's two is dropped on the way out — and must not
    // then be deleted by a save that never showed it.
    fixture.patch(
        &id,
        json!({"titles": {"t1": {
            "@type": "Title",
            "name": "Knight of the Realm",
            "kind": "x-honour",
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("Knight"), "{vcard}");
    // The user types a job title into a contact that appears to have none.
    // The reader counts only the entries it can see, so the key it invents
    // for this one is `t1` — the key the hidden entry already holds.
    let edited = vcard.replace("END:VCARD\r\n", "TITLE:Research Scientist\r\nEND:VCARD\r\n");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let titles = fixture.card(&id).titles.expect("titles");
    assert_eq!(
        titles["t1"].name, "Knight of the Realm",
        "an entry the vCard never showed was overwritten: {titles:?}"
    );
    assert_eq!(titles["t1"].kind.as_deref(), Some("x-honour"));
    assert!(
        titles
            .values()
            .any(|title| title.name == "Research Scientist"),
        "the title the user typed was not saved: {titles:?}"
    );
    assert_eq!(titles.len(), 2);
}

#[test]
fn removing_the_title_line_removes_the_title() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"titles": {"t1": {"name": "Research Scientist"}}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited: String = vcard
        .lines()
        .filter(|line| !line.starts_with("TITLE"))
        .map(|line| format!("{line}\r\n"))
        .collect();
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.card(&id).titles, None);
}

#[test]
fn editing_an_address_keeps_what_the_adr_line_cannot_carry() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // Three kinds of thing an ADR line has no room for: members of the
    // address itself, a component of a kind vCard has no field for, and a
    // member of a component that does have one.
    fixture.patch(
        &id,
        json!({"addresses": {"a1": {
            "@type": "Address",
            "contexts": {"work": true},
            "components": [
                {"@type": "AddressComponent", "kind": "name", "value": "Hauptstraße",
                 "phonetic": "howptshtrahse"},
                {"@type": "AddressComponent", "kind": "number", "value": "1"},
                {"@type": "AddressComponent", "kind": "locality", "value": "Berlin"},
            ],
            "full": "Hauptstraße 1\n10115 Berlin",
            "coordinates": "geo:52.5,13.4",
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("ADR;X-JMAP-KEY=a1;TYPE=WORK:;;Hauptstraße;Berlin;;;"),
        "{vcard}"
    );

    // Saving it back untouched must be a no-op, or every open of a contact
    // rewrites its address: the invisible parts have to come back in the
    // order and shape they went out in, not merely survive.
    let (state_before, _) = sync.list_existing().unwrap();
    sync.save_contact(&vcard, Some(id.as_str())).unwrap();
    assert_eq!(
        sync.list_existing().unwrap().0,
        state_before,
        "a save that changed nothing rewrote the address"
    );

    // The user moves the contact to Potsdam.
    let edited = vcard.replace("Berlin", "Potsdam");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.card(&id);
    let addresses = stored.addresses.as_ref().expect("addresses");
    assert_eq!(addresses.len(), 1, "patched in place, not re-added");
    let address = &addresses["a1"];
    assert_eq!(
        address.extra.get("full"),
        Some(&json!("Hauptstraße 1\n10115 Berlin")),
        "a member the ADR line cannot carry was overwritten"
    );
    assert_eq!(
        address.extra.get("coordinates"),
        Some(&json!("geo:52.5,13.4"))
    );
    let components = address.components.as_ref().expect("components");
    let by_kind: Vec<(&str, &str)> = components
        .iter()
        .map(|component| (component.kind.as_str(), component.value.as_str()))
        .collect();
    assert_eq!(
        by_kind,
        vec![
            ("name", "Hauptstraße"),
            ("number", "1"),
            ("locality", "Potsdam"),
        ],
        "a component kind the ADR value has no field for was dropped"
    );
    assert_eq!(
        components[0].extra.get("phonetic"),
        Some(&json!("howptshtrahse")),
        "a component that kept its value kept nothing else"
    );
}

#[test]
fn an_address_the_vcard_cannot_state_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // An address stated only in components vCard has no field for gets no
    // ADR line, so it never reaches the user — and must not then be deleted
    // by a save, nor have its key taken by the address the user types.
    fixture.patch(
        &id,
        json!({"addresses": {"a1": {
            "@type": "Address",
            "components": [{"@type": "AddressComponent", "kind": "floor", "value": "3"}],
            "full": "third floor",
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("\r\nADR"), "{vcard}");
    // The reader counts only the entries it can see, so the key it invents
    // for this one is `a1` — the key the hidden entry already holds.
    let edited = vcard.replace(
        "END:VCARD\r\n",
        "ADR;TYPE=HOME:;;Hauptstraße 1;Berlin;;10115;Germany\r\nEND:VCARD\r\n",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let addresses = fixture.card(&id).addresses.expect("addresses");
    assert_eq!(
        addresses["a1"].extra.get("full"),
        Some(&json!("third floor")),
        "an entry the vCard never showed was overwritten: {addresses:?}"
    );
    assert!(
        addresses.values().any(|address| {
            address
                .components
                .iter()
                .flatten()
                .any(|component| component.value == "Hauptstraße 1")
        }),
        "the address the user typed was not saved: {addresses:?}"
    );
    assert_eq!(addresses.len(), 2);
}

#[test]
fn removing_the_adr_line_removes_the_address() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"addresses": {"a1": {"components": [{"kind": "locality", "value": "Berlin"}]}}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited: String = vcard
        .lines()
        .filter(|line| !line.starts_with("ADR"))
        .map(|line| format!("{line}\r\n"))
        .collect();
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.card(&id).addresses, None);
}

#[test]
fn a_save_that_changes_nothing_sends_no_patch() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();

    let before = sync.load_contact(id.as_str()).unwrap();
    let (state_before, _) = sync.list_existing().unwrap();
    let after = sync.save_contact(&before.vcard, Some(id.as_str())).unwrap();

    assert_eq!(after, before);
    let (state_after, _) = sync.list_existing().unwrap();
    assert_eq!(
        state_after, state_before,
        "a no-op save must not bump the server state and wake every other client"
    );
}

#[test]
fn saving_over_an_unknown_identifier_is_not_found() {
    let fixture = Fixture::start();
    let error = fixture
        .sync()
        .save_contact(NEW_CONTACT, Some("no-such-card"))
        .unwrap_err();

    assert!(
        matches!(&error, jmap_book_sync::SyncError::NotFound(uid) if uid == "no-such-card"),
        "{error:?}"
    );
}

#[test]
fn saving_something_that_is_not_a_vcard_fails_before_any_request() {
    let fixture = Fixture::start();
    let error = fixture
        .sync()
        .save_contact("not a vCard", None)
        .unwrap_err();

    assert!(
        matches!(error, jmap_book_sync::SyncError::VCard(_)),
        "{error:?}"
    );
    assert!(fixture.sync().list_existing().unwrap().1.is_empty());
}
