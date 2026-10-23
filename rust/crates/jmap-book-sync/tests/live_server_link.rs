// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the homepage-URL patch path of `BookSync::save_contact` against a real
//! JMAP server.
//!
//! When the URI of an entry changes, `patch::diff_links` (`src/patch.rs`)
//! writes `links/<key>/uri` as a nested scalar PatchObject member. This is the
//! per-entry callback of `diff_entries`, the shape that `live_server_title.rs`
//! confirms for `titles/<key>/name`. `tests/save.rs` proves this patch against
//! `jmap-mockd` in
//! `reclassifying_a_homepage_link_to_a_blog_or_video_url_patches_kind` and
//! `a_link_for_getting_in_touch_survives_a_save_it_was_never_part_of`, but
//! `jmap-mockd` applies any patch it receives. This test confirms that a real
//! server replaces the URI and does not reject the segment or add a second
//! entry.
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

/// Saves a contact with one `URL` line, then saves it again with a different
/// URI. Confirms that the real server replaces the URI and does not reject the
/// patch or add a second entry.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn retyping_a_homepage_url_replaces_it_on_the_real_server() {
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
    let name = format!("agent-bslink-{suffix}");
    let old_url = format!("https://old-{suffix}.example/");
    let new_url = format!("https://new-{suffix}.example/");
    let vcard = format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-not-a-server-id\r\n\
         FN:{name}\r\n\
         N:{name};;;;\r\n\
         URL:{old_url}\r\n\
         END:VCARD\r\n"
    );
    let saved = sync
        .save_contact(&vcard, None)
        .expect("ContactCard/set create failed against the real server");
    assert!(
        saved.vcard.contains(&old_url),
        "the created card should carry the URL we sent: {}",
        saved.vcard
    );

    let edited_vcard = saved.vcard.replace(&old_url, &new_url);
    let updated = sync
        .save_contact(&edited_vcard, Some(&saved.uid))
        .expect("ContactCard/set update failed against the real server");
    assert!(
        updated.vcard.contains(&new_url) && !updated.vcard.contains(&old_url),
        "the retyped URL should show up right after the save, and the old one \
         should be gone, not kept alongside it: {}",
        updated.vcard
    );

    let reloaded = sync
        .load_contact(&saved.uid)
        .expect("loading the edited card failed");
    assert!(
        reloaded.vcard.contains(&new_url) && !reloaded.vcard.contains(&old_url),
        "the retyped URL should stay in place on reload, with the old one gone: {}",
        reloaded.vcard
    );

    sync.remove_contact(&saved.uid)
        .expect("ContactCard/set destroy failed against the real server");
}
