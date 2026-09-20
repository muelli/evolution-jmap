// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `subscribe::set_subscribed` against a real JMAP server.
//!
//! `jmap-mail/tests/live_server_folder.rs` already proves
//! `JmapStore::set_subscribed` (by mailbox id) against real Stalwart. What
//! it does not reach is `subscribe::set_subscribed`, the function
//! `subscribe_folder_sync`/`unsubscribe_folder_sync` (the actual
//! `CamelSubscribable` vfuncs Evolution's subscription editor calls) hand a
//! Camel *path* to: resolving that path against the store's held folder
//! tree via `tree_holding`, including its "look again" refresh for a folder
//! created by another client since the last listing, before writing through
//! to `set_subscribed`. Every existing test of that resolution
//! (`tests/subscriptions.rs`) runs against `jmap-mockd`. This is the
//! live-server counterpart, the same shape as `tests/live_server_connect.rs`
//! proving `open_mail` (the layer above a already-live-tested primitive)
//! against a real server.
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
use jmap_mail::subscribe;
use jmap_mail_sync::MailSync;
use jmap_proto::session::CAPABILITY_MAIL;

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-mail/tests/live_server_folder.rs::connect_for_write`.
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

/// Resolves a real folder's path to a write, toggles it off and back on
/// again, and confirms a folder created by another connection *after* the
/// store's last listing is still found by `tree_holding`'s look-again
/// refresh rather than reported missing.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn set_subscribed_resolves_a_real_path_and_looks_again_for_a_new_one() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let sync = MailSync::new(client, account_id.clone());

    let store = JmapStore::detached();
    store.store_connection(sync);

    // Primes the cache with whatever the account already holds, same as
    // live_server_folder.rs.
    store.folders(0).expect("initial listing failed");

    let name = format!("agent-jmapstore-subscribe-{}", unique_suffix());
    let created = store
        .create_folder(None, &name)
        .expect("create_folder failed against the real server");

    // The listing above predates this folder, so resolving its path must go
    // through tree_holding's look-again refresh, not the stale cache.
    let folder = subscribe::set_subscribed(&store, &name, false)
        .expect("set_subscribed by path failed against the real server");
    assert_eq!(folder.path, name);
    assert!(!folder.subscribed, "the answer describes the new state");

    // Read back over a connection of its own, so the answer is the
    // server's, not the store's own copy of it.
    let client = connect_for_write().expect("write account still configured");
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let (_, tree) = MailSync::new(client, account_id)
        .folder_tree()
        .expect("listed");
    assert_eq!(
        tree.find(&name).map(|folder| folder.subscribed),
        Some(false),
        "the real server must agree the folder was unsubscribed"
    );

    let folder = subscribe::set_subscribed(&store, &name, true)
        .expect("re-subscribing by path failed against the real server");
    assert!(folder.subscribed);

    store
        .delete_folder(&created.id)
        .expect("delete_folder failed against the real server");
}
