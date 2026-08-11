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
