// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `BookSync::save_contact`'s scalar nested-field patch path for `notes`,
//! against a real JMAP server.
//!
//! `patch::diff_notes` (`src/patch.rs`) writes one scalar field at a path
//! inside an already-present map entry (`notes/<key>/note`), leaving
//! sibling members (`created`, `author`) untouched — a shape distinct from
//! the nested-*object*-merge `live_server_phone_reclassify.rs` confirmed
//! (there, the patched value is itself a JSON object, `phones/<key>/
//! features`) and the whole-property replace `live_server_keywords.rs`
//! confirmed. Even `live_server_phone_reclassify.rs` never writes a plain
//! scalar member at a nested path: it only ever reclassifies a phone's
//! `TYPE`, never its `number`. `jmap-book-sync/tests/save.rs`'s
//! `editing_a_note_keeps_when_it_was_written_and_by_whom` already proves
//! the mapping produces that patch against `jmap-mockd`; nothing in this
//! repository's live-server suite has ever confirmed a real server accepts
//! a bare scalar write at a nested path and merges it into the entry,
//! rather than rejecting the shape or clobbering the siblings a raw patch
//! put there in the first place.
//!
//! ## Running it
//!
//! Same environment as `live_server.rs` — see
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
use jmap_proto::Id;
use jmap_proto::session::CAPABILITY_CONTACTS;
use serde_json::json;

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

/// Seeds a note carrying `created`/`author` members no `NOTE` line can
/// state, edits only the note text through the vCard, and confirms the real
/// server merges the scalar write in place: the text changes, the
/// vCard-unreachable members survive untouched, and no second entry is
/// created.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn editing_a_note_through_the_vcard_keeps_members_only_a_raw_patch_could_set() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    // A second connection to the same account, used only for the raw
    // `ContactCard/set`/`get` calls `BookSync`'s vCard-only API cannot make:
    // seeding the vCard-unreachable members and reading the merged entry back
    // directly. `Client` is not `Clone` (it owns a transport), so this
    // mirrors `live_server_keywords.rs`'s own two-connection shape rather
    // than sharing one.
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

    // Only a raw patch can put the card in this shape: `created`/`author`
    // have nowhere to live on a `NOTE` line.
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
    // Real Stalwart drops the seeded `author` object's own `@type` member on
    // round trip (RFC 9553 only requires `@type` on the top-level Card, so
    // this is conforming, just a real divergence from `jmap-mockd`, which
    // keeps whatever JSON it was handed verbatim). Compared by `name` alone
    // for that reason; the point of this assertion is that `author` survives
    // the scalar `note` edit at all, not its exact JSON shape.
    assert_eq!(
        note.author.as_ref().and_then(|author| author.get("name")),
        Some(&json!("Agent")),
        "the real server must have merged the scalar `note` write into the entry, not \
         replaced it or dropped sibling members a raw patch put there: {notes:?}"
    );

    sync.remove_contact(&saved.uid)
        .expect("ContactCard/set destroy failed against the real server");
}
