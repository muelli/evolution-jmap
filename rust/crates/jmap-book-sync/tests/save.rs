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
            "preferredLanguages": {"l1": {"language": "de-DE", "pref": 1}},
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
        stored.extra.get("preferredLanguages"),
        Some(&json!({"l1": {"language": "de-DE", "pref": 1}})),
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
    //
    // The order is the server's for the components that survived the edit —
    // which is what keeps a save that changed nothing from rewriting the list,
    // see `saving_a_name_back_untouched_does_not_rewrite_its_components` — and
    // the vCard's for the ones it added, here the title the user just typed.
    assert_eq!(
        by_kind,
        vec![
            ("given", "Vera"),
            ("generation", "III"),
            ("title", "Dr."),
            ("surname", "Oldenburg-Meier"),
        ]
    );
}

#[test]
fn a_name_the_user_did_not_retype_keeps_how_it_is_pronounced() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"name/components": [
            {"kind": "surname", "value": "Oldenburg", "phonetic": "OL-den-boork"},
            {"kind": "given", "value": "Vera", "phonetic": "VEH-ra"},
        ]}),
    );
    let sync = fixture.sync();

    // The user changes the surname and leaves the given name where it was.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(vcard.contains("N:Oldenburg;Vera;;;"), "{vcard}");
    let edited = vcard.replace("N:Oldenburg;Vera;;;", "N:Oldenburg-Meier;Vera;;;");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let components = fixture.card(&id).name.unwrap().components.unwrap();
    let phonetic = |kind: &str| {
        components
            .iter()
            .find(|component| component.kind == kind)
            .unwrap_or_else(|| panic!("no {kind} component: {components:?}"))
            .extra
            .get("phonetic")
            .and_then(|value| value.as_str())
            .map(str::to_owned)
    };
    // The `N` value has no field for a pronunciation, so the user never saw it
    // and cannot have deleted it.
    assert_eq!(phonetic("given").as_deref(), Some("VEH-ra"));
    // The name it spelled out is gone, though, so keeping it would tell the
    // server that "Oldenburg-Meier" is pronounced "OL-den-boork".
    assert_eq!(phonetic("surname"), None);
}

#[test]
fn a_double_barrelled_given_name_survives_a_save_that_left_it_alone() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Jean Paul Oldenburg", "vera@example.com");
    // Two components of one kind — RFC 9553 §2.2.1 states a double-barrelled
    // given name as two `given` components — share the single `N` field their
    // kind is written into, exactly as a street name and its house number
    // share theirs.
    fixture.patch(
        &id,
        json!({"name/components": [
            {"kind": "surname", "value": "Oldenburg"},
            {"kind": "given", "value": "Jean", "phonetic": "zhon"},
            {"kind": "given", "value": "Paul", "phonetic": "pol"},
        ]}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(vcard.contains("N:Oldenburg;Jean Paul;;;"), "{vcard}");
    let (state_before, _) = sync.list_existing().unwrap();
    sync.save_contact(&vcard, Some(id.as_str())).unwrap();

    assert_eq!(
        sync.list_existing().unwrap().0,
        state_before,
        "a save that changed nothing rewrote the name"
    );
    let components = fixture.card(&id).name.unwrap().components.unwrap();
    let stated: Vec<(&str, &str, Option<&str>)> = components
        .iter()
        .map(|component| {
            (
                component.kind.as_str(),
                component.value.as_str(),
                component
                    .extra
                    .get("phonetic")
                    .and_then(|value| value.as_str()),
            )
        })
        .collect();
    assert_eq!(
        stated,
        vec![
            ("surname", "Oldenburg", None),
            ("given", "Jean", Some("zhon")),
            ("given", "Paul", Some("pol")),
        ],
        "the two halves of the given name came back as their own concatenation"
    );
}

#[test]
fn retyping_a_double_barrelled_given_name_replaces_the_parts_it_was_built_from() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Jean Paul Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"name/components": [
            {"kind": "surname", "value": "Oldenburg"},
            {"kind": "given", "value": "Jean"},
            {"kind": "given", "value": "Paul"},
        ]}),
    );
    let sync = fixture.sync();

    // Nothing in `Hans` says which half of `Jean Paul` it replaced, so both are
    // gone: what must not happen is one of the old halves standing next to the
    // name the user typed.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace("Jean Paul", "Hans");
    assert_ne!(edited, vcard, "the given name was not on the line: {vcard}");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let components = fixture.card(&id).name.unwrap().components.unwrap();
    let by_kind: Vec<(&str, &str)> = components
        .iter()
        .map(|component| (component.kind.as_str(), component.value.as_str()))
        .collect();
    assert_eq!(
        by_kind,
        vec![("surname", "Oldenburg"), ("given", "Hans")],
        "the name the user retyped did not replace the parts it was built \
         from: {components:?}"
    );
}

