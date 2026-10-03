// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `BookSync::save_contact`'s `PHOTO` scalar-field patch path against a real
//! JMAP server.
//!
//! `patch::diff_media` (`src/patch.rs`) writes `media/<key>/uri` and
//! `media/<key>/mediaType` as nested PatchObject members when a `PHOTO`
//! line's own stated shape changes -- gated by `same_photo`
//! (`jmap-vcard/src/contact.rs`), which compares the line as stated rather
//! than the entry's members, so a padding-only or type-spelling difference
//! is not mistaken for an edit. `jmap-book-sync/tests/save.rs`'s
//! `the_photo_the_user_chose_reaches_the_server` already proves the mapping
//! produces that patch against `jmap-mockd`, which just applies whatever
//! patch it is handed; nothing in this repository's live-server suite has
//! ever confirmed a real server accepts a `media/<key>/uri` +
//! `media/<key>/mediaType` segment and actually replaces the picture rather
//! than rejecting it or adding a second entry.
//!
//! ## Running it
//!
//! Same environment as `live_server.rs` -- see
//! `docs/manual-test-live-server.md`.
//!
//! ```console
//! $ cargo test -p evolution-jmap-book-sync -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

use std::env;

use jmap_book_sync::BookSync;
use jmap_client::{Client, Credentials};
use jmap_proto::session::CAPABILITY_CONTACTS;

/// A value unique to this process invocation, so a concurrent or prior run's
/// leftover contact can never be mistaken for this run's own.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

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

/// Saves a contact with one `PHOTO` line, confirms it round-trips, then
/// saves an edited vCard with the whole line replaced by a different
/// picture (same shape EDS writes when the user picks a new photo: no key,
/// a different type and payload) and confirms the real server actually
/// replaced the picture in place rather than rejecting the patch or filing
/// a second entry alongside the old one.
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
    // "hello-photo" / "new-photo!!", standing in for the JPEG/PNG bytes a
    // real card carries.
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

    // The key the server assigned is read back off whichever line now
    // carries the payload we sent, exactly as a real editor would: the
    // whole line is replaced with a new, keyless one stating a different
    // picture, mirroring what EDS writes when the user picks a new photo.
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
