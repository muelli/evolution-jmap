// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapStore`'s own folder-listing cache against a real JMAP server.
//!
//! `jmap-mail-sync/tests/live_server_folder.rs` already proves
//! `MailSync::create_folder`/`delete_folder` round-trip through real
//! Stalwart. What that file cannot reach is `JmapStore`'s layer on top:
//! [`jmap_mail::store::JmapStore::folders`] holds a cached tree that
//! `create_folder`/`delete_folder`/`rename_folder`/`set_subscribed` edit in
//! place, and `folders(REFRESH)` decides, from a real `Mailbox/changes`
//! answer, whether to keep that cache or rebuild it
//! ([`jmap_mail_sync::FolderUpdate`]). Every test of that cache and that
//! decision so far has run against `jmap-mock`
//! (`jmap-mail/tests/folders.rs`, `manage.rs`,
//! `folder_listing_race.rs`) or a mock wrapped in a delayed transport, never
//! against a real server's own `Mailbox/changes` semantics. This is that
//! confirmation.
//!
//! ## Running it
//!
//! Same environment as the other live-server tests — see
//! `docs/manual-test-live-server.md`:
//!
//! ```console
//! $ cargo test -p jmap-mail --features testing -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_mail::store::JmapStore;
use jmap_mail_sync::MailSync;
use jmap_proto::session::CAPABILITY_MAIL;

const REFRESH: eds_sys::CamelStoreGetFolderInfoFlags = eds_sys::CAMEL_STORE_FOLDER_INFO_REFRESH;

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-mail-sync/tests/live_server_folder.rs::connect_for_write`.
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

/// Creates a folder, renames it and toggles its subscription entirely
/// through `JmapStore`'s cache (`folders(0)`, no refresh), then drives one
/// real `folders(REFRESH)` round trip against the live server to confirm
/// `Mailbox/changes` is read the way `folder_tree_since` expects, and
/// finally deletes the folder and confirms a second refresh agrees it is
/// gone.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn folder_lifecycle_round_trips_through_the_store_cache_against_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let sync = MailSync::new(client, account_id);

    let store = JmapStore::detached();
    store.store_connection(sync);

    // Primes the cache with whatever the account already holds.
    store.folders(0).expect("initial listing failed");

    let name = format!("agent-jmapstore-folder-{}", unique_suffix());
    let created = store
        .create_folder(None, &name)
        .expect("create_folder failed against the real server");
    assert_eq!(created.display_name, name);

    // No REFRESH flag: this must be answered from the cache `create_folder`
    // just edited, with no further request to the server.
    let cached = store.folders(0).expect("cached listing after create");
    assert!(
        cached.iter().any(|folder| folder.id == created.id),
        "the store's own cache should already hold the folder create_folder just made"
    );

    let renamed_name = format!("{name}-renamed");
    let renamed = store
        .rename_folder(&created, None, &renamed_name)
        .expect("rename_folder failed against the real server");
    assert_eq!(renamed.display_name, renamed_name);
    let cached = store.folders(0).expect("cached listing after rename");
    assert!(
        cached
            .iter()
            .any(|folder| folder.id == renamed.id && folder.display_name == renamed_name),
        "the store's cache should reflect the rename without a refresh"
    );

    store
        .set_subscribed(&renamed.id, false)
        .expect("set_subscribed failed against the real server");
    let cached = store.folders(0).expect("cached listing after unsubscribe");
    let found = cached
        .iter()
        .find(|folder| folder.id == renamed.id)
        .expect("the renamed folder should still be in the cache");
    assert!(
        !found.subscribed,
        "set_subscribed(false) should be visible in the store's cache with no refresh"
    );

    // A real `Mailbox/changes` round trip: the account moved (three writes
    // above) since the cache's state, so this must come back `Rebuilt`
    // rather than `Unchanged`, and the rebuilt tree must still show the
    // folder as the store's own edits left it.
    let refreshed = store
        .folders(REFRESH)
        .expect("refresh against the real server failed");
    let found = refreshed
        .iter()
        .find(|folder| folder.id == renamed.id)
        .expect("the folder should survive a real Mailbox/changes-driven refresh");
    assert_eq!(found.display_name, renamed_name);
    assert!(!found.subscribed);

    store
        .delete_folder(&renamed.id)
        .expect("delete_folder failed against the real server");
    let cached = store.folders(0).expect("cached listing after delete");
    assert!(
        !cached.iter().any(|folder| folder.id == renamed.id),
        "the store's cache should drop the folder delete_folder just removed"
    );

    // And a second real refresh agrees the server itself no longer has it.
    let refreshed = store
        .folders(REFRESH)
        .expect("post-delete refresh against the real server failed");
    assert!(
        !refreshed.iter().any(|folder| folder.id == renamed.id),
        "a real Mailbox/changes refresh should confirm the deletion"
    );
}
