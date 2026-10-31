// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the contact-list (JSContact group card) round trip of `BookSync` against
//! a real JMAP server.
//!
//! `tests/contact_groups.rs` proves this path against `jmap-mockd`, including the
//! id-vs-uid member key rewrite `resolve_member_uids`/`resolve_member_ids` do
//! (`src/lib.rs`). This test confirms a real server accepts the same `members`
//! shape on create, add, remove and rename, through the public vCard-only API
//! only (`BookSync::save_contact`/`load_contact`), the same way
//! `live_server_anniversary.rs` confirms a scalar patch.
//!
//! ## Running it
//!
//! Use the same environment as `live_server.rs` (see
//! `docs/manual-test-live-server.md`).
//!
//! ```console
//! $ cargo test -p evolution-jmap-book-sync -- --ignored
//! ```
//!
//! A test skips (it does not fail) when `JMAP_LIVE_SERVER_WRITE_USER` is unset.

use jmap_book_sync::BookSync;
use jmap_proto::contacts::{ContactCard, Name};
use jmap_proto::session::CAPABILITY_CONTACTS;
use jmap_test_support::{connect_for_write, unique_suffix};
use jmap_vcard::{GROUP_KIND, card_to_vcard};

/// The vCard Evolution would save for a contact list, naming members by the
/// id their own vCard `UID` line carries (the same shape
/// `tests/contact_groups.rs::group_vcard` sends to `jmap-mockd`).
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

/// Creates two member contacts, then a contact list naming both by id.
/// Confirms the real server accepts the create, an added and a removed
/// member, and a rename, and that the loaded vCard keeps naming members by
/// id (round-tripped through the server's own uid key).
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_contact_list_round_trips_through_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };

    let account_id = client
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");
    let address_book_id = client
        .address_books(&account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the write-test account needs a default address book")
        .id
        .expect("the server named the address book");

    let sync = BookSync::new(client, account_id, address_book_id);

    let suffix = format!("{:x}", unique_suffix() & 0xffff_ffff);
    let member_one_name = format!("agent-cg-one-{suffix}");
    let member_two_name = format!("agent-cg-two-{suffix}");
    let member_one = sync
        .save_contact(
            &format!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:{member_one_name}\r\n\
                 N:{member_one_name};;;;\r\nEND:VCARD\r\n"
            ),
            None,
        )
        .expect("creating the first member failed against the real server");
    let member_two = sync
        .save_contact(
            &format!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:{member_two_name}\r\n\
                 N:{member_two_name};;;;\r\nEND:VCARD\r\n"
            ),
            None,
        )
        .expect("creating the second member failed against the real server");

    // Create, with one member.
    let list_name = format!("agent-cg-list-{suffix}");
    let created = sync
        .save_contact(&group_vcard(&list_name, &[&member_one.uid]), None)
        .expect("ContactCard/set create failed against the real server");
    assert!(
        created.vcard.contains("X-EVOLUTION-LIST:TRUE"),
        "the created card should be a contact list: {}",
        created.vcard
    );
    assert!(
        created
            .vcard
            .contains(&format!("X-EVOLUTION-DEST-CONTACT-UID={}", member_one.uid)),
        "the created card should name the first member: {}",
        created.vcard
    );

    // Add the second member.
    let added = sync
        .save_contact(
            &group_vcard(&list_name, &[&member_one.uid, &member_two.uid]),
            Some(&created.uid),
        )
        .expect("ContactCard/set update (add a member) failed against the real server");
    assert!(
        added
            .vcard
            .contains(&format!("X-EVOLUTION-DEST-CONTACT-UID={}", member_one.uid))
            && added
                .vcard
                .contains(&format!("X-EVOLUTION-DEST-CONTACT-UID={}", member_two.uid)),
        "adding a member should keep the one already there: {}",
        added.vcard
    );

    // Remove the first member.
    let removed = sync
        .save_contact(
            &group_vcard(&list_name, &[&member_two.uid]),
            Some(&added.uid),
        )
        .expect("ContactCard/set update (remove a member) failed against the real server");
    assert!(
        !removed
            .vcard
            .contains(&format!("X-EVOLUTION-DEST-CONTACT-UID={}", member_one.uid))
            && removed
                .vcard
                .contains(&format!("X-EVOLUTION-DEST-CONTACT-UID={}", member_two.uid)),
        "removing a member should drop only that member: {}",
        removed.vcard
    );

    // Rename.
    let renamed_list_name = format!("agent-cg-renamed-{suffix}");
    let renamed = sync
        .save_contact(
            &group_vcard(&renamed_list_name, &[&member_two.uid]),
            Some(&removed.uid),
        )
        .expect("ContactCard/set update (rename) failed against the real server");
    assert!(
        renamed.vcard.contains(&format!("FN:{renamed_list_name}")),
        "the rename should show up on reload: {}",
        renamed.vcard
    );
    assert!(
        renamed
            .vcard
            .contains(&format!("X-EVOLUTION-DEST-CONTACT-UID={}", member_two.uid)),
        "the rename should not disturb membership: {}",
        renamed.vcard
    );

    // Reload from a fresh `ContactCard/get`, not just the `/set` echo.
    let loaded = sync
        .load_contact(&renamed.uid)
        .expect("loading the renamed list failed");
    assert!(
        loaded.vcard.contains(&format!("FN:{renamed_list_name}"))
            && loaded
                .vcard
                .contains(&format!("X-EVOLUTION-DEST-CONTACT-UID={}", member_two.uid)),
        "the list should still be as it was on a fresh load: {}",
        loaded.vcard
    );

    // Delete.
    sync.remove_contact(&renamed.uid)
        .expect("ContactCard/set destroy (list) failed against the real server");
    assert!(
        sync.load_contact(&renamed.uid).is_err(),
        "the list should be gone after delete"
    );
    sync.remove_contact(&member_one.uid)
        .expect("ContactCard/set destroy (first member) failed against the real server");
    sync.remove_contact(&member_two.uid)
        .expect("ContactCard/set destroy (second member) failed against the real server");
}
