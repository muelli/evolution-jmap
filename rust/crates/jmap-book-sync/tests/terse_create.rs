// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The create path of `BookSync::save_contact` must not expect the `created`
//! object of `ContactCard/set` to hold the properties that the client sent.
//!
//! RFC 8620 §5.3 requires only the properties that the server set.
//! Stalwart (found with `jmap-book-sync/tests/live_server.rs`) answers a create
//! with only `{"id": "..."}`. EDS caches the returned vCard immediately, so a
//! vCard rendered from that object has no name and no other sent property.

use jmap_book_sync::BookSync;
use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;

const NEW_CONTACT: &str = "BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
UID:pas-id-68A2F1C400000000\r\n\
FN:Vera Oldenburg\r\n\
N:Oldenburg;Vera;;;\r\n\
EMAIL;TYPE=WORK:vera@example.com\r\n\
END:VCARD\r\n";

#[test]
fn saving_a_new_contact_against_a_terse_server_still_renders_what_was_sent() {
    let server = MockServer::builder().terse_contact_create().start();
    let account_id = server.account_id();
    let book_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_address_book("Personal", true)
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let sync = BookSync::new(client, account_id, book_id);

    let saved = sync.save_contact(NEW_CONTACT, None).unwrap();

    assert!(
        saved.vcard.contains("FN:Vera Oldenburg"),
        "a terse create response must not lose the name the client just sent: {}",
        saved.vcard
    );
    assert!(
        saved.vcard.contains("vera@example.com"),
        "a terse create response must not lose the email the client just sent: {}",
        saved.vcard
    );

    // The revision must match a normal load. If not, the next `get_changes` shows an external edit.
    let reloaded = sync.load_contact(&saved.uid).unwrap();
    assert_eq!(saved.revision, reloaded.revision);
}
