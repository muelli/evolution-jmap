// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `BookSync::save_contact`'s nested-object patch path, against a real JMAP
//! server.
//!
//! `patch::diff_flags` (`src/patch.rs`) writes a full-object-replace
//! PatchObject segment (`phones/<key>/features: {...}`), a structurally
//! different shape from the entry-level `organizations/<key>: null` removal
//! `live_server_organization.rs` already confirmed live: here the value at
//! the patched path is itself a JSON object, three levels deep, not a
//! scalar or `null`. `jmap-book-sync/tests/save.rs`'s
//! `moving_a_number_to_another_kind_of_phone_field_reclassifies_it` already
//! proves the mapping produces that patch against `jmap-mockd`, which just
//! applies whatever patch it is handed; nothing in this repository's
//! live-server suite has ever confirmed a real server accepts a nested
//! object value at a `/`-joined path and actually merges it into the
//! property the way `diff_flags` assumes, rather than rejecting the shape
//! or replacing the whole `phones` map.
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
use jmap_proto::methods::GetRequest;
use jmap_proto::session::{CAPABILITY_CONTACTS, CAPABILITY_CORE};
use serde_json::Value;

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

/// The raw JSContact `phones/<key>/features` object for a card, fetched
/// straight through `ContactCard/get` rather than through `BookSync`'s
/// vCard-only API. Needed because a `TEL` line's `TYPE` can only ever spell
/// one feature at a time ([`feature_slot`] in `jmap-vcard`) — a "kept but
/// unstated" feature like `voice` here is real on the card but invisible in
/// the vCard string `BookSync` hands back, so only a raw fetch can confirm
/// the *merge* actually happened server-side rather than a plain replace.
fn phone_features(client: &Client, account_id: &jmap_proto::Id, card_id: &str) -> Value {
    let response = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_CONTACTS],
            "ContactCard/get",
            &GetRequest::ids(account_id.clone(), [card_id]).properties(["phones"]),
        )
        .expect("ContactCard/get failed against the real server");
    response["list"][0]["phones"]["p1"]["features"].clone()
}

/// Creates a contact with a phone line stating both `voice` and `fax`,
/// confirms the server folds it down to one `TYPE` (`FAX`, per
/// `feature_slot`'s ordering) while keeping the unstated `voice` feature on
/// the card, then retypes the number into the Mobile field (`TYPE=CELL`)
/// and confirms, via a raw `ContactCard/get`, that the card now holds both
/// the new `mobile` feature and the kept `voice` one — proving real
/// Stalwart accepts and merges a nested-object `phones/<key>/features`
/// patch segment the way `diff_flags` assumes, rather than rejecting it or
/// replacing the whole `phones` map with just what was sent.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn reclassifying_a_phone_feature_merges_on_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    // A second connection to the same account, used only to read back the
    // raw ContactCard JSON `BookSync`'s API cannot expose.
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

    let name = format!("agent-booksync-phone-{}", unique_suffix());
    let vcard = format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-not-a-server-id\r\n\
         FN:{name}\r\n\
         N:{name};;;;\r\n\
         TEL;TYPE=VOICE,FAX:+49 30 111\r\n\
         END:VCARD\r\n"
    );
    let saved = sync
        .save_contact(&vcard, None)
        .expect("ContactCard/set create failed against the real server");
    assert!(
        saved.vcard.contains("TEL;X-JMAP-KEY=p1;TYPE=FAX:"),
        "the unstated voice feature must not vanish, only lose its TYPE slot: {}",
        saved.vcard
    );
    assert_eq!(
        phone_features(&verify, &account_id, &saved.uid),
        serde_json::json!({"voice": true, "fax": true}),
        "voice must be on the card even though FAX alone won the TYPE slot",
    );

    // The user retypes the number into the Mobile field: it is no longer
    // the office fax, and it is now a mobile. The feature the line never
    // stated (voice) is not theirs to have cleared, so it must stay.
    let edited_vcard = saved.vcard.replace("TYPE=FAX:", "TYPE=CELL:");
    let updated = sync
        .save_contact(&edited_vcard, Some(&saved.uid))
        .expect("ContactCard/set update failed against the real server");
    assert!(
        updated.vcard.contains("TEL;X-JMAP-KEY=p1;TYPE=CELL:"),
        "the reclassified number should show the new mobile feature: {}",
        updated.vcard
    );
    assert_eq!(
        phone_features(&verify, &account_id, &saved.uid),
        serde_json::json!({"voice": true, "mobile": true}),
        "the real server must have merged mobile in and fax out, keeping the untouched voice \
         feature — not replaced the whole features object with just what this update sent",
    );

    let reloaded = sync
        .load_contact(&saved.uid)
        .expect("loading the edited card failed");
    assert!(
        reloaded.vcard.contains("TEL;X-JMAP-KEY=p1;TYPE=CELL:"),
        "the merged feature set should survive a reload from the real server: {}",
        reloaded.vcard
    );

    sync.remove_contact(&saved.uid)
        .expect("ContactCard/set destroy failed against the real server");
}
