// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `BookSync::save_contact`'s whole-object patch path for `keywords`,
//! against a real JMAP server.
//!
//! `patch::diff_keywords` (`src/patch.rs`) writes the entire `keywords` set
//! as one value (a full map, or `Value::Null` when empty), composed by
//! carrying forward any server-side tag `states_keyword` says a `CATEGORIES`
//! line cannot draw (not `true`, empty, holding a `\r`, or edged with
//! whitespace) alongside whatever the edited vCard's `CATEGORIES` line
//! states — a shape distinct from the entry-level `organizations/<key>: null`
//! removal `live_server_organization.rs` confirmed and the triply-nested
//! scalar merge `live_server_related_to.rs` confirmed: here the whole
//! property is replaced at once. `jmap-book-sync/tests/save.rs`'s
//! `a_set_holding_a_tag_with_no_line_is_not_an_edit_waiting_to_happen`
//! already proves the mapping produces that patch against `jmap-mockd` with
//! a `" quiet"` tag (EDS trims leading/trailing whitespace, so vCard can
//! never draw it); nothing in this repository's live-server suite has ever
//! confirmed a real server accepts that whole-object replace or preserves
//! an unstatable tag's exact, whitespace-edged key through the round trip.
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

/// Seeds a tag no `CATEGORIES` line can state (`" quiet"`, edged with
/// whitespace EDS would trim), then adds a tag the only way a vCard can:
/// `CATEGORIES:hiking`. Confirms the real server accepts the whole-object
/// replace `diff_keywords` sends and keeps the unstatable tag's exact key
/// alongside the new one, rather than rejecting the shape, trimming the
/// key itself, or dropping the carried-forward member.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn adding_a_tag_beside_one_the_vcard_cannot_show_merges_on_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    // A second connection to the same account, used only for the raw
    // `ContactCard/set`/`get` calls `BookSync`'s vCard-only API cannot make:
    // seeding the vCard-unreachable tag and reading the merged set back
    // directly. `Client` is not `Clone` (it owns a transport), so this
    // mirrors `live_server_related_to.rs`'s own two-connection shape rather
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

    // Only a raw patch can put the card in this shape: a leading space makes
    // the tag one `states_keyword` refuses to draw, so no vCard states it.
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
