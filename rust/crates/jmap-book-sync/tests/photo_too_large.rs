// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A contact photo the server will not take.
//!
//! A `PHOTO` goes to the server inline, as a `data:` URI in `ContactCard/set`.
//! Thus, a large picture makes the full card too large. RFC 9610 advertises no
//! card size limit, so the client learns of the limit from the refusal.
//! These tests cover two refusals: the server's `tooLarge` SetError (RFC 8620
//! section 5.3), and a request over `maxSizeRequest`, which the client refuses
//! before sending (RFC 8620 section 2). In both, the user must learn that the
//! picture causes the problem.

mod common;

use base64::Engine;
use common::Fixture;
use jmap_book_sync::SyncError;
use jmap_mock::MockServer;

/// A new contact whose `PHOTO` carries `octets` octets of picture.
fn contact_with_photo(octets: usize) -> String {
    let photo = base64::engine::general_purpose::STANDARD.encode(vec![0x5a_u8; octets]);
    format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-68A2F1C400000001\r\n\
         FN:Vera Oldenburg\r\n\
         N:Oldenburg;Vera;;;\r\n\
         PHOTO;ENCODING=b;TYPE=JPEG:{photo}\r\n\
         END:VCARD\r\n"
    )
}

/// A new contact with no photo, whose note carries `octets` octets of text.
fn contact_with_note(octets: usize) -> String {
    format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-68A2F1C400000002\r\n\
         FN:Vera Oldenburg\r\n\
         NOTE:{}\r\n\
         END:VCARD\r\n",
        "n".repeat(octets)
    )
}

fn photo_octets(failure: SyncError) -> u64 {
    match failure {
        SyncError::PhotoTooLarge { photo_octets } => photo_octets,
        other => panic!("expected PhotoTooLarge, got {other:?}"),
    }
}

#[test]
fn a_photo_the_server_refuses_as_too_large_says_so_with_its_size() {
    let fixture = Fixture::start_with(MockServer::builder().max_contact_size(64 * 1024));
    let sync = fixture.sync();

    let failure = sync
        .save_contact(&contact_with_photo(100_000), None)
        .unwrap_err();

    assert_eq!(photo_octets(failure), 100_000);
}

#[test]
fn a_photo_that_makes_the_request_too_large_says_so_with_its_size() {
    let fixture = Fixture::start_with(MockServer::builder().size_request(64 * 1024));
    let sync = fixture.sync();

    let failure = sync
        .save_contact(&contact_with_photo(100_000), None)
        .unwrap_err();

    assert_eq!(photo_octets(failure), 100_000);
}

#[test]
fn a_photo_added_in_an_edit_that_is_too_large_says_so_with_its_size() {
    let fixture = Fixture::start_with(MockServer::builder().size_request(64 * 1024));
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();
    let vcard = sync.load_contact(id.as_str()).unwrap().vcard;
    let photo = base64::engine::general_purpose::STANDARD.encode(vec![0x5a_u8; 100_000]);
    let edited = vcard.replace(
        "END:VCARD",
        &format!("PHOTO;ENCODING=b;TYPE=JPEG:{photo}\r\nEND:VCARD"),
    );

    let failure = sync.save_contact(&edited, Some(id.as_str())).unwrap_err();

    assert_eq!(photo_octets(failure), 100_000);
}

#[test]
fn a_small_photo_on_a_card_too_large_for_another_reason_is_not_blamed() {
    let fixture = Fixture::start_with(MockServer::builder().size_request(64 * 1024));
    let sync = fixture.sync();
    let vcard = contact_with_note(100_000).replace(
        "END:VCARD",
        "PHOTO;ENCODING=b;TYPE=JPEG:aGVsbG8tcGhvdG8=\r\nEND:VCARD",
    );

    let failure = sync.save_contact(&vcard, None).unwrap_err();

    assert!(
        matches!(failure, SyncError::Client(_)),
        "a 100 kB note is the problem, not an 11-octet photo: {failure:?}"
    );
}

#[test]
fn a_card_with_no_photo_that_is_too_large_keeps_the_servers_error() {
    let fixture = Fixture::start_with(MockServer::builder().size_request(64 * 1024));
    let sync = fixture.sync();

    let failure = sync
        .save_contact(&contact_with_note(100_000), None)
        .unwrap_err();

    assert!(
        matches!(failure, SyncError::Client(_)),
        "no photo to blame: {failure:?}"
    );
}
