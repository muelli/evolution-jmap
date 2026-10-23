// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the nested-object patch path of `BookSync::save_contact` against a
//! real JMAP server.
//!
//! `patch::diff_flags` (`src/patch.rs`) writes a full-object PatchObject
//! segment (`phones/<key>/features: {...}`). Here the value at the patched path
//! is a JSON object, three levels deep, not a scalar or `null`. This shape
//! differs from the entry-level `organizations/<key>: null` removal that
//! `live_server_organization.rs` confirms. `tests/save.rs` proves this patch
//! against `jmap-mockd` in
//! `moving_a_number_to_another_kind_of_phone_field_reclassifies_it`, but
//! `jmap-mockd` applies any patch it receives. This test confirms that a real
//! server merges a nested object value at a `/`-joined path into the property,
//! as `diff_flags` expects. The server does not reject the shape or replace the
//! whole `phones` map.
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
use jmap_client::Client;
use jmap_proto::methods::GetRequest;
use jmap_proto::session::{CAPABILITY_CONTACTS, CAPABILITY_CORE};
use jmap_test_support::{connect_for_write, unique_suffix};
use serde_json::Value;

/// Fetches the raw JSContact `phones/<key>/features` object of a card through
/// `ContactCard/get`, not through the vCard-only API of `BookSync`. The `TYPE`
/// of a `TEL` line can spell only one feature ([`feature_slot`] in
/// `jmap-vcard`). Thus a kept but unstated feature, such as `voice` here, is on
/// the card but not in the vCard from `BookSync`. Only a raw fetch can confirm
/// that the server merged the features and did not replace them.
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

/// Creates a contact with a phone line that states `voice` and `fax`. Confirms
/// that the server folds the line to one `TYPE` (`FAX`, per the order of
/// `feature_slot`) and keeps the unstated `voice` feature. Then moves the
/// number to the Mobile field (`TYPE=CELL`). A raw `ContactCard/get` confirms
/// that the card holds the new `mobile` feature and the kept `voice` feature.
/// This proves that real Stalwart merges a nested-object
/// `phones/<key>/features` patch segment, as `diff_flags` expects. Stalwart
/// does not reject the segment or replace the whole `phones` map with the sent
/// value.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn reclassifying_a_phone_feature_merges_on_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    // A second connection to the same account reads the raw ContactCard JSON
    // that the API of `BookSync` cannot expose.
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

    // The user moves the number to the Mobile field: it is now a mobile, not
    // the office fax. The line never stated `voice`, so the save must keep it.
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
