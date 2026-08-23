// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `BookSync::get_changes` against a real JMAP server — the
//! `get_changes_sync` vfunc's state-token delta path, which is what EDS
//! actually uses for incremental sync (a full `list_existing` re-download is
//! the fallback, not the common case).
//!
//! `jmap-book-sync/tests/live_server.rs` already proves
//! create/list/load/update/remove round-trip against real Stalwart, but
//! nothing there drives `get_changes` itself — every assertion goes through
//! `list_existing`/`load_contact` instead. `jmap-book-sync/tests/sync.rs`
//! proves the classification logic (`ChangeSet` → `Changes`) against
//! `jmap-mockd`; this file is its live-server counterpart, following the
//! same recipe as `live_server.rs`.
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
use jmap_proto::session::CAPABILITY_CONTACTS;

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

/// Creates a contact, then confirms `get_changes` reports it as changed from
/// the state captured just before the create; edits it and confirms the edit
/// shows up from the post-create state; removes it and confirms the removal
/// shows up from the post-edit state. Mirrors
/// `tests/sync.rs::get_changes_reports_creations_updates_and_destructions`'s
/// assertions, now against the real server rather than `jmap-mockd`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn get_changes_reports_a_create_an_edit_and_a_removal_against_the_real_server() {
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

    let (state_before_create, _) = sync
        .list_existing()
        .expect("listing the book failed before the create");

    let name = format!("agent-booksync-changes-{}", unique_suffix());
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

    let after_create = sync
        .get_changes(&state_before_create)
        .expect("get_changes after the create failed against the real server");
    assert!(
        after_create.changed.iter().any(|c| c.uid == saved.uid),
        "the created card should be reported as changed since before it existed: {:?}",
        after_create
            .changed
            .iter()
            .map(|c| &c.uid)
            .collect::<Vec<_>>()
    );
    assert!(
        !after_create.removed.contains(&saved.uid),
        "a brand-new card must not also be reported as removed"
    );

    let new_name = format!("{name}-renamed");
    let edited_vcard = vcard.replacen(&name, &new_name, 2);
    sync.save_contact(&edited_vcard, Some(&saved.uid))
        .expect("ContactCard/set update failed against the real server");

    let after_edit = sync
        .get_changes(&after_create.new_state)
        .expect("get_changes after the edit failed against the real server");
    let edited = after_edit
        .changed
        .iter()
        .find(|c| c.uid == saved.uid)
        .unwrap_or_else(|| {
            panic!(
                "the edited card should be reported as changed since right after its creation: {:?}",
                after_edit.changed.iter().map(|c| &c.uid).collect::<Vec<_>>()
            )
        });
    assert!(
        edited.vcard.contains(&new_name),
        "the changed card get_changes reports should carry the edit: {}",
        edited.vcard
    );

    sync.remove_contact(&saved.uid)
        .expect("ContactCard/set destroy failed against the real server");

    let after_remove = sync
        .get_changes(&after_edit.new_state)
        .expect("get_changes after the removal failed against the real server");
    assert!(
        after_remove.removed.contains(&saved.uid),
        "the removed card should be reported as removed since right after its edit: {:?}",
        after_remove.removed
    );
    assert!(
        !after_remove.changed.iter().any(|c| c.uid == saved.uid),
        "a removed card must not also be reported as changed"
    );
}
