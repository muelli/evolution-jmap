// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the nested-member patch path of `BookSync::save_contact` for
//! `relatedTo` against a real JMAP server.
//!
//! When the related entity already has a relation object,
//! `patch::diff_related_to` (`src/patch.rs`) writes a scalar member three
//! segments deep (`relatedTo/<key>/relation/<kind>: true`). Other tests confirm
//! three different shapes: the `organizations/<key>: null` removal
//! (`live_server_organization.rs`), the `phones/<key>/features` object merge
//! (`live_server_phone_reclassify.rs`) and the `name/components` array replace
//! (`live_server_name_components.rs`). Here the written value is a single
//! `true`, one level deeper than all of those. `tests/save.rs` proves this
//! patch against `jmap-mockd` in
//! `a_spouse_the_card_already_relates_to_gains_the_marriage`. This test
//! confirms that a real server merges this triply-nested scalar member into the
//! relation object. The server does not reject the shape or replace the whole
//! entity.
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
use jmap_proto::Id;
use jmap_proto::session::CAPABILITY_CONTACTS;
use jmap_test_support::{connect_for_write, unique_suffix};
use serde_json::json;

/// Seeds a `kin` relation that no vCard field can state (one of the nineteen
/// relation types without an Evolution field). Then adds a `spouse` relation to
/// the same entity through the only path that a vCard has:
/// `X-EVOLUTION-SPOUSE`. Confirms that the real server merges the new kind into
/// the relation object. The server does not replace the object or reject the
/// triply-nested scalar member from `diff_related_to`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn adding_a_spouse_to_an_already_related_entity_merges_on_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    // Use a second connection to the same account for the raw `ContactCard/set`
    // and `/get` calls. The vCard-only API of `BookSync` cannot make them, and
    // `Client` is not `Clone`.
    let verify = connect_for_write().expect("just checked JMAP_LIVE_SERVER_WRITE_USER is set");

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

    let sync = BookSync::new(client, account_id.clone(), address_book_id);

    let name = format!("agent-booksync-related-{}", unique_suffix());
    let vcard = format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-not-a-server-id\r\n\
         FN:{name}\r\n\
         N:{name};;;;\r\n\
         END:VCARD\r\n"
    );
    let saved = sync
        .save_contact(&vcard, None)
        .expect("ContactCard/set create failed against the real server");

    // Only a raw patch can put the card in this shape: `kin` has no
    // X-EVOLUTION-* field, so no vCard states it.
    verify
        .contact_update(
            &account_id,
            &Id::from(saved.uid.clone()),
            json!({"relatedTo": {"Jean Paul Oldenburg": {
                "@type": "Relation",
                "relation": {"kin": true},
            }}}),
        )
        .expect("the raw relatedTo seed patch failed against the real server");

    let loaded = sync
        .load_contact(&saved.uid)
        .expect("loading the seeded card failed");
    assert!(
        !loaded.vcard.contains("SPOUSE"),
        "kin has no field to show up on: {}",
        loaded.vcard
    );

    let edited_vcard = loaded.vcard.replace(
        "END:VCARD\r\n",
        "X-EVOLUTION-SPOUSE:Jean Paul Oldenburg\r\nEND:VCARD\r\n",
    );
    let updated = sync
        .save_contact(&edited_vcard, Some(&saved.uid))
        .expect("ContactCard/set update failed against the real server");
    assert!(
        updated
            .vcard
            .contains("X-EVOLUTION-SPOUSE:Jean Paul Oldenburg"),
        "the new marriage should show up right after the save: {}",
        updated.vcard
    );

    let fetched = verify
        .contact_get(&account_id, &[Id::from(saved.uid.clone())])
        .expect("ContactCard/get failed against the real server");
    let related = fetched.list[0]
        .related_to
        .as_ref()
        .expect("the card should still relate to someone");
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
                ("spouse".to_owned(), json!(true)),
            ]
            .into()
        ),
        "the real server should have merged spouse into the existing relation \
         object, not replaced it or rejected the triply-nested member: {related:?}"
    );

    sync.remove_contact(&saved.uid)
        .expect("ContactCard/set destroy failed against the real server");
}
