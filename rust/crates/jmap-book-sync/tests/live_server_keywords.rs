// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the whole-object patch path of `BookSync::save_contact` for `keywords`
//! against a real JMAP server.
//!
//! `patch::diff_keywords` (`src/patch.rs`) writes the whole `keywords` set as
//! one value: a full map, or `Value::Null` when the set is empty. The set holds
//! the tags of the edited `CATEGORIES` line. It also keeps each server-side tag
//! that a `CATEGORIES` line cannot draw, per `states_keyword`. Such a tag is
//! not `true`, is empty, holds a `\r`, or has whitespace at an edge. This shape
//! replaces the whole property at once. It differs from the
//! `organizations/<key>: null` removal in `live_server_organization.rs` and the
//! triply-nested scalar merge in `live_server_related_to.rs`. `tests/save.rs`
//! proves this patch against `jmap-mockd` in
//! `a_set_holding_a_tag_with_no_line_is_not_an_edit_waiting_to_happen`, with a
//! `" quiet"` tag. EDS trims leading and trailing whitespace, so a vCard cannot
//! draw that tag. This test confirms that a real server accepts the
//! whole-object replace and keeps the exact key of a tag with whitespace at an
//! edge.
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
use jmap_proto::Id;
use jmap_proto::session::CAPABILITY_CONTACTS;
use jmap_test_support::{connect_for_write, unique_suffix};
use serde_json::json;

/// Seeds a tag that no `CATEGORIES` line can state (`" quiet"`, with whitespace
/// that EDS trims). Then adds a tag through the only path that a vCard has:
/// `CATEGORIES:hiking`. Confirms that the real server accepts the whole-object
/// replace from `diff_keywords` and keeps the exact key of the unstatable tag
/// beside the new tag. The server does not reject the shape, trim the key, or
/// drop the kept member.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn adding_a_tag_beside_one_the_vcard_cannot_show_merges_on_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    // Use a second connection to the same account for the raw `ContactCard/set`
    // and `/get` calls. The vCard-only API of `BookSync` cannot make them, and
    // `Client` is not `Clone`.
    let verify = connect_for_write().expect("just checked JMAP_LIVE_SERVER_WRITE_USER is set");

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

    let sync = BookSync::new(client, account_id.clone(), address_book_id);

    let name = format!("agent-booksync-keywords-{}", unique_suffix());
    let vcard = format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-not-a-server-id\r\n\
         FN:{name}\r\n\
         N:{name};;;;\r\n\
         END:VCARD\r\n"
    );
    let saved = sync
        .save_contact(&vcard, None)
        .expect("ContactCard/set create failed against the real server");

    // Only a raw patch can put the card in this shape. `states_keyword` refuses
    // to draw a tag with a leading space, so no vCard states it.
    verify
        .contact_update(
            &account_id,
            &Id::from(saved.uid.clone()),
            json!({"keywords": {" quiet": true}}),
        )
        .expect("the raw keywords seed patch failed against the real server");

    let loaded = sync
        .load_contact(&saved.uid)
        .expect("loading the seeded card failed");
    assert!(
        !loaded.vcard.contains("CATEGORIES"),
        "an unstatable tag has no line to show up on: {}",
        loaded.vcard
    );

    let edited_vcard = loaded
        .vcard
        .replace("END:VCARD\r\n", "CATEGORIES:hiking\r\nEND:VCARD\r\n");
    let updated = sync
        .save_contact(&edited_vcard, Some(&saved.uid))
        .expect("ContactCard/set update failed against the real server");
    assert!(
        updated.vcard.contains("CATEGORIES:hiking"),
        "the new tag should show up right after the save: {}",
        updated.vcard
    );

    let fetched = verify
        .contact_get(&account_id, &[Id::from(saved.uid.clone())])
        .expect("ContactCard/get failed against the real server");
    let keywords = fetched.list[0]
        .keywords
        .as_ref()
        .expect("the card should still carry tags");
    assert_eq!(
        keywords,
        &[
            (" quiet".to_owned(), json!(true)),
            ("hiking".to_owned(), json!(true)),
        ]
        .into(),
        "the real server should have merged the new tag into the whole-object \
         replace, keeping the unstatable tag's exact key, not rejecting the \
         shape or trimming/dropping the carried-forward member: {keywords:?}"
    );

    sync.remove_contact(&saved.uid)
        .expect("ContactCard/set destroy failed against the real server");
}
