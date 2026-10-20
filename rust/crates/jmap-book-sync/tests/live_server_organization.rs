// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the property-removal diff path of `BookSync::save_contact` against a
//! real JMAP server.
//!
//! For a keyed-map entry that the edit removes, `patch::diff` writes a
//! nested-null PatchObject segment (`organizations/o1: null`). See
//! `diff_entries` in `src/patch.rs`. `tests/save.rs` proves this patch against
//! `jmap-mockd` in `removing_the_org_line_removes_the_organization`, but
//! `jmap-mockd` applies any patch it receives. This test confirms that a real
//! server deletes the entry for a `<property>/<key>: null` segment, and does
//! not reject the segment or keep the entry. `live_server.rs` changes only FN
//! and N, which do not reach this path.
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
use jmap_proto::session::CAPABILITY_CONTACTS;

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

/// Saves a contact with an `ORG` line, then saves it again without the `ORG`
/// line. Confirms on reload that the organization is gone. This proves that
/// real Stalwart applies an `organizations/<key>: null` patch segment, as
/// `diff_organizations` and `diff_entries` expect.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn removing_the_org_line_removes_the_organization_on_the_real_server() {
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

    let name = format!("agent-booksync-org-{}", unique_suffix());
    let vcard = format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-not-a-server-id\r\n\
         FN:{name}\r\n\
         N:{name};;;;\r\n\
         ORG:Acme Corp\r\n\
         END:VCARD\r\n"
    );
    let saved = sync
        .save_contact(&vcard, None)
        .expect("ContactCard/set create failed against the real server");
    assert!(
        saved.vcard.contains("Acme Corp"),
        "the created card should carry the organisation we sent: {}",
        saved.vcard
    );

    let edited_vcard: String = saved
        .vcard
        .lines()
        .filter(|line| !line.starts_with("ORG"))
        .map(|line| format!("{line}\r\n"))
        .collect();
    let updated = sync
        .save_contact(&edited_vcard, Some(&saved.uid))
        .expect("ContactCard/set update failed against the real server");
    assert!(
        !updated.vcard.contains("ORG"),
        "the organisation should be gone right after the save: {}",
        updated.vcard
    );

    let reloaded = sync
        .load_contact(&saved.uid)
        .expect("loading the edited card failed");
    assert!(
        !reloaded.vcard.contains("ORG"),
        "the organisation should stay gone on reload: {}",
        reloaded.vcard
    );

    sync.remove_contact(&saved.uid)
        .expect("ContactCard/set destroy failed against the real server");
}