#[test]
fn saving_a_name_back_untouched_does_not_rewrite_its_components() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // The server's own order, which is not the order the `N` value states the
    // fields in, and one component the value has no field for at all.
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
    let (state_before, _) = sync.list_existing().unwrap();
    sync.save_contact(&vcard, Some(id.as_str())).unwrap();

    assert_eq!(
        sync.list_existing().unwrap().0,
        state_before,
        "a save that changed nothing rewrote the name"
    );
    // Nothing was reshuffled either: RFC 9553 leaves the order meaningless
    // unless `isOrdered` says otherwise, but rewriting it still wakes every
    // other client for nothing.
    let components = fixture.card(&id).name.unwrap().components.unwrap();
    let by_kind: Vec<(&str, &str)> = components
        .iter()
        .map(|component| (component.kind.as_str(), component.value.as_str()))
        .collect();
    assert_eq!(
        by_kind,
        vec![
            ("surname", "Oldenburg"),
            ("given", "Vera"),
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
fn emptying_one_note_line_of_two_withdraws_that_note_alone() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // Two notes, of which Evolution's Notes field shows the user the first —
    // `E_CONTACT_NOTE` is the first `NOTE` line and stops there. So clearing
    // that field is an entry withdrawn from a map through a field that cannot
    // express the map, and the note behind it is not the user's to lose.
    fixture.patch(
        &id,
        json!({"notes": {
            "n1": {"@type": "Note", "note": "met at FOSDEM", "created": "2026-02-01T09:15:00Z"},
            "n2": {"@type": "Note", "note": "allergic to cats"},
        }}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    // The line left standing with no value on it, rather than struck off the
    // card. That is what libebook-contacts 3.52 does with a field the user
    // emptied — measured on the spouse line, and measured on this one by
    // `jmap-functional`'s `unnote` leg — and it is the shape that could go
    // wrong here: a note read back as the empty string would be a note the
    // save *keeps*, spelled as nothing.
    let edited = vcard.replace(
        "NOTE;X-JMAP-KEY=n1:met at FOSDEM\r\n",
        "NOTE;X-JMAP-KEY=n1:\r\n",
    );
    assert_ne!(edited, vcard, "the emitter did not write the note: {vcard}");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let notes = fixture
        .card(&id)
        .notes
        .expect("the note behind the cleared one");
    assert_eq!(
        notes.keys().collect::<Vec<_>>(),
        vec!["n2"],
        "clearing the Notes field did not withdraw exactly the note it showed: {notes:?}"
    );
    assert_eq!(notes["n2"].note, "allergic to cats");
}

#[test]
fn emptying_the_only_note_line_withdraws_the_property() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // One note, so that emptying the field the user is shown leaves nothing on
    // the card the mapping can see — the other branch of
    // `emptying_one_note_line_of_two_withdraws_that_note_alone`. There is no
    // surviving key to null, so what the save must say is that the property
    // itself is gone.
    fixture.patch(
        &id,
        json!({"notes": {
            "n1": {"@type": "Note", "note": "met at FOSDEM", "created": "2026-02-01T09:15:00Z"},
        }}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    // The line left standing with no value on it rather than struck off the
    // card, which is what libebook-contacts 3.52 does with a field the user
    // emptied — measured on this very shape by `jmap-functional`'s
    // `clearing_the_only_note_through_eds_withdraws_the_whole_property`, whose
    // client reports one `NOTE` line and no value on it. The distinction from
    // `removing_the_note_line_removes_the_note` beside this test is therefore
    // the input rather than the outcome: that one states a card EDS does not
    // produce for a cleared field, and this one states the card it does.
    let edited = vcard.replace(
        "NOTE;X-JMAP-KEY=n1:met at FOSDEM\r\n",
        "NOTE;X-JMAP-KEY=n1:\r\n",
    );
    assert_ne!(edited, vcard, "the emitter did not write the note: {vcard}");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    // Gone, rather than an empty map: a `notes` still there saying nothing is a
    // property the server holds for no reason, and the patch that leaves one is
    // the per-entry withdrawal aimed at a card that had nothing to keep.
    assert_eq!(
        fixture.card(&id).notes,
        None,
        "clearing the only note did not withdraw the property"
    );
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

#[test]
fn editing_a_calendar_address_keeps_what_the_caluri_line_cannot_carry() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // RFC 9553 §2.4.1 hangs a `mediaType`, a `pref` and a `label` off a
    // calendar, and RFC 6350 §6.9.3's CALURI is a bare URI, so none of them
    // reaches the vCard. They survive only if the patch reaches in.
    fixture.patch(
        &id,
        json!({"calendars": {"c1": {
            "@type": "Calendar",
            "kind": "calendar",
            "uri": "https://vera.example/cal/vera.ics",
            "mediaType": "text/calendar",
            "pref": 1,
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("CALURI;X-JMAP-KEY=c1:https://vera.example/cal/vera.ics"),
        "{vcard}"
    );

    // EDS rewrites the value of that line in place and leaves the parameters
    // where they were, so the key comes back — measured against
    // libebook-contacts 3.52, where a set on `E_CONTACT_CALENDAR_URI` keeps
    // the `X-JMAP-KEY`, exactly as one on `E_CONTACT_HOMEPAGE_URL` does.
    let edited = vcard.replace(
        "CALURI;X-JMAP-KEY=c1:https://vera.example/cal/vera.ics",
        "CALURI;X-JMAP-KEY=c1:https://vera.example/cal/new.ics",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let calendars = fixture.card(&id).calendars.expect("calendars");
    assert_eq!(calendars.len(), 1, "patched in place, not re-added");
    assert_eq!(calendars["c1"].uri, "https://vera.example/cal/new.ics");
    assert_eq!(
        calendars["c1"].kind.as_deref(),
        Some("calendar"),
        "the kind is what put the URI on that line and cannot have been edited"
    );
    assert_eq!(
        calendars["c1"].extra.get("mediaType"),
        Some(&json!("text/calendar")),
        "a member the CALURI line cannot carry was overwritten"
    );
    assert_eq!(calendars["c1"].extra.get("pref"), Some(&json!(1)));
}

#[test]
fn a_calendar_of_no_stated_kind_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // RFC 9553 §2.4.1 makes the kind mandatory, so an entry naming none says
    // nothing about which line its URI belongs on: it gets no line and never
    // reaches the user — and must not then be deleted by a save, nor have its
    // key taken by the calendar address the user types.
    fixture.patch(
        &id,
        json!({"calendars": {"c1": {
            "@type": "Calendar",
            "uri": "https://vera.example/cal/nameless",
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("\r\nCALURI"), "{vcard}");
    // The reader counts only the entries it can see, so the key it invents for
    // a CALURI line with no parameters is `c1` — the key the hidden entry holds.
    let edited = vcard.replace(
        "END:VCARD\r\n",
        "CALURI:https://vera.example/cal/vera.ics\r\nEND:VCARD\r\n",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let calendars = fixture.card(&id).calendars.expect("calendars");
    assert_eq!(
        calendars["c1"].uri, "https://vera.example/cal/nameless",
        "an entry the vCard never showed was overwritten: {calendars:?}"
    );
    assert_eq!(calendars["c1"].kind, None);
    assert!(
        calendars.values().any(|calendar| {
            calendar.uri == "https://vera.example/cal/vera.ics"
                && calendar.kind.as_deref() == Some("calendar")
        }),
        "the calendar address the user typed was not saved: {calendars:?}"
    );
    assert_eq!(calendars.len(), 2);
}

#[test]
fn clearing_the_free_busy_address_removes_the_calendar() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"calendars": {"c1": {"kind": "freeBusy", "uri": "https://vera.example/fb"}}}),
    );
    let sync = fixture.sync();

    // What EDS leaves behind when the user empties Evolution's Free/Busy
    // field: the line stays, with nothing on it. Measured against
    // libebook-contacts 3.52 — a set to the empty string rewrites the value
    // and keeps the line, and only a set to NULL drops the line outright — so
    // the save has to read an empty line as a deletion just as a missing one.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace(
        "FBURL;X-JMAP-KEY=c1:https://vera.example/fb",
        "FBURL;X-JMAP-KEY=c1:",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.card(&id).calendars, None);
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

#[test]
fn filing_a_contact_under_a_category_sends_the_whole_set() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(&id, json!({"keywords": {"hiking": true}}));
    let sync = fixture.sync();

    // RFC 2426 §3.7.1's `CATEGORIES` holds the tags on one line, which is what
    // EDS rewrites when the user edits Evolution's Categories field: there is no
    // key to patch by, so the set goes back replaced whole.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(vcard.contains("\r\nCATEGORIES:hiking\r\n"), "{vcard}");
    let edited = vcard.replace("CATEGORIES:hiking", "CATEGORIES:hiking,climbing");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.card(&id).keywords.expect("keywords"),
        [
            ("climbing".to_owned(), json!(true)),
            ("hiking".to_owned(), json!(true)),
        ]
        .into()
    );
}

#[test]
fn clearing_the_categories_leaves_the_contact_filed_under_nothing() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(&id, json!({"keywords": {"hiking": true}}));
    let sync = fixture.sync();

    // Which is a `"keywords": null` rather than an empty map: RFC 9553 §2.8.2's
    // default is no tags, and an empty set would be a different thing to store.
    // EDS removes the attribute outright when the field is cleared, measured
    // against libebook-contacts 3.52, so no line at all is what a save sees.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace("CATEGORIES:hiking\r\n", "");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.card(&id).keywords, None);
}

#[test]
fn a_tag_the_categories_line_cannot_carry_survives_the_set_being_rewritten() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // EDS trims the ends of a category, so this tag would come back renamed and
    // the next save would rename it on the server. It gets no line, and the
    // tags that *can* be stated still do — the user sees most of the truth
    // rather than none of it. The set is replaced whole rather than patched
    // entry by entry, so the tag the vCard never showed has to be put back on
    // the set the save writes: it was not shown, so its absence from the edited
    // card is not the user asking for it to go.
    fixture.patch(&id, json!({"keywords": {" quiet": true, "hiking": true}}));
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(vcard.contains("\r\nCATEGORIES:hiking\r\n"), "{vcard}");
    let edited = vcard.replace("CATEGORIES:hiking", "CATEGORIES:hiking,climbing");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.card(&id).keywords.expect("keywords"),
        [
            (" quiet".to_owned(), json!(true)),
            ("climbing".to_owned(), json!(true)),
            ("hiking".to_owned(), json!(true)),
        ]
        .into(),
        "the tag the user typed was dropped, or the one they never saw was"
    );
}

#[test]
fn clearing_the_categories_leaves_the_tag_nobody_saw_behind() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // Emptying the field deletes the tags it showed and nothing else. A `null`
    // here would delete the tag that had no line to be shown on, which is the
    // one thing the user cannot have meant by clearing a field it was not in.
    fixture.patch(&id, json!({"keywords": {" quiet": true, "hiking": true}}));
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace("CATEGORIES:hiking\r\n", "");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.card(&id).keywords.expect("keywords"),
        [(" quiet".to_owned(), json!(true))].into()
    );
}

#[test]
fn a_tag_the_server_set_to_something_else_is_carried_back_as_the_server_had_it() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // RFC 9553 §1.4.3 has every value of a Set be `true`, so a `false` is the
    // server contradicting itself and the line cannot state it either way. It
    // is still the server's own word: carried back untouched rather than
    // corrected, because correcting it would be this mapping inventing a change
    // nobody made.
    fixture.patch(&id, json!({"keywords": {"hiking": false, "loud": true}}));
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(vcard.contains("\r\nCATEGORIES:loud\r\n"), "{vcard}");
    let edited = vcard.replace("CATEGORIES:loud", "CATEGORIES:loud,climbing");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.card(&id).keywords.expect("keywords"),
        [
            ("climbing".to_owned(), json!(true)),
            ("hiking".to_owned(), json!(false)),
            ("loud".to_owned(), json!(true)),
        ]
        .into()
    );
}

