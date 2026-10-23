// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the `PHOTO` patch path of `BookSync::save_contact` against a real JMAP
//! server.
//!
//! When the stated shape of a `PHOTO` line changes, `patch::diff_media`
//! (`src/patch.rs`) writes `media/<key>/uri` and `media/<key>/mediaType` as
//! nested PatchObject members. `same_photo` (`jmap-vcard/src/contact.rs`) gates
//! this patch. It compares the line as stated, not the members of the entry, so
//! a difference in padding or type spelling is not an edit. `tests/save.rs`
//! proves this patch against `jmap-mockd` in
//! `the_photo_the_user_chose_reaches_the_server`, but `jmap-mockd` applies any
//! patch it receives. This test confirms that a real server replaces the
//! picture through a `media/<key>/uri` + `media/<key>/mediaType` segment. The
//! server does not reject the segment or add a second entry.
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
use jmap_proto::session::CAPABILITY_CONTACTS;
use jmap_test_support::{connect_for_write, unique_suffix};

/// Saves a contact with one `PHOTO` line, then replaces the whole line with a
/// different picture. EDS writes this shape when the user picks a new photo: no
/// key, a different type and a different payload. Confirms that the real server
/// replaces the picture and does not reject the patch or add a second entry.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn retyping_a_photo_replaces_it_on_the_real_server() {
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
    let name = format!("agent-bsmedia-{suffix}");
    // The base64 of "hello-photo" and "new-photo!!", in place of the JPEG or
    // PNG bytes of a real card.
    let old_photo = "aGVsbG8tcGhvdG8=";
    let new_photo = "bmV3LXBob3RvISE=";
    let vcard = format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-not-a-server-id\r\n\
         FN:{name}\r\n\
         N:{name};;;;\r\n\
         PHOTO;ENCODING=b;TYPE=JPEG:{old_photo}\r\n\
         END:VCARD\r\n"
    );
    let saved = sync
        .save_contact(&vcard, None)
        .expect("ContactCard/set create failed against the real server");
    assert!(
        saved.vcard.contains(old_photo),
        "the created card should carry the PHOTO payload we sent: {}",
        saved.vcard
    );

    // Find the line that carries the sent payload, as a real editor does.
    // Replace the whole line with a keyless line for a different picture,
    // as EDS does when the user picks a new photo.
    let old_line = saved
        .vcard
        .lines()
        .find(|line| line.contains(old_photo))
        .expect("the saved vCard should have a PHOTO line carrying our payload")
        .trim_end_matches('\r')
        .to_string();
    let new_line = format!("PHOTO;TYPE=PNG;ENCODING=b:{new_photo}");
    let edited_vcard = saved.vcard.replace(&old_line, &new_line);
    let updated = sync
        .save_contact(&edited_vcard, Some(&saved.uid))
        .expect("ContactCard/set update failed against the real server");
    assert!(
        updated.vcard.contains(new_photo) && !updated.vcard.contains(old_photo),
        "the retyped PHOTO should show up right after the save, and the old \
         one should be gone, not kept alongside it: {}",
        updated.vcard
    );
    assert_eq!(
        updated.vcard.matches("\r\nPHOTO").count(),
        1,
        "patched in place, not re-added as a second PHOTO line: {}",
        updated.vcard
    );

    let reloaded = sync
        .load_contact(&saved.uid)
        .expect("loading the edited card failed");
    assert!(
        reloaded.vcard.contains(new_photo) && !reloaded.vcard.contains(old_photo),
        "the retyped PHOTO should stay in place on reload, with the old one \
         gone: {}",
        reloaded.vcard
    );
    assert_eq!(
        reloaded.vcard.matches("\r\nPHOTO").count(),
        1,
        "still one PHOTO line after reload: {}",
        reloaded.vcard
    );

    sync.remove_contact(&saved.uid)
        .expect("ContactCard/set destroy failed against the real server");
}
