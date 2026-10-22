// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for the write side. A save must not destroy data that the vCard cannot carry.
//! The mapping drops most of a JSContact card. Thus a save that replaces whole
//! properties deletes data that the user cannot see.

mod common;

use common::Fixture;
use jmap_book_sync::{Base, OnConflict};
use serde_json::json;

/// The vCard that Evolution gives to `save_contact_sync` for a new contact.
/// The `UID` is a name from the local cache, not a server identifier.
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
    let (_, contacts) = sync.list_existing().unwrap();
    assert_eq!(contacts, vec![saved]);
}

#[test]
fn editing_a_contact_leaves_unmapped_properties_alone() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // Properties that no vCard from this mapping can carry.
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
    assert!(
        stored.preferred_languages.is_some() || stored.extra.contains_key("preferredLanguages"),
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
    // No vCard TYPE states "school". It survives the round trip only if the patch
    // merges the contexts.
    fixture.patch(
        &id,
        json!({"emails/e0/contexts": {"work": true, "school": true}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(vcard.contains("TYPE=WORK"), "{vcard}");
    // The user changes the address to private.
    let edited = vcard.replace("TYPE=WORK", "TYPE=HOME");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let contexts = fixture.card(&id).emails.as_ref().unwrap()["e0"]
        .contexts
        .clone()
        .unwrap();
    assert_eq!(contexts, json!({"private": true, "school": true}));
}

#[test]
fn editing_preserves_the_context_a_line_could_state_only_one_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // A phone and an address for work and private use. Each line states one slot.
    // EDS reads a line with both `TYPE`s into two editor fields, and an edit of one field
    // rewrites the line of both. See `eds-sys`'
    // `a_line_wearing_both_context_types_fills_two_slots_that_overwrite_each_other`.
    fixture.patch(
        &id,
        json!({
            "phones/p1": {
                "number": "+49 30 111",
                "contexts": {"work": true, "private": true},
                "features": {"voice": true},
            },
            "addresses/a1": {
                "components": [{"kind": "name", "value": "Hauptstraße 1"}],
                "contexts": {"work": true, "private": true},
            },
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("TEL;X-JMAP-KEY=p1;TYPE=HOME,VOICE:"),
        "{vcard}"
    );
    assert!(vcard.contains("ADR;X-JMAP-KEY=a1;TYPE=HOME:"), "{vcard}");

    // The user edits a different field.
    let edited = vcard.replace("Vera Oldenburg", "Vera Oldenburg-Schmidt");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    let both = json!({"private": true, "work": true});
    assert_eq!(
        card.phones.as_ref().unwrap()["p1"].contexts,
        Some(both.clone()),
        "the context the line had no slot for must not be read as removed",
    );
    assert_eq!(card.addresses.as_ref().unwrap()["a1"].contexts, Some(both));
}

#[test]
fn moving_an_entry_to_the_other_slot_still_reclassifies_it() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "phones/p1": {
                "number": "+49 30 111",
                "contexts": {"work": true, "private": true},
                "features": {"voice": true},
            },
        }),
    );
    let sync = fixture.sync();

    // The user can still change the slot that the line states. Removing the number from
    // the Home field removes the private context. Only the unstated context stays.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace("TYPE=HOME,VOICE", "TYPE=VOICE");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.card(&id).phones.as_ref().unwrap()["p1"].contexts,
        Some(json!({"work": true})),
    );
}

#[test]
fn editing_preserves_the_feature_a_line_could_state_only_one_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // A work number that is a voice line and a fax. The line states one feature:
    // EDS reads `TYPE=WORK,VOICE,FAX` into the Business Phone and Business Fax fields,
    // and an edit of one field rewrites the line of both.
    fixture.patch(
        &id,
        json!({
            "phones/p1": {
                "number": "+49 30 111",
                "contexts": {"work": true},
                "features": {"voice": true, "fax": true},
            },
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("TEL;X-JMAP-KEY=p1;TYPE=WORK,FAX:"),
        "{vcard}"
    );

    // The user edits a different field.
    let edited = vcard.replace("Vera Oldenburg", "Vera Oldenburg-Schmidt");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.card(&id).phones.as_ref().unwrap()["p1"].features,
        Some(json!({"voice": true, "fax": true})),
        "the feature the line had no slot for must not be read as removed",
    );
}

#[test]
fn moving_a_number_to_another_kind_of_phone_field_reclassifies_it() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "phones/p1": {
                "number": "+49 30 111",
                "contexts": {"work": true},
                "features": {"voice": true, "fax": true},
            },
        }),
    );
    let sync = fixture.sync();

    // The user moves the number to the Mobile field: it is a mobile now, not a fax.
    // The line did not state the voice feature, so the voice feature stays.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("TEL;X-JMAP-KEY=p1;TYPE=WORK,FAX:"),
        "{vcard}"
    );
    let edited = vcard.replace("TYPE=WORK,FAX:", "TYPE=CELL:");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.card(&id).phones.as_ref().unwrap()["p1"].features,
        Some(json!({"voice": true, "mobile": true})),
    );
}

#[test]
fn reclassifying_an_entry_that_had_only_one_context_gives_it_the_other() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // The entry has one context, so the line states all of it.
    fixture.patch(
        &id,
        json!({
            "phones/p1": {
                "number": "+49 30 111",
                "contexts": {"private": true},
                "features": {"voice": true},
            },
        }),
    );
    let sync = fixture.sync();

    // The user moves the number from the Home field to the Business field.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace("TYPE=HOME,VOICE", "TYPE=WORK,VOICE");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.card(&id).phones.as_ref().unwrap()["p1"].contexts,
        Some(json!({"work": true})),
        "the context the user typed in must arrive, not vanish with the old one",
    );
}

#[test]
fn editing_preserves_a_preference_ranking_the_vcard_flattens() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // vCard 3.0 has only a PREF flag, so a rank of 30 becomes "PREF".
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
    // Remove the phone and add a second email address.
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
    // The replaced mapped components must not stay, or the card gets two surnames.
    // Kept components stay in the server order, so a save without changes does not
    // rewrite the list (see `saving_a_name_back_untouched_does_not_rewrite_its_components`).
    // Added components follow in the vCard order.
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

    // The user changes the surname and keeps the given name.
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
    // The `N` value has no field for a pronunciation, so the user did not delete it.
    assert_eq!(phonetic("given").as_deref(), Some("VEH-ra"));
    // The surname changed. If the old pronunciation stays, the card pronounces
    // "Oldenburg-Meier" as "OL-den-boork".
    assert_eq!(phonetic("surname"), None);
}

#[test]
fn a_double_barrelled_given_name_survives_a_save_that_left_it_alone() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Jean Paul Oldenburg", "vera@example.com");
    // RFC 9553 §2.2.1 states a double-barrelled given name as two `given` components.
    // Both share one `N` field, as a street name and a house number share one `ADR` field.
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

    // `Hans` does not show which half of `Jean Paul` it replaces, so both halves go.
    // No old half may stay next to the new name.
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
    // The server order differs from the `N` field order, and one component has no `N` field.
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
    // The order also stays. RFC 9553 gives the order no meaning unless `isOrdered` is set,
    // but a rewrite still notifies every other client without cause.
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
fn a_name_component_the_n_value_left_out_for_saying_nothing_survives_an_edit() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // A component of a kind with an `N` field, but with an empty value. The emitter
    // omits it, so the user did not see it and did not clear it.
    fixture.patch(
        &id,
        json!({"name/components": [
            {"kind": "surname", "value": "Oldenburg"},
            {"kind": "given2", "value": ""},
            {"kind": "given", "value": "Vera"},
        ]}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("\r\nN:Oldenburg;Vera;;;\r\n"),
        "the empty middle name reached the value: {vcard}"
    );

    let edited = vcard.replace("Vera", "Verena");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let components = fixture.card(&id).name.unwrap().components.unwrap();
    let by_kind: Vec<(&str, &str)> = components
        .iter()
        .map(|component| (component.kind.as_str(), component.value.as_str()))
        .collect();
    assert_eq!(
        by_kind,
        vec![
            ("surname", "Oldenburg"),
            ("given2", ""),
            ("given", "Verena")
        ],
        "a component the `N` value never stated was deleted by an edit beside it"
    );
}

#[test]
fn renaming_an_employer_keeps_what_the_org_line_cannot_carry() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // `sortAs` and `contexts` have no ORG component and no ORG parameter.
    // They survive only if the patch edits the entry member by member.
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
    // The user renames the employer and removes the second department.
    let edited = vcard.replace("Acme;Research;Optics", "Acme Ltd;Research");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.card(&id);
    let organizations = stored.organizations.as_ref().expect("organizations");
    assert_eq!(organizations.len(), 1, "patched in place, not re-added");
    let organization = &organizations["o1"];
    assert_eq!(organization.name.as_deref(), Some("Acme Ltd"));
    assert_eq!(
        organization
            .sort_as
            .as_deref()
            .or_else(|| organization.extra.get("sortAs").and_then(|v| v.as_str())),
        Some("Acme"),
        "a member the ORG line cannot carry was overwritten"
    );
    assert_eq!(
        organization
            .contexts
            .as_ref()
            .or_else(|| organization.extra.get("contexts")),
        Some(&json!({"work": true}))
    );
    let units = organization.units.as_ref().expect("units");
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].name, "Research");
    assert_eq!(
        units[0]
            .sort_as
            .as_deref()
            .or_else(|| units[0].extra.get("sortAs").and_then(|v| v.as_str())),
        Some("Res"),
        "a unit that kept its name kept nothing else"
    );
}

