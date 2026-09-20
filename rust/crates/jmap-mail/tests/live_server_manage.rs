// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `manage::create_folder`/`delete_folder`/`rename_folder` against a real
//! JMAP server — the Camel-path-resolving entry points behind Evolution's
//! "New Folder", "Rename Folder" and "Delete Folder" menu items.
//!
//! `jmap-mail/tests/live_server_folder.rs` already proves `JmapStore`'s own
//! `create_folder`/`rename_folder`/`delete_folder` (which take a
//! `FolderInfo`/mailbox id directly) round-trip through real Stalwart, and
//! `jmap-mail-sync/tests/live_server_folder.rs` proves the sync layer below
//! that. What neither reaches is `manage.rs`'s own layer on top: resolving a
//! Camel *path* to a folder through `tree_holding` (which looks again at a
//! real `Mailbox/changes` answer before giving up), and, for a rename,
//! splitting the destination path into a parent and deciding whether the
//! last component is a drag's unchanged encoding or a typed name. Every test
//! of that layer so far (`tests/manage.rs`) has run against `jmap-mockd`.
//! This is that confirmation.
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
use jmap_mail::manage;
use jmap_mail::store::JmapStore;
use jmap_mail_sync::MailSync;
use jmap_proto::session::CAPABILITY_MAIL;

const CACHED: eds_sys::CamelStoreGetFolderInfoFlags = 0;

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

fn store_against(client: Client) -> Box<JmapStore> {
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let store = JmapStore::detached();
    store.store_connection(MailSync::new(client, account_id));
    store
}

/// Creates a top-level folder, a folder nested under it (parent resolved by
/// Camel path, not id), renames the nested one in place, then deletes both by
/// path — round-tripping every one of `manage.rs`'s three vfunc entry points
/// through real Stalwart.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn folder_management_by_camel_path_round_trips_against_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let store = store_against(client);
    store.folders(CACHED).expect("initial listing failed");

    let top_name = format!("agent-manage-top-{}", unique_suffix());
    let top = manage::create_folder(&store, None, &top_name)
        .expect("create_folder(None, ..) failed against the real server");
    assert_eq!(top.path, top_name);

    let child_name = "Child";
    let child = manage::create_folder(&store, Some(&top.path), child_name)
        .expect("create_folder under a Camel-path parent failed against the real server");
    let expected_child_path = format!("{top_name}/{child_name}");
    assert_eq!(
        child.path, expected_child_path,
        "the parent must be resolved by the Camel path just created, not by id"
    );

    let renamed_name = format!("{child_name}-renamed");
    let rename_to = format!("{top_name}/{renamed_name}");
    let renamed = manage::rename_folder(&store, &child.path, &rename_to)
        .expect("rename_folder by Camel path failed against the real server");
    assert_eq!(renamed.display_name, renamed_name);
    assert_eq!(renamed.path, rename_to);

    manage::delete_folder(&store, &renamed.path)
        .expect("delete_folder of the renamed child failed against the real server");
    manage::delete_folder(&store, &top.path)
        .expect("delete_folder of the top-level folder failed against the real server");

    let refreshed = store
        .folders(eds_sys::CAMEL_STORE_FOLDER_INFO_REFRESH)
        .expect("post-delete refresh against the real server failed");
    assert!(
        !refreshed.iter().any(|folder| folder.path == top.path),
        "a real Mailbox/changes refresh should confirm both folders are gone"
    );
}

/// A parent another client just made, which this store has not listed yet, is
/// found by `tree_holding`'s look-again rather than reported missing — the
/// real-server counterpart of `tests/manage.rs`'s
/// `a_parent_created_since_the_listing_is_looked_for_again`, this time through
/// a real `Mailbox/changes` answer instead of `jmap-mockd`'s.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_parent_made_by_another_connection_is_found_by_looking_again_on_the_real_server() {
    let (Some(setup_client), Some(store_client)) = (connect_for_write(), connect_for_write())
    else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };

    let store = store_against(store_client);
    // Primes the store's cache *before* the other connection makes the
    // parent, so the parent is genuinely missing from it and only a real
    // look-again refresh can find it.
    store.folders(CACHED).expect("initial listing failed");

    let parent_name = format!("agent-manage-lookagain-{}", unique_suffix());
    let account_id = setup_client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let setup_sync = MailSync::new(setup_client, account_id);
    let parent = setup_sync
        .create_folder(None, &parent_name)
        .expect("the setup connection could not create the parent folder");

    let created = manage::create_folder(&store, Some(&parent_name), "Found")
        .expect("create_folder should look again and find a parent made since the last listing");
    assert_eq!(created.path, format!("{parent_name}/Found"));

    manage::delete_folder(&store, &created.path).expect("cleanup: delete_folder(child) failed");
    store
        .delete_folder(&parent.id)
        .expect("cleanup: delete_folder(parent) failed");
}
