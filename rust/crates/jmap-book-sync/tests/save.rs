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
            "keywords": {"hiking": true},
            "anniversaries": {"y1": {"kind": "birth", "date": {"year": 1964}}},
            "emails/e0/label": "day job",
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace("vera@example.com", "vera@example.org");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.card(&id);
    assert_eq!(
        stored.extra.get("keywords"),
        Some(&json!({"hiking": true})),
        "an unmapped property was overwritten"
    );
    let anniversaries = stored.anniversaries.as_ref().expect("anniversaries");
    assert_eq!(
        anniversaries["y1"].date,
        Some(json!({"year": 1964})),
        "a date the vCard could not state was overwritten"
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
        vcard.contains("ADR;X-JMAP-KEY=a1;TYPE=WORK:;;Hauptstraße 1;Berlin;;;"),
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
    // The written-out address does have a line of its own — LABEL — so the
    // town changes there too: the replacement above edited both lines,
    // which is what a user retyping an address in Evolution does.
    assert_eq!(
        address.full.as_deref(),
        Some("Hauptstraße 1\n10115 Potsdam")
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
        "a street the ADR line stated in one field came back as one component"
    );
    assert_eq!(
        components[0].extra.get("phonetic"),
        Some(&json!("howptshtrahse")),
        "a component that kept its value kept nothing else"
    );
}