#[test]
fn editing_one_org_unit_of_two_keeps_the_one_the_line_left_out() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // A unit with no name gets no component on the ORG line, so the user does not see it.
    // A save that edits its sibling must not delete it.
    fixture.patch(
        &id,
        json!({"organizations": {"o1": {
            "@type": "Organization",
            "name": "Acme",
            "units": [
                {"@type": "OrgUnit", "name": "Research"},
                {"@type": "OrgUnit", "name": "", "sortAs": "Optics"},
            ],
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("ORG;X-JMAP-KEY=o1:Acme;Research\r\n"),
        "the unnamed unit has no component on the line: {vcard}"
    );

    let (state_before, _) = sync.list_existing().unwrap();
    sync.save_contact(&vcard, Some(id.as_str())).unwrap();
    assert_eq!(
        sync.list_existing().unwrap().0,
        state_before,
        "a save that changed nothing rewrote the organization"
    );

    let edited = vcard.replace(";Research", ";Advanced Research");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.card(&id);
    let organization = &stored.organizations.as_ref().expect("organizations")["o1"];
    assert_eq!(organization.name.as_deref(), Some("Acme"));
    let units = organization.units.as_ref().expect("units");
    let by_name: Vec<&str> = units.iter().map(|unit| unit.name.as_str()).collect();
    assert_eq!(
        by_name,
        vec!["Advanced Research", ""],
        "a unit the ORG line never stated was deleted by an edit beside it"
    );
    assert_eq!(
        units[1]
            .sort_as
            .as_deref()
            .or_else(|| units[1].extra.get("sortAs").and_then(|v| v.as_str())),
        Some("Optics"),
        "and it kept what it was carrying, in the place it was carrying it"
    );
}

#[test]
fn deleting_a_visible_unit_keeps_its_invisible_neighbour_in_place() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // The unnamed unit is between the two visible units. Its place follows the
    // surviving visible units, not its original index.
    fixture.patch(
        &id,
        json!({"organizations": {"o1": {
            "@type": "Organization",
            "name": "Acme",
            "units": [
                {"@type": "OrgUnit", "name": "Research"},
                {"@type": "OrgUnit", "name": "", "sortAs": "Legacy"},
                {"@type": "OrgUnit", "name": "Optics"},
            ],
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("ORG;X-JMAP-KEY=o1:Acme;Research;Optics\r\n"),
        "the unnamed unit has no component on the line: {vcard}"
    );

    // The user deletes "Research" from the line and keeps "Optics".
    let edited = vcard.replace(";Research;Optics", ";Optics");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.card(&id);
    let organization = &stored.organizations.as_ref().expect("organizations")["o1"];
    let units = organization.units.as_ref().expect("units");
    let by_name: Vec<&str> = units.iter().map(|unit| unit.name.as_str()).collect();
    assert_eq!(
        by_name,
        vec!["", "Optics"],
        "the unnamed unit was pushed past the one it used to precede"
    );
    assert_eq!(
        units[0]
            .sort_as
            .as_deref()
            .or_else(|| units[0].extra.get("sortAs").and_then(|v| v.as_str())),
        Some("Legacy")
    );
}

#[test]
fn saving_an_org_back_untouched_does_not_reshuffle_the_unit_the_line_left_out() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // The only unit has no name, so the line states no unit. The save must not read
    // the empty unit list as a removal.
    fixture.patch(
        &id,
        json!({"organizations": {"o1": {
            "@type": "Organization",
            "name": "Acme",
            "units": [
                {"@type": "OrgUnit", "name": "", "sortAs": "Optics"},
            ],
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("ORG;X-JMAP-KEY=o1:Acme\r\n"),
        "the line states the employer and nothing else: {vcard}"
    );

    let edited = vcard.replace("Acme", "Acme Ltd");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.card(&id);
    let organization = &stored.organizations.as_ref().expect("organizations")["o1"];
    assert_eq!(organization.name.as_deref(), Some("Acme Ltd"));
    let units = organization
        .units
        .as_ref()
        .expect("units the line never stated");
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].name, "");
    assert_eq!(
        units[0]
            .sort_as
            .as_deref()
            .or_else(|| units[0].extra.get("sortAs").and_then(|v| v.as_str())),
        Some("Optics")
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
    // A TITLE line is plain text with no room for the organisation of the title.
    // The organisation survives only if the patch edits the entry member by member.
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
        titles["t1"]
            .organization_id
            .as_deref()
            .or_else(|| titles["t1"]
                .extra
                .get("organizationId")
                .and_then(|v| v.as_str())),
        Some("o1"),
        "a member the TITLE line cannot carry was overwritten"
    );
}

#[test]
fn a_title_of_a_vendor_kind_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // vCard 3.0 has only TITLE and ROLE. The emitter drops an entry of a kind outside
    // the two of RFC 9553 §2.2.5, so a save must not delete it.
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
    // The user types a job title into a contact that shows no title. The reader counts
    // only visible entries, so it gives the new entry `t1`, the key of the hidden entry.
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
    // An ADR line has no room for three things. These are members of the address, a
    // component without a vCard field, and a member of a component with a field.
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

    // A save without changes must change nothing, or each open of a contact rewrites its
    // address. The hidden parts must keep their order and shape, not only survive.
    let (state_before, _) = sync.list_existing().unwrap();
    sync.save_contact(&vcard, Some(id.as_str())).unwrap();
    assert_eq!(
        sync.list_existing().unwrap().0,
        state_before,
        "a save that changed nothing rewrote the address"
    );

    let edited = vcard.replace("Berlin", "Potsdam");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.card(&id);
    let addresses = stored.addresses.as_ref().expect("addresses");
    assert_eq!(addresses.len(), 1, "patched in place, not re-added");
    let address = &addresses["a1"];
    // The written-out address has its own LABEL line, so the town changes there too.
    // The replacement above edits both lines, as an address edit in Evolution does.
    assert_eq!(
        address.full.as_deref(),
        Some("Hauptstraße 1\n10115 Potsdam")
    );
    assert_eq!(
        address
            .coordinates
            .as_deref()
            .or_else(|| address.extra.get("coordinates").and_then(|v| v.as_str())),
        Some("geo:52.5,13.4")
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
    // The street name and the house number share one ADR field, so an edit of that field
    // changes both. The text does not show where the number ends, so the parts are lost.
    // The old number must not stay next to the new street.
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
    // An address with only components that have no vCard field gets no ADR line, so the
    // user does not see it. A save must not delete it or give its key to a new address.
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
    // The entry also gets no LABEL, because it has no data for a written-out address.
    assert!(!vcard.contains("\r\nLABEL"), "{vcard}");
    // The reader counts only visible entries, so it gives the new entry `a1`,
    // the key of the hidden entry.
    let edited = vcard.replace(
        "END:VCARD\r\n",
        "ADR;TYPE=HOME:;;Hauptstraße 1;Berlin;;10115;Germany\r\nEND:VCARD\r\n",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let addresses = fixture.card(&id).addresses.expect("addresses");
    assert_eq!(
        addresses["a1"]
            .coordinates
            .as_deref()
            .or_else(|| addresses["a1"]
                .extra
                .get("coordinates")
                .and_then(|v| v.as_str())),
        Some("geo:52.5,13.4"),
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
    // RFC 9553 §2.5.1 allows an address with no known components, written only in `full`.
    // It gets a LABEL line and no ADR, so its key goes on the LABEL. Without the key,
    // this save adds the address again and loses the members that no line can carry.
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
        addresses["a1"]
            .coordinates
            .as_deref()
            .or_else(|| addresses["a1"]
                .extra
                .get("coordinates")
                .and_then(|v| v.as_str())),
        Some("geo:52.5,13.4"),
        "a member no line can carry was overwritten"
    );
}

#[test]
fn an_address_shown_only_on_its_label_keeps_the_components_no_adr_field_states() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // No component here has one of the seven ADR fields, so the address gets a LABEL
    // and no ADR. The user sees only the written-out form, so an edit of that line
    // does not clear the components.
    fixture.patch(
        &id,
        json!({"addresses": {"a1": {
            "@type": "Address",
            "components": [
                {"@type": "AddressComponent", "kind": "building", "value": "Turm A"},
                {"@type": "AddressComponent", "kind": "floor", "value": "3"},
            ],
            "full": "Turm A, 3. Stock\n10115 Berlin",
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("\r\nADR"), "{vcard}");
    assert!(vcard.contains("\r\nLABEL"), "{vcard}");

    let (state_before, _) = sync.list_existing().unwrap();
    sync.save_contact(&vcard, Some(id.as_str())).unwrap();
    assert_eq!(
        sync.list_existing().unwrap().0,
        state_before,
        "a save that changed nothing rewrote the address"
    );

    let edited = vcard.replace("Berlin", "Potsdam");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let addresses = fixture.card(&id).addresses.expect("addresses");
    assert_eq!(
        addresses["a1"].full.as_deref(),
        Some("Turm A, 3. Stock\n10115 Potsdam")
    );
    let by_kind: Vec<(&str, &str)> = addresses["a1"]
        .components
        .iter()
        .flatten()
        .map(|component| (component.kind.as_str(), component.value.as_str()))
        .collect();
    assert_eq!(
        by_kind,
        vec![("building", "Turm A"), ("floor", "3")],
        "editing the written-out address deleted the components behind it"
    );
}

#[test]
fn clearing_every_adr_field_leaves_the_components_the_line_could_not_state() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // An ADR with all fields empty states nothing, so the save sees the address only on
    // its LABEL, as in the test above. The user can clear what the line stated, but
    // not the floor, which the line did not state.
    fixture.patch(
        &id,
        json!({"addresses": {"a1": {
            "@type": "Address",
            "components": [
                {"@type": "AddressComponent", "kind": "name", "value": "Hauptstraße"},
                {"@type": "AddressComponent", "kind": "locality", "value": "Berlin"},
                {"@type": "AddressComponent", "kind": "floor", "value": "3"},
            ],
            "full": "Hauptstraße, 3. Stock\n10115 Berlin",
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let adr = vcard
        .lines()
        .find(|line| line.starts_with("ADR"))
        .unwrap_or_else(|| panic!("no ADR line: {vcard}"));
    let cleared = format!("{}:;;;;;;", adr.split_once(':').expect("an ADR value").0);
    let edited = vcard.replace(adr, &cleared);
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let addresses = fixture.card(&id).addresses.expect("addresses");
    let by_kind: Vec<(&str, &str)> = addresses["a1"]
        .components
        .iter()
        .flatten()
        .map(|component| (component.kind.as_str(), component.value.as_str()))
        .collect();
    assert_eq!(
        by_kind,
        vec![("floor", "3")],
        "clearing the ADR fields took a component that was never on the line"
    );
}

#[test]
fn a_component_the_line_left_out_for_saying_nothing_survives_an_edit() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // A component of a kind with an ADR field, but with an empty value. The emitter
    // omits it, as it omits an empty entry from a card. Thus its absence is not a
    // field that the user cleared.
    fixture.patch(
        &id,
        json!({"addresses": {"a1": {
            "@type": "Address",
            "components": [
                {"@type": "AddressComponent", "kind": "name", "value": "Hauptstraße"},
                {"@type": "AddressComponent", "kind": "postcode", "value": ""},
                {"@type": "AddressComponent", "kind": "locality", "value": "Berlin"},
            ],
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("ADR;X-JMAP-KEY=a1:;;Hauptstraße;Berlin;;;"),
        "the empty postcode reached the line: {vcard}"
    );

    let edited = vcard.replace("Berlin", "Potsdam");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let addresses = fixture.card(&id).addresses.expect("addresses");
    let by_kind: Vec<(&str, &str)> = addresses["a1"]
        .components
        .iter()
        .flatten()
        .map(|component| (component.kind.as_str(), component.value.as_str()))
        .collect();
    assert_eq!(
        by_kind,
        vec![
            ("name", "Hauptstraße"),
            ("postcode", ""),
            ("locality", "Potsdam")
        ],
        "a component the line never stated was deleted by an edit beside it"
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

    // The address stays: the user cleared only the written-out form.
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
    // RFC 9553 §2.8.3 gives a note a timestamp and an author. A `NOTE` line
    // (RFC 2426 §3.6.2) is plain text with no room for either. They survive only
    // if the patch edits the entry member by member.
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
        notes["n1"]
            .created
            .as_ref()
            .map(|d| d.as_str())
            .or_else(|| notes["n1"].extra.get("created").and_then(|v| v.as_str())),
        Some("2026-02-01T09:15:00Z"),
        "a member the NOTE line cannot carry was overwritten"
    );
    assert_eq!(
        notes["n1"]
            .author
            .as_ref()
            .or_else(|| notes["n1"].extra.get("author")),
        Some(&json!({"@type": "Author", "name": "Vera Oldenburg"}))
    );
}

#[test]
fn a_note_with_no_text_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // A note with no text gets no NOTE line, so the user does not see it.
    // A save must not delete it or give its key to a new note.
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
    // The reader counts only visible entries, so it gives the new entry `n1`,
    // the key of the hidden entry.
    let edited = vcard.replace("END:VCARD\r\n", "NOTE:owes me a beer\r\nEND:VCARD\r\n");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let notes = fixture.card(&id).notes.expect("notes");
    assert_eq!(
        notes["n1"]
            .created
            .as_ref()
            .map(|d| d.as_str())
            .or_else(|| notes["n1"].extra.get("created").and_then(|v| v.as_str())),
        Some("2026-02-01T09:15:00Z"),
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
    // Two notes. The Notes field of Evolution shows only the first, because
    // `E_CONTACT_NOTE` reads only the first `NOTE` line. Clearing that field removes
    // the first note, and the second note must stay.
    fixture.patch(
        &id,
        json!({"notes": {
            "n1": {"@type": "Note", "note": "met at FOSDEM", "created": "2026-02-01T09:15:00Z"},
            "n2": {"@type": "Note", "note": "allergic to cats"},
        }}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    // The line stays with no value. libebook-contacts 3.52 does this with an empty field.
    // This was measured on the spouse line, and on this line by the `unnote` leg of
    // `jmap-functional`. A save that reads an empty note keeps it as empty text.
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
    // One note, so an empty field leaves no note that the mapping can see. This is the
    // other branch of `emptying_one_note_line_of_two_withdraws_that_note_alone`.
    // No key survives to set to null, so the save must remove the property.
    fixture.patch(
        &id,
        json!({"notes": {
            "n1": {"@type": "Note", "note": "met at FOSDEM", "created": "2026-02-01T09:15:00Z"},
        }}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    // The line stays with no value, as libebook-contacts 3.52 does with an empty field.
    // `jmap-functional`'s `clearing_the_only_note_through_eds_withdraws_the_whole_property`
    // measures this shape: its client reports one `NOTE` line with no value.
    // `removing_the_note_line_removes_the_note` has the same result but a different input:
    // a card that EDS does not produce for a cleared field.
    let edited = vcard.replace(
        "NOTE;X-JMAP-KEY=n1:met at FOSDEM\r\n",
        "NOTE;X-JMAP-KEY=n1:\r\n",
    );
    assert_ne!(edited, vcard, "the emitter did not write the note: {vcard}");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    // The property goes, not only its entries. An empty `notes` map is a property that
    // the server keeps without a reason.
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
    // RFC 9553 §2.2.2 gives a nickname a `contexts` and a `pref`. The NICKNAME of
    // RFC 2426 §3.1.3 has no TYPE and no ranking, so the vCard states neither.
    // They survive only if the patch edits the entry member by member.
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
        nicknames["k1"]
            .contexts
            .as_ref()
            .or_else(|| nicknames["k1"].extra.get("contexts")),
        Some(&json!({"private": true})),
        "a member the NICKNAME line cannot carry was overwritten"
    );
    assert_eq!(
        nicknames["k1"].pref.or_else(|| nicknames["k1"]
            .extra
            .get("pref")
            .and_then(|v| v.as_u64().map(|n| n as u32))),
        Some(1)
    );
}

#[test]
fn a_nickname_with_no_name_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // An entry with no name gets no NICKNAME line, so the user does not see it.
    // A save must not delete it or give its key to a new nickname.
    fixture.patch(
        &id,
        json!({"nicknames": {"k1": {"@type": "Nickname", "name": "", "pref": 1}}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("\r\nNICKNAME"), "{vcard}");
    // The reader counts only visible entries, so it gives the new entry `k1`,
    // the key of the hidden entry.
    let edited = vcard.replace("END:VCARD\r\n", "NICKNAME:Vee\r\nEND:VCARD\r\n");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let nicknames = fixture.card(&id).nicknames.expect("nicknames");
    assert_eq!(
        nicknames["k1"].pref.or_else(|| nicknames["k1"]
            .extra
            .get("pref")
            .and_then(|v| v.as_u64().map(|n| n as u32))),
        Some(1),
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
    // RFC 9553 §2.6.3 gives a link a `mediaType`, a `pref` and a `label`. The URL of
    // RFC 2426 §3.6.8 is only a URI, so the vCard states none of them.
    // They survive only if the patch edits the entry member by member.
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

    // EDS rewrites only the value of the line and keeps the parameters, so the key
    // survives. In libebook-contacts 3.52, a set on `E_CONTACT_HOMEPAGE_URL` keeps
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
        links["l1"]
            .media_type
            .as_deref()
            .or_else(|| links["l1"].extra.get("mediaType").and_then(|v| v.as_str())),
        Some("text/html"),
        "a member the URL line cannot carry was overwritten"
    );
    assert_eq!(
        links["l1"].pref.or_else(|| links["l1"]
            .extra
            .get("pref")
            .and_then(|v| v.as_u64().map(|n| n as u32))),
        Some(1)
    );
}

#[test]
fn a_link_for_getting_in_touch_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // The `contact` kind of RFC 9553 §2.6.3 has no vCard 3.0 property, so the entry
    // gets no line and the user does not see it. A save must not delete it or give
    // its key to a new home page.
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
    // The reader counts only visible entries, so it gives a URL line with no
    // parameters the key `l1`, the key of the hidden entry.
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

    // When the user empties the Home Page field, EDS keeps the line with no value.
    // libebook-contacts 3.52 rewrites the value and keeps the attribute (measured).
    // Thus the save must read an empty line as a deletion.
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
    // RFC 9553 §2.4.1 gives a calendar a `mediaType`, a `pref` and a `label`. The
    // CALURI of RFC 6350 §6.9.3 is only a URI, so the vCard states none of them.
    // They survive only if the patch edits the entry member by member.
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

    // EDS rewrites only the value of the line and keeps the parameters, so the key
    // survives. In libebook-contacts 3.52, a set on `E_CONTACT_CALENDAR_URI` keeps
    // the `X-JMAP-KEY`, as a set on `E_CONTACT_HOMEPAGE_URL` does.
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
        calendars["c1"]
            .media_type
            .as_deref()
            .or_else(|| calendars["c1"]
                .extra
                .get("mediaType")
                .and_then(|v| v.as_str())),
        Some("text/calendar"),
        "a member the CALURI line cannot carry was overwritten"
    );
    assert_eq!(
        calendars["c1"].pref.or_else(|| calendars["c1"]
            .extra
            .get("pref")
            .and_then(|v| v.as_u64().map(|n| n as u32))),
        Some(1)
    );
}

#[test]
fn a_calendar_of_no_stated_kind_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // RFC 9553 §2.4.1 makes the kind mandatory. An entry with no kind does not show
    // the line for its URI. Thus it gets no line, and the user does not see it.
    // A save must not delete it or give its key to a new calendar address.
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
    // The reader counts only visible entries, so it gives a CALURI line with no
    // parameters the key `c1`, the key of the hidden entry.
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

    // When the user empties the Free/Busy field, EDS keeps the line with no value.
    // In libebook-contacts 3.52, a set to the empty string keeps the line, and only
    // a set to NULL removes it (measured). Thus the save must read an empty line
    // as a deletion, as it reads a missing line.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace(
        "FBURL;X-JMAP-KEY=c1:https://vera.example/fb",
        "FBURL;X-JMAP-KEY=c1:",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.card(&id).calendars, None);
}

/// The birthday that EDS returns after the user edits it. EDS builds the date line
/// again from `E_CONTACT_BIRTH_DATE`, which removes the `X-JMAP-KEY`. In
/// libebook-contacts 3.52, an unchanged line keeps its parameters and a rewritten
/// line does not (verified).
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
    // A date line has no room for `place` (where the birth happened). Thus `place`
    // survives only if the patch edits the entry and does not replace it. Evolution
    // removes the key, so the save finds the entry by its kind of date.
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
    // RFC 9553 §2.8.1 also allows a Timestamp as the date of an anniversary. The line
    // states only the day, so a save must not read the missing hour as an edit.
    // This also applies when Evolution rewrites the line and removes the key.
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

    // The hour goes too. The user stated a day, and a kept 23:10 adds a time that nobody gave.
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

    // A client that keeps the parameters (as the round trip of this mapping does) moves
    // the date from the birthday field to the anniversary field. Evolution removes the
    // key and causes a delete and an add. In both cases, the date must stop being a birthday.
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
    // A year alone and a deathday: the user sees neither. A save must not delete
    // them or give their keys to a new birthday.
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
fn a_birthday_eds_would_move_to_the_year_1000_is_not_stated_and_not_lost() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Karl der Grosse", "karl@example.com");
    // `e_contact_date_to_string()` CLAMPs the year to 1000..=9999, so the contact editor
    // returns an earlier year as 1000 (measured in `eds-sys/tests/contacts.rs`). If the
    // emitter writes the line, the next save changes 800 to 1000 without the user knowing.
    fixture.patch(
        &id,
        json!({"anniversaries": {"k8": {
            "kind": "birth",
            "date": {"@type": "PartialDate", "year": 800, "month": 6, "day": 21},
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("BDAY"), "{vcard}");

    // The user edits a different field.
    let edited = vcard.replace("karl@example.com", "karl@aachen.example");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let anniversaries = fixture.card(&id).anniversaries.expect("anniversaries");
    assert_eq!(
        anniversaries["k8"].date,
        Some(json!({"@type": "PartialDate", "year": 800, "month": 6, "day": 21})),
        "a date the vCard never stated was rewritten: {anniversaries:?}"
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

// Hidden entries, one property at a time. `emails`, `phones` and `organizations` also
// have entries that the vCard omits (entries with an empty value). The save must treat
// them as it treats hidden `titles`, `addresses` and `notes`.

#[test]
fn an_email_with_no_address_survives_a_save_it_was_never_part_of() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // An entry with no address gets no EMAIL line, so the user does not see it.
    // A save must not delete it or give its key to a new address.
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
    // The reader counts only visible entries, so it gives a new address the key `e1`,
    // the key of the hidden entry.
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
    // A title of a mapped kind, but with no name. The emitter omits it because of its
    // value, which `maps_title_kind` alone does not check.
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
        titles.get("t1").and_then(|title| title
            .organization_id
            .as_deref()
            .or_else(|| title.extra.get("organizationId").and_then(|v| v.as_str()))),
        Some("o1"),
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
    // No name and no unit, so `organization_components` has nothing for an ORG line.
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
        organizations.get("o1").and_then(|organization| organization
            .sort_as
            .as_deref()
            .or_else(|| organization.extra.get("sortAs").and_then(|v| v.as_str()))),
        Some("Oldenburg"),
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

    // The `CATEGORIES` of RFC 2426 §3.6.1 holds all tags on one line. EDS rewrites that
    // line when the user edits the Categories field. No key exists to patch by, so the
    // save replaces the whole set.
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

    // The result is `"keywords": null`, not an empty map. The default of RFC 9553 §2.8.2
    // is no tags, and an empty set is a different value. When the user clears the field,
    // libebook-contacts 3.52 removes the attribute (measured), so the save sees no line.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard.replace("CATEGORIES:hiking\r\n", "");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.card(&id).keywords, None);
}

#[test]
fn a_tag_the_categories_line_cannot_carry_survives_the_set_being_rewritten() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // EDS trims the ends of a category, so the next save renames this tag on the server.
    // Thus the tag gets no line, but the other tags stay on the line. The save replaces
    // the whole set, so it must add the hidden tag again. Its absence is not a deletion.
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
    // An empty field deletes only the tags that it showed. A `null` also deletes the
    // hidden tag, which the user did not see in the field.
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
    // RFC 9553 §1.4.3 requires `true` for every value of a Set, so a `false` is invalid,
    // and the line cannot state it. The save keeps the server value unchanged. A
    // correction is a change that nobody made.
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
    // Here both sides name the same tag, and the user wins. The Categories field
    // states only "filed under", so a typed tag is set, whatever the server value was.
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
    // The save adds the hidden tag again, so its set equals the server set. A patch
    // that names the property can undo a concurrent edit on another client. Thus the
    // patch must not name the property at all.
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

    // The patch must not name the property, or it can undo a concurrent edit on
    // another client. This assertion checks only the result.
    assert_eq!(
        fixture.card(&id).keywords.expect("keywords"),
        [("hiking".to_owned(), json!(true))].into()
    );
}

#[test]
fn an_empty_set_on_the_server_is_not_an_edit_waiting_to_happen() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // An empty `keywords` and no `keywords` give the same vCard (no `CATEGORIES` line),
    // so the two must compare as equal. If not, each save sets the property to null.
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

    // The handle goes on the `X-JABBER` line with its key, so an edit patches that
    // entry and does not replace the property. The line has no parameter for `pref`,
    // so `pref` stays unchanged.
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
    assert_eq!(
        services["s1"].pref.or_else(|| services["s1"]
            .extra
            .get("pref")
            .and_then(|v| v.as_u64().map(|n| n as u32))),
        Some(1)
    );
}

#[test]
fn renaming_a_handle_drops_the_uri_that_named_the_old_one() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // An entry can state a handle and a URI, but only the handle has a line. After a
    // rename, the URI names the old handle. A new URI needs the URI scheme of the
    // service, which the mapping does not guess. Thus the URI goes, as the `sortAs` of
    // a renamed unit goes.
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
    // The server states the entry only as a URI. The line shows the handle from the
    // URI, so the user can edit it, and the save must write the edit into the URI.
    // A `user` changes the shape of the entry without a reason. The URI scheme gives
    // the way to read the handle and also the way to write it.
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
    // The line states a handle, so the card from the reader has a `user` where the
    // server has a `uri`. The save must compare the handles, not the members. If not,
    // each save patches the entry and the server state changes.
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

    // An edit of a different field keeps the entry in the server shape.
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
    // The field is free text, so the typed handle can be invalid in a URI. A URI with
    // a space is an identifier that no service can resolve. Thus the handle goes into
    // `user`, and the old URI goes, as for a rename of an entry that states both.
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

    // Only a rename removes the URI. An edit of a different field must not change
    // the URI, the handle or the service.
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
    // RFC 9553 §2.3.2 allows any capitalisation of a service name and compares names
    // case-insensitively. The `X-JABBER` line states no spelling, so the reader uses
    // the spelling of its table. A save must not write that spelling to the server.
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
fn clearing_the_last_im_handle_leaves_the_contact_on_no_service() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"onlineServices": {"s1": {"service": "Jabber", "user": "vera@jabber.example"}}}),
    );
    let sync = fixture.sync();

    // EDS removes the attribute when the user clears the field, so the save sees no
    // line. All entries of the card were visible, so the whole property goes.
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

    // Evolution does not produce this: a service change clears one field and sets
    // another, which loses the key. Another client can produce it, and then the entry
    // is at a different service. The line states the service, so the save follows it.
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

/// "hello-photo", in place of the JPEG of a real card.
const PHOTO: &str = "aGVsbG8tcGhvdG8=";
/// What EDS writes for a new picture from the user. It has no `X-JMAP-KEY`, because
/// `e_contact_set` builds the line again from its photo (measured on libebook-contacts 3.52).
const CHOSEN: &str = "PHOTO;TYPE=png;ENCODING=b:bmV3LXBob3RvISE=";

/// A card with a picture on the server, and with members that a `PHOTO` line cannot carry.
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

    // The editor writes the line without the key. The save must patch the entry of
    // the line that the new line replaces.
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
        media["m1"].pref.or_else(|| media["m1"]
            .extra
            .get("pref")
            .and_then(|v| v.as_u64().map(|n| n as u32))),
        Some(1),
        "a member the PHOTO line cannot carry was overwritten: {media:?}"
    );
}

#[test]
fn a_photo_nobody_touched_is_not_written_back() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // A `data:` URI without padding, as hand-written URIs often are (RFC 4648 §3.2 makes
    // padding optional). The glib base64 decoder gives the line the canonical spelling.
    // Thus the returned URI differs from the server URI, and a text compare sees an edit.
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

    // In libebook-contacts 3.52, clearing the photo removes the attribute (measured),
    // so the save sees no line.
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
    // In libebook-contacts 3.52 (measured), `E_CONTACT_PHOTO` reads the first `PHOTO` line,
    // and a `set` replaces only that line. The other lines keep their parameters. Thus
    // with two pictures, only the first changes, and the second keeps its key.
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
    // RFC 9553 §2.6.4 keeps all three kinds of media in one map. Evolution shows only
    // a photo, so the logo gets no line. The key that the reader makes for the photo
    // line is the key of the logo.
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

/// The spouse line that EDS returns after the user types in the field.
/// `e_contact_set(E_CONTACT_SPOUSE, …)` rewrites only the value of the first line
/// with that name. A cleared field gives the empty name. A set to the empty string
/// keeps the line with no value, and only a set to NULL removes the line. This was
/// measured on libebook-contacts 3.52, as for the Free/Busy field.
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
    // RFC 9553 §2.1.8 uses the related entity as the key of `relatedTo`, so the name on
    // the line is the key. A changed name is a different entry. The line stated only
    // the marriage, so the hidden `kin` stays on the old entry.
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
    // The usual case: the entry states only the marriage, so the whole entry goes.
    // The second spouse uses the `uid` of the related Card, as §2.1.8 asks. A URN under
    // the "Spouse" heading is not a person, so it gets no line, and the save must keep it.
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
    // The `@type` tag states the object type, not a relation. Thus an entry with
    // `@type` and the marriage is empty when the marriage goes, and the entry goes too.
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

    // The property goes, not only its entries: that is how RFC 9553 §2.1.8 states
    // the default of no relations.
    assert_eq!(fixture.card(&id).related_to, None);
}

#[test]
fn clearing_the_spouse_field_keeps_a_relation_the_line_never_showed() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // Evolution has fields only for the spouse, the manager and the assistant. The user
    // does not see a child, so an empty Spouse field does not change it.
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
    // The key is the person, so this is a merge, not an addition. A typed name that
    // the card already has adds a relation to that entry, not a second entry.
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

    // RFC 8620 §5.3 requires each path segment before the last to exist. A card with
    // no relations has no `relatedTo`, so the save writes the whole property. (This mock
    // creates missing intermediate objects; a server that obeys §5.3 does not.)
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
    // The key of `relatedTo` is a person's name, and `/` and `~` have a meaning in a patch
    // path (RFC 6901 §3). Without escapes, the removal goes to a wrong object, and the new
    // spouse goes under a name split in two.
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

/// The manager line that EDS returns after the user types in the field.
fn as_evolution_retypes_the_manager(vcard: &str, name: &str) -> String {
    let mut rewritten = false;
    let rebuilt: String = vcard
        .lines()
        .map(
            |line| match !rewritten && line.starts_with("X-EVOLUTION-MANAGER") {
                true => {
                    rewritten = true;
                    format!("X-EVOLUTION-MANAGER:{name}\r\n")
                }
                false => format!("{line}\r\n"),
            },
        )
        .collect();
    assert!(rewritten, "no manager line to rewrite in\n{vcard}");
    rebuilt
}

/// The assistant line that EDS returns after the user types in the field.
fn as_evolution_retypes_the_assistant(vcard: &str, name: &str) -> String {
    let mut rewritten = false;
    let rebuilt: String = vcard
        .lines()
        .map(
            |line| match !rewritten && line.starts_with("X-EVOLUTION-ASSISTANT") {
                true => {
                    rewritten = true;
                    format!("X-EVOLUTION-ASSISTANT:{name}\r\n")
                }
                false => format!("{line}\r\n"),
            },
        )
        .collect();
    assert!(rewritten, "no assistant line to rewrite in\n{vcard}");
    rebuilt
}

#[test]
fn retyping_a_manager_withdraws_manager_and_keeps_what_else_was_said() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"relatedTo": {"Sarah Connor": {
            "@type": "Relation",
            "relation": {"manager": true, "kin": true},
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("X-EVOLUTION-MANAGER:Sarah Connor"),
        "{vcard}"
    );

    let edited = as_evolution_retypes_the_manager(&vcard, "Miles Dyson");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let related = fixture.card(&id).related_to.expect("relatedTo");
    assert_eq!(
        related.keys().collect::<Vec<_>>(),
        vec!["Miles Dyson", "Sarah Connor"],
        "{related:?}"
    );
    assert_eq!(
        related["Sarah Connor"].relation,
        Some([("kin".to_owned(), json!(true))].into()),
        "the relation the line never showed went with the manager: {related:?}"
    );
    assert_eq!(
        related["Miles Dyson"].relation,
        Some([("manager".to_owned(), json!(true))].into()),
        "the name the user typed is a manager and nothing more: {related:?}"
    );
}

#[test]
fn retyping_a_manager_who_was_nothing_else_leaves_no_entry_behind() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"relatedTo": {
            "Sarah Connor": {"relation": {"manager": true}},
            "urn:uuid:e1f0a1c2-0f6b-4d2e-9c3a-2b1f9d0e7c44": {"relation": {"manager": true}},
        }}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert_eq!(vcard.matches("X-EVOLUTION-MANAGER").count(), 1, "{vcard}");

    let edited = as_evolution_retypes_the_manager(&vcard, "Miles Dyson");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let related = fixture.card(&id).related_to.expect("relatedTo");
    assert_eq!(
        related.keys().collect::<Vec<_>>(),
        vec![
            "Miles Dyson",
            "urn:uuid:e1f0a1c2-0f6b-4d2e-9c3a-2b1f9d0e7c44"
        ],
        "{related:?}"
    );
    assert_eq!(
        related["urn:uuid:e1f0a1c2-0f6b-4d2e-9c3a-2b1f9d0e7c44"].relation,
        Some([("manager".to_owned(), json!(true))].into()),
        "an entry the vCard never showed was rewritten: {related:?}"
    );
}

#[test]
fn clearing_the_manager_field_removes_the_relation() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"relatedTo": {"Sarah Connor": {
            "@type": "Relation",
            "relation": {"manager": true},
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = as_evolution_retypes_the_manager(&vcard, "");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.card(&id).related_to, None);
}

#[test]
fn retyping_an_assistant_withdraws_assistant_and_keeps_what_else_was_said() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"relatedTo": {"John Connor": {
            "@type": "Relation",
            "relation": {"assistant": true, "kin": true},
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("X-EVOLUTION-ASSISTANT:John Connor"),
        "{vcard}"
    );

    let edited = as_evolution_retypes_the_assistant(&vcard, "Kyle Reese");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let related = fixture.card(&id).related_to.expect("relatedTo");
    assert_eq!(
        related.keys().collect::<Vec<_>>(),
        vec!["John Connor", "Kyle Reese"],
        "{related:?}"
    );
    assert_eq!(
        related["John Connor"].relation,
        Some([("kin".to_owned(), json!(true))].into())
    );
    assert_eq!(
        related["Kyle Reese"].relation,
        Some([("assistant".to_owned(), json!(true))].into())
    );
}

#[test]
fn clearing_the_assistant_field_removes_the_relation() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"relatedTo": {"John Connor": {
            "@type": "Relation",
            "relation": {"assistant": true},
        }}}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = as_evolution_retypes_the_assistant(&vcard, "");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.card(&id).related_to, None);
}

#[test]
fn a_card_with_spouse_manager_and_assistant_can_edit_each_independently() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "relatedTo": {
                "Alex Spouse": {"@type": "Relation", "relation": {"spouse": true}},
                "Morgan Manager": {"@type": "Relation", "relation": {"manager": true}},
                "Sam Assistant": {"@type": "Relation", "relation": {"assistant": true}},
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(vcard.contains("X-EVOLUTION-SPOUSE:Alex Spouse"));
    assert!(vcard.contains("X-EVOLUTION-MANAGER:Morgan Manager"));
    assert!(vcard.contains("X-EVOLUTION-ASSISTANT:Sam Assistant"));

    // The user edits only the manager.
    let edited = vcard.replace("Morgan Manager", "Taylor Manager");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    let rels = card.related_to.expect("relatedTo");
    assert_eq!(rels.len(), 3);
    assert_eq!(
        rels["Alex Spouse"].relation,
        Some([("spouse".to_owned(), json!(true))].into())
    );
    assert_eq!(
        rels["Taylor Manager"].relation,
        Some([("manager".to_owned(), json!(true))].into())
    );
    assert_eq!(
        rels["Sam Assistant"].relation,
        Some([("assistant".to_owned(), json!(true))].into())
    );
}

#[test]
fn unmapped_or_unslotted_services_are_preserved_across_saves() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    // A mapped service (Jabber) and two services without an EDS field (Twitter, SIP).
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

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:vera@jabber.example"),
        "{vcard}"
    );
    assert!(!vcard.contains("vera_tw"), "{vcard}");
    assert!(!vcard.contains("sip:vera@example.com"), "{vcard}");

    let edited = vcard.replace("vera@jabber.example", "vera@xmpp.example");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

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

    // The vCard states none of these action or query URIs on an X- line.
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(!vcard.contains("X-AIM"), "{vcard}");
    assert!(!vcard.contains("X-MSN"), "{vcard}");
    assert!(!vcard.contains("X-YAHOO"), "{vcard}");
    assert!(!vcard.contains("X-ICQ"), "{vcard}");

    // An edit of a different field keeps all four online services.
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

    // Edit the wedding anniversary and add a note.
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

    // Edit the first ORG and the first TITLE.
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
        addresses["a1"]
            .coordinates
            .as_deref()
            .or_else(|| addresses["a1"]
                .extra
                .get("coordinates")
                .and_then(|v| v.as_str())),
        Some("geo:52.5,13.4")
    );

    assert_eq!(
        addresses["a2"].full.as_deref(),
        Some("Heimweg 2\n80331 München\nGermany")
    );
    assert_eq!(
        addresses["a2"].pref.or_else(|| addresses["a2"]
            .extra
            .get("pref")
            .and_then(|v| v.as_u64().map(|n| n as u32))),
        Some(1)
    );

    assert_eq!(
        addresses["a3"].full.as_deref(),
        Some("Postfach 42\n20095 Hamburg")
    );
    assert_eq!(
        addresses["a3"]
            .time_zone
            .as_deref()
            .or_else(|| addresses["a3"]
                .extra
                .get("timeZone")
                .and_then(|v| v.as_str())),
        Some("Europe/Berlin")
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
    // EDS sets a new label on the work address.
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

    // An edit of a different field keeps all five online services.
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
    // Edit s1 (Jabber HOME) and s4 (Skype HOME).
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

#[test]
fn replacing_inlined_photo_with_uri_photo_and_preserving_unmodeled_logo() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"media": {
            "m1": {"@type": "Media", "kind": "photo",
                   "uri": format!("data:image/jpeg;base64,{PHOTO}"),
                   "mediaType": "image/jpeg"},
            "l1": {"@type": "Media", "kind": "logo",
                   "uri": "https://example.com/corporate-logo.png"},
        }}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains(&format!("PHOTO;X-JMAP-KEY=m1;TYPE=jpeg;ENCODING=b:{PHOTO}")),
        "{vcard}"
    );
    assert!(!vcard.contains("LOGO"), "{vcard}");

    let edited = vcard.replace(
        &format!("PHOTO;X-JMAP-KEY=m1;TYPE=jpeg;ENCODING=b:{PHOTO}"),
        "PHOTO;VALUE=uri:https://example.com/new_avatar.png",
    );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    let media = card.media.expect("media");
    assert_eq!(media.len(), 2, "{media:?}");
    assert_eq!(media["m1"].kind.as_deref(), Some("photo"));
    assert_eq!(media["m1"].uri, "https://example.com/new_avatar.png");
    assert_eq!(media["m1"].media_type, None);
    assert_eq!(media["l1"].kind.as_deref(), Some("logo"));
    assert_eq!(media["l1"].uri, "https://example.com/corporate-logo.png");
}

#[test]
fn clearing_photo_preserves_unmodeled_logo_and_sound_media() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({"media": {
            "m1": {"@type": "Media", "kind": "photo",
                   "uri": format!("data:image/jpeg;base64,{PHOTO}")},
            "l1": {"@type": "Media", "kind": "logo",
                   "uri": "https://example.com/logo.png"},
            "s1": {"@type": "Media", "kind": "sound",
                   "uri": "https://example.com/name-audio.ogg"},
        }}),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let lines: Vec<&str> = vcard
        .lines()
        .filter(|line| !line.starts_with("PHOTO"))
        .collect();
    let edited = lines.join("\r\n") + "\r\n";
    assert!(!edited.contains("PHOTO"), "{edited}");

    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    let media = card.media.expect("media");
    assert_eq!(media.len(), 2, "{media:?}");
    assert!(
        !media.contains_key("m1"),
        "photo m1 should be deleted: {media:?}"
    );
    assert_eq!(media["l1"].kind.as_deref(), Some("logo"));
    assert_eq!(media["l1"].uri, "https://example.com/logo.png");
    assert_eq!(media["s1"].kind.as_deref(), Some("sound"));
    assert_eq!(media["s1"].uri, "https://example.com/name-audio.ogg");
}

#[test]
fn editing_notes_and_links_preserves_unmodeled_cards_and_properties() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "notes": {
                "n1": {
                    "@type": "Note",
                    "note": "Original note text",
                    "created": "2026-08-01T12:00:00Z"
                }
            },
            "links": {
                "l1": {
                    "@type": "Link",
                    "uri": "https://example.com/old-website",
                    "pref": 1
                }
            },
            "preferredLanguages": {
                "de": {"@type": "LanguagePref", "pref": 1}
            },
            "onlineServices": {
                "os1": {
                    "@type": "OnlineService",
                    "service": "Matrix",
                    "user": "@vera:matrix.org"
                }
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard
        .replace("Original note text", "Updated note with new details")
        .replace(
            "https://example.com/old-website",
            "https://example.com/new-website",
        );
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    let notes = card.notes.expect("notes");
    assert_eq!(notes["n1"].note, "Updated note with new details");
    assert_eq!(
        notes["n1"]
            .created
            .as_ref()
            .map(|d| d.as_str())
            .or_else(|| notes["n1"].extra.get("created").and_then(|v| v.as_str())),
        Some("2026-08-01T12:00:00Z"),
        "created timestamp should be preserved in extra"
    );

    let links = card.links.expect("links");
    assert_eq!(links["l1"].uri, "https://example.com/new-website");

    // The unmapped `preferredLanguages` and the `onlineServices` stay.
    assert!(card.preferred_languages.is_some() || card.extra.contains_key("preferredLanguages"));

    let services = card.online_services.expect("onlineServices");
    assert_eq!(services["os1"].user.as_deref(), Some("@vera:matrix.org"));
}

#[test]
fn editing_calendars_nicknames_and_spouse_preserves_unmodeled_contact_fields() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "calendars": {
                "c1": {"@type": "Calendar", "kind": "calendar", "uri": "https://example.com/old-cal.ics"},
                "f1": {"@type": "Calendar", "kind": "freeBusy", "uri": "https://example.com/old-fb.ifb"}
            },
            "nicknames": {
                "k1": {"@type": "Nickname", "name": "OldNick"}
            },
            "relatedTo": {
                "Alex Olden": {"@type": "Relation", "relation": {"spouse": true}}
            },
            "cryptoKeys": {
                "key1": {"@type": "CryptoKey", "uri": "https://example.com/pgp.asc"}
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard
        .replace(
            "https://example.com/old-cal.ics",
            "https://example.com/new-cal.ics",
        )
        .replace(
            "https://example.com/old-fb.ifb",
            "https://example.com/new-fb.ifb",
        )
        .replace("OldNick", "NewNick")
        .replace("Alex Olden", "Taylor Olden");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    let cals = card.calendars.expect("calendars");
    assert_eq!(cals["c1"].uri, "https://example.com/new-cal.ics");
    assert_eq!(cals["f1"].uri, "https://example.com/new-fb.ifb");

    let nicks = card.nicknames.expect("nicknames");
    assert_eq!(nicks["k1"].name, "NewNick");

    let rels = card.related_to.expect("relatedTo");
    assert!(rels.contains_key("Taylor Olden"));

    // The `cryptoKeys` stay.
    let crypto = card.crypto_keys.as_ref().expect("cryptoKeys");
    assert!(crypto.contains_key("key1"));
}

#[test]
fn clearing_categories_and_notes_patches_server_fields() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "keywords": {"Engineering": true, "Rust": true},
            "notes": {
                "n1": {"@type": "Note", "note": "To be removed"}
            },
            "phones": {
                "p1": {"@type": "Phone", "number": "+41221234567"}
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let lines: Vec<&str> = vcard
        .lines()
        .filter(|l| !l.starts_with("CATEGORIES") && !l.starts_with("NOTE"))
        .collect();
    let edited = lines.join("\r\n") + "\r\n";

    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    assert_eq!(card.keywords, None, "keywords should be removed: {card:?}");
    assert_eq!(card.notes, None, "notes should be removed: {card:?}");

    // The emails and phones stay.
    let emails = card.emails.expect("emails");
    assert_eq!(
        emails.values().next().map(|e| e.address.as_str()),
        Some("vera@example.com")
    );
    let phones = card.phones.expect("phones");
    assert_eq!(phones["p1"].number, "+41221234567");
}

#[test]
fn editing_phones_and_emails_preserves_unmodeled_contact_fields() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "emails": {
                "e1": {"@type": "EmailAddress", "address": "vera.work@example.com", "contexts": {"work": true}, "pref": 1},
                "e2": {"@type": "EmailAddress", "address": "vera.home@example.com", "contexts": {"private": true}}
            },
            "phones": {
                "p1": {"@type": "Phone", "number": "+49 30 111111", "contexts": {"work": true}, "features": {"voice": true}},
                "p2": {"@type": "Phone", "number": "+49 30 222222", "contexts": {"private": true}, "features": {"voice": true}}
            },
            "preferredLanguages": {
                "de": {"@type": "LanguagePref", "pref": 1}
            },
            "personalInfo": {
                "expert": {"@type": "PersonalInfo", "kind": "expertise", "value": "Rust"}
            },
            "cryptoKeys": {
                "key1": {"@type": "CryptoKey", "uri": "https://example.com/key.asc"}
            },
            "onlineServices": {
                "chat1": {"@type": "OnlineService", "service": "Matrix", "uri": "matrix:u/vera:matrix.org"}
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;

    // Change the e1 email address and the p2 phone number.
    let modified = vcard
        .replace(
            "EMAIL;X-JMAP-KEY=e1;TYPE=WORK,PREF:vera.work@example.com",
            "EMAIL;X-JMAP-KEY=e1;TYPE=WORK,PREF:vera.newwork@example.com",
        )
        .replace(
            "EMAIL;TYPE=WORK,PREF;X-JMAP-KEY=e1:vera.work@example.com",
            "EMAIL;TYPE=WORK,PREF;X-JMAP-KEY=e1:vera.newwork@example.com",
        )
        .replace(
            "TEL;X-JMAP-KEY=p2;TYPE=HOME,VOICE:+49 30 222222",
            "TEL;X-JMAP-KEY=p2;TYPE=HOME,VOICE:+49 30 999999",
        )
        .replace(
            "TEL;TYPE=HOME,VOICE;X-JMAP-KEY=p2:+49 30 222222",
            "TEL;TYPE=HOME,VOICE;X-JMAP-KEY=p2:+49 30 999999",
        );

    sync.save_contact(&modified, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    let emails = card.emails.expect("emails");
    assert_eq!(emails["e1"].address, "vera.newwork@example.com");
    assert_eq!(emails["e1"].pref, Some(1));
    assert_eq!(emails["e2"].address, "vera.home@example.com");

    let phones = card.phones.expect("phones");
    assert_eq!(phones["p1"].number, "+49 30 111111");
    assert_eq!(phones["p2"].number, "+49 30 999999");

    // The unmapped properties stay.
    assert!(card.preferred_languages.is_some() || card.extra.contains_key("preferredLanguages"));
    assert!(card.personal_info.as_ref().unwrap().contains_key("expert"));
    assert!(card.crypto_keys.as_ref().unwrap().contains_key("key1"));
    assert!(card.online_services.as_ref().unwrap().contains_key("chat1"));
}

#[test]
fn clearing_phones_and_emails_patches_server_fields() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "emails": {
                "e1": {"@type": "EmailAddress", "address": "vera.work@example.com"}
            },
            "phones": {
                "p1": {"@type": "Phone", "number": "+49 30 111111"}
            },
            "nicknames": {
                "k1": {"@type": "Nickname", "name": "Vee"}
            },
            "notes": {
                "n1": {"@type": "Note", "note": "Persistent note"}
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let lines: Vec<&str> = vcard
        .lines()
        .filter(|l| !l.starts_with("TEL") && !l.starts_with("EMAIL"))
        .collect();
    let edited = lines.join("\r\n") + "\r\n";

    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    assert_eq!(card.emails, None, "emails should be cleared: {card:?}");
    assert_eq!(card.phones, None, "phones should be cleared: {card:?}");

    let nicks = card.nicknames.expect("nicknames");
    assert_eq!(nicks["k1"].name, "Vee");
    let notes = card.notes.expect("notes");
    assert_eq!(notes["n1"].note, "Persistent note");
}

#[test]
fn editing_structured_name_components_preserves_unmodeled_crypto_and_personal_info() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Dr. Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "name": {
                "@type": "Name",
                "full": "Dr. Vera Oldenburg",
                "components": [
                    {"@type": "NameComponent", "kind": "title", "value": "Dr."},
                    {"@type": "NameComponent", "kind": "given", "value": "Vera"},
                    {"@type": "NameComponent", "kind": "surname", "value": "Oldenburg"}
                ]
            },
            "cryptoKeys": {
                "k1": {"uri": "https://keys.example.com/openpgp.asc"}
            },
            "personalInfo": {
                "pi1": {"kind": "expertise", "value": "Rust"}
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let modified = vcard
        .replace(
            "N:Oldenburg;Vera;;Dr.;",
            "N:Oldenburg-Smith;Vera;Marie;Prof. Dr.;MSc",
        )
        .replace(
            "FN:Dr. Vera Oldenburg",
            "FN:Prof. Dr. Vera Marie Oldenburg-Smith MSc",
        );

    sync.save_contact(&modified, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    let name = card.name.expect("name");
    assert_eq!(
        name.full.as_deref(),
        Some("Prof. Dr. Vera Marie Oldenburg-Smith MSc")
    );
    let comps = name.components.expect("components");
    assert_eq!(comps.len(), 5);
    let find_comp = |k: &str| comps.iter().find(|c| c.kind == k).map(|c| c.value.as_str());
    assert_eq!(find_comp("title"), Some("Prof. Dr."));
    assert_eq!(find_comp("given"), Some("Vera"));
    assert_eq!(find_comp("given2"), Some("Marie"));
    assert_eq!(find_comp("surname"), Some("Oldenburg-Smith"));
    assert_eq!(find_comp("credential"), Some("MSc"));

    // The unmapped properties stay.
    assert!(card.crypto_keys.is_some() || card.extra.contains_key("cryptoKeys"));
    assert!(card.personal_info.is_some() || card.extra.contains_key("personalInfo"));
}

#[test]
fn editing_multiple_addresses_preserves_unmodeled_contact_fields() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "addresses": {
                "a1": {
                    "contexts": {"work": true},
                    "components": [
                        {"kind": "name", "value": "Hauptstraße 1"},
                        {"kind": "locality", "value": "Berlin"},
                        {"kind": "postcode", "value": "10115"},
                        {"kind": "country", "value": "Germany"},
                    ],
                    "full": "Hauptstraße 1\n10115 Berlin\nGermany",
                },
                "a2": {
                    "contexts": {"private": true},
                    "components": [
                        {"kind": "name", "value": "Heimweg 2"},
                        {"kind": "locality", "value": "München"},
                        {"kind": "region", "value": "Bayern"},
                        {"kind": "postcode", "value": "80331"},
                        {"kind": "country", "value": "Germany"},
                    ],
                    "full": "Heimweg 2\n80331 München\nGermany",
                },
            },
            "preferredLanguages": {"de": 1, "en": 2},
            "cryptoKeys": {"k1": {"uri": "https://keys.example.com/pkr.asc"}},
            "personalInfo": {"pi1": {"kind": "expertise", "value": "Rust"}},
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let modified = vcard
        .replace(
            ";;Hauptstraße 1;Berlin;;10115;Germany",
            ";;Unter den Linden 5;Berlin;;10117;Germany",
        )
        .replace(
            "Hauptstraße 1\\n10115 Berlin\\nGermany",
            "Unter den Linden 5\\n10117 Berlin\\nGermany",
        )
        .replace(
            ";;Heimweg 2;München;Bayern;80331;Germany",
            ";;Heimweg 99;München;Bayern;80331;Germany",
        )
        .replace(
            "Heimweg 2\\n80331 München\\nGermany",
            "Heimweg 99\\n80331 München\\nGermany",
        );

    sync.save_contact(&modified, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    let addresses = card.addresses.expect("addresses");
    assert_eq!(addresses.len(), 2);

    let a1 = &addresses["a1"];
    assert_eq!(
        a1.full.as_deref(),
        Some("Unter den Linden 5\n10117 Berlin\nGermany")
    );
    let a1_comps = a1.components.as_ref().expect("a1 components");
    assert!(
        a1_comps
            .iter()
            .any(|c| c.kind == "name" && c.value == "Unter den Linden 5")
    );
    assert!(
        a1_comps
            .iter()
            .any(|c| c.kind == "postcode" && c.value == "10117")
    );

    let a2 = &addresses["a2"];
    assert_eq!(
        a2.full.as_deref(),
        Some("Heimweg 99\n80331 München\nGermany")
    );
    let a2_comps = a2.components.as_ref().expect("a2 components");
    assert!(
        a2_comps
            .iter()
            .any(|c| c.kind == "name" && c.value == "Heimweg 99")
    );

    // The unmapped properties stay.
    assert!(card.preferred_languages.is_some() || card.extra.contains_key("preferredLanguages"));
    assert!(card.crypto_keys.is_some() || card.extra.contains_key("cryptoKeys"));
    assert!(card.personal_info.is_some() || card.extra.contains_key("personalInfo"));
}

#[test]
fn clearing_all_addresses_patches_server_fields() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "addresses": {
                "a1": {
                    "contexts": {"work": true},
                    "components": [
                        {"kind": "name", "value": "Hauptstraße 1"},
                        {"kind": "locality", "value": "Berlin"},
                    ],
                },
                "a2": {
                    "contexts": {"private": true},
                    "components": [
                        {"kind": "name", "value": "Heimweg 2"},
                        {"kind": "locality", "value": "München"},
                    ],
                },
            },
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let modified: String = vcard
        .lines()
        .filter(|l| !l.starts_with("ADR") && !l.starts_with("LABEL"))
        .map(|l| format!("{l}\r\n"))
        .collect();

    sync.save_contact(&modified, Some(id.as_str())).unwrap();

    let card = fixture.card(&id);
    assert_eq!(card.addresses, None);
    assert!(card.name.is_some());
    assert!(card.emails.is_some());
}

#[test]
fn no_op_save_mints_no_patch_across_all_contact_mapped_surfaces_matrix() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    // The cases cover:
    // - media/photo (data URI and remote URL)
    // - cryptoKeys and unknown properties
    // - links (contact and custom)
    // - labels and multi-component addresses with contexts
    // - categories / keywords (with a line and without a line)
    // - relations / relatedTo (spouses and unmapped kin)
    // - phones (multiple context slots, features, pref)
    // - emails (contexts, pref)
    // - organizations (empty employer name with units, named employer with units, sortAs)
    // - titles and roles
    // - notes
    // - nicknames
    // - anniversaries (birthdays, wedding anniversaries, timestamps)
    // - onlineServices (Jabber, Matrix, custom URIs)
    let cases = vec![
        (
            "composite_full_card",
            json!({
                "name": {
                    "full": "Dr. Vera Elisabeth von Oldenburg",
                    "components": [
                        {"kind": "prefix", "value": "Dr."},
                        {"kind": "given", "value": "Vera"},
                        {"kind": "given2", "value": "Elisabeth"},
                        {"kind": "surname", "value": "Oldenburg"},
                        {"kind": "surname2", "value": "von"},
                    ]
                },
                "nicknames": {
                    "n1": {"name": "Vee", "contexts": {"private": true}, "pref": 1}
                },
                "emails": {
                    "e1": {"address": "vera@example.com", "contexts": {"work": true}, "pref": 1},
                    "e2": {"address": "vera.home@example.org", "contexts": {"private": true}}
                },
                "phones": {
                    "p1": {"number": "+49 30 111", "contexts": {"work": true}, "features": {"voice": true}, "pref": 1},
                    "p2": {"number": "+49 170 222", "contexts": {"private": true}, "features": {"cell": true, "voice": true}}
                },
                "organizations": {
                    "o1": {
                        "name": "Acme Corp",
                        "units": [{"name": "Engineering"}, {"name": "Platform"}],
                        "sortAs": "Acme"
                    }
                },
                "titles": {
                    "t1": {"name": "Principal Architect", "kind": "title"},
                    "t2": {"name": "Security Lead", "kind": "role"}
                },
                "addresses": {
                    "a1": {
                        "components": [
                            {"kind": "name", "value": "Unter den Linden 1"},
                            {"kind": "locality", "value": "Berlin"},
                            {"kind": "postcode", "value": "10117"},
                            {"kind": "country", "value": "Germany"}
                        ],
                        "full": "Unter den Linden 1\n10117 Berlin\nGermany",
                        "contexts": {"work": true}
                    }
                },
                "notes": {
                    "not1": {"note": "Key contact for protocol engineering."}
                },
                "anniversaries": {
                    "y1": {"kind": "birth", "date": {"year": 1988, "month": 5, "day": 12}},
                    "y2": {"kind": "wedding", "date": {"year": 2016, "month": 8, "day": 20}}
                },
                "links": {
                    "l1": {"uri": "https://example.com/vera", "kind": "contact"}
                },
                "calendars": {
                    "c1": {"uri": "https://cal.example.com/vera", "kind": "calendar"},
                    "c2": {"uri": "https://cal.example.com/fb/vera", "kind": "freeBusy"}
                },
                "media": {
                    "m1": {
                        "kind": "photo",
                        "uri": "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAP//////////////////////////////////////////////////////////////////////////////////////wgALCAABAAEBAREA/8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABPxA=",
                        "mediaType": "image/jpeg"
                    }
                },
                "onlineServices": {
                    "os1": {"service": "Jabber", "user": "vera@jabber.example.org"},
                    "os2": {"service": "Matrix", "user": "@vera:matrix.example.org"}
                },
                "relatedTo": {
                    "Max Oldenburg": {"relation": {"spouse": true, "kin": true}}
                },
                "keywords": {
                    "Colleague": true,
                    "VIP": true,
                    "Internal": false
                },
                "preferredLanguages": {"de": 1, "en": 2},
                "cryptoKeys": {"k1": {"uri": "https://keys.example.com/vera.asc"}},
                "customBags": {"team": "Architecture"}
            }),
        ),
        (
            "empty_org_name_with_units",
            json!({
                "name": {"full": "Nameless Org Employee"},
                "organizations": {
                    "o1": {
                        "name": "",
                        "units": [{"name": "Platform Infrastructure"}]
                    }
                },
                "emails": {"e1": {"address": "infra@example.org"}}
            }),
        ),
        (
            "timestamp_anniversaries_and_remote_photo",
            json!({
                "name": {"full": "Point In Time Contact"},
                "anniversaries": {
                    "y1": {"kind": "birth", "date": {"utc": "1992-11-04T14:30:00Z"}},
                    "y2": {"kind": "wedding", "date": {"utc": "2020-06-18T00:00:00Z"}}
                },
                "media": {
                    "m1": {
                        "kind": "photo",
                        "uri": "https://cdn.example.com/avatars/contact.jpg",
                        "mediaType": "image/jpeg"
                    }
                },
                "emails": {"e1": {"address": "timestamp@example.org"}}
            }),
        ),
        (
            "unmapped_contexts_and_features_with_pref",
            json!({
                "name": {"full": "Pref and Context Contact"},
                "emails": {
                    "e1": {"address": "pref@example.com", "contexts": {"work": true, "school": true}, "pref": 42}
                },
                "phones": {
                    "p1": {
                        "number": "+1 555 0100",
                        "contexts": {"work": true, "school": true},
                        "features": {"voice": true, "video": true},
                        "pref": 10
                    }
                }
            }),
        ),
        (
            "label_only_address_and_impp_online_services",
            json!({
                "name": {"full": "Label Only Address"},
                "addresses": {
                    "a1": {
                        "full": "Post Office Box 123\n10001 New York\nUSA",
                        "contexts": {"work": true}
                    }
                },
                "onlineServices": {
                    "os1": {"uri": "xmpp:alice@example.com"},
                    "os2": {"uri": "sip:alice@example.com"}
                }
            }),
        ),
        (
            "multi_relations_and_unmapped",
            json!({
                "name": {"full": "Relations Matrix Contact"},
                "relatedTo": {
                    "Alex Spouse": {"@type": "Relation", "relation": {"spouse": true}},
                    "Morgan Manager": {"@type": "Relation", "relation": {"manager": true}},
                    "Sam Assistant": {"@type": "Relation", "relation": {"assistant": true}},
                    "Taylor Mixed": {"@type": "Relation", "relation": {"spouse": true, "manager": true, "assistant": true, "kin": true}},
                    "Jordan Kin": {"@type": "Relation", "relation": {"kin": true}},
                    "urn:uuid:e1f0a1c2-0f6b-4d2e-9c3a-2b1f9d0e7c44": {"@type": "Relation", "relation": {"spouse": true}}
                },
                "emails": {"e1": {"address": "relmatrix@example.org"}}
            }),
        ),
    ];

    for (name, initial_patch) in cases {
        let id = fixture.seed(&fixture.ours, "Seed Name", "seed@example.com");
        fixture.patch(&id, initial_patch);

        let loaded1 = sync.load_contact(id.as_str()).expect(name);

        // Save the vCard back without changes.
        let saved1 = sync
            .save_contact(&loaded1.vcard, Some(id.as_str()))
            .expect(name);

        let loaded2 = sync.load_contact(id.as_str()).expect(name);

        // The second save must send no patch, so the revision stays the same.
        let saved2 = sync
            .save_contact(&loaded2.vcard, Some(id.as_str()))
            .expect(name);
        assert_eq!(
            saved1.revision, saved2.revision,
            "case '{name}': second save must not update revision"
        );

        // The diff between the server card and the parsed vCard must be empty.
        let current_card = fixture.card(&id);
        let parsed_card = jmap_vcard::vcard_to_card(&loaded2.vcard).expect(name);
        let patch_diff = jmap_book_sync::patch::diff(&current_card, &parsed_card);
        assert!(
            patch_diff.is_empty(),
            "case '{name}': diff after round-trip must be empty, found: {patch_diff:?}"
        );
    }
}

#[test]
fn no_op_save_mints_no_patch_on_empty_container_collections() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let id = fixture.seed(&fixture.ours, "Seed Name", "seed@example.com");

    fixture.patch(
        &id,
        json!({
            "name": {
                "full": "Empty Collections Contact",
                "components": []
            },
            "organizations": {
                "o1": {
                    "name": "Acme Corp",
                    "units": []
                }
            },
            "addresses": {
                "a1": {
                    "full": "Test Street 1",
                    "components": [],
                    "contexts": {}
                }
            },
            "emails": {
                "e1": {
                    "address": "empty@example.com",
                    "contexts": {}
                }
            },
            "phones": {
                "p1": {
                    "number": "+123456",
                    "contexts": {},
                    "features": {}
                }
            }
        }),
    );

    let initial_card = fixture.card(&id);
    let loaded = sync.load_contact(id.as_str()).unwrap();
    let parsed_card = jmap_vcard::vcard_to_card(&loaded.vcard).unwrap();

    let patch_diff = jmap_book_sync::patch::diff(&initial_card, &parsed_card);
    assert!(
        patch_diff.is_empty(),
        "first save must mint no patch for empty container collections, found: {patch_diff:?}"
    );

    let initial_revision = initial_card.extra.get("revision").cloned();
    let saved1 = sync.save_contact(&loaded.vcard, Some(id.as_str())).unwrap();
    assert_eq!(
        saved1.revision,
        initial_revision
            .and_then(|r| r.as_str().map(str::to_owned))
            .unwrap_or(saved1.revision.clone()),
        "first save must not increment revision for no-op change"
    );

    let loaded2 = sync.load_contact(id.as_str()).unwrap();
    let saved2 = sync
        .save_contact(&loaded2.vcard, Some(id.as_str()))
        .unwrap();
    assert_eq!(
        saved1.revision, saved2.revision,
        "second save must preserve identical revision"
    );
}

#[test]
fn no_op_save_mints_no_patch_on_mixed_populated_and_empty_container_collections() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let id = fixture.seed(&fixture.ours, "Seed Name", "seed@example.com");

    fixture.patch(
        &id,
        json!({
            "name": {
                "full": "Mixed Collections Contact",
                "components": [
                    {"kind": "given", "value": "Mixed"},
                    {"kind": "surname", "value": "Contact"}
                ]
            },
            "organizations": {
                "o1": {
                    "name": "Acme Corp",
                    "units": [{"name": "Engineering"}]
                },
                "o2": {
                    "name": "Subsidiary Inc",
                    "units": []
                }
            },
            "addresses": {
                "a1": {
                    "full": "Main St 10",
                    "components": [{"kind": "name", "value": "Main St 10"}],
                    "contexts": {"work": true}
                },
                "a2": {
                    "full": "Branch Box 20",
                    "components": [],
                    "contexts": {}
                }
            },
            "emails": {
                "e1": {
                    "address": "work@example.com",
                    "contexts": {"work": true}
                },
                "e2": {
                    "address": "other@example.com",
                    "contexts": {}
                }
            },
            "phones": {
                "p1": {
                    "number": "+111111",
                    "contexts": {"work": true},
                    "features": {"voice": true}
                },
                "p2": {
                    "number": "+222222",
                    "contexts": {},
                    "features": {}
                }
            }
        }),
    );

    let initial_card = fixture.card(&id);
    let loaded1 = sync.load_contact(id.as_str()).unwrap();
    let parsed_card1 = jmap_vcard::vcard_to_card(&loaded1.vcard).unwrap();

    let patch_diff1 = jmap_book_sync::patch::diff(&initial_card, &parsed_card1);
    assert!(
        patch_diff1.is_empty(),
        "first save must mint no patch for mixed container collections, found: {patch_diff1:?}"
    );

    let saved1 = sync
        .save_contact(&loaded1.vcard, Some(id.as_str()))
        .unwrap();
    let loaded2 = sync.load_contact(id.as_str()).unwrap();
    let saved2 = sync
        .save_contact(&loaded2.vcard, Some(id.as_str()))
        .unwrap();
    assert_eq!(
        saved1.revision, saved2.revision,
        "second save must preserve identical revision"
    );
}

#[test]
fn editing_an_address_preserves_a_preference_ranking_the_vcard_flattens() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "addresses/a1": {
                "components": [{"kind": "name", "value": "Hauptstraße 1"}],
                "pref": 30
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(vcard.contains("PREF"), "{vcard}");
    let edited = vcard.replace("Hauptstraße 1", "Hauptstraße 2");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.card(&id);
    assert_eq!(
        stored.addresses.as_ref().unwrap()["a1"].pref,
        Some(30),
        "address pref rank must not be flattened to 1 by a save that did not touch it"
    );
}

#[test]
fn clearing_an_address_preference_in_the_vcard_clears_it_on_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "addresses/a1": {
                "components": [{"kind": "name", "value": "Hauptstraße 1"}],
                "pref": 30
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited = vcard
        .replace(";TYPE=PREF", "")
        .replace("TYPE=PREF;", "")
        .replace("TYPE=PREF:", ":");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.card(&id);
    assert_eq!(
        stored.addresses.as_ref().unwrap()["a1"].pref,
        None,
        "clearing PREF on address line must clear pref on server"
    );
}

#[test]
fn reclassifying_a_homepage_link_to_a_blog_or_video_url_patches_kind() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "links/l1": {
                "uri": "https://vera.example.com"
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("URL;X-JMAP-KEY=l1:https://vera.example.com"),
        "{vcard}"
    );

    // Move the link from the Homepage URL to the Blog URL.
    let edited = vcard.replace("URL;X-JMAP-KEY=l1:", "X-EVOLUTION-BLOG-URL;X-JMAP-KEY=l1:");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.card(&id);
    assert_eq!(
        stored.links.as_ref().unwrap()["l1"].kind.as_deref(),
        Some("blog"),
        "reclassifying URL to X-EVOLUTION-BLOG-URL must patch kind to 'blog'"
    );

    // Move the link from the Blog URL to the Video URL.
    let vcard2 = sync.load_contact(id.as_str()).unwrap().vcard;
    let edited2 = vcard2.replace(
        "X-EVOLUTION-BLOG-URL;X-JMAP-KEY=l1:",
        "X-EVOLUTION-VIDEO-URL;X-JMAP-KEY=l1:",
    );
    sync.save_contact(&edited2, Some(id.as_str())).unwrap();

    let stored2 = fixture.card(&id);
    assert_eq!(
        stored2.links.as_ref().unwrap()["l1"].kind.as_deref(),
        Some("video"),
        "reclassifying X-EVOLUTION-BLOG-URL to X-EVOLUTION-VIDEO-URL must patch kind to 'video'"
    );
}

#[test]
fn reclassifying_a_blog_url_back_to_homepage_patches_kind_to_null() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "links/l1": {
                "kind": "blog",
                "uri": "https://blog.example.com"
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(
        vcard.contains("X-EVOLUTION-BLOG-URL;X-JMAP-KEY=l1:"),
        "{vcard}"
    );

    // Move the link back to the Homepage URL.
    let edited = vcard.replace("X-EVOLUTION-BLOG-URL;X-JMAP-KEY=l1:", "URL;X-JMAP-KEY=l1:");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.card(&id);
    assert_eq!(
        stored.links.as_ref().unwrap()["l1"].kind,
        None,
        "reclassifying blog URL to homepage URL must clear kind"
    );
}

#[test]
fn reclassifying_a_calendar_uri_to_freebusy_url_patches_kind() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(
        &id,
        json!({
            "calendars/c1": {
                "kind": "calendar",
                "uri": "https://cal.example.com/user"
            }
        }),
    );
    let sync = fixture.sync();

    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(vcard.contains("CALURI;X-JMAP-KEY=c1:"), "{vcard}");

    // Move the address from the Calendar URI to the Free/Busy URL.
    let edited = vcard.replace("CALURI;X-JMAP-KEY=c1:", "FBURL;X-JMAP-KEY=c1:");
    sync.save_contact(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.card(&id);
    assert_eq!(
        stored.calendars.as_ref().unwrap()["c1"].kind.as_deref(),
        Some("freeBusy"),
        "reclassifying CALURI to FBURL must patch kind to 'freeBusy'"
    );
}

#[test]
fn no_op_save_mints_no_patch_on_address_pref_and_typed_links_and_calendars() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let id = fixture.seed(&fixture.ours, "Seed Name", "seed@example.com");

    fixture.patch(
        &id,
        json!({
            "addresses": {
                "a1": {
                    "full": "Preferred Address 123",
                    "pref": 20,
                    "contexts": {"work": true}
                }
            },
            "links": {
                "l1": {
                    "kind": "blog",
                    "uri": "https://blog.example.com"
                },
                "l2": {
                    "kind": "video",
                    "uri": "https://video.example.com/channel"
                }
            },
            "calendars": {
                "c1": {
                    "kind": "calendar",
                    "uri": "https://cal.example.com/events"
                },
                "c2": {
                    "kind": "freeBusy",
                    "uri": "https://cal.example.com/freebusy"
                }
            }
        }),
    );

    let initial_card = fixture.card(&id);
    let loaded1 = sync.load_contact(id.as_str()).unwrap();
    let parsed_card1 = jmap_vcard::vcard_to_card(&loaded1.vcard).unwrap();

    let patch_diff1 = jmap_book_sync::patch::diff(&initial_card, &parsed_card1);
    assert!(
        patch_diff1.is_empty(),
        "first save must mint no patch for address pref and typed links/calendars, found: {patch_diff1:?}"
    );

    let saved1 = sync
        .save_contact(&loaded1.vcard, Some(id.as_str()))
        .unwrap();
    let loaded2 = sync.load_contact(id.as_str()).unwrap();
    let saved2 = sync
        .save_contact(&loaded2.vcard, Some(id.as_str()))
        .unwrap();
    assert_eq!(
        saved1.revision, saved2.revision,
        "second save must preserve identical revision"
    );
}

