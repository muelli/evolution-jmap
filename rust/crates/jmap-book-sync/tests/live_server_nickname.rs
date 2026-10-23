// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the nickname patch path of `BookSync::save_contact` against a real
//! JMAP server.
//!
//! When the name of an entry changes, `patch::diff_nicknames` (`src/patch.rs`)
//! writes `nicknames/<key>/name` as a nested scalar PatchObject member. This is
//! the per-entry callback of `diff_entries`, the shape that
//! `live_server_email_address.rs` confirms for `emails/<key>/address`.
//! `tests/save.rs` proves this patch against `jmap-mockd` in
//! `editing_a_nickname_keeps_the_context_and_ranking_the_line_cannot_carry`,
//! but `jmap-mockd` applies any patch it receives. This test confirms that a
//! real server replaces the name and does not reject the segment or add a
//! second entry.
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

/// Saves a contact with one `NICKNAME` line, then saves it again with a
/// different nickname. Confirms that the real server replaces the nickname and
/// does not reject the patch or add a second entry.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn retyping_a_nickname_replaces_it_on_the_real_server() {
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
    let name = format!("agent-bsn-{suffix}");
    let old_nickname = format!("Old{suffix}");
    let new_nickname = format!("New{suffix}");
    let vcard = format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-not-a-server-id\r\n\
         FN:{name}\r\n\
         N:{name};;;;\r\n\
         NICKNAME:{old_nickname}\r\n\
         END:VCARD\r\n"
    );
    let saved = sync
        .save_contact(&vcard, None)
        .expect("ContactCard/set create failed against the real server");
    assert!(
        saved.vcard.contains(&old_nickname),
        "the created card should carry the nickname we sent: {}",
        saved.vcard
    );

    let edited_vcard = saved.vcard.replace(&old_nickname, &new_nickname);
    let updated = sync
        .save_contact(&edited_vcard, Some(&saved.uid))
        .expect("ContactCard/set update failed against the real server");
    assert!(
        updated.vcard.contains(&new_nickname) && !updated.vcard.contains(&old_nickname),
        "the retyped nickname should show up right after the save, and the old one \
         should be gone, not kept alongside it: {}",
        updated.vcard
    );

    let reloaded = sync
        .load_contact(&saved.uid)
        .expect("loading the edited card failed");
    assert!(
        reloaded.vcard.contains(&new_nickname) && !reloaded.vcard.contains(&old_nickname),
        "the retyped nickname should stay in place on reload, with the old one gone: {}",
        reloaded.vcard
    );

    sync.remove_contact(&saved.uid)
        .expect("ContactCard/set destroy failed against the real server");
}
