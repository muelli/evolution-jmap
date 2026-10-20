// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `subscribe::set_subscribed` against a real JMAP server.
//!
//! `jmap-mail/tests/live_server_folder.rs` already proves
//! `JmapStore::set_subscribed` (by mailbox id) against real Stalwart. It
//! does not reach `subscribe::set_subscribed`, the function that
//! `subscribe_folder_sync` and `unsubscribe_folder_sync` call. These two
//! are the actual `CamelSubscribable` vfuncs that Evolution's subscription
//! editor calls, and they hand `subscribe::set_subscribed` a Camel *path*.
//! `subscribe::set_subscribed` resolves that path against the store's held
//! folder tree through `tree_holding`, including its "look again" refresh
//! for a folder created by another client since the last listing, before
//! it writes through to `set_subscribed`. Every existing test of that
//! resolution (`tests/subscriptions.rs`) runs against `jmap-mockd`. This
//! file is the live-server counterpart. It has the same shape as
//! `tests/live_server_connect.rs`, which proves `open_mail` (the layer
//! above an already-live-tested primitive) against a real server.
//!
//! ## Running it
//!
//! This test needs the same environment as the other live-server tests.
//! See `docs/manual-test-live-server.md`:
//!
//! ```console
//! $ cargo test -p jmap-mail --features testing -- --ignored
//! ```
//!
//! The test is skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER` or
//! `_PASSWORD` is unset.

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

/// Resolves a real folder's path for a write. Toggles the subscription
/// off, then back on. Confirms that a folder another connection created
/// *after* the store's last listing is still found by `tree_holding`'s
/// look-again refresh, not reported missing.
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

    // Primes the cache with whatever the account already holds. Same
    // priming as in live_server_folder.rs.
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

    // Reads back over a separate connection, so the answer comes from
    // the server, not from the store's own copy.
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