#[test]
fn no_op_save_mints_no_patch_on_empty_or_unstated_name() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();

    fixture.patch(
        &id,
        json!({
            "name": {
                "full": "",
                "components": []
            }
        }),
    );

    let initial_card = fixture.card(&id);
    let loaded1 = sync.load_contact(id.as_str()).unwrap();
    let parsed_card1 = jmap_vcard::vcard_to_card(&loaded1.vcard).unwrap();

    let patch_diff1 = jmap_book_sync::patch::diff(&initial_card, &parsed_card1);
    assert!(
        patch_diff1.is_empty(),
        "no-op save on empty/unstated name must mint no patch, found: {patch_diff1:?}"
    );

    let saved1 = sync
        .save_contact(&loaded1.vcard, Some(id.as_str()))
        .unwrap();
    let loaded2 = sync.load_contact(id.as_str()).unwrap();
    let saved2 = sync
        .save_contact(&loaded2.vcard, Some(id.as_str()))
        .unwrap();
    assert_eq!(
        saved1.revision, saved2.revision,
        "second save must preserve identical revision on empty/unstated name"
    );
}

#[test]
fn clearing_stated_name_with_unmapped_fields_preserves_unmapped_name_metadata() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();

    fixture.patch(
        &id,
        json!({
            "name": {
                "full": "Vera Oldenburg",
                "components": [{"kind": "given", "value": "Vera"}],
                "isOrdered": true,
                "sortAs": {"de": "Oldenburg, Vera"},
                "phonetic": "veːʁa"
            }
        }),
    );

    let loaded = sync.load_contact(id.as_str()).unwrap();
    // The user clears the name in Evolution, which removes the FN and N lines.
    let mut cleared_vcard = String::new();
    for line in loaded.vcard.lines() {
        if !line.starts_with("FN:") && !line.starts_with("N:") {
            cleared_vcard.push_str(line);
            cleared_vcard.push_str("\r\n");
        }
    }

    sync.save_contact(&cleared_vcard, Some(id.as_str()))
        .unwrap();

    let stored = fixture.card(&id);
    let name = stored
        .name
        .as_ref()
        .expect("name object should be preserved");
    assert_eq!(name.full, None, "full name should be cleared");
    assert_eq!(name.components, None, "components should be cleared");
    assert_eq!(name.is_ordered, Some(true), "isOrdered should be preserved");
    assert_eq!(
        name.sort_as,
        Some(std::collections::BTreeMap::from([(
            "de".to_string(),
            "Oldenburg, Vera".to_string()
        )])),
        "sortAs should be preserved"
    );
    assert_eq!(
        name.extra.get("phonetic"),
        Some(&json!("veːʁa")),
        "extra phonetic field should be preserved"
    );
}

