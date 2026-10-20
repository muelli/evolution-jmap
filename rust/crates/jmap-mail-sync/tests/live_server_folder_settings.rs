// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `MailSync::set_subscribed` and `MailSync::rename_folder` against a
//! real JMAP server. `tests/subscriptions.rs` and `tests/manage.rs` test
//! these functions against `jmap-mock` only. This file is their live-server
//! counterpart.
//!
//! ## Running it
//!
//! See `docs/manual-test-live-server.md`. Set `JMAP_LIVE_SERVER_URL`,
//! `JMAP_LIVE_SERVER_WRITE_USER` and `JMAP_LIVE_SERVER_WRITE_PASSWORD`, then:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_folder_settings -- --ignored
//! ```
//!
//! This crate has no `live-server` feature. `#[ignore]` keeps the test out of
//! a plain `cargo test`.
//!
//! The test skips when `JMAP_LIVE_SERVER_WRITE_USER` is unset. It panics when
//! that variable is set but `JMAP_LIVE_SERVER_WRITE_PASSWORD` or
//! `JMAP_LIVE_SERVER_URL` is unset.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_mail_sync::MailSync;
use jmap_proto::Id;
use jmap_proto::session::CAPABILITY_MAIL;

/// Returns a value unique to this run, so that a folder from a different run
/// cannot match.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Same as `connect_for_write` in `live_server.rs`.
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

/// Creates a top-level folder, unsubscribes and subscribes it again, renames
/// it, then deletes it.
///
/// `set_subscribed` does not return the new state, thus a new `folder_tree`
/// listing checks each change. The cleanup at the end runs only when all
/// assertions pass.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn subscribing_and_renaming_a_folder_reach_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    let sync = MailSync::new(client, account_id);

    let name = format!("agent-mailsync-settings-{}", unique_suffix());
    let created = sync
        .create_folder(None, &name)
        .expect("Mailbox/set create failed against the real server");

    let subscribed_of = |sync: &MailSync, id: &Id| -> bool {
        let (_, tree) = sync
            .folder_tree()
            .expect("listing the folder tree failed against the real server");
        tree.iter()
            .find(|folder| &folder.id == id)
            .expect("the folder should still be listed")
            .subscribed
    };

    sync.set_subscribed(&created.id, false)
        .expect("unsubscribing failed against the real server");
    assert!(
        !subscribed_of(&sync, &created.id),
        "the folder should read back unsubscribed after MailSync::set_subscribed(false)"
    );

    sync.set_subscribed(&created.id, true)
        .expect("resubscribing failed against the real server");
    assert!(
        subscribed_of(&sync, &created.id),
        "the folder should read back subscribed after MailSync::set_subscribed(true)"
    );

    let new_name = format!("{name}-renamed");
    let new_path = sync
        .rename_folder(&created.id, None, &new_name)
        .expect("renaming failed against the real server");
    assert_eq!(
        new_path, new_name,
        "rename_folder's returned path should be the new top-level name, unencoded \
         since it contains no character path.rs's encoding would touch"
    );

    let (_, tree) = sync
        .folder_tree()
        .expect("listing the folder tree failed after rename");
    let renamed = tree
        .iter()
        .find(|folder| folder.id == created.id)
        .expect("the renamed folder should still be listed under its id");
    assert_eq!(
        renamed.display_name, new_name,
        "the folder's display name should reflect the rename on the real server"
    );

    sync.delete_folder(&created.id)
        .expect("Mailbox/set destroy failed against the real server");
}
