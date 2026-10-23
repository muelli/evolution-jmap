// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the nested scalar patch path of `BookSync::save_contact` for `notes`
//! against a real JMAP server.
//!
//! `patch::diff_notes` (`src/patch.rs`) writes one scalar field at a path
//! inside a map entry that is already present (`notes/<key>/note`). It does not
//! change the sibling members (`created`, `author`). This shape differs from
//! the nested-object merge in `live_server_phone_reclassify.rs` (where the
//! patched value, `phones/<key>/features`, is a JSON object) and the
//! whole-property replace in `live_server_keywords.rs`.
//! `live_server_phone_reclassify.rs` never writes a plain scalar member at a
//! nested path: it changes only the `TYPE` of a phone, never its `number`.
//! `tests/save.rs` proves this patch against `jmap-mockd` in
//! `editing_a_note_keeps_when_it_was_written_and_by_whom`. This test confirms
//! that a real server merges a bare scalar write at a nested path into the
//! entry. The server does not reject the shape or overwrite the siblings that a
//! raw patch put there.
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

/// Seeds a note with `created` and `author` members that no `NOTE` line can
/// state. Edits only the note text through the vCard. Confirms that the real
/// server merges the scalar write in place: the text changes, the
/// vCard-unreachable members stay unchanged, and no second entry appears.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn editing_a_note_through_the_vcard_keeps_members_only_a_raw_patch_could_set() {
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

    let name = format!("agent-booksync-notes-{}", unique_suffix());
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

    // Only a raw patch can put the card in this shape: a `NOTE` line has no
    // place for `created` or `author`.
    verify
        .contact_update(
            &account_id,
            &Id::from(saved.uid.clone()),
            json!({"notes": {"n1": {
                "@type": "Note",
                "note": "met at FOSDEM",
                "created": "2026-02-01T09:15:00Z",
                "author": {"@type": "Author", "name": "Agent"},
            }}}),
        )
        .expect("the raw notes seed patch failed against the real server");

    let loaded = sync
        .load_contact(&saved.uid)
        .expect("loading the seeded card failed");
    assert!(
        loaded.vcard.contains("NOTE;X-JMAP-KEY=n1:met at FOSDEM"),
        "{}",
        loaded.vcard
    );

    let edited_vcard = loaded
        .vcard
        .replace("met at FOSDEM", "met at FOSDEM and owes me a beer");
    let updated = sync
        .save_contact(&edited_vcard, Some(&saved.uid))
        .expect("ContactCard/set update failed against the real server");
    assert!(
        updated
            .vcard
            .contains("NOTE;X-JMAP-KEY=n1:met at FOSDEM and owes me a beer"),
        "the edited text should show up right after the save: {}",
        updated.vcard
    );

    let fetched = verify
        .contact_get(&account_id, &[Id::from(saved.uid.clone())])
        .expect("ContactCard/get failed against the real server");
    let notes = fetched.list[0]
        .notes
        .as_ref()
        .expect("the card should still carry its note");
    assert_eq!(
        notes.len(),
        1,
        "the scalar write should patch the entry in place, not add a second one: {notes:?}"
    );
    let note = &notes["n1"];
    assert_eq!(note.note, "met at FOSDEM and owes me a beer");
    assert_eq!(
        note.created.as_ref().map(|d| d.as_str()),
        Some("2026-02-01T09:15:00Z"),
        "a member no NOTE line can carry must survive a scalar edit to the text: {notes:?}"
    );
    // Real Stalwart drops the `@type` member of the seeded `author` object on a
    // round trip. RFC 9553 requires `@type` only on the top-level Card, so this
    // conforms, but `jmap-mockd` keeps the JSON verbatim. For this reason,
    // compare `author` by `name` only. The check is that `author` survives the
    // scalar `note` edit, not its exact JSON shape.
    assert_eq!(
        note.author.as_ref().and_then(|author| author.get("name")),
        Some(&json!("Agent")),
        "the real server must have merged the scalar `note` write into the entry, not \
         replaced it or dropped sibling members a raw patch put there: {notes:?}"
    );

    sync.remove_contact(&saved.uid)
        .expect("ContactCard/set destroy failed against the real server");
}
