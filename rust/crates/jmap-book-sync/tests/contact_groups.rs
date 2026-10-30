// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! An Evolution contact list (a JSContact group card) round-trips through the
//! server: create, rename, add and remove a member, delete.
//!
//! `ContactCard/set` keys `members` by a member's JSContact `uid`, but EDS
//! names a member by the JMAP id its own vCard `UID` line carries (see
//! `jmap_book_sync::resolve_member_uids` and `resolve_member_ids`). These
//! tests go through both: the vCard that `save_contact` takes names members by
//! id, and the server state that `fixture.card` reads back names them by uid.

mod common;

use std::collections::BTreeSet;

use common::Fixture;
use jmap_proto::contacts::{ContactCard, Name};
use jmap_proto::Id;
use jmap_vcard::{GROUP_KIND, card_to_vcard, vcard_to_card};

/// The vCard Evolution would save for a contact list, naming members by the
/// id their own vCard `UID` line carries.
fn group_vcard(name: &str, member_ids: &[&str]) -> String {
    let mut card = ContactCard::new()
        .with_kind(GROUP_KIND)
        .with_name(Name::new(name));
    card.members = Some(
        member_ids
            .iter()
            .map(|id| ((*id).to_owned(), true))
            .collect(),
    );
    card_to_vcard(&card)
}

fn member_keys(card: &ContactCard) -> BTreeSet<String> {
    card.members.iter().flatten().map(|(key, _)| key.clone()).collect()
}

#[test]
fn a_contact_list_round_trips_through_the_server() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let member_one = fixture
        .seed(&fixture.ours, "Vera Oldenburg", "vera@example.com")
        .to_string();
    let member_two = fixture
        .seed(&fixture.ours, "Alex Bauer", "alex@example.com")
        .to_string();
    let uid_one = fixture.card(&Id::from(member_one.as_str())).uid.unwrap();
    let uid_two = fixture.card(&Id::from(member_two.as_str())).uid.unwrap();

    // Create, with one member. The server stores it keyed by the member's
    // uid, not the id the vCard named.
    let created = sync
        .save_contact(&group_vcard("Book Club", &[&member_one]), None)
        .unwrap();
    let stored = fixture.card(&Id::from(created.uid.as_str()));
    assert_eq!(stored.kind.as_deref(), Some(GROUP_KIND));
    assert_eq!(member_keys(&stored), [uid_one.clone()].into());

    // The vCard EDS gets back names the member by id again, round-tripped
    // through the server's uid.
    let loaded = vcard_to_card(&sync.load_contact(&created.uid).unwrap().vcard).unwrap();
    assert_eq!(member_keys(&loaded), [member_one.clone()].into());

    // Add a member.
    let added = sync
        .save_contact(
            &group_vcard("Book Club", &[&member_one, &member_two]),
            Some(created.uid.as_str()),
        )
        .unwrap();
    let stored = fixture.card(&Id::from(added.uid.as_str()));
    assert_eq!(
        member_keys(&stored),
        [uid_one.clone(), uid_two.clone()].into(),
        "adding a member keeps the one already there"
    );

    // Remove the first member.
    let removed = sync
        .save_contact(
            &group_vcard("Book Club", &[&member_two]),
            Some(added.uid.as_str()),
        )
        .unwrap();
    let stored = fixture.card(&Id::from(removed.uid.as_str()));
    assert_eq!(member_keys(&stored), [uid_two.clone()].into());

    // Rename.
    let renamed = sync
        .save_contact(
            &group_vcard("Reading Circle", &[&member_two]),
            Some(removed.uid.as_str()),
        )
        .unwrap();
    let stored = fixture.card(&Id::from(renamed.uid.as_str()));
    assert_eq!(
        stored.name.as_ref().unwrap().full.as_deref(),
        Some("Reading Circle")
    );
    assert_eq!(
        member_keys(&stored),
        [uid_two].into(),
        "the rename does not disturb membership"
    );

    // Delete.
    sync.remove_contact(&renamed.uid).unwrap();
    assert!(
        sync.load_contact(&renamed.uid).is_err(),
        "the group is gone after delete"
    );
}
