// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `BookSync::save_contact`'s bare array-valued patch path, against a real
//! JMAP server.
//!
//! `patch::diff_name` (`src/patch.rs`) writes `name/components` as a plain
//! top-level PatchObject member whose value is a JSON array of component
//! objects — a third shape, distinct from the entry-level `organizations/
//! <key>: null` removal `live_server_organization.rs` confirmed and the
//! nested-object `phones/<key>/features` merge `live_server_phone_reclassify.rs`
//! confirmed: here the patched path has no keyed-map segment at all, and the
//! value replaces a whole list in one move. `jmap-book-sync/tests/save.rs`'s
//! `retyping_a_double_barrelled_given_name_replaces_the_parts_it_was_built_from`
//! already proves the mapping produces that patch against `jmap-mockd`, which
//! just applies whatever patch it is handed; nothing in this repository's
//! live-server suite has ever confirmed a real server accepts a bare
//! array-valued PatchObject member and actually replaces the list with it.
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

/// Creates a contact, raw-patches its name into a double-barrelled given
/// name split across two `given` components (each with a `phonetic` vCard
/// has no field for — the same precondition `jmap-mockd`'s
/// `a_double_barrelled_given_name_survives_a_save_that_left_it_alone` and
/// `retyping_a_double_barrelled_given_name_replaces_the_parts_it_was_built_from`
/// start from), confirms it round-trips into a single `N` field
/// (`N:Oldenburg;Jean Paul;;;`), then retypes the given name into one the
/// vCard never split, and confirms via a raw `ContactCard/get` that the real
/// server actually replaced the whole `components` list with the new one
/// `diff_name` sent — not rejecting the array shape, not leaving the old
/// entries sitting alongside the new one, and not inventing a merge the
/// property has no key to perform.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn retyping_a_double_barrelled_given_name_replaces_it_on_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    // A second connection to the same account, used only for the raw
    // `ContactCard/set`/`get` calls `BookSync`'s vCard-only API cannot make:
    // seeding the double-given-name precondition and reading back the
    // `components` list directly. `Client` is not `Clone` (it owns a
    // transport), so this mirrors `live_server_phone_reclassify.rs`'s own
    // two-connection shape rather than sharing one.
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

    let name = format!("agent-booksync-name-{}", unique_suffix());
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

    // Only a raw patch can put the card in this shape: a plain `N` line has
    // one slot for the given name, so there is no vCard that states two
    // `given` components directly.
    verify
        .contact_update(
            &account_id,
            &Id::from(saved.uid.clone()),
            json!({"name/components": [
                {"kind": "surname", "value": "Oldenburg"},
                {"kind": "given", "value": "Jean", "phonetic": "zhon"},
                {"kind": "given", "value": "Paul", "phonetic": "pol"},
            ]}),
        )
        .expect("the raw name/components seed patch failed against the real server");

    let loaded = sync
        .load_contact(&saved.uid)
        .expect("loading the seeded card failed");
    assert!(
        loaded.vcard.contains("N:Oldenburg;Jean Paul;;;"),
        "the two given components should have joined into one N field: {}",
        loaded.vcard
    );

    // Nothing in `Hans` says which half of `Jean Paul` it replaced, so both
    // must be gone, and so must their phonetic spellings — nothing on the
    // card says those belong to `Hans`.
    let edited_vcard = loaded.vcard.replace("Jean Paul", "Hans");
    assert_ne!(
        edited_vcard, loaded.vcard,
        "the given name was not on the line: {}",
        loaded.vcard
    );
    let updated = sync
        .save_contact(&edited_vcard, Some(&saved.uid))
        .expect("ContactCard/set update failed against the real server");
    assert!(
        updated.vcard.contains("N:Oldenburg;Hans;;;"),
        "the retyped name should show up right after the save: {}",
        updated.vcard
    );

    let fetched = verify
        .contact_get(&account_id, &[Id::from(saved.uid.clone())])
        .expect("ContactCard/get failed against the real server");
    let components = fetched.list[0]
        .name
        .as_ref()
        .expect("the card should still have a name")
        .components
        .as_ref()
        .expect("the card should still have components");
    let by_kind: Vec<(&str, &str)> = components
        .iter()
        .map(|component| (component.kind.as_str(), component.value.as_str()))
        .collect();
    assert_eq!(
        by_kind,
        vec![("surname", "Oldenburg"), ("given", "Hans")],
        "the real server should have replaced the whole components list, not merged \
         into it or rejected the shape: {components:?}"
    );
    assert!(
        components
            .iter()
            .all(|component| !component.extra.contains_key("phonetic")),
        "no phonetic spelling should have survived for a name the user retyped whole: \
         {components:?}"
    );

    sync.remove_contact(&saved.uid)
        .expect("ContactCard/set destroy failed against the real server");
}