#[test]
fn typing_a_tag_the_server_had_set_to_something_else_sets_it() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // The one place the two sides name the same tag. The user's word wins: they
    // typed it into a field that says nothing but "filed under", so they mean it
    // set, whatever the server had against that name before.
    fixture.patch(&id, json!({"keywords": {"hiking": false}}));
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("\r\nCATEGORIES"), "{vcard}");
    let edited = vcard.replace("FN:", "CATEGORIES:hiking\r\nFN:");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.card(&id).keywords.expect("keywords"),
        [("hiking".to_owned(), json!(true))].into()
    );
}

#[test]
fn a_set_holding_a_tag_with_no_line_is_not_an_edit_waiting_to_happen() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // The tag put back is the tag that was already there, so the set the save
    // would write is the set the server holds — and a patch naming it would
    // undo a concurrent edit on another client for no reason at all. The
    // property must be left unnamed, not merely written back unchanged.
    fixture.patch(&id, json!({"keywords": {" quiet": true, "hiking": true}}));
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let (state_before, _) = sync.list_existing().unwrap();
    sync.save_contact(&vcard, Some(id.as_str())).unwrap();
    let (state_after, _) = sync.list_existing().unwrap();

    assert_eq!(
        fixture.card(&id).keywords.expect("keywords"),
        [
            (" quiet".to_owned(), json!(true)),
            ("hiking".to_owned(), json!(true)),
        ]
        .into()
    );
    assert_eq!(
        state_after, state_before,
        "a save with nothing to say about tags rewrote them anyway"
    );
}

#[test]
fn an_edit_that_left_the_categories_alone_does_not_rewrite_them() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(&id, json!({"keywords": {"hiking": true}}));
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace("vera@example.com", "vera@example.org");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    // Not merely equal in the end — the patch must not have named the property
    // at all, or a concurrent edit on another client would be undone by a save
    // that had nothing to say about tags.
    assert_eq!(
        fixture.card(&id).keywords.expect("keywords"),
        [("hiking".to_owned(), json!(true))].into()
    );
}

#[test]
fn an_empty_set_on_the_server_is_not_an_edit_waiting_to_happen() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // A card whose `keywords` is present but holds nothing draws the same
    // vCard as one with no `keywords` at all — no `CATEGORIES` line — so the
    // two have to compare as the same set. Otherwise every save of such a card
    // would patch the property to a null, an edit nobody made.
    fixture.patch(&id, json!({"keywords": {}}));
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("\r\nCATEGORIES"), "{vcard}");
    let edited = vcard.replace("vera@example.com", "vera@example.org");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.card(&id).keywords,
        Some(std::collections::BTreeMap::new()),
        "the empty set the server held was rewritten"
    );
}

#[test]
fn editing_an_im_handle_patches_the_entry_by_its_key() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "onlineServices": {
                "s1": {
                    "@type": "OnlineService",
                    "service": "Jabber",
                    "user": "vera@jabber.example",
                    "pref": 1,
                },
            },
        }),
    );
    let sync = fixture.sync();

    // The handle crosses on the `X-JABBER` line EDS keeps it on, wearing the
    // key it is filed under, so an edit patches that entry rather than
    // replacing the property — and the `pref` the line has no parameter for
    // stays where the server put it.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("\r\nX-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera@jabber.example\r\n"),
        "{vcard}"
    );
    let edited = vcard.replace("vera@jabber.example", "vera@xmpp.example");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let services = fixture.card(&id).online_services.expect("onlineServices");
    assert_eq!(services.keys().collect::<Vec<_>>(), vec!["s1"]);
    assert_eq!(services["s1"].user.as_deref(), Some("vera@xmpp.example"));
    assert_eq!(services["s1"].service.as_deref(), Some("Jabber"));
    assert_eq!(services["s1"].extra.get("pref"), Some(&json!(1)));
}

