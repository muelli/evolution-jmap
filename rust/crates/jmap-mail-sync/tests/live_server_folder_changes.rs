// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync::folder_tree_since` against a real JMAP server — the
//! `Mailbox/changes` state-token delta path that `get_folder_info_sync`'s
//! refresh half actually uses for incremental sync (a full `Mailbox/get`
//! listing is the fallback, not the common case).
//!
//! `jmap-mail-sync/tests/live_server_folder.rs` already proves
//! `create_folder`/`delete_folder` round-trip against real Stalwart, but
//! every assertion there goes through `folder_tree` (a full listing) rather
//! than `folder_tree_since`. `jmap-mail-sync/tests/refresh.rs` proves the
//! Unchanged/Rebuilt logic against `jmap-mockd`; this file is its
//! live-server counterpart, following the same recipe as this crate's own
//! `live_server_changes.rs` (`messages_since`) and
//! `jmap-book-sync`/`jmap-cal-sync`'s `live_server_changes.rs`.
//!
//! ## Running it
//!
//! Same environment as the other `jmap-mail-sync` live-server tests — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_folder_changes -- --ignored
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
use jmap_mail_sync::{FolderTree, FolderUpdate, MailSync};
use jmap_proto::State;
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

/// Mirrors `jmap-mail-sync/tests/refresh.rs::rebuilt` exactly.
fn rebuilt(update: FolderUpdate) -> (State, FolderTree) {
    match update {
        FolderUpdate::Rebuilt { state, tree } => (state, tree),
        FolderUpdate::Unchanged(state) => {
            panic!("expected a rebuilt tree, the server reported nothing new at {state}")
        }
    }
}

/// Creates a top-level folder, then confirms `folder_tree_since` from the
/// state captured just before the create reports the rebuilt tree with it
/// present; renames it and confirms the rebuilt tree from the post-create
/// state carries the new name; deletes it and confirms the rebuilt tree from
/// the post-rename state no longer lists it. Mirrors `tests/refresh.rs`'s
/// assertions, now against the real server rather than `jmap-mockd`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn folder_tree_since_reports_a_create_a_rename_and_a_removal_against_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    let sync = MailSync::new(client, account_id);

    let (state_before_create, _) = sync
        .folder_tree()
        .expect("listing the folder tree failed before the create");

    let name = format!("agent-mailsync-foldertree-{}", unique_suffix());
    let created = sync
        .create_folder(None, &name)
        .expect("Mailbox/set create failed against the real server");

    let (state_after_create, tree) = rebuilt(
        sync.folder_tree_since(&state_before_create)
            .expect("folder_tree_since after the create failed against the real server"),
    );
    assert!(
        tree.iter().any(|folder| folder.id == created.id),
        "the newly created folder should be in the tree folder_tree_since rebuilt: {:?}",
        tree.iter().map(|folder| &folder.id).collect::<Vec<_>>()
    );

    let renamed_name = format!("{name}-renamed");
    sync.rename_folder(&created.id, None, &renamed_name)
        .expect("Mailbox/set rename failed against the real server");

    let (state_after_rename, tree) = rebuilt(
        sync.folder_tree_since(&state_after_create)
            .expect("folder_tree_since after the rename failed against the real server"),
    );
    let renamed = tree
        .iter()
        .find(|folder| folder.id == created.id)
        .unwrap_or_else(|| panic!("the renamed folder should still be in the rebuilt tree"));
    assert_eq!(
        renamed.display_name, renamed_name,
        "the rebuilt tree should carry the rename"
    );

    sync.delete_folder(&created.id)
        .expect("Mailbox/set destroy failed against the real server");

    let (_, tree) = rebuilt(
        sync.folder_tree_since(&state_after_rename)
            .expect("folder_tree_since after the delete failed against the real server"),
    );
    assert!(
        !tree.iter().any(|folder| folder.id == created.id),
        "the deleted folder should no longer be in the rebuilt tree"
    );
}
