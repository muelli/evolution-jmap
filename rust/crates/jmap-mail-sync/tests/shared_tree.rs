// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Merging shared accounts' folder trees into one, item 163's first slice.
//!
//! `folders.rs`'s own `get_folder_info_sync` wiring is a later claim; this
//! only tests the pure merge in `jmap-mail-sync`.

use jmap_mail_sync::{FolderRole, FolderTree, SharedMailAccount};
use jmap_proto::Id;
use jmap_proto::mail::{Mailbox, MailboxRights, role};

fn mailbox(id: &str, name: &str) -> Mailbox {
    Mailbox::new(name).with_id(Id::new(id))
}

fn child(id: &str, name: &str, parent: &str) -> Mailbox {
    mailbox(id, name).with_parent_id(Id::new(parent))
}

fn with_role(mut mailbox: Mailbox, role: &str) -> Mailbox {
    mailbox.role = Some(role.to_owned());
    mailbox
}

fn with_may_add_items(mut mailbox: Mailbox, allowed: bool) -> Mailbox {
    mailbox.my_rights = Some(MailboxRights::new().with_may_add_items(allowed));
    mailbox
}

fn tree(mailboxes: &[Mailbox]) -> FolderTree {
    FolderTree::from_mailboxes(mailboxes).expect("a well-formed mailbox list")
}

/// The paths of all folders, parents before children.
fn paths(tree: &FolderTree) -> Vec<String> {
    tree.iter().map(|folder| folder.path.clone()).collect()
}

fn owner(id: &str, name: &str, read_only: bool) -> SharedMailAccount {
    SharedMailAccount {
        id: Id::new(id),
        name: name.to_owned(),
        read_only,
    }
}

#[test]
fn no_shared_accounts_leaves_the_tree_unchanged() {
    let primary = tree(&[mailbox("M1", "Inbox")]);

    let merged = primary.clone().merge_shared(&[]);

    assert_eq!(merged, primary);
}

#[test]
fn a_shared_account_s_folders_sit_under_a_shared_root_named_for_the_owner() {
    let primary = tree(&[mailbox("M1", "Inbox")]);
    let shared = tree(&[mailbox("S1", "Inbox"), child("S2", "Work", "S1")]);

    let merged = primary.merge_shared(&[(owner("b", "bob@example.com", false), shared)]);

    assert_eq!(
        paths(&merged),
        [
            "Inbox",
            "Shared",
            "Shared/bob@example.com",
            "Shared/bob@example.com/Inbox",
            "Shared/bob@example.com/Inbox/Work",
        ]
    );
}

#[test]
fn a_shared_folder_keeps_its_own_mailbox_id_and_role() {
    let primary = tree(&[mailbox("M1", "Inbox")]);
    let shared = tree(&[with_role(mailbox("S1", "Inbox"), role::INBOX)]);

    let merged = primary.merge_shared(&[(owner("b", "bob@example.com", false), shared)]);

    let folder = merged
        .find("Shared/bob@example.com/Inbox")
        .expect("the merged folder exists");
    assert_eq!(folder.id, Id::new("S1"));
    assert_eq!(folder.role, Some(FolderRole::Inbox));
}

#[test]
fn the_primary_account_s_own_folders_come_before_the_shared_root() {
    let primary = tree(&[mailbox("M1", "Inbox"), mailbox("M2", "Sent")]);
    let shared = tree(&[mailbox("S1", "Inbox")]);

    let merged = primary.merge_shared(&[(owner("b", "bob@example.com", false), shared)]);

    assert_eq!(
        paths(&merged),
        [
            "Inbox",
            "Sent",
            "Shared",
            "Shared/bob@example.com",
            "Shared/bob@example.com/Inbox"
        ]
    );
}

#[test]
fn a_read_only_shared_account_locks_every_one_of_its_folders() {
    let primary = tree(&[mailbox("M1", "Inbox")]);
    let shared = tree(&[
        with_may_add_items(mailbox("S1", "Inbox"), true),
        child("S2", "Work", "S1"),
    ]);

    let merged = primary.merge_shared(&[(owner("b", "bob@example.com", true), shared)]);

    assert_eq!(
        merged
            .find("Shared/bob@example.com/Inbox")
            .expect("the merged folder exists")
            .may_add_items,
        Some(false),
        "the account's own read-only flag overrides a looser myRights"
    );
    assert_eq!(
        merged
            .find("Shared/bob@example.com/Inbox/Work")
            .expect("the merged child folder exists")
            .may_add_items,
        Some(false),
        "a child folder is locked too, not only the account's top level"
    );
}

#[test]
fn a_writable_shared_account_keeps_each_folder_s_own_myrights() {
    let primary = tree(&[mailbox("M1", "Inbox")]);
    let shared = tree(&[with_may_add_items(mailbox("S1", "Inbox"), true)]);

    let merged = primary.merge_shared(&[(owner("b", "bob@example.com", false), shared)]);

    assert_eq!(
        merged
            .find("Shared/bob@example.com/Inbox")
            .expect("the merged folder exists")
            .may_add_items,
        Some(true)
    );
}

#[test]
fn two_shared_accounts_each_get_their_own_branch_in_the_given_order() {
    let primary = tree(&[mailbox("M1", "Inbox")]);
    let bob = tree(&[mailbox("S1", "Inbox")]);
    let alice = tree(&[mailbox("S2", "Inbox")]);

    let merged = primary.merge_shared(&[
        (owner("b", "bob@example.com", false), bob),
        (owner("a", "alice@example.com", false), alice),
    ]);

    assert_eq!(
        paths(&merged),
        [
            "Inbox",
            "Shared",
            "Shared/bob@example.com",
            "Shared/bob@example.com/Inbox",
            "Shared/alice@example.com",
            "Shared/alice@example.com/Inbox",
        ]
    );
}

#[test]
fn an_owner_name_with_a_separator_does_not_split_into_two_path_components() {
    let primary = tree(&[mailbox("M1", "Inbox")]);
    let shared = tree(&[mailbox("S1", "Inbox")]);

    let merged = primary.merge_shared(&[(owner("b", "bob/fake", false), shared)]);

    assert_eq!(
        paths(&merged),
        [
            "Inbox",
            "Shared",
            "Shared/bob%2Ffake",
            "Shared/bob%2Ffake/Inbox"
        ]
    );
}