#[test]
fn renaming_a_handle_drops_the_uri_that_named_the_old_one() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // An entry may state both, and only the handle has a line. Once the user
    // has replaced the handle, the URI names somebody the entry no longer
    // claims to be — and it cannot be rebuilt from the new handle without
    // knowing the service's URI scheme, which is a guess this mapping refuses
    // to write. So it goes with the name it belonged to, exactly as an
    // organisation unit's `sortAs` does when the unit is renamed.
    fixture.patch(
        &id,
        json!({
            "onlineServices": {
                "s1": {
                    "service": "Jabber",
                    "user": "vera@jabber.example",
                    "uri": "xmpp:vera@jabber.example",
                },
            },
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace(
        "X-JMAP-KEY=s1;TYPE=HOME:vera@jabber.example",
        "X-JMAP-KEY=s1;TYPE=HOME:vera@xmpp.example",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let services = fixture.card(&id).online_services.expect("onlineServices");
    assert_eq!(services["s1"].user.as_deref(), Some("vera@xmpp.example"));
    assert_eq!(services["s1"].uri, None, "the stale URI was left behind");
}

#[test]
fn renaming_a_handle_the_uri_alone_stated_rewrites_that_uri() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // An entry the server stated as a URI and nothing else. The line draws the
    // handle out of it, so the entry is now the user's to edit — and the edit
    // has to go back the way it came. Writing a `user` here would answer a card
    // shaped one way with a card shaped another, for no reason the user gave;
    // the scheme that let the handle be read lets it be written.
    fixture.patch(
        &id,
        json!({
            "onlineServices": {
                "s1": {"service": "Jabber", "uri": "xmpp:vera@jabber.example"},
            },
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("\r\nX-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera@jabber.example\r\n"),
        "{vcard}"
    );
    let edited = vcard.replace("vera@jabber.example", "vera@xmpp.example");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let services = fixture.card(&id).online_services.expect("onlineServices");
    assert_eq!(services.keys().collect::<Vec<_>>(), vec!["s1"]);
    assert_eq!(
        services["s1"].uri.as_deref(),
        Some("xmpp:vera@xmpp.example")
    );
    assert_eq!(services["s1"].user, None, "a handle the entry never stated");
}

#[test]
fn renaming_a_gadu_gadu_handle_the_uri_alone_stated_rewrites_that_uri() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "onlineServices": {
                "s1": {"service": "Gadu-Gadu", "uri": "gg:12345678"},
            },
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("\r\nX-GADUGADU;X-JMAP-KEY=s1;TYPE=HOME:12345678\r\n"),
        "{vcard}"
    );
    let edited = vcard.replace("12345678", "87654321");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let services = fixture.card(&id).online_services.expect("onlineServices");
    assert_eq!(services.keys().collect::<Vec<_>>(), vec!["s1"]);
    assert_eq!(services["s1"].uri.as_deref(), Some("gg:87654321"));
    assert_eq!(services["s1"].user, None, "a handle the entry never stated");
}

#[test]
fn an_edit_that_left_a_uri_only_handle_alone_writes_nothing() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // The entry crosses as a handle and comes back as one, so the card the
    // reader builds states a `user` where the server states a `uri`. Comparing
    // those members rather than the handle they both spell would make every
    // save an edit of this entry — a patch, and a state the server bumps, each
    // time the contact is touched for any reason at all.
    fixture.patch(
        &id,
        json!({
            "onlineServices": {
                "s1": {"service": "Jabber", "uri": "xmpp:vera@jabber.example"},
            },
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains(":vera@jabber.example\r\n"),
        "the handle was not drawn: {vcard}"
    );
    let (state_before, _) = sync.list_existing().unwrap();
    sync.save_contact(&vcard, Some(id.as_str())).unwrap();
    let (state_after, _) = sync.list_existing().unwrap();
    assert_eq!(
        state_after, state_before,
        "saving the vCard back unchanged rewrote the entry"
    );

    // And an edit elsewhere on the contact leaves the entry in the shape the
    // server chose for it.
    let edited = vcard.replace("vera@example.com", "vera@example.org");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let services = fixture.card(&id).online_services.expect("onlineServices");
    assert_eq!(
        services["s1"].uri.as_deref(),
        Some("xmpp:vera@jabber.example")
    );
    assert_eq!(services["s1"].user, None);
}

#[test]
fn a_handle_no_uri_can_state_is_written_as_the_handle_it_is() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // The user is typing into a free-text field, so what they type need not fit
    // in a URI at all. Rebuilding one around a space would state an identifier
    // no service could resolve, so the entry changes shape instead: the handle
    // goes on the `user`, and the URI that named the old one goes, exactly as
    // it does when an entry stating both is renamed.
    fixture.patch(
        &id,
        json!({
            "onlineServices": {
                "s1": {"service": "Jabber", "uri": "xmpp:vera@jabber.example"},
            },
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace("vera@jabber.example", "vera oldenburg");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let services = fixture.card(&id).online_services.expect("onlineServices");
    assert_eq!(services["s1"].user.as_deref(), Some("vera oldenburg"));
    assert_eq!(services["s1"].uri, None, "an unresolvable URI was written");
}

#[test]
fn an_edit_that_left_the_handle_alone_keeps_the_uri_it_came_with() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "onlineServices": {
                "s1": {
                    "service": "Jabber",
                    "user": "vera@jabber.example",
                    "uri": "xmpp:vera@jabber.example",
                },
            },
        }),
    );
    let sync = fixture.sync();

    // The URI is only dropped by a rename. An edit elsewhere on the contact
    // must not touch it — nor the handle, nor the service.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace("vera@example.com", "vera@example.org");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let services = fixture.card(&id).online_services.expect("onlineServices");
    assert_eq!(services["s1"].user.as_deref(), Some("vera@jabber.example"));
    assert_eq!(
        services["s1"].uri.as_deref(),
        Some("xmpp:vera@jabber.example")
    );
}

#[test]
fn the_service_spelling_the_server_chose_is_not_rewritten() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // RFC 9553 §2.3.2 lets a service be capitalised as the service itself
    // does and has two names be equal when they match case-insensitively. The
    // `X-JABBER` line states no spelling at all, so this side reads back the
    // one the table holds — and a save that wrote it would rename the service
    // on the server for no reason the user gave.
    fixture.patch(
        &id,
        json!({"onlineServices": {"s1": {"service": "jabber", "user": "vera@jabber.example"}}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace("vera@jabber.example", "vera@xmpp.example");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let services = fixture.card(&id).online_services.expect("onlineServices");
    assert_eq!(services["s1"].service.as_deref(), Some("jabber"));
    assert_eq!(services["s1"].user.as_deref(), Some("vera@xmpp.example"));
}

#[test]
fn typing_a_handle_evolution_had_no_line_for_creates_an_entry() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();

    // The line EDS writes when the user fills in one of the instant-messaging
    // fields: no `X-JMAP-KEY`, since the entry is new, and a `TYPE` naming the
    // slot they typed it into.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace(
        "END:VCARD",
        "X-MATRIX;TYPE=WORK:@vera:matrix.example\r\nEND:VCARD",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let services = fixture.card(&id).online_services.expect("onlineServices");
    let service = services.values().next().expect("one service");
    assert_eq!(service.service.as_deref(), Some("Matrix"));
    assert_eq!(service.user.as_deref(), Some("@vera:matrix.example"));
    // The slot is not the entry's contexts, so nothing about where the user
    // typed it reaches the server.
    assert_eq!(service.extra.get("contexts"), None, "{service:?}");
}

#[test]
fn a_service_the_vcard_cannot_state_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // Two entries with no line: one at a service EDS has no field for, one
    // stated as a URI whose scheme this mapping cannot read a handle out of.
    // Neither was ever visible, so neither may be deleted for being absent from
    // the edited vCard — and the handle the user does type must not be filed
    // under a key one of them already holds.
    fixture.patch(
        &id,
        json!({
            "onlineServices": {
                "s1": {"service": "Signal", "user": "+49301234"},
                "s2": {"service": "Matrix", "uri": "matrix:u/vera:matrix.example"},
            },
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("X-MATRIX"), "{vcard}");
    assert!(!vcard.contains("49301234"), "{vcard}");
    let edited = vcard.replace("END:VCARD", "X-SKYPE;TYPE=HOME:vera.oldenburg\r\nEND:VCARD");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let services = fixture.card(&id).online_services.expect("onlineServices");
    assert_eq!(services["s1"].user.as_deref(), Some("+49301234"));
    assert_eq!(
        services["s2"].uri.as_deref(),
        Some("matrix:u/vera:matrix.example")
    );
    assert_eq!(services.len(), 3, "{services:?}");
    let typed = services
        .values()
        .find(|service| service.service.as_deref() == Some("Skype"))
        .unwrap_or_else(|| panic!("the handle the user typed is gone: {services:?}"));
    assert_eq!(typed.user.as_deref(), Some("vera.oldenburg"));
}

#[test]
fn clearing_the_last_im_handle_leaves_the_contact_on_no_service() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"onlineServices": {"s1": {"service": "Jabber", "user": "vera@jabber.example"}}}),
    );
    let sync = fixture.sync();

    // EDS removes the attribute outright when the field is cleared, so no line
    // at all is what a save sees — and every entry the card had was visible,
    // so the whole property goes.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace(
        "X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera@jabber.example\r\n",
        "",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.card(&id).online_services, None);
}

#[test]
fn a_key_that_arrived_on_another_services_line_moves_the_service_with_it() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"onlineServices": {"s1": {"service": "Jabber", "user": "vera@jabber.example"}}}),
    );
    let sync = fixture.sync();

    // Nothing Evolution does produces this — changing the service means EDS
    // clearing one field and setting another, which loses the key — but another
    // client writing the vCard can, and then the entry the key names really is
    // at a different service. The line is what states the service, so the save
    // has to follow it rather than leave the entry claiming the old one.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace(
        "X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera@jabber.example",
        "X-SKYPE;X-JMAP-KEY=s1;TYPE=HOME:vera.oldenburg",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let services = fixture.card(&id).online_services.expect("onlineServices");
    assert_eq!(services.keys().collect::<Vec<_>>(), vec!["s1"]);
    assert_eq!(services["s1"].service.as_deref(), Some("Skype"));
    assert_eq!(services["s1"].user.as_deref(), Some("vera.oldenburg"));
}

/// "hello-photo", standing in for the JPEG a real card carries.
const PHOTO: &str = "aGVsbG8tcGhvdG8=";
/// What EDS writes for a picture the user has just chosen: no `X-JMAP-KEY`,
/// because `e_contact_set` rebuilds the line out of the photo it holds —
/// measured against libebook-contacts 3.52.
const CHOSEN: &str = "PHOTO;TYPE=png;ENCODING=b:bmV3LXBob3RvISE=";

/// A card whose picture the server holds, with the members a `PHOTO` line
/// cannot carry hung off it.
fn seed_photo(fixture: &Fixture, id: &jmap_proto::Id) {
    fixture.patch(
        id,
        json!({"media": {"m1": {
            "@type": "Media",
            "kind": "photo",
            "uri": format!("data:image/jpeg;base64,{PHOTO}"),
            "mediaType": "image/jpeg",
            "pref": 1,
        }}}),
    );
}

