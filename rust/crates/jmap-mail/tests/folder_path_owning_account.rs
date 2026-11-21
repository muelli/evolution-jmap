// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapStore::folder_path`, scoped to the mailbox's own account when it
//! belongs to a shared account merged into the folder tree.
//!
//! `tests/file_message_owning_account.rs` covers the same id-collision case
//! for `JmapStore::file_message`; this file reaches the id-collision case no
//! FFI test needs a live folder for, the same way that file does (item 192
//! row R4).

mod common;

use common::sync_against;
use jmap_mail::store::JmapStore;
use jmap_mock::MockServer;
use jmap_proto::Id;

/// No flags: the tree the store already has, with no new request. Matches
/// `file_message_owning_account.rs`'s `CACHED`.
const CACHED: eds_sys::CamelStoreGetFolderInfoFlags = 0;

/// The id collision: a shared mailbox can have the same mailbox id as a
/// mailbox of the store's own account. `JmapStore::folder_path` must use the
/// owner the caller gives it, not look the mailbox id up in the merged tree
/// with no account to scope it, because that answers with the first match
/// in tree order, which is not always the caller's account.
#[test]
fn a_shared_mailboxs_path_comes_from_its_own_account_when_the_mailbox_ids_collide() {
    let server = MockServer::builder().start();

    let shared_account_id = Id::new("agent-shared-owner");
    let (own_archive_id, shared_archive_id) = {
        let state = server.state();
        let mut state = state.lock().unwrap();

        let own = state
            .account_mut(&server.account_id())
            .expect("the mock's own account");
        let own_archive_id = own.seed_mailbox("Archive", None);

        let sharer = state.add_account(shared_account_id.clone(), "Bob");
        sharer.is_personal = false;
        let shared_archive_id = sharer.seed_mailbox("Archive", None);

        assert_eq!(
            own_archive_id, shared_archive_id,
            "the two accounts must number their mailboxes from the same id"
        );
        (own_archive_id, shared_archive_id)
    };

    let store = JmapStore::detached();
    store.store_connection(sync_against(&server));
    store.folders(CACHED).expect("listed");

    let own_path = store
        .folder_path(&own_archive_id, None)
        .expect("the store's own Archive has a path");
    let shared_path = store
        .folder_path(&shared_archive_id, Some(&shared_account_id))
        .expect("the shared account's Archive has a path");

    assert_ne!(
        own_path, shared_path,
        "the id collision must not answer with the store's own account's path"
    );
}
