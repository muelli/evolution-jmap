// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `BookSync::save_contact`'s nested-member patch path for `relatedTo`,
//! against a real JMAP server.
//!
//! `patch::diff_related_to` (`src/patch.rs`) writes a scalar member three
//! segments deep (`relatedTo/<key>/relation/<kind>: true`) when the related
//! entity already has a relation object on it — a shape distinct from the
//! entry-level `organizations/<key>: null` removal `live_server_organization.rs`
//! confirmed, the nested-object `phones/<key>/features` full-object merge
//! `live_server_phone_reclassify.rs` confirmed, and the bare array-valued
//! `name/components` replace `live_server_name_components.rs` confirmed: here
//! the value being written is a lone `true`, one level deeper than any of
//! those. `jmap-book-sync/tests/save.rs`'s
//! `a_spouse_the_card_already_relates_to_gains_the_marriage` already proves
//! the mapping produces that patch against `jmap-mockd`; nothing in this
//! repository's live-server suite has ever confirmed a real server merges
//! this triply-nested scalar member into the existing relation object
//! rather than rejecting the shape or replacing the whole entity.
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

/// Seeds a `kin` relation no vCard field can state (one of the nineteen
/// relation types Evolution has no field for), then adds a `spouse` relation
/// to the same entity the only way a vCard can: `X-EVOLUTION-SPOUSE`.
/// Confirms the real server merges the new kind into the existing relation
/// object rather than replacing it or rejecting the triply-nested scalar
/// member `diff_related_to` sends for an addition to an entity it already
/// knows.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn adding_a_spouse_to_an_already_related_entity_merges_on_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    // A second connection to the same account, used only for the raw
    // `ContactCard/set`/`get` calls `BookSync`'s vCard-only API cannot make:
    // seeding the vCard-unreachable `kin` relation and reading the merged
    // relation object back directly. `Client` is not `Clone` (it owns a
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

    let name = format!("agent-booksync-related-{}", unique_suffix());
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

    // Only a raw patch can put the card in this shape: `kin` has no
    // X-EVOLUTION-* field of its own, so no vCard states it.
    verify
        .contact_update(
            &account_id,
            &Id::from(saved.uid.clone()),
            json!({"relatedTo": {"Jean Paul Oldenburg": {
                "@type": "Relation",
                "relation": {"kin": true},
            }}}),
        )
        .expect("the raw relatedTo seed patch failed against the real server");

    let loaded = sync
        .load_contact(&saved.uid)
        .expect("loading the seeded card failed");
    assert!(
        !loaded.vcard.contains("SPOUSE"),
        "kin has no field to show up on: {}",
        loaded.vcard
    );

    let edited_vcard = loaded.vcard.replace(
        "END:VCARD\r\n",
        "X-EVOLUTION-SPOUSE:Jean Paul Oldenburg\r\nEND:VCARD\r\n",
    );
    let updated = sync
        .save_contact(&edited_vcard, Some(&saved.uid))
        .expect("ContactCard/set update failed against the real server");
    assert!(
        updated
            .vcard
            .contains("X-EVOLUTION-SPOUSE:Jean Paul Oldenburg"),
        "the new marriage should show up right after the save: {}",
        updated.vcard
    );

    let fetched = verify
        .contact_get(&account_id, &[Id::from(saved.uid.clone())])
        .expect("ContactCard/get failed against the real server");
    let related = fetched.list[0]
        .related_to
        .as_ref()
        .expect("the card should still relate to someone");
    assert_eq!(
        related.keys().collect::<Vec<_>>(),
        vec!["Jean Paul Oldenburg"],
        "the same person was named twice: {related:?}"
    );
    assert_eq!(
        related["Jean Paul Oldenburg"].relation,
        Some(
            [
                ("kin".to_owned(), json!(true)),
                ("spouse".to_owned(), json!(true)),
            ]
            .into()
        ),
        "the real server should have merged spouse into the existing relation \
         object, not replaced it or rejected the triply-nested member: {related:?}"
    );

    sync.remove_contact(&saved.uid)
        .expect("ContactCard/set destroy failed against the real server");
}