#[test]
fn the_photo_the_user_chose_reaches_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    seed_photo(&fixture, &id);
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains(&format!("PHOTO;X-JMAP-KEY=m1;TYPE=jpeg;ENCODING=b:{PHOTO}")),
        "{vcard}"
    );

    // The key is gone from the line the editor writes back, so the entry it
    // replaces is the one the line it replaced belonged to.
    let edited = vcard.replace(
        &format!("PHOTO;X-JMAP-KEY=m1;TYPE=jpeg;ENCODING=b:{PHOTO}"),
        CHOSEN,
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let media = fixture.card(&id).media.expect("media");
    assert_eq!(media.len(), 1, "patched in place, not re-added: {media:?}");
    assert_eq!(media["m1"].uri, "data:image/png;base64,bmV3LXBob3RvISE=");
    assert_eq!(media["m1"].media_type.as_deref(), Some("image/png"));
    assert_eq!(
        media["m1"].extra.get("pref"),
        Some(&json!(1)),
        "a member the PHOTO line cannot carry was overwritten: {media:?}"
    );
}

#[test]
fn a_photo_nobody_touched_is_not_written_back() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // Spelled the way a hand-written `data:` URI often is — RFC 4648 §3.2 makes
    // the padding optional — while the line carries the canonical spelling,
    // because it is glib's base64 reader that decodes it. So the URI that comes
    // back is *not* the URI the server holds, and comparing the two would make
    // every save an edit of a picture nobody chose.
    let loose = format!("data:image/jpeg;base64,{}", PHOTO.trim_end_matches('='));
    fixture.patch(
        &id,
        json!({"media": {"m1": {"@type": "Media", "kind": "photo", "uri": loose}}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    sync.save_contact(&vcard, Some(id.as_str())).unwrap();

    let media = fixture.card(&id).media.expect("media");
    assert_eq!(media["m1"].uri, loose, "the save rewrote it: {media:?}");
    assert_eq!(media["m1"].media_type, None, "{media:?}");
}

#[test]
fn removing_the_photo_line_removes_the_picture() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    seed_photo(&fixture, &id);
    let sync = fixture.sync();

    // Measured against libebook-contacts 3.52: clearing the photo removes the
    // attribute outright, so no line at all is what the save sees.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited: String = vcard
        .lines()
        .filter(|line| !line.starts_with("PHOTO"))
        .map(|line| format!("{line}\r\n"))
        .collect();
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.card(&id).media, None);
}

#[test]
fn a_picture_chosen_for_a_contact_that_had_none_is_added() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("PHOTO"), "{vcard}");
    let edited = vcard.replace("END:VCARD\r\n", &format!("{CHOSEN}\r\nEND:VCARD\r\n"));
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let media = fixture.card(&id).media.expect("media");
    assert_eq!(media.len(), 1, "{media:?}");
    let photo = media.values().next().expect("one entry");
    assert_eq!(photo.kind.as_deref(), Some("photo"));
    assert_eq!(photo.uri, "data:image/png;base64,bmV3LXBob3RvISE=");
    assert_eq!(photo.media_type.as_deref(), Some("image/png"));
}