#[test]
fn editing_preserves_extra_pref_ranking_on_emails_and_phones() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();

    fixture.patch(
        &id,
        json!({
            "emails/e0/pref": 42,
            "phones/p1": {
                "number": "+49 30 111",
                "pref": 55
            }
        }),
    );

    let initial_card = fixture.card(&id);
    let loaded1 = sync.load_contact(id.as_str()).unwrap();
    let parsed_card1 = jmap_vcard::vcard_to_card(&loaded1.vcard).unwrap();

    let patch_diff1 = jmap_book_sync::patch::diff(&initial_card, &parsed_card1);
    assert!(
        patch_diff1.is_empty(),
        "first save must mint no patch for email and phone pref ranks, found: {patch_diff1:?}"
    );

    let saved1 = sync
        .save_contact(&loaded1.vcard, Some(id.as_str()))
        .unwrap();
    let loaded2 = sync.load_contact(id.as_str()).unwrap();
    let saved2 = sync
        .save_contact(&loaded2.vcard, Some(id.as_str()))
        .unwrap();
    assert_eq!(
        saved1.revision, saved2.revision,
        "second save must preserve identical revision for email and phone pref ranks"
    );

    let stored = fixture.card(&id);
    assert_eq!(stored.emails.as_ref().unwrap()["e0"].pref, Some(42));
    assert_eq!(stored.phones.as_ref().unwrap()["p1"].pref, Some(55));
}