#[test]
fn retyping_a_street_replaces_the_parts_the_server_stated_it_in() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // The street name and the house number share one ADR field, so a user who
    // retypes that field has edited both at once. Nothing in the text says
    // where the number ends, so the parts cannot be recovered — what must not
    // happen is the old number staying behind next to the new street.
    fixture.patch(
        &id,
        json!({"addresses": {"a1": {
            "@type": "Address",
            "components": [
                {"@type": "AddressComponent", "kind": "name", "value": "Hauptstraße"},
                {"@type": "AddressComponent", "kind": "number", "value": "1"},
                {"@type": "AddressComponent", "kind": "floor", "value": "3"},
            ],
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace("Hauptstraße 1", "Nebenstraße 2");
    assert_ne!(edited, vcard, "the street was not on the line: {vcard}");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let addresses = fixture.card(&id).addresses.expect("addresses");
    let components: Vec<(&str, &str)> = addresses["a1"]
        .components
        .iter()
        .flatten()
        .map(|component| (component.kind.as_str(), component.value.as_str()))
        .collect();
    assert_eq!(
        components,
        vec![("floor", "3"), ("name", "Nebenstraße 2")],
        "the street the user retyped did not replace the parts it was built \
         from: {addresses:?}"
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
            "coordinates": "geo:52.5,13.4",
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("\r\nADR"), "{vcard}");
    // Nor a LABEL: the entry says nothing a written-out address could be
    // built from either, so there is no line of any kind to show it on.
    assert!(!vcard.contains("\r\nLABEL"), "{vcard}");
    // The reader counts only the entries it can see, so the key it invents
    // for this one is `a1` — the key the hidden entry already holds.
    let edited = vcard.replace(
        "END:VCARD\r\n",
        "ADR;TYPE=HOME:;;Hauptstraße 1;Berlin;;10115;Germany\r\nEND:VCARD\r\n",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let addresses = fixture.card(&id).addresses.expect("addresses");
    assert_eq!(
        addresses["a1"].extra.get("coordinates"),
        Some(&json!("geo:52.5,13.4")),
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
fn an_address_stated_only_as_a_label_is_patched_in_place() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // RFC 9553 §2.5.1 allows an address whose components are simply not
    // known, written out in `full` and nowhere else. It gets a LABEL line
    // and no ADR, so the key it is filed under crosses on the LABEL — and if
    // it did not, this save would re-add the address instead of editing it,
    // losing the members no line can carry.
    fixture.patch(
        &id,
        json!({"addresses": {"a1": {
            "@type": "Address",
            "contexts": {"private": true},
            "full": "Postfach 42\n10115 Berlin",
            "coordinates": "geo:52.5,13.4",
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("\r\nADR"), "{vcard}");
    assert!(
        vcard.contains("LABEL;X-JMAP-KEY=a1;TYPE=HOME:Postfach 42\\n10115 Berlin"),
        "{vcard}"
    );

    let (state_before, _) = sync.list_existing().unwrap();
    sync.save_contact(&vcard, Some(id.as_str())).unwrap();
    assert_eq!(
        sync.list_existing().unwrap().0,
        state_before,
        "a save that changed nothing rewrote the address"
    );

    let edited = vcard.replace("Postfach 42", "Postfach 43");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let addresses = fixture.card(&id).addresses.expect("addresses");
    assert_eq!(addresses.len(), 1, "patched in place, not re-added");
    assert_eq!(
        addresses["a1"].full.as_deref(),
        Some("Postfach 43\n10115 Berlin")
    );
    assert_eq!(
        addresses["a1"].extra.get("coordinates"),
        Some(&json!("geo:52.5,13.4")),
        "a member no line can carry was overwritten"
    );
}

#[test]
fn removing_the_label_line_clears_the_written_out_address() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"addresses": {"a1": {
            "@type": "Address",
            "components": [{"@type": "AddressComponent", "kind": "locality", "value": "Berlin"}],
            "full": "10115 Berlin",
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited: String = vcard
        .lines()
        .filter(|line| !line.starts_with("LABEL"))
        .map(|line| format!("{line}\r\n"))
        .collect();
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    // The address itself stays — the user cleared the written-out form, not
    // the address it was a form of.
    let addresses = fixture.card(&id).addresses.expect("addresses");
    assert_eq!(addresses["a1"].full, None);
    assert_eq!(
        addresses["a1"]
            .components
            .as_ref()
            .map(|components| components.len()),
        Some(1)
    );
}

#[test]
fn editing_a_note_keeps_when_it_was_written_and_by_whom() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // RFC 9553 §2.8.1 hangs a timestamp and an author off a note, and a
    // `NOTE` line (RFC 2426 §3.6.2) is plain text with nowhere to put
    // either. They survive only if the patch reaches in.
    fixture.patch(
        &id,
        json!({"notes": {"n1": {
            "@type": "Note",
            "note": "met at FOSDEM",
            "created": "2026-02-01T09:15:00Z",
            "author": {"@type": "Author", "name": "Vera Oldenburg"},
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("NOTE;X-JMAP-KEY=n1:met at FOSDEM"),
        "{vcard}"
    );

    let edited = vcard.replace("met at FOSDEM", "met at FOSDEM and owes me a beer");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let notes = fixture.card(&id).notes.expect("notes");
    assert_eq!(notes.len(), 1, "patched in place, not re-added");
    assert_eq!(notes["n1"].note, "met at FOSDEM and owes me a beer");
    assert_eq!(
        notes["n1"].extra.get("created"),
        Some(&json!("2026-02-01T09:15:00Z")),
        "a member the NOTE line cannot carry was overwritten"
    );
    assert_eq!(
        notes["n1"].extra.get("author"),
        Some(&json!({"@type": "Author", "name": "Vera Oldenburg"}))
    );
}

#[test]
fn a_note_with_no_text_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // A note that says nothing gets no NOTE line, so it never reaches the
    // user — and must not then be deleted by a save, nor have its key taken
    // by the note the user types.
    fixture.patch(
        &id,
        json!({"notes": {"n1": {
            "@type": "Note",
            "note": "",
            "created": "2026-02-01T09:15:00Z",
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("\r\nNOTE"), "{vcard}");
    // The reader counts only the entries it can see, so the key it invents
    // for this one is `n1` — the key the hidden entry already holds.
    let edited = vcard.replace("END:VCARD\r\n", "NOTE:owes me a beer\r\nEND:VCARD\r\n");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let notes = fixture.card(&id).notes.expect("notes");
    assert_eq!(
        notes["n1"].extra.get("created"),
        Some(&json!("2026-02-01T09:15:00Z")),
        "an entry the vCard never showed was overwritten: {notes:?}"
    );
    assert!(
        notes.values().any(|note| note.note == "owes me a beer"),
        "the note the user typed was not saved: {notes:?}"
    );
    assert_eq!(notes.len(), 2);
}

#[test]
fn removing_the_note_line_removes_the_note() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(&id, json!({"notes": {"n1": {"note": "met at FOSDEM"}}}));
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited: String = vcard
        .lines()
        .filter(|line| !line.starts_with("NOTE"))
        .map(|line| format!("{line}\r\n"))
        .collect();
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.card(&id).notes, None);
}

#[test]
fn editing_a_nickname_keeps_the_context_and_ranking_the_line_cannot_carry() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // RFC 9553 §2.2.2 hangs a `contexts` and a `pref` off a nickname, and RFC
    // 2426 §3.1.3's NICKNAME takes no TYPE and no ranking, so neither reaches
    // the vCard. They survive only if the patch reaches in.
    fixture.patch(
        &id,
        json!({"nicknames": {"k1": {
            "@type": "Nickname",
            "name": "Vee",
            "contexts": {"private": true},
            "pref": 1,
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(vcard.contains("NICKNAME;X-JMAP-KEY=k1:Vee"), "{vcard}");

    let edited = vcard.replace(
        "NICKNAME;X-JMAP-KEY=k1:Vee",
        "NICKNAME;X-JMAP-KEY=k1:Vee-Vee",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let nicknames = fixture.card(&id).nicknames.expect("nicknames");
    assert_eq!(nicknames.len(), 1, "patched in place, not re-added");
    assert_eq!(nicknames["k1"].name, "Vee-Vee");
    assert_eq!(
        nicknames["k1"].extra.get("contexts"),
        Some(&json!({"private": true})),
        "a member the NICKNAME line cannot carry was overwritten"
    );
    assert_eq!(nicknames["k1"].extra.get("pref"), Some(&json!(1)));
}

#[test]
fn a_nickname_with_no_name_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // An entry that names nothing gets no NICKNAME line, so it never reaches
    // the user — and must not then be deleted by a save, nor have its key
    // taken by the nickname the user types.
    fixture.patch(
        &id,
        json!({"nicknames": {"k1": {"@type": "Nickname", "name": "", "pref": 1}}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("\r\nNICKNAME"), "{vcard}");
    // The reader counts only the entries it can see, so the key it invents
    // for this one is `k1` — the key the hidden entry already holds.
    let edited = vcard.replace("END:VCARD\r\n", "NICKNAME:Vee\r\nEND:VCARD\r\n");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let nicknames = fixture.card(&id).nicknames.expect("nicknames");
    assert_eq!(
        nicknames["k1"].extra.get("pref"),
        Some(&json!(1)),
        "an entry the vCard never showed was overwritten: {nicknames:?}"
    );
    assert!(
        nicknames.values().any(|nickname| nickname.name == "Vee"),
        "the nickname the user typed was not saved: {nicknames:?}"
    );
    assert_eq!(nicknames.len(), 2);
}

#[test]
fn removing_the_nickname_line_removes_the_nickname() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(&id, json!({"nicknames": {"k1": {"name": "Vee"}}}));
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited: String = vcard
        .lines()
        .filter(|line| !line.starts_with("NICKNAME"))
        .map(|line| format!("{line}\r\n"))
        .collect();
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.card(&id).nicknames, None);
}

#[test]
fn editing_a_home_page_keeps_what_the_url_line_cannot_carry() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // RFC 9553 §2.6.3 hangs a `mediaType`, a `pref` and a `label` off a link,
    // and RFC 2426 §3.6.8's URL is a bare URI, so none of them reaches the
    // vCard. They survive only if the patch reaches in.
    fixture.patch(
        &id,
        json!({"links": {"l1": {
            "@type": "Link",
            "uri": "https://vera.example/",
            "mediaType": "text/html",
            "pref": 1,
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("URL;X-JMAP-KEY=l1:https://vera.example/"),
        "{vcard}"
    );

    // EDS rewrites the value of that line in place and leaves the parameters
    // where they were, so the key comes back — verified against
    // libebook-contacts 3.52, where a set on `E_CONTACT_HOMEPAGE_URL` keeps
    // the `X-JMAP-KEY`.
    let edited = vcard.replace(
        "URL;X-JMAP-KEY=l1:https://vera.example/",
        "URL;X-JMAP-KEY=l1:https://vera.example/new",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let links = fixture.card(&id).links.expect("links");
    assert_eq!(links.len(), 1, "patched in place, not re-added");
    assert_eq!(links["l1"].uri, "https://vera.example/new");
    assert_eq!(
        links["l1"].extra.get("mediaType"),
        Some(&json!("text/html")),
        "a member the URL line cannot carry was overwritten"
    );
    assert_eq!(links["l1"].extra.get("pref"), Some(&json!(1)));
}

#[test]
fn a_link_for_getting_in_touch_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // RFC 9553 §2.6.3's `contact` kind has no vCard 3.0 property, so the entry
    // gets no line and never reaches the user — and must not then be deleted
    // by a save, nor have its key taken by the home page the user types.
    fixture.patch(
        &id,
        json!({"links": {"l1": {
            "@type": "Link",
            "kind": "contact",
            "uri": "https://vera.example/write-to-me",
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("\r\nURL"), "{vcard}");
    // The reader counts only the entries it can see, so the key it invents for
    // a URL line with no parameters is `l1` — the key the hidden entry holds.
    let edited = vcard.replace(
        "END:VCARD\r\n",
        "URL:https://vera.example/\r\nEND:VCARD\r\n",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let links = fixture.card(&id).links.expect("links");
    assert_eq!(
        links["l1"].uri, "https://vera.example/write-to-me",
        "an entry the vCard never showed was overwritten: {links:?}"
    );
    assert_eq!(links["l1"].kind.as_deref(), Some("contact"));
    assert!(
        links
            .values()
            .any(|link| link.uri == "https://vera.example/" && link.kind.is_none()),
        "the home page the user typed was not saved: {links:?}"
    );
    assert_eq!(links.len(), 2);
}

#[test]
fn clearing_the_home_page_removes_the_link() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"links": {"l1": {"uri": "https://vera.example/"}}}),
    );
    let sync = fixture.sync();

    // What EDS leaves behind when the user empties Evolution's Home Page
    // field: the line stays, with nothing on it. Measured against
    // libebook-contacts 3.52, which rewrites the value rather than dropping
    // the attribute — so the save has to read an empty line as a deletion.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace(
        "URL;X-JMAP-KEY=l1:https://vera.example/",
        "URL;X-JMAP-KEY=l1:",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.card(&id).links, None);
}

/// The birthday as EDS hands it back after the user has edited it: the date
/// line is rebuilt from `E_CONTACT_BIRTH_DATE`, which drops the `X-JMAP-KEY`
/// this side wrote on it. Verified against libebook-contacts 3.52 — an
/// untouched line keeps its parameters, a rewritten one does not.
fn as_evolution_rewrites_it(vcard: &str, day: &str) -> String {
    let rebuilt: String = vcard
        .lines()
        .map(|line| match line.starts_with("BDAY") {
            true => format!("BDAY:{day}\r\n"),
            false => format!("{line}\r\n"),
        })
        .collect();
    assert_ne!(rebuilt, vcard, "no BDAY line to rewrite in\n{vcard}");
    rebuilt
}

#[test]
fn editing_a_birthday_patches_the_date_the_server_stated_it_in() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // `place` — where the birth happened — has no room on a date line, so it
    // survives only if the entry is patched rather than replaced. And the
    // entry has to be *found* first: Evolution rebuilds the line from its own
    // field and drops the key, so it is matched by the kind of date it is.
    fixture.patch(
        &id,
        json!({"anniversaries": {"k8": {
            "@type": "Anniversary",
            "kind": "birth",
            "date": {"@type": "PartialDate", "year": 1964, "month": 3, "day": 27},
            "place": {"full": "Bremen"},
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(vcard.contains("BDAY;X-JMAP-KEY=k8:1964-03-27"), "{vcard}");

    let edited = as_evolution_rewrites_it(&vcard, "1964-03-28");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let anniversaries = fixture.card(&id).anniversaries.expect("anniversaries");
    assert_eq!(anniversaries.len(), 1, "patched in place, not re-added");
    assert_eq!(
        anniversaries["k8"].date,
        Some(json!({"@type": "PartialDate", "year": 1964, "month": 3, "day": 28}))
    );
    assert_eq!(
        anniversaries["k8"].extra.get("place"),
        Some(&json!({"full": "Bremen"})),
        "a member the date line cannot carry was overwritten"
    );
}

#[test]
fn an_untouched_point_in_time_birthday_keeps_the_hour_it_names() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // RFC 9553 §2.8.1 also dates an anniversary by a Timestamp. The line
    // states the day, so a save must not read the missing hour as an edit —
    // not even when Evolution has rewritten the line and lost the key.
    fixture.patch(
        &id,
        json!({"anniversaries": {"k8": {
            "kind": "birth",
            "date": {"@type": "Timestamp", "utc": "1964-03-27T23:10:00Z"},
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(vcard.contains("BDAY;X-JMAP-KEY=k8:1964-03-27"), "{vcard}");

    let edited = as_evolution_rewrites_it(&vcard, "1964-03-27");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let anniversaries = fixture.card(&id).anniversaries.expect("anniversaries");
    assert_eq!(
        anniversaries["k8"].date,
        Some(json!({"@type": "Timestamp", "utc": "1964-03-27T23:10:00Z"})),
        "the point in time was flattened by a save that changed nothing"
    );
}

#[test]
fn retyping_a_point_in_time_birthday_states_the_day_the_user_typed() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"anniversaries": {"k8": {
            "kind": "birth",
            "date": {"@type": "Timestamp", "utc": "1964-03-27T23:10:00Z"},
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = as_evolution_rewrites_it(&vcard, "1964-03-28");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    // The hour goes with it: the user stated a day, and keeping 23:10 would
    // be this mapping inventing a time on a date nobody gave one for.
    let anniversaries = fixture.card(&id).anniversaries.expect("anniversaries");
    assert_eq!(
        anniversaries["k8"].date,
        Some(json!({"@type": "PartialDate", "year": 1964, "month": 3, "day": 28}))
    );
}

#[test]
fn moving_a_date_to_the_anniversary_line_changes_the_kind_it_is_stated_under() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"anniversaries": {"k8": {
            "kind": "birth",
            "date": {"year": 1996, "month": 8, "day": 3},
        }}}),
    );
    let sync = fixture.sync();

    // A client that keeps the parameters it was given — our own round trip
    // does — moving the date from the birthday field to the anniversary one.
    // Evolution itself drops the key here and takes the delete-and-add path
    // instead; either way the card must stop calling the date a birthday.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace(
        "BDAY;X-JMAP-KEY=k8",
        "X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=k8",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let anniversaries = fixture.card(&id).anniversaries.expect("anniversaries");
    assert_eq!(anniversaries["k8"].kind, "wedding");
    assert_eq!(
        anniversaries["k8"].date,
        Some(json!({"year": 1996, "month": 8, "day": 3})),
        "the date itself did not change, so it must not have been rewritten"
    );
}

#[test]
fn clearing_the_birthday_removes_the_anniversary() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"anniversaries": {"k8": {
            "kind": "birth",
            "date": {"year": 1964, "month": 3, "day": 27},
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited: String = vcard
        .lines()
        .filter(|line| !line.starts_with("BDAY"))
        .map(|line| format!("{line}\r\n"))
        .collect();
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.card(&id).anniversaries, None);
}

#[test]
fn a_date_the_vcard_could_not_state_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // A year on its own and a deathday: neither reaches the user, so neither
    // may be deleted by a save, nor have its key taken by the birthday the
    // user types.
    fixture.patch(
        &id,
        json!({"anniversaries": {
            "y1": {"kind": "birth", "date": {"year": 1964}},
            "y2": {"kind": "death", "date": {"year": 2019, "month": 10, "day": 15}},
        }}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("BDAY"), "{vcard}");
    let edited = vcard.replace("END:VCARD\r\n", "BDAY:1964-03-27\r\nEND:VCARD\r\n");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let anniversaries = fixture.card(&id).anniversaries.expect("anniversaries");
    assert_eq!(
        anniversaries["y1"].date,
        Some(json!({"year": 1964})),
        "an entry the vCard never showed was overwritten: {anniversaries:?}"
    );
    assert_eq!(anniversaries["y2"].kind, "death");
    assert_eq!(
        anniversaries.len(),
        3,
        "the birthday the user typed was not added: {anniversaries:?}"
    );
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

// The same invisibility one property at a time. `titles`, `addresses` and
// `notes` learned it first, each when it landed; `emails`, `phones` and
// `organizations` have entries the vCard leaves out too — one whose value is
// empty — and the save has to see them the same way.

#[test]
fn an_email_with_no_address_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // An entry with nothing to state gets no EMAIL line, so it never reaches
    // the user — and must not then be deleted by a save, nor have its key
    // taken by the address the user types.
    fixture.patch(
        &id,
        json!({"emails": {
            "e0": {"@type": "EmailAddress", "address": "vera@example.com"},
            "e1": {"@type": "EmailAddress", "address": "", "label": "spare"},
        }}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert_eq!(vcard.matches("\r\nEMAIL").count(), 1, "{vcard}");
    // The reader counts only the entries it can see, so the key it invents
    // for a typed-in address is `e1` — the key the hidden entry holds.
    let edited = vcard.replace("END:VCARD\r\n", "EMAIL:beer@example.com\r\nEND:VCARD\r\n");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let emails = fixture.card(&id).emails.expect("emails");
    let hidden = emails.get("e1").expect("the hidden entry");
    assert_eq!(
        (hidden.address.as_str(), hidden.extra.get("label")),
        ("", Some(&json!("spare"))),
        "an entry the vCard never showed was overwritten: {emails:?}"
    );
    assert!(
        emails
            .values()
            .any(|email| email.address == "beer@example.com"),
        "the address the user typed was not saved: {emails:?}"
    );
    assert_eq!(emails.len(), 3);
}

#[test]
fn a_phone_with_no_number_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"phones": {"p1": {"@type": "Phone", "number": "", "label": "spare"}}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("\r\nTEL"), "{vcard}");
    let edited = vcard.replace("END:VCARD\r\n", "TEL:+49 30 123456\r\nEND:VCARD\r\n");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let phones = fixture.card(&id).phones.expect("phones");
    let hidden = phones.get("p1").expect("the hidden entry");
    assert_eq!(
        (hidden.number.as_str(), hidden.extra.get("label")),
        ("", Some(&json!("spare"))),
        "an entry the vCard never showed was overwritten: {phones:?}"
    );
    assert!(
        phones.values().any(|phone| phone.number == "+49 30 123456"),
        "the number the user typed was not saved: {phones:?}"
    );
    assert_eq!(phones.len(), 2);
}

#[test]
fn a_title_with_no_name_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // A title of a mapped kind, but naming nothing: the emitter skips it for
    // the value, which `maps_title_kind` alone cannot see.
    fixture.patch(
        &id,
        json!({"titles": {"t1": {
            "@type": "Title",
            "name": "",
            "kind": "title",
            "organizationId": "o1",
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("\r\nTITLE"), "{vcard}");
    let edited = vcard.replace("END:VCARD\r\n", "TITLE:Head of Beer\r\nEND:VCARD\r\n");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let titles = fixture.card(&id).titles.expect("titles");
    assert_eq!(
        titles
            .get("t1")
            .and_then(|title| title.extra.get("organizationId")),
        Some(&json!("o1")),
        "an entry the vCard never showed was overwritten: {titles:?}"
    );
    assert!(
        titles.values().any(|title| title.name == "Head of Beer"),
        "the title the user typed was not saved: {titles:?}"
    );
    assert_eq!(titles.len(), 2);
}

#[test]
fn an_organization_with_nothing_to_name_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // Neither a name nor a unit, so `organization_components` has nothing to
    // put on an ORG line.
    fixture.patch(
        &id,
        json!({"organizations": {"o1": {"@type": "Organization", "sortAs": "Oldenburg"}}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("\r\nORG"), "{vcard}");
    let edited = vcard.replace("END:VCARD\r\n", "ORG:Brauerei\r\nEND:VCARD\r\n");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let organizations = fixture.card(&id).organizations.expect("organizations");
    assert_eq!(
        organizations
            .get("o1")
            .and_then(|organization| organization.extra.get("sortAs")),
        Some(&json!("Oldenburg")),
        "an entry the vCard never showed was overwritten: {organizations:?}"
    );
    assert!(
        organizations
            .values()
            .any(|organization| organization.name.as_deref() == Some("Brauerei")),
        "the organisation the user typed was not saved: {organizations:?}"
    );
    assert_eq!(organizations.len(), 2);
}