#[test]
fn only_the_first_of_several_pictures_is_the_one_the_user_edits() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // Measured against libebook-contacts 3.52: `E_CONTACT_PHOTO` reports the
    // first `PHOTO` line and a `set` replaces that line in place, leaving the
    // others — parameters and all — where they were. So a card carrying two
    // pictures comes back with the first rewritten and the second still wearing
    // its key, and the save must not read the rewrite as both of them changing.
    fixture.patch(
        &id,
        json!({"media": {
            "m1": {"@type": "Media", "kind": "photo",
                   "uri": format!("data:image/jpeg;base64,{PHOTO}")},
            "m9": {"@type": "Media", "kind": "photo",
                   "uri": "https://vera.example/other.png"},
        }}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace(
        &format!("PHOTO;X-JMAP-KEY=m1;TYPE=jpeg;ENCODING=b:{PHOTO}"),
        CHOSEN,
    );
    assert!(
        edited.contains("PHOTO;X-JMAP-KEY=m9;VALUE=uri:https://vera.example/other.png"),
        "{edited}"
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let media = fixture.card(&id).media.expect("media");
    assert_eq!(media.len(), 2, "{media:?}");
    assert_eq!(media["m1"].uri, "data:image/png;base64,bmV3LXBob3RvISE=");
    assert_eq!(
        media["m9"].uri, "https://vera.example/other.png",
        "the picture the user never saw was rewritten: {media:?}"
    );
}

#[test]
fn a_logo_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // RFC 9553 §2.6.4 keeps all three kinds of media in one map and only a
    // photo is the picture Evolution shows, so the logo gets no line — and the
    // key the reader invents for the one line there is happens to be the logo's.
    fixture.patch(
        &id,
        json!({"media": {
            "m1": {"@type": "Media", "kind": "logo",
                   "uri": "https://vera.example/logo.png"},
            "m2": {"@type": "Media", "kind": "photo",
                   "uri": format!("data:image/jpeg;base64,{PHOTO}")},
        }}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert_eq!(vcard.matches("\r\nPHOTO").count(), 1, "{vcard}");
    let edited = vcard.replace(
        &format!("PHOTO;X-JMAP-KEY=m2;TYPE=jpeg;ENCODING=b:{PHOTO}"),
        CHOSEN,
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let media = fixture.card(&id).media.expect("media");
    assert_eq!(
        media["m1"].uri, "https://vera.example/logo.png",
        "the logo was overwritten by the photo the user chose: {media:?}"
    );
    assert_eq!(
        media["m2"].uri, "data:image/png;base64,bmV3LXBob3RvISE=",
        "the picture was re-added rather than patched: {media:?}"
    );
    assert_eq!(media.len(), 2, "{media:?}");
}

/// The spouse line as EDS hands it back after the user has typed in the field:
/// `e_contact_set(E_CONTACT_SPOUSE, …)` rewrites the value of the first line of
/// that name in place. The empty name is what clearing the field leaves — a set
/// to the empty string keeps the line with nothing on it, and only a set to NULL
/// drops the line outright, measured against libebook-contacts 3.52 as it was
/// for the Free/Busy field.
fn as_evolution_retypes_the_spouse(vcard: &str, name: &str) -> String {
    let mut rewritten = false;
    let rebuilt: String = vcard
        .lines()
        .map(
            |line| match !rewritten && line.starts_with("X-EVOLUTION-SPOUSE") {
                true => {
                    rewritten = true;
                    format!("X-EVOLUTION-SPOUSE:{name}\r\n")
                }
                false => format!("{line}\r\n"),
            },
        )
        .collect();
    assert!(rewritten, "no spouse line to rewrite in\n{vcard}");
    rebuilt
}

#[test]
fn retyping_a_spouse_withdraws_the_marriage_and_keeps_what_else_was_said() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // RFC 9553 §2.1.8 keys `relatedTo` by the related entity itself, so the
    // name on the line *is* the entry's key: a name the user respells is not a
    // renamed value, it is another entry. What the line stated about the old one
    // was the marriage and nothing else — the `kin` it never showed is not the
    // user's to have deleted.
    fixture.patch(
        &id,
        json!({"relatedTo": {"Jean Paul Oldenburg": {
            "@type": "Relation",
            "relation": {"spouse": true, "kin": true},
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("X-EVOLUTION-SPOUSE:Jean Paul Oldenburg"),
        "{vcard}"
    );

    let edited = as_evolution_retypes_the_spouse(&vcard, "Jean-Paul Oldenburg");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let related = fixture.card(&id).related_to.expect("relatedTo");
    assert_eq!(
        related.keys().collect::<Vec<_>>(),
        vec!["Jean Paul Oldenburg", "Jean-Paul Oldenburg"],
        "{related:?}"
    );
    assert_eq!(
        related["Jean Paul Oldenburg"].relation,
        Some([("kin".to_owned(), json!(true))].into()),
        "the relation the line never showed went with the marriage: {related:?}"
    );
    assert_eq!(
        related["Jean Paul Oldenburg"].extra.get("@type"),
        Some(&json!("Relation")),
        "the entry was replaced rather than patched"
    );
    assert_eq!(
        related["Jean-Paul Oldenburg"].relation,
        Some([("spouse".to_owned(), json!(true))].into()),
        "the name the user typed is a spouse and nothing more: {related:?}"
    );
}

#[test]
fn retyping_a_spouse_who_was_nothing_else_leaves_no_entry_behind() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // The common case: the entry said the marriage and no more, so withdrawing
    // it leaves nothing worth keeping and the whole entry goes. Beside it, a
    // spouse the card names the way §2.1.8 asks — by the related Card's `uid`,
    // which gets no line because a URN under the heading "Spouse" is not a
    // person — and which the save therefore may not touch either.
    fixture.patch(
        &id,
        json!({"relatedTo": {
            "Jean Paul Oldenburg": {"relation": {"spouse": true}},
            "urn:uuid:e1f0a1c2-0f6b-4d2e-9c3a-2b1f9d0e7c44": {"relation": {"spouse": true}},
        }}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert_eq!(vcard.matches("X-EVOLUTION-SPOUSE").count(), 1, "{vcard}");

    let edited = as_evolution_retypes_the_spouse(&vcard, "Jean-Paul Oldenburg");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let related = fixture.card(&id).related_to.expect("relatedTo");
    assert_eq!(
        related.keys().collect::<Vec<_>>(),
        vec![
            "Jean-Paul Oldenburg",
            "urn:uuid:e1f0a1c2-0f6b-4d2e-9c3a-2b1f9d0e7c44"
        ],
        "{related:?}"
    );
    assert_eq!(
        related["urn:uuid:e1f0a1c2-0f6b-4d2e-9c3a-2b1f9d0e7c44"].relation,
        Some([("spouse".to_owned(), json!(true))].into()),
        "an entry the vCard never showed was rewritten: {related:?}"
    );
}

#[test]
fn clearing_the_spouse_field_removes_the_relation() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // The `@type` tag says what the object is, not anything about the relation,
    // so an entry wearing it and the marriage still has nothing left once the
    // marriage is withdrawn — the same judgement the calendar side makes about a
    // location that named nothing but its name.
    fixture.patch(
        &id,
        json!({"relatedTo": {"Jean Paul Oldenburg": {
            "@type": "Relation",
            "relation": {"spouse": true},
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = as_evolution_retypes_the_spouse(&vcard, "");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    // The property goes rather than being left as an empty map: that is what
    // RFC 9553 §2.1.8's default of no relations is stated as.
    assert_eq!(fixture.card(&id).related_to, None);
}

#[test]
fn clearing_the_spouse_field_keeps_a_relation_the_line_never_showed() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // Nineteen of the twenty relation types have no field in Evolution, so a
    // child is an entry the user was never shown — and emptying the Spouse
    // field says nothing about it.
    fixture.patch(
        &id,
        json!({"relatedTo": {
            "Jean Paul Oldenburg": {"relation": {"spouse": true}},
            "Nils Oldenburg": {"relation": {"child": true}},
        }}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = as_evolution_retypes_the_spouse(&vcard, "");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let related = fixture.card(&id).related_to.expect("relatedTo");
    assert_eq!(related.keys().collect::<Vec<_>>(), vec!["Nils Oldenburg"]);
    assert_eq!(
        related["Nils Oldenburg"].relation,
        Some([("child".to_owned(), json!(true))].into())
    );
}

#[test]
fn a_spouse_the_card_already_relates_to_gains_the_marriage() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // The key being the person is what makes this a merge rather than an
    // addition: typing a name the card already relates to is saying one more
    // thing about that entry, not naming a second one.
    fixture.patch(
        &id,
        json!({"relatedTo": {"Jean Paul Oldenburg": {
            "@type": "Relation",
            "relation": {"kin": true},
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("SPOUSE"), "{vcard}");
    let edited = vcard.replace(
        "END:VCARD\r\n",
        "X-EVOLUTION-SPOUSE:Jean Paul Oldenburg\r\nEND:VCARD\r\n",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let related = fixture.card(&id).related_to.expect("relatedTo");
    assert_eq!(
        related.keys().collect::<Vec<_>>(),
        vec!["Jean Paul Oldenburg"],
        "the same person was named twice: {related:?}"
    );
    assert_eq!(
        related["Jean Paul Oldenburg"].relation,
        Some(
            [
                ("kin".to_owned(), json!(true)),
                ("spouse".to_owned(), json!(true))
            ]
            .into()
        ),
        "the relation set was replaced rather than added to: {related:?}"
    );
}

#[test]
fn the_spouse_the_user_types_reaches_a_card_that_relates_to_nobody() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();

    // RFC 8620 §5.3 wants every path segment before the last to exist on the
    // object already, and a card relating to nobody has no `relatedTo` for a
    // path to reach into, so the property is written whole. (This mock creates
    // intermediate objects on demand; a server holding to §5.3 would not.)
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace(
        "END:VCARD\r\n",
        "X-EVOLUTION-SPOUSE:Jean Paul Oldenburg\r\nEND:VCARD\r\n",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let related = fixture.card(&id).related_to.expect("relatedTo");
    assert_eq!(
        related.keys().collect::<Vec<_>>(),
        vec!["Jean Paul Oldenburg"]
    );
    assert_eq!(
        related["Jean Paul Oldenburg"].relation,
        Some([("spouse".to_owned(), json!(true))].into())
    );
}

#[test]
fn a_spouse_whose_name_holds_a_pointer_character_is_patched_under_that_name() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // A key this side did not choose: `relatedTo` is keyed by a person's name,
    // and RFC 6901 §3's `/` and `~` mean something inside a patch path. Unescaped
    // they would send the withdrawal into an object nobody named — and, on the
    // other side of the edit, file the new spouse under a name split in two.
    fixture.patch(
        &id,
        json!({"relatedTo": {"Anne/Marie Oldenburg": {
            "relation": {"spouse": true, "kin": true},
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("X-EVOLUTION-SPOUSE:Anne/Marie Oldenburg"),
        "{vcard}"
    );

    let edited = as_evolution_retypes_the_spouse(&vcard, "Jo~Ann Oldenburg");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let related = fixture.card(&id).related_to.expect("relatedTo");
    assert_eq!(
        related.keys().collect::<Vec<_>>(),
        vec!["Anne/Marie Oldenburg", "Jo~Ann Oldenburg"],
        "{related:?}"
    );
    assert_eq!(
        related["Anne/Marie Oldenburg"].relation,
        Some([("kin".to_owned(), json!(true))].into()),
        "{related:?}"
    );
    assert_eq!(
        related["Jo~Ann Oldenburg"].relation,
        Some([("spouse".to_owned(), json!(true))].into()),
        "{related:?}"
    );
}

#[test]
fn unmapped_or_unslotted_services_are_preserved_across_saves() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // A card on the server carrying both a mapped service (Jabber) and unslotted /
    // unmodeled services (Twitter, SIP) which EDS has no per-slot fields for.
    fixture.patch(
        &id,
        json!({
            "onlineServices": {
                "s1": {"service": "Jabber", "user": "vera@jabber.example"},
                "s_tw": {"service": "Twitter", "user": "vera_tw"},
                "s_sip": {"service": "SIP", "uri": "sip:vera@example.com"},
            },
        }),
    );
    let sync = fixture.sync();

    // The vCard carries only the mapped Jabber handle.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera@jabber.example"),
        "{vcard}"
    );
    assert!(!vcard.contains("vera_tw"), "{vcard}");
    assert!(!vcard.contains("sip:vera@example.com"), "{vcard}");

    // Edit the Jabber handle
    let edited = vcard.replace("vera@jabber.example", "vera@xmpp.example");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    // On the server, Jabber was patched while Twitter and SIP remain completely intact.
    let services = fixture.card(&id).online_services.expect("onlineServices");
    assert_eq!(services["s1"].user.as_deref(), Some("vera@xmpp.example"));
    assert_eq!(services["s_tw"].user.as_deref(), Some("vera_tw"));
    assert_eq!(
        services["s_sip"].uri.as_deref(),
        Some("sip:vera@example.com")
    );
}

#[test]
fn conventional_im_uri_schemes_are_preserved_across_saves() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "onlineServices": {
                "s_aim": {"service": "AIM", "uri": "aim:goim?screenname=alice"},
                "s_msn": {"service": "MSN", "uri": "msnim:chat?contact=bob@example.com"},
                "s_yahoo": {"service": "Yahoo", "uri": "ymsgr:sendim?carol"},
                "s_icq": {"service": "ICQ", "uri": "icq:message?uin=12345678"},
            },
        }),
    );
    let sync = fixture.sync();

    // The vCard loaded by EDS contains none of these action/query URIs as X- lines.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("X-AIM"), "{vcard}");
    assert!(!vcard.contains("X-MSN"), "{vcard}");
    assert!(!vcard.contains("X-YAHOO"), "{vcard}");
    assert!(!vcard.contains("X-ICQ"), "{vcard}");

    // An edit elsewhere on the contact (e.g. email) preserves all four onlineServices intact.
    let edited = vcard.replace("vera@example.com", "vera@example.org");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let services = fixture.card(&id).online_services.expect("onlineServices");
    assert_eq!(
        services["s_aim"].uri.as_deref(),
        Some("aim:goim?screenname=alice")
    );
    assert_eq!(services["s_aim"].user, None);
    assert_eq!(
        services["s_msn"].uri.as_deref(),
        Some("msnim:chat?contact=bob@example.com")
    );
    assert_eq!(services["s_msn"].user, None);
    assert_eq!(
        services["s_yahoo"].uri.as_deref(),
        Some("ymsgr:sendim?carol")
    );
    assert_eq!(services["s_yahoo"].user, None);
    assert_eq!(
        services["s_icq"].uri.as_deref(),
        Some("icq:message?uin=12345678")
    );
    assert_eq!(services["s_icq"].user, None);
}

#[test]
fn slotted_conventional_im_handles_are_patched_correctly() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "onlineServices": {
                "s_aim": {"service": "AIM", "user": "alice_aim"},
                "s_icq": {"service": "ICQ", "user": "12345678"},
                "s_msn": {"service": "MSN", "user": "bob@example.com"},
                "s_yahoo": {"service": "Yahoo", "user": "carol_yahoo"},
            },
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("X-AIM;X-JMAP-KEY=s_aim;TYPE=HOME:alice_aim\r\n"),
        "{vcard}"
    );
    assert!(
        vcard.contains("X-ICQ;X-JMAP-KEY=s_icq;TYPE=HOME:12345678\r\n"),
        "{vcard}"
    );
    assert!(
        vcard.contains("X-MSN;X-JMAP-KEY=s_msn;TYPE=HOME:bob@example.com\r\n"),
        "{vcard}"
    );
    assert!(
        vcard.contains("X-YAHOO;X-JMAP-KEY=s_yahoo;TYPE=HOME:carol_yahoo\r\n"),
        "{vcard}"
    );

    // Edit handles in vCard
    let edited = vcard
        .replace("alice_aim", "alice_aim_2")
        .replace("12345678", "87654321")
        .replace("bob@example.com", "bob@example.org")
        .replace("carol_yahoo", "carol_yahoo_2");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let services = fixture.card(&id).online_services.expect("onlineServices");
    assert_eq!(services["s_aim"].user.as_deref(), Some("alice_aim_2"));
    assert_eq!(services["s_icq"].user.as_deref(), Some("87654321"));
    assert_eq!(services["s_msn"].user.as_deref(), Some("bob@example.org"));
    assert_eq!(services["s_yahoo"].user.as_deref(), Some("carol_yahoo_2"));
}

#[test]
fn editing_unrelated_field_preserves_year_only_birthday_and_deathday() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "anniversaries": {
                "y1": {"kind": "birth", "date": {"year": 1964}},
                "y2": {"kind": "death", "date": {"year": 2019, "month": 10, "day": 15}},
                "y3": {"kind": "wedding", "date": {"year": 1996, "month": 8, "day": 3}},
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("BDAY"), "{vcard}");
    assert!(!vcard.contains("death"), "{vcard}");
    assert!(
        vcard.contains("X-EVOLUTION-ANNIVERSARY;X-JMAP-KEY=y3:1996-08-03\r\n"),
        "{vcard}"
    );

    // Edit the wedding anniversary and add a note
    let edited = vcard.replace("1996-08-03", "1996-08-04").replace(
        "END:VCARD\r\n",
        "NOTE:Preserve unmodeled dates\r\nEND:VCARD\r\n",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    let anniversaries = card.anniversaries.expect("anniversaries");
    assert_eq!(
        anniversaries["y1"].date,
        Some(json!({"year": 1964})),
        "year-only birthday was mangled: {anniversaries:?}"
    );
    assert_eq!(anniversaries["y1"].kind, "birth");
    assert_eq!(
        anniversaries["y2"].date,
        Some(json!({"year": 2019, "month": 10, "day": 15})),
        "deathday was mangled: {anniversaries:?}"
    );
    assert_eq!(anniversaries["y2"].kind, "death");
    assert_eq!(
        anniversaries["y3"].date,
        Some(json!({"year": 1996, "month": 8, "day": 4})),
        "wedding date was not updated: {anniversaries:?}"
    );
    assert_eq!(anniversaries["y3"].kind, "wedding");
    assert_eq!(anniversaries.len(), 3);
}

#[test]
fn editing_first_org_preserves_secondary_orgs_and_titles() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "organizations": {
                "o1": {"name": "Acme Ltd", "units": [{"@type": "OrgUnit", "name": "Research"}]},
                "o2": {"name": "Brauerei", "units": [{"@type": "OrgUnit", "name": "Logistics"}]},
            },
            "titles": {
                "t1": {"name": "Research Scientist"},
                "t2": {"name": "Director of Engineering"},
                "r1": {"name": "Lead Investigator", "kind": "role"},
                "r2": {"name": "Project Manager", "kind": "role"},
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("ORG;X-JMAP-KEY=o1:Acme Ltd;Research\r\n"),
        "{vcard}"
    );
    assert!(
        vcard.contains("ORG;X-JMAP-KEY=o2:Brauerei;Logistics\r\n"),
        "{vcard}"
    );
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

    // Edit the first ORG and first TITLE in place
    let edited = vcard
        .replace("Acme Ltd;Research", "Acme Corporation;Optics")
        .replace("Research Scientist", "Principal Scientist");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    let organizations = card.organizations.expect("organizations");
    assert_eq!(organizations.len(), 2, "{organizations:?}");
    assert_eq!(
        organizations["o1"].name.as_deref(),
        Some("Acme Corporation")
    );
    assert_eq!(
        organizations["o1"].units.as_ref().unwrap()[0].name,
        "Optics"
    );
    assert_eq!(organizations["o2"].name.as_deref(), Some("Brauerei"));
    assert_eq!(
        organizations["o2"].units.as_ref().unwrap()[0].name,
        "Logistics"
    );

    let titles = card.titles.expect("titles");
    assert_eq!(titles.len(), 4, "{titles:?}");
    assert_eq!(titles["t1"].name, "Principal Scientist");
    assert_eq!(titles["t1"].kind, None);
    assert_eq!(titles["t2"].name, "Director of Engineering");
    assert_eq!(titles["t2"].kind, None);
    assert_eq!(titles["r1"].name, "Lead Investigator");
    assert_eq!(titles["r1"].kind.as_deref(), Some("role"));
    assert_eq!(titles["r2"].name, "Project Manager");
    assert_eq!(titles["r2"].kind.as_deref(), Some("role"));
}

#[test]
fn editing_unrelated_field_preserves_all_multiple_orgs_and_titles() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "organizations": {
                "o1": {"name": "Acme Ltd", "units": [{"@type": "OrgUnit", "name": "Research"}]},
                "o2": {"name": "Brauerei", "units": [{"@type": "OrgUnit", "name": "Logistics"}]},
            },
            "titles": {
                "t1": {"name": "Research Scientist"},
                "t2": {"name": "Director of Engineering"},
                "r1": {"name": "Lead Investigator", "kind": "role"},
                "r2": {"name": "Project Manager", "kind": "role"},
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace(
        "END:VCARD\r\n",
        "NOTE:Preserve orgs and titles\r\nEND:VCARD\r\n",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    let organizations = card.organizations.expect("organizations");
    assert_eq!(organizations.len(), 2, "{organizations:?}");
    assert_eq!(organizations["o1"].name.as_deref(), Some("Acme Ltd"));
    assert_eq!(
        organizations["o1"].units.as_ref().unwrap()[0].name,
        "Research"
    );
    assert_eq!(organizations["o2"].name.as_deref(), Some("Brauerei"));
    assert_eq!(
        organizations["o2"].units.as_ref().unwrap()[0].name,
        "Logistics"
    );

    let titles = card.titles.expect("titles");
    assert_eq!(titles.len(), 4, "{titles:?}");
    assert_eq!(titles["t1"].name, "Research Scientist");
    assert_eq!(titles["t2"].name, "Director of Engineering");
    assert_eq!(titles["r1"].name, "Lead Investigator");
    assert_eq!(titles["r2"].name, "Project Manager");
}

#[test]
fn saving_contact_with_multiple_addresses_and_labels_preserves_all_entries() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "addresses": {
                "a1": {
                    "@type": "Address",
                    "contexts": {"work": true},
                    "components": [
                        {"@type": "AddressComponent", "kind": "name", "value": "Hauptstraße 1"},
                        {"@type": "AddressComponent", "kind": "locality", "value": "Berlin"},
                        {"@type": "AddressComponent", "kind": "postcode", "value": "10115"},
                        {"@type": "AddressComponent", "kind": "country", "value": "Germany"},
                    ],
                    "full": "Hauptstraße 1\n10115 Berlin\nGermany",
                    "coordinates": "geo:52.5,13.4",
                },
                "a2": {
                    "@type": "Address",
                    "contexts": {"private": true},
                    "components": [
                        {"@type": "AddressComponent", "kind": "name", "value": "Heimweg 2"},
                        {"@type": "AddressComponent", "kind": "locality", "value": "München"},
                        {"@type": "AddressComponent", "kind": "postcode", "value": "80331"},
                        {"@type": "AddressComponent", "kind": "country", "value": "Germany"},
                    ],
                    "full": "Heimweg 2\n80331 München\nGermany",
                    "pref": 1,
                },
                "a3": {
                    "@type": "Address",
                    "full": "Postfach 42\n20095 Hamburg",
                    "timeZone": "Europe/Berlin",
                }
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace(
        "END:VCARD\r\n",
        "NOTE:Preserve all addresses\r\nEND:VCARD\r\n",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    let addresses = card.addresses.expect("addresses");
    assert_eq!(addresses.len(), 3, "{addresses:?}");

    assert_eq!(
        addresses["a1"].full.as_deref(),
        Some("Hauptstraße 1\n10115 Berlin\nGermany")
    );
    assert_eq!(
        addresses["a1"].extra.get("coordinates"),
        Some(&json!("geo:52.5,13.4"))
    );

    assert_eq!(
        addresses["a2"].full.as_deref(),
        Some("Heimweg 2\n80331 München\nGermany")
    );
    assert_eq!(addresses["a2"].extra.get("pref"), Some(&json!(1)));

    assert_eq!(
        addresses["a3"].full.as_deref(),
        Some("Postfach 42\n20095 Hamburg")
    );
    assert_eq!(
        addresses["a3"].extra.get("timeZone"),
        Some(&json!("Europe/Berlin"))
    );
}

#[test]
fn editing_work_address_label_preserves_secondary_home_address_and_label() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "addresses": {
                "a1": {
                    "@type": "Address",
                    "contexts": {"work": true},
                    "components": [
                        {"@type": "AddressComponent", "kind": "name", "value": "Hauptstraße 1"},
                        {"@type": "AddressComponent", "kind": "locality", "value": "Berlin"},
                    ],
                    "full": "Hauptstraße 1\n10115 Berlin\nGermany",
                },
                "a2": {
                    "@type": "Address",
                    "contexts": {"private": true},
                    "components": [
                        {"@type": "AddressComponent", "kind": "name", "value": "Heimweg 2"},
                        {"@type": "AddressComponent", "kind": "locality", "value": "München"},
                    ],
                    "full": "Heimweg 2\n80331 München\nGermany",
                }
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    // Simulate EDS setting a new work address label
    let edited = vcard.replace(
        "LABEL;X-JMAP-KEY=a1;TYPE=WORK:Hauptstraße 1\\n10115 Berlin\\nGermany",
        "LABEL;X-JMAP-KEY=a1;TYPE=WORK:Updated Work Label\\nBerlin",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    let addresses = card.addresses.expect("addresses");
    assert_eq!(addresses.len(), 2, "{addresses:?}");
    assert_eq!(
        addresses["a1"].full.as_deref(),
        Some("Updated Work Label\nBerlin")
    );
    assert_eq!(
        addresses["a2"].full.as_deref(),
        Some("Heimweg 2\n80331 München\nGermany")
    );
}

#[test]
fn saving_contact_with_multiple_im_services_and_contexts_preserves_all_entries() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "onlineServices": {
                "s1": {"service": "Jabber", "user": "vera@home.example", "contexts": {"private": true}},
                "s2": {"service": "Jabber", "user": "vera@work.example", "contexts": {"work": true}},
                "s3": {"service": "Matrix", "user": "@vera:matrix.example", "contexts": {"work": true}},
                "s4": {"service": "Skype", "user": "vera_skype", "contexts": {"private": true}},
                "s5": {"service": "Gadu-Gadu", "user": "123456", "contexts": {"work": true}},
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(vcard.contains("X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera@home.example"));
    assert!(vcard.contains("X-JABBER;X-JMAP-KEY=s2;TYPE=WORK:vera@work.example"));
    assert!(vcard.contains("X-MATRIX;X-JMAP-KEY=s3;TYPE=WORK:@vera:matrix.example"));
    assert!(vcard.contains("X-SKYPE;X-JMAP-KEY=s4;TYPE=HOME:vera_skype"));
    assert!(vcard.contains("X-GADUGADU;X-JMAP-KEY=s5;TYPE=WORK:123456"));

    // An edit on an unrelated field preserves all 5 onlineServices intact
    let edited = vcard.replace(
        "END:VCARD\r\n",
        "NOTE:Preserve all IM services\r\nEND:VCARD\r\n",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    let services = card.online_services.expect("onlineServices");
    assert_eq!(services.len(), 5, "{services:?}");
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
fn editing_one_im_handle_preserves_secondary_im_handles_of_same_and_different_services() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "onlineServices": {
                "s1": {"service": "Jabber", "user": "vera@home.example", "contexts": {"private": true}},
                "s2": {"service": "Jabber", "user": "vera@work.example", "contexts": {"work": true}},
                "s3": {"service": "Matrix", "user": "@vera:matrix.example", "contexts": {"work": true}},
                "s4": {"service": "Skype", "user": "vera_skype", "contexts": {"private": true}},
                "s5": {"service": "Gadu-Gadu", "user": "123456", "contexts": {"work": true}},
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    // Edit s1 (Jabber HOME) and s4 (Skype HOME) in place
    let edited = vcard
        .replace(
            "X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera@home.example",
            "X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera_updated@home.example",
        )
        .replace(
            "X-SKYPE;X-JMAP-KEY=s4;TYPE=HOME:vera_skype",
            "X-SKYPE;X-JMAP-KEY=s4;TYPE=HOME:alice_work_skype",
        );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    let services = card.online_services.expect("onlineServices");
    assert_eq!(services.len(), 5, "{services:?}");
    assert_eq!(
        services["s1"].user.as_deref(),
        Some("vera_updated@home.example")
    );
    assert_eq!(services["s1"].service.as_deref(), Some("Jabber"));
    assert_eq!(services["s2"].user.as_deref(), Some("vera@work.example"));
    assert_eq!(services["s2"].service.as_deref(), Some("Jabber"));
    assert_eq!(services["s3"].user.as_deref(), Some("@vera:matrix.example"));
    assert_eq!(services["s3"].service.as_deref(), Some("Matrix"));
    assert_eq!(services["s4"].user.as_deref(), Some("alice_work_skype"));
    assert_eq!(services["s4"].service.as_deref(), Some("Skype"));
    assert_eq!(services["s5"].user.as_deref(), Some("123456"));
    assert_eq!(services["s5"].service.as_deref(), Some("Gadu-Gadu"));
}
