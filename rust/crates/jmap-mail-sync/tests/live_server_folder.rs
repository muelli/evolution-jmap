// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync::create_folder`/`delete_folder` against a real JMAP server —
//! the functions `create_folder_sync`/`delete_folder_sync` actually call,
//! exercised end to end for the first time.
//!
//! `tests/live_server.rs` already proves `import_message`/`expunge_message`
//! against real Stalwart; this file covers the folder-management half of
//! this crate's writes, a materially different code path (`Mailbox/set`
//! create/destroy rather than `Email/import`/`Email/set`). Only
//! `jmap-mockd` has ever exercised `create_folder`/`delete_folder`
//! (`jmap-mail-sync/tests/{create,delete}_folder.rs`); this is their
//! live-server counterpart, following the same recipe as the other sync
//! crates' live-server tests.
//!
//! ## Running it
//!
//! Same environment as the other sync crates' live-server tests — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up for
//! `jmap-client`'s write-path tests:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_folder -- --ignored
//! ```
//!
//! No `--features live-server` gate is needed here — like the other sync
//! crates, this crate has no such feature, and `#[ignore]` alone already
//! keeps it out of a plain `cargo test`.
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset — the same tolerance every write-path test in this repository gives
//! an unconfigured environment.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_mail_sync::MailSync;
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

/// Creates a top-level folder via `MailSync::create_folder`, confirms it via
/// `folder_tree`, then deletes it via `MailSync::delete_folder` and confirms
/// it is gone.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn creating_then_deleting_a_folder_round_trips_through_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    let sync = MailSync::new(client, account_id);

    let name = format!("agent-mailsync-folder-{}", unique_suffix());
    let created = sync
        .create_folder(None, &name)
        .expect("Mailbox/set create failed against the real server");
    assert_eq!(created.display_name, name);
    assert!(
        created.subscribed,
        "a freshly created folder should be subscribed, whether the server said so \
         explicitly or stayed silent about it"
    );

    let (_, tree) = sync.folder_tree().expect("listing the folder tree failed");
    assert!(
        tree.iter().any(|folder| folder.id == created.id),
        "the newly created folder should be listed in the account's folder tree"
    );

    sync.delete_folder(&created.id)
        .expect("Mailbox/set destroy failed against the real server");

    let (_, tree) = sync
        .folder_tree()
        .expect("listing the folder tree failed after delete");
    assert!(
        !tree.iter().any(|folder| folder.id == created.id),
        "the deleted folder should no longer be listed"
    );
}