#[test]
fn no_op_save_mints_no_patch_across_all_contact_surfaces_matrix() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();

    // Set mapped and unmapped properties across the JSContact schema.
    fixture.patch(
        &id,
        json!({
            "name": {
                "full": "Dr. Vera Maria Oldenburg Jr.",
                "components": [
                    {"kind": "title", "value": "Dr."},
                    {"kind": "given", "value": "Vera"},
                    {"kind": "given2", "value": "Maria"},
                    {"kind": "surname", "value": "Oldenburg"},
                    {"kind": "credential", "value": "Jr."}
                ],
                "isOrdered": true,
                "sortAs": {"de": "Oldenburg, Vera"},
                "phonetic": "veːʁa"
            },
            "nicknames": {
                "n1": {"name": "Vee", "contexts": {"private": true}, "pref": 10}
            },
            "emails": {
                "e0": {
                    "address": "vera@example.com",
                    "contexts": {"work": true, "school": true},
                    "pref": 25,
                    "label": "Direct"
                }
            },
            "phones": {
                "p1": {
                    "number": "+49 30 123456",
                    "contexts": {"work": true, "private": true},
                    "features": {"voice": true, "fax": true},
                    "pref": 15
                }
            },
            "organizations": {
                "o1": {
                    "name": "Acme Corp",
                    "units": [
                        {"name": "Research & Development", "sortAs": "R&D"},
                        {"name": "Applied Cryptography"}
                    ],
                    "sortAs": "Acme",
                    "contexts": {"work": true}
                }
            },
            "titles": {
                "t1": {"name": "Chief Scientist", "kind": "title", "organizationId": "o1"},
                "t2": {"name": "Project Lead", "kind": "role"}
            },
            "addresses": {
                "a1": {
                    "components": [
                        {"kind": "name", "value": "Hauptstraße 42"},
                        {"kind": "locality", "value": "Berlin"},
                        {"kind": "postcode", "value": "10115"},
                        {"kind": "country", "value": "Germany"}
                    ],
                    "full": "Hauptstraße 42\n10115 Berlin\nGermany",
                    "contexts": {"work": true, "private": true},
                    "pref": 30
                }
            },
            "notes": {
                "n1": {"note": "Key collaborator on JMAP standards", "created": "2026-01-01T00:00:00Z"}
            },
            "anniversaries": {
                "y1": {"kind": "birth", "date": {"year": 1985, "month": 6, "day": 15}},
                "y2": {"kind": "wedding", "date": {"year": 2015, "month": 9, "day": 20}}
            },
            "links": {
                "l1": {"uri": "https://example.com/vera", "kind": null, "pref": 5},
                "l2": {"uri": "https://blog.example.com/vera", "kind": "blog"},
                "l3": {"uri": "https://video.example.com/vera", "kind": "video"}
            },
            "calendars": {
                "c1": {"uri": "https://cal.example.com/vera", "kind": "calendar"},
                "c2": {"uri": "https://cal.example.com/vera/freebusy", "kind": "freeBusy"}
            },
            "media": {
                "m1": {
                    "kind": "photo",
                    "uri": "https://example.com/vera.jpg",
                    "mediaType": "image/jpeg"
                }
            },
            "onlineServices": {
                "s1": {"service": "Jabber", "user": "vera@jabber.org"},
                "s2": {"service": "Matrix", "user": "@vera:matrix.org"}
            },
            "relatedTo": {
                "Bob Example": {"relation": {"spouse": true}},
                "Alice Manager": {"relation": {"manager": true}},
                "Charlie Assistant": {"relation": {"assistant": true}},
                "Dave Colleague": {"relation": {"colleague": true}}
            },
            "keywords": {
                "VIP": true,
                "WG-Member": true,
                "Unstatable Category \r\n": true
            },
            "cryptoKeys": {
                "k1": {"kind": "key", "uri": "https://keys.example.com/vera.asc"}
            },
            "directories": {
                "d1": {"kind": "directory", "uri": "ldap://ldap.example.com"}
            },
            "personalInfo": {
                "pi1": {"kind": "expertise", "value": "Protocols"}
            },
            "speakToAs": {
                "grammaticalGender": "feminine",
                "pronouns": "she/her"
            },
            "preferredLanguages": {
                "de": {"language": "de-DE", "pref": 1}
            },
            "localizations": {
                "en": {"name/full": "Vera Oldenburg"}
            },
            "customPreservedField": "preserved-value"
        }),
    );

    // First load and save, as when the user opens and closes a contact in Evolution.
    let loaded1 = sync.load_contact(id.as_str()).expect("load 1");
    let saved1 = sync
        .save_contact(&loaded1.vcard, Some(id.as_str()))
        .expect("save 1");

    // Second load and save.
    let loaded2 = sync.load_contact(id.as_str()).expect("load 2");
    let parsed_card2 = jmap_vcard::vcard_to_card(&loaded2.vcard).expect("parse 2");
    let current_card = fixture.card(&id);

    let second_patch = jmap_book_sync::patch::diff(&current_card, &parsed_card2);
    assert!(
        second_patch.is_empty(),
        "second diff on contact after round-trip must mint no patch, found: {second_patch:?}"
    );

    let saved2 = sync
        .save_contact(&loaded2.vcard, Some(id.as_str()))
        .expect("save 2");
    assert_eq!(
        saved1.revision, saved2.revision,
        "second save must not change server revision for untouched contact"
    );

    // The server keeps all mapped and unmapped properties.
    let stored = fixture.card(&id);
    assert!(stored.crypto_keys.is_some() || stored.extra.contains_key("cryptoKeys"));
    assert!(stored.directories.is_some() || stored.extra.contains_key("directories"));
    assert!(stored.personal_info.is_some() || stored.extra.contains_key("personalInfo"));
    assert!(stored.speak_to_as.is_some() || stored.extra.contains_key("speakToAs"));
    assert!(
        stored.preferred_languages.is_some() || stored.extra.contains_key("preferredLanguages")
    );
    assert!(stored.localizations.is_some() || stored.extra.contains_key("localizations"));
    assert_eq!(
        stored.extra.get("customPreservedField"),
        Some(&json!("preserved-value"))
    );
}

