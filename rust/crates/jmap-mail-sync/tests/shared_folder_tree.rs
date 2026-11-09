// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync::folder_tree_with_shared` against a mock server, item 163's
//! next slice after the pure merge in `tests/shared_tree.rs`.

use jmap_mail_sync::FolderUpdate;
use jmap_proto::Id;

mod common;
use common::Fixture;

#[test]
fn no_shared_accounts_leaves_the_tree_unchanged() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox", Some("inbox"));

    let (_, tree) = fixture.sync().folder_tree_with_shared().unwrap();

    assert!(tree.find("Inbox").is_some());
    assert!(tree.find("Shared").is_none());
}

#[test]
fn a_shared_account_s_own_mailboxes_appear_under_shared() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox", Some("inbox"));

    let shared = Id::new("A2");
    let projects = {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let sharer = state.add_account(shared.clone(), "Bob");
        sharer.is_personal = false;
        sharer.seed_mailbox("Projects", None)
    };

    let (_, tree) = fixture.sync().folder_tree_with_shared().unwrap();

    assert!(
        tree.find("Inbox").is_some(),
        "the primary account's own folder stays"
    );
    let shared_projects = tree
        .find("Shared/Bob/Projects")
        .expect("the shared account's folder merges in under its owner");
    assert_eq!(shared_projects.id, projects);
}

#[test]
fn a_rebuild_triggered_by_the_primary_account_still_carries_shared_mailboxes() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox", Some("inbox"));

    let shared = Id::new("A2");
    let projects = {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let sharer = state.add_account(shared.clone(), "Bob");
        sharer.is_personal = false;
        sharer.seed_mailbox("Projects", None)
    };

    let sync = fixture.sync();
    let (state, _) = sync.folder_tree_with_shared().unwrap();

    // A structural change to the primary account's own mailboxes, the kind
    // `Mailbox/changes` reports and that makes `folder_tree_since` rebuild
    // the whole tree. The shared account's own mailboxes did not change, but
    // a rebuild must not drop them: only `Mailbox/changes` on the primary
    // account's state decides whether to rebuild, not whether the result
    // still needs the shared merge.
    fixture.edit(|account| account.create_mailbox("Receipts", None, None));

    let tree = match sync.folder_tree_since(&state).unwrap() {
        FolderUpdate::Rebuilt { tree, .. } => tree,
        FolderUpdate::Unchanged(state) => {
            panic!("expected a rebuilt tree, the server reported nothing new at {state}")
        }
    };

    assert!(tree.find("Receipts").is_some(), "the new mailbox is there");
    let shared_projects = tree
        .find("Shared/Bob/Projects")
        .expect("the shared account's folder survives the rebuild");
    assert_eq!(shared_projects.id, projects);
}
