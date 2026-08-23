// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `BookSync::save_contact`'s create path must not trust `ContactCard/set`'s
//! `created` object to carry the properties the client itself just sent.
//!
//! RFC 8620 §5.3 only requires the server to report properties it set
//! itself; a real deployment (Stalwart, found via `jmap-book-sync/tests/
//! live_server.rs` against the live test server) takes this literally and
//! answers a `ContactCard/set` create with `{"id": "..."}` alone. Before
//! this fix, `save_contact`'s create branch rendered its return value
//! straight from that terse object, so the vCard `save_contact_sync` hands
//! back to EDS — the record EDS caches immediately, before any later sync —
//! was missing the name and every other property the caller just wrote.

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

    // The revision must match a normal load of the same card — otherwise
    // the very next `get_changes` looks like an external edit happened.
    let reloaded = sync.load_contact(&saved.uid).unwrap();
    assert_eq!(saved.revision, reloaded.revision);
}