#[test]
fn an_edit_of_one_property_does_not_revert_a_concurrent_remote_edit_of_another() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();
    let base = sync.load_contact(id.as_str()).unwrap().vcard;
    // A second client renames the contact after this client loaded it.
    fixture.patch(&id, json!({"name/full": "Vera Kranz"}));

    let edited = base.replace("vera@example.com", "vera@example.org");
    sync.save_contact_since(
        Some(Base {
            vcard: &base,
            on_conflict: OnConflict::KeepLocal,
        }),
        &edited,
        Some(id.as_str()),
    )
    .unwrap();

    let stored = fixture.card(&id);
    assert_eq!(
        stored.name.as_ref().unwrap().full.as_deref(),
        Some("Vera Kranz"),
        "the save reverted a name that the user did not edit"
    );
    let emails = stored.emails.as_ref().expect("emails");
    assert!(
        emails.values().any(|e| e.address == "vera@example.org"),
        "{emails:?}"
    );
}

#[test]
fn a_property_that_the_user_edited_takes_the_users_value_over_a_remote_edit() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();
    let base = sync.load_contact(id.as_str()).unwrap().vcard;
    fixture.patch(&id, json!({"emails/e0/address": "vera@example.net"}));

    let edited = base.replace("vera@example.com", "vera@example.org");
    sync.save_contact_since(
        Some(Base {
            vcard: &base,
            on_conflict: OnConflict::KeepLocal,
        }),
        &edited,
        Some(id.as_str()),
    )
    .unwrap();

    let emails = fixture.card(&id).emails.expect("emails");
    let addresses: Vec<_> = emails.values().map(|e| e.address.as_str()).collect();
    assert_eq!(addresses, ["vera@example.org"]);
}

#[test]
fn a_save_with_no_change_since_the_base_writes_nothing() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();
    let base = sync.load_contact(id.as_str()).unwrap().vcard;
    fixture.patch(&id, json!({"name/full": "Vera Kranz"}));

    let saved = sync
        .save_contact_since(
            Some(Base {
                vcard: &base,
                on_conflict: OnConflict::KeepLocal,
            }),
            &base,
            Some(id.as_str()),
        )
        .unwrap();

    assert!(saved.vcard.contains("Vera Kranz"), "{}", saved.vcard);
    assert_eq!(
        fixture.card(&id).name.unwrap().full.as_deref(),
        Some("Vera Kranz")
    );
}
