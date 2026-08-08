// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The mapping against what a server actually returns, rather than against a
//! fixture that could drift away from it.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::contacts::ContactCard;
use jmap_vcard::{card_to_vcard, vcard_to_card};
use serde_json::json;

#[test]
fn a_stored_card_survives_the_trip_through_vcard() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let book = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_address_book("Personal", true)
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let stored = client
        .contact_create(
            &account_id,
            &ContactCard::simple(book, "Vera Oldenburg", "vera@example.com"),
        )
        .unwrap();
    let id = stored.id.clone().expect("server assigned id");

    // The vCard EDS gets to see.
    let vcard = card_to_vcard(&stored);
    assert!(vcard.contains(&format!("\r\nUID:{id}\r\n")), "{vcard}");
    assert!(vcard.contains("\r\nFN:Vera Oldenburg\r\n"), "{vcard}");
    assert!(vcard.contains("vera@example.com"), "{vcard}");
    assert!(
        vcard.contains(&format!(
            "\r\nX-JMAP-UID:{}\r\n",
            stored.uid.as_ref().unwrap()
        )),
        "{vcard}"
    );

    // …and back, with the identifiers and the email key intact.
    let back = vcard_to_card(&vcard).expect("parse");
    assert_eq!(back.id, stored.id);
    assert_eq!(back.uid, stored.uid);
    let key = back.emails.as_ref().unwrap().keys().next().unwrap().clone();
    assert_eq!(stored.emails.as_ref().unwrap().keys().next(), Some(&key));

    // The point of preserving the key: a patch built from the round-tripped
    // card addresses the entry the server already has, so an edit stays an
    // edit instead of becoming a remove-and-re-add.
    client
        .contact_update(
            &account_id,
            &id,
            json!({format!("emails/{key}/address"): "vera@example.org"}),
        )
        .unwrap();

    let updated = client.contact_get(&account_id, &[id]).unwrap();
    let emails = updated.list[0].emails.as_ref().unwrap();
    assert_eq!(emails.len(), 1, "patched in place, not appended");
    assert_eq!(emails[&key].address, "vera@example.org");
}
