// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `MailSync::create_folder` and `MailSync::delete_folder` against a
//! real JMAP server. `create_folder_sync` and `delete_folder_sync` call these
//! functions.
//!
//! These functions use `Mailbox/set` create and destroy, a different code
//! path from `Email/import` and `Email/set` in `tests/live_server.rs`.
//! `tests/manage.rs` tests them against `jmap-mock` only. This file is their
//! live-server counterpart.
//!
//! ## Running it
//!
//! See `docs/manual-test-live-server.md`. Set `JMAP_LIVE_SERVER_URL`,
//! `JMAP_LIVE_SERVER_WRITE_USER` and `JMAP_LIVE_SERVER_WRITE_PASSWORD`, then:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_folder -- --ignored
//! ```
//!
//! This crate has no `live-server` feature. `#[ignore]` keeps the test out of
//! a plain `cargo test`.
//!
//! The test skips when `JMAP_LIVE_SERVER_WRITE_USER` is unset. It panics when
//! that variable is set but `JMAP_LIVE_SERVER_WRITE_PASSWORD` or
//! `JMAP_LIVE_SERVER_URL` is unset.

use jmap_mail_sync::MailSync;
use jmap_proto::session::CAPABILITY_MAIL;
use jmap_test_support::{connect_for_write, unique_suffix};

/// Creates a top-level folder, finds it with `folder_tree`, then deletes it
/// and makes sure that it is gone.
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

/// When `Mailbox/set` returns no name, `created_folder_name` uses the
/// requested name: RFC 8620 section 5.3 reads this as "accepted as asked".
///
/// For a padded name, the Stalwart `created` response has no `name`. This
/// alone would make the create result and the next listing disagree, since
/// `Mailbox/get` shows a trimmed name. `jmap_client::Client::mailbox_create`
/// closes this gap at its own layer: it trims the requested name itself
/// before using it as the fallback, so `create_folder`'s result already
/// matches the trimmed name the next listing will show.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_padded_name_is_trimmed_consistently_between_create_and_the_next_listing() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    let sync = MailSync::new(client, account_id);

    let requested = format!("  agent-mailsync-padded-{}  ", unique_suffix());
    let trimmed = requested.trim().to_owned();
    let created = sync
        .create_folder(None, &requested)
        .expect("Mailbox/set create failed against the real server");
    assert_eq!(
        created.display_name, trimmed,
        "the server reported no name, so the client falls back to its own \
         trimmed copy of the requested name"
    );

    let (_, tree) = sync.folder_tree().expect("listing the folder tree failed");
    let listed = tree
        .iter()
        .find(|folder| folder.id == created.id)
        .expect("the newly created folder should be listed in the account's folder tree");
    assert_eq!(
        listed.display_name, trimmed,
        "a fresh listing reads the server's own stored name, which Stalwart \
         trimmed on create even though its created response said nothing \
         about name"
    );

    sync.delete_folder(&created.id)
        .expect("Mailbox/set destroy failed against the real server");
}
