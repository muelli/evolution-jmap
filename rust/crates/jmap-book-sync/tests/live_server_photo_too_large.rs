// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A contact photo bigger than a real server takes.
//!
//! Real Stalwart (0.16) refuses a card larger than 512 KiB, and does not
//! advertise that limit anywhere: RFC 9610's contacts capability has no field
//! for it, and Stalwart's refusal is `invalidProperties` with the reason only
//! in its description, not RFC 8620's `tooLarge`. `tests/photo_too_large.rs`
//! proves the mapping against `jmap-mockd`'s `tooLarge`; this proves that the
//! real server's own wording is recognised too, so the user is told to choose
//! a smaller picture rather than shown a protocol error.
//!
//! ## Running it
//!
//! Same environment as `live_server.rs`.
//!
//! ```console
//! $ cargo test -p evolution-jmap-book-sync -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

use std::env;

use base64::Engine;
use jmap_book_sync::{BookSync, SyncError};
use jmap_client::{Client, Credentials};
use jmap_proto::session::CAPABILITY_CONTACTS;

/// Mirrors `live_server.rs::connect_for_write` exactly.
fn connect_for_write() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user, password))
        .expect("could not fetch the session document for the write-test account");
    Some(client)
}

/// Saves a new contact with a 1 MB photo, twice Stalwart's limit and well
/// under its 10 MB `maxSizeRequest`, so the refusal is the server's own.
#[test]
#[ignore = "needs a real JMAP server; see live_server.rs"]
fn a_photo_the_real_server_refuses_is_reported_as_too_large() {
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

    let photo = base64::engine::general_purpose::STANDARD.encode(vec![0x5a_u8; 1_000_000]);
    let vcard = format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-not-a-server-id\r\n\
         FN:agent-bsphotosize\r\n\
         PHOTO;ENCODING=b;TYPE=JPEG:{photo}\r\n\
         END:VCARD\r\n"
    );

    match sync.save_contact(&vcard, None) {
        Err(SyncError::PhotoTooLarge { photo_octets }) => assert_eq!(photo_octets, 1_000_000),
        Err(other) => panic!("expected PhotoTooLarge, got {other:?}"),
        Ok(saved) => {
            sync.remove_contact(&saved.uid).ok();
            panic!("the server stored a 1 MB photo; its limit has changed, revisit this test");
        }
    }
}
