// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `MailSync::folder_tree_since` against a real JMAP server. This is
//! the `Mailbox/changes` delta path that `get_folder_info_sync` uses for a
//! refresh. A full `Mailbox/get` listing is the fallback.
//!
//! `live_server_folder.rs` checks the result with `folder_tree` only.
//! `jmap-mail-sync/tests/refresh.rs` tests the `Unchanged` and `Rebuilt`
//! results against `jmap-mock`. This file is its live-server counterpart,
//! with the same structure as `live_server_changes.rs` in this crate,
//! `jmap-book-sync` and `jmap-cal-sync`.
//!
//! ## Running it
//!
//! See `docs/manual-test-live-server.md`. Set `JMAP_LIVE_SERVER_URL`,
//! `JMAP_LIVE_SERVER_WRITE_USER` and `JMAP_LIVE_SERVER_WRITE_PASSWORD`, then:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_folder_changes -- --ignored
//! ```
//!
//! This crate has no `live-server` feature. `#[ignore]` keeps the test out of
//! a plain `cargo test`.
//!
//! The tests skip when `JMAP_LIVE_SERVER_WRITE_USER` is unset. They panic when
//! that variable is set but `JMAP_LIVE_SERVER_WRITE_PASSWORD` or
//! `JMAP_LIVE_SERVER_URL` is unset. The cross-account test also skips when
//! `JMAP_LIVE_SERVER_RECIPIENT_USER` (a second account in the same domain) is
//! unset, and panics when only `JMAP_LIVE_SERVER_RECIPIENT_PASSWORD` is unset.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{FolderTree, FolderUpdate, MailSync};
use jmap_proto::State;
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

/// Same as `connect_recipient` in `jmap-book-sync/tests/live_server_changes.rs`.
fn connect_recipient() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_RECIPIENT_PASSWORD").expect(
        "JMAP_LIVE_SERVER_RECIPIENT_USER is set but JMAP_LIVE_SERVER_RECIPIENT_PASSWORD is not",
    );
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_RECIPIENT_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    Some(
        Client::builder()
            .rebase_urls_to_origin(rebase)
            .connect(&origin, Credentials::basic(user, password))
            .expect("could not fetch the session document for the recipient account"),
    )
}

/// Same as `rebuilt` in `tests/refresh.rs`.
fn rebuilt(update: FolderUpdate) -> (State, FolderTree) {
    match update {
        FolderUpdate::Rebuilt { state, tree } => (state, tree),
        FolderUpdate::Unchanged(state) => {
            panic!("expected a rebuilt tree, the server reported nothing new at {state}")
        }
    }
}

/// Creates, renames and deletes a top-level folder. After each step,
/// `folder_tree_since` from the previous state must return a rebuilt tree
/// that shows the change. `tests/refresh.rs` makes the same assertions
/// against `jmap-mock`.
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

/// Gives `folder_tree_since` a state from a different account. The tree must
/// be rebuilt (`FolderUpdate::Rebuilt`).
///
/// RFC 8620 makes a state opaque. Stalwart rejects most hand-written states
/// with `invalidArguments` (see `jmap-book-sync/tests/live_server_changes.rs`).
/// A valid state from a different account causes `cannotCalculateChanges`.
/// `MailSync::folder_tree_since` catches that error and rebuilds the tree.
/// This test makes sure that the fallback occurs on the real error, not only
/// on the mock error of `jmap-mock/src/setops.rs`.
///
/// The test does not delete the folder that it creates.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn folder_tree_since_rebuilds_the_tree_for_another_accounts_state() {
    let Some(owner) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let Some(other) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the write-path test"
        );
        return;
    };

    let owner_account_id = owner
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let owner_sync = MailSync::new(owner, owner_account_id);

    // A live check showed that the "no changes yet" state of a new account
    // is valid for each other new account too. A new folder moves the owner
    // to a position in its own changelog. The other account does not have
    // that position, thus the server returns cannotCalculateChanges.
    let name = format!("agent-mailsync-cannotcalc-{}", unique_suffix());
    owner_sync
        .create_folder(None, &name)
        .expect("Mailbox/set create failed against the real server");
    let (owner_state, _) = owner_sync
        .folder_tree()
        .expect("listing the owner's folder tree failed against the real server");

    let other_account_id = other
        .primary_account(CAPABILITY_MAIL)
        .expect("the recipient account needs the mail capability");
    let other_sync = MailSync::new(other, other_account_id);

    let update = other_sync
        .folder_tree_since(&owner_state)
        .expect("folder_tree_since should rebuild rather than propagate cannotCalculateChanges");
    assert!(
        matches!(update, FolderUpdate::Rebuilt { .. }),
        "expected the tree rebuilt for another account's state, got: {update:?}"
    );
}
