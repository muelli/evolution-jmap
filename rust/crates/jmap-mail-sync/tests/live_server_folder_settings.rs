// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync::set_subscribed`/`rename_folder` against a real JMAP server —
//! the two `jmap-mail-sync` writes `live_server_folder.rs` (create/delete)
//! and the rest of this crate's live-server suite (import/expunge, keywords,
//! filing, send) leave uncovered. Only `jmap-mockd` has ever exercised
//! either (`jmap-mail-sync/tests/{subscribe,rename_folder}.rs`); this is
//! their live-server counterpart, following the same recipe as the other
//! sync crates' live-server tests.
//!
//! ## Running it
//!
//! Same environment as the other `jmap-mail-sync` live-server tests — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_folder_settings -- --ignored
//! ```
//!
//! No `--features live-server` gate is needed here — like the other files,
//! this crate has no such feature, and `#[ignore]` alone already keeps it
//! out of a plain `cargo test`.
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset — the same tolerance every write-path test in this repository
//! gives an unconfigured environment.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_mail_sync::MailSync;
use jmap_proto::Id;
use jmap_proto::session::CAPABILITY_MAIL;

/// A value unique to this process invocation, so a concurrent or prior run's
/// leftover folder can never be mistaken for this run's own.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-cal-sync/tests/live_server.rs::connect_for_write` exactly.
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

/// Creates a top-level folder, flips its subscription both ways (confirming
/// each via a fresh `folder_tree` listing, since `set_subscribed` writes to
/// the server rather than returning the new state), renames it (confirming
/// the returned path and the listing's new display name), then deletes it.
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
