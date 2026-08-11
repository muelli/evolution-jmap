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
        stored.extra.get("nicknames"),
        Some(&json!({"k1": {"name": "Vee"}})),
        "an unmapped property was overwritten"
    );
    assert_eq!(
        stored.extra.get("anniversaries"),
        Some(&json!({"y1": {"kind": "birth", "date": {"year": 1964}}}))
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
