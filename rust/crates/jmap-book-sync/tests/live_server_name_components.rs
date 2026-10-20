// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the bare array-valued patch path of `BookSync::save_contact` against a
//! real JMAP server.
//!
//! `patch::diff_name` (`src/patch.rs`) writes `name/components` as a plain
//! top-level PatchObject member. Its value is a JSON array of component
//! objects. This third shape differs from the entry-level `organizations/<key>:
//! null` removal in `live_server_organization.rs` and the nested-object
//! `phones/<key>/features` merge in `live_server_phone_reclassify.rs`. Here the
//! patched path has no keyed-map segment, and the value replaces a whole list
//! at once. `tests/save.rs` proves this patch against `jmap-mockd` in
//! `retyping_a_double_barrelled_given_name_replaces_the_parts_it_was_built_from`,
//! but `jmap-mockd` applies any patch it receives. This test confirms that a
//! real server accepts a bare array-valued PatchObject member and replaces the
//! list with it.
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

use std::env;

use jmap_book_sync::BookSync;
use jmap_client::{Client, Credentials};
use jmap_proto::Id;
use jmap_proto::session::CAPABILITY_CONTACTS;
use serde_json::json;

/// Returns a value unique to this process run, so a leftover contact from
/// another run never matches this run.
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

/// Creates a contact and raw-patches its name into a double-barrelled given
/// name: two `given` components, each with a `phonetic` that vCard has no field
/// for. The `jmap-mockd` tests
/// `a_double_barrelled_given_name_survives_a_save_that_left_it_alone` and
/// `retyping_a_double_barrelled_given_name_replaces_the_parts_it_was_built_from`
/// start from the same precondition. Confirms that the name round-trips into
/// one `N` field (`N:Oldenburg;Jean Paul;;;`). Then changes the given name to
/// one that the vCard never split. A raw `ContactCard/get` confirms that the
/// real server replaced the whole `components` list with the list from
/// `diff_name`. The server does not reject the array or keep the old entries
/// beside the new entry. It also does not invent a merge: the property has no
/// key to merge by.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn retyping_a_double_barrelled_given_name_replaces_it_on_the_real_server() {
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

    // Only a raw patch can put the card in this shape. A plain `N` line has one
    // slot for the given name, so no vCard states two `given` components.
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

    // Nothing in `Hans` tells which half of `Jean Paul` it replaced, so both
    // halves must be gone. Their phonetic spellings must be gone too: nothing
    // on the card links them to `Hans`.
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
