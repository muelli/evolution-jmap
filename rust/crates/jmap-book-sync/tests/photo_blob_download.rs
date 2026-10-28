// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A photo a server states as `blobId` (RFC 9610 section 2), not `uri`.
//!
//! EDS only shows a `PHOTO` line with inline bytes or a `uri` it already
//! understands. A `blobId`-only photo must become an inline `data:` URI for
//! EDS to show it at all, downloaded through the `Blob/get` download URL.

mod common;

use std::collections::BTreeMap;

use base64::Engine;
use common::Fixture;
use jmap_proto::contacts::{ContactCard, Media};

/// A card with one `photo` media entry naming `blob_id`, uploaded first.
fn seed_blob_photo(fixture: &Fixture, bytes: &[u8], media_type: &str) -> jmap_proto::Id {
    let uploaded = fixture
        .client()
        .upload_blob(&fixture.account_id, media_type, bytes.to_vec())
        .unwrap();
    let mut card = ContactCard::simple(fixture.ours.clone(), "Vera Oldenburg", "vera@example.test");
    card.media = Some(BTreeMap::from([(
        "m".to_owned(),
        Media::from_blob_id(uploaded.blob_id)
            .with_kind("photo")
            .with_media_type(media_type),
    )]));
    fixture
        .client()
        .contact_create(&fixture.account_id, &card)
        .unwrap()
        .id
        .expect("server assigned id")
}

#[test]
fn a_blob_id_photo_is_downloaded_and_shown_inline() {
    let fixture = Fixture::start();
    let bytes = b"hello-photo-bytes".to_vec();
    let id = seed_blob_photo(&fixture, &bytes, "image/png");

    let info = fixture.sync().load_contact(id.as_str()).unwrap();

    let expected = base64::engine::general_purpose::STANDARD.encode(&bytes);
    assert!(
        info.vcard.contains(&expected),
        "expected the downloaded bytes inline in {:?}",
        info.vcard
    );
    assert!(info.vcard.contains("PHOTO"), "{:?}", info.vcard);
}

#[test]
fn a_non_image_blob_id_photo_stays_hidden() {
    let fixture = Fixture::start();
    let id = seed_blob_photo(&fixture, b"not-a-picture", "application/octet-stream");

    let info = fixture.sync().load_contact(id.as_str()).unwrap();

    assert!(
        !info.vcard.contains("PHOTO"),
        "a non-image blob must not become a PHOTO line: {:?}",
        info.vcard
    );
}

#[test]
fn a_blob_id_photo_too_large_to_inline_stays_hidden() {
    let fixture = Fixture::start();
    // Over `MAX_INLINE_PHOTO_BYTES` (512 KiB).
    let bytes = vec![0x5a_u8; 600 * 1024];
    let id = seed_blob_photo(&fixture, &bytes, "image/png");

    let info = fixture.sync().load_contact(id.as_str()).unwrap();

    assert!(
        !info.vcard.contains("PHOTO"),
        "an oversized blob must not become a PHOTO line: {:?}",
        info.vcard
    );
}

#[test]
fn a_photo_well_under_the_byte_cap_is_still_inlined() {
    let fixture = Fixture::start();
    // Comfortably under `MAX_INLINE_PHOTO_BYTES` (512 KiB), but far bigger
    // than a few bytes, so a much smaller accidental cap would reject it.
    let bytes = vec![0x5a_u8; 100 * 1024];
    let id = seed_blob_photo(&fixture, &bytes, "image/png");

    let info = fixture.sync().load_contact(id.as_str()).unwrap();

    assert!(
        info.vcard.contains("PHOTO"),
        "a photo well under the cap must be inlined: {:?}",
        info.vcard
    );
}

#[test]
fn a_photo_that_already_has_a_uri_is_never_re_downloaded() {
    let fixture = Fixture::start();
    // A `photo` entry that already carries a `uri` must stay as it is, even if
    // it also names a `blob_id`: only an entry with no `uri` yet is resolved.
    let other = fixture
        .client()
        .upload_blob(&fixture.account_id, "image/png", b"replaced-bytes".to_vec())
        .unwrap();
    let original = base64::engine::general_purpose::STANDARD.encode(b"original-bytes");
    let original_uri = format!("data:image/png;base64,{original}");
    let mut card = ContactCard::simple(fixture.ours.clone(), "Vera Oldenburg", "vera@example.test");
    card.media = Some(BTreeMap::from([(
        "m".to_owned(),
        Media::new(original_uri)
            .with_kind("photo")
            .with_blob_id(other.blob_id)
            .with_media_type("image/png"),
    )]));
    let id = fixture
        .client()
        .contact_create(&fixture.account_id, &card)
        .unwrap()
        .id
        .expect("server assigned id");

    let info = fixture.sync().load_contact(id.as_str()).unwrap();

    assert!(
        info.vcard.contains(&original),
        "a photo with its own uri already set must keep it: {:?}",
        info.vcard
    );
}

#[test]
fn a_blob_id_photo_nobody_touched_keeps_its_blob_id_on_save() {
    let fixture = Fixture::start();
    let bytes = b"hello-photo-bytes".to_vec();
    let id = seed_blob_photo(&fixture, &bytes, "image/png");

    let info = fixture.sync().load_contact(id.as_str()).unwrap();
    fixture
        .sync()
        .save_contact(&info.vcard, Some(id.as_str()))
        .unwrap();

    let media = fixture.card(&id).media.expect("media");
    assert_eq!(
        media.len(),
        1,
        "the unedited photo must not be duplicated under a new key: {media:?}"
    );
    assert!(
        media["m"].blob_id.is_some(),
        "an unedited blobId photo must keep its blobId, not be rewritten as a plain uri: {:?}",
        media["m"]
    );
}
