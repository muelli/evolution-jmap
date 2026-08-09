// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The mailbox-list-to-folder-tree mapping, on hand-written mailbox lists.
//!
//! Fixtures rather than a live server, because the interesting inputs here are
//! the ones no correct server produces: a name with a `/` in it, a parent that
//! is not in the list, a cycle. `folders.rs` covers the well-behaved case
//! against `jmap-mockd`.

use jmap_mail_sync::{FolderInfo, FolderRole, FolderTree};
use jmap_proto::Id;
use jmap_proto::mail::{Mailbox, role};

/// A mailbox with an id, a name and nothing else set.
fn mailbox(id: &str, name: &str) -> Mailbox {
    Mailbox {
        id: Some(Id::new(id)),
        name: name.to_owned(),
        ..Mailbox::default()
    }
}

fn child(id: &str, name: &str, parent: &str) -> Mailbox {
    Mailbox {
        parent_id: Some(Id::new(parent)),
        ..mailbox(id, name)
    }
}

fn with_role(mut mailbox: Mailbox, role: &str) -> Mailbox {
    mailbox.role = Some(role.to_owned());
    mailbox
}

fn with_sort_order(mut mailbox: Mailbox, sort_order: u32) -> Mailbox {
    mailbox.sort_order = Some(sort_order);
    mailbox
}

fn tree(mailboxes: &[Mailbox]) -> FolderTree {
    FolderTree::from_mailboxes(mailboxes).expect("a well-formed mailbox list")
}

/// The paths of every folder, parents before children.
fn paths(tree: &FolderTree) -> Vec<String> {
    tree.iter().map(|folder| folder.path.clone()).collect()
}

#[test]
fn a_flat_list_becomes_top_level_folders() {
    let tree = tree(&[mailbox("M1", "Inbox"), mailbox("M2", "Sent")]);

    assert_eq!(paths(&tree), ["Inbox", "Sent"]);
    assert!(tree.roots().iter().all(|folder| folder.children.is_empty()));
}

#[test]
fn siblings_are_ordered_by_sort_order_then_name() {
    let tree = tree(&[
        with_sort_order(mailbox("M1", "Zebra"), 1),
        with_sort_order(mailbox("M2", "Beta"), 7),
        with_sort_order(mailbox("M3", "Alpha"), 7),
        // No sortOrder at all sorts as 0 — RFC 8621's own default.
        mailbox("M4", "Omega"),
    ]);

    assert_eq!(paths(&tree), ["Omega", "Zebra", "Alpha", "Beta"]);
}

#[test]
fn a_child_hangs_off_its_parent_and_its_path_carries_it() {
    let tree = tree(&[
        mailbox("M1", "Projects"),
        child("M2", "JMAP", "M1"),
        child("M3", "Drafts", "M2"),
    ]);

    assert_eq!(
        paths(&tree),
        ["Projects", "Projects/JMAP", "Projects/JMAP/Drafts"]
    );
    let projects = &tree.roots()[0];
    assert_eq!(projects.children.len(), 1);
    assert_eq!(projects.children[0].children[0].display_name, "Drafts");
    // The display name is the mailbox name, never the path.
    assert_eq!(projects.children[0].display_name, "JMAP");
}

#[test]
fn subfolders_are_ordered_among_themselves() {
    let tree = tree(&[
        mailbox("M1", "Projects"),
        child("M2", "Zebra", "M1"),
        child("M3", "Alpha", "M1"),
    ]);

    assert_eq!(
        paths(&tree),
        ["Projects", "Projects/Alpha", "Projects/Zebra"]
    );
}

#[test]
fn iter_yields_a_parent_before_its_children() {
    let tree = tree(&[
        child("M2", "JMAP", "M1"),
        mailbox("M1", "Projects"),
        mailbox("M3", "Sent"),
    ]);

    assert_eq!(paths(&tree), ["Projects", "Projects/JMAP", "Sent"]);
}

#[test]
fn known_roles_map_and_unknown_ones_do_not() {
    let tree = tree(&[
        with_role(mailbox("M1", "Inbox"), role::INBOX),
        with_role(mailbox("M2", "Papierkorb"), role::TRASH),
        with_role(mailbox("M3", "Important"), "important"),
        mailbox("M4", "Project"),
    ]);
    let roles: Vec<(String, Option<FolderRole>)> = tree
        .iter()
        .map(|folder| (folder.display_name.clone(), folder.role))
        .collect();

    assert_eq!(
        roles,
        [
            ("Important".to_owned(), None),
            ("Inbox".to_owned(), Some(FolderRole::Inbox)),
            ("Papierkorb".to_owned(), Some(FolderRole::Trash)),
            ("Project".to_owned(), None),
        ]
    );
}

#[test]
fn a_role_in_the_wrong_case_is_still_the_role() {
    let tree = tree(&[with_role(mailbox("M1", "Inbox"), "Inbox")]);

    assert_eq!(tree.roots()[0].role, Some(FolderRole::Inbox));
}

#[test]
fn only_the_first_mailbox_claiming_a_role_keeps_it() {
    let tree = tree(&[
        with_role(
            with_sort_order(mailbox("M1", "Second inbox"), 5),
            role::INBOX,
        ),
        with_role(with_sort_order(mailbox("M2", "Real inbox"), 1), role::INBOX),
    ]);

    let by_name: Vec<(String, Option<FolderRole>)> = tree
        .iter()
        .map(|folder| (folder.display_name.clone(), folder.role))
        .collect();
    assert_eq!(
        by_name,
        [
            ("Real inbox".to_owned(), Some(FolderRole::Inbox)),
            ("Second inbox".to_owned(), None),
        ]
    );
}

#[test]
fn a_slash_in_a_name_cannot_invent_a_level() {
    let tree = tree(&[mailbox("M1", "and/or"), mailbox("M2", "100% done")]);

    assert_eq!(paths(&tree), ["100%25 done", "and%2For"]);
    // What the user is shown is unaffected.
    assert_eq!(
        tree.find("and%2For")
            .map(|folder| folder.display_name.clone()),
        Some("and/or".to_owned())
    );
}

#[test]
fn a_name_made_of_dots_cannot_escape_the_cache_directory() {
    let tree = tree(&[
        mailbox("M1", ".."),
        mailbox("M2", "."),
        mailbox("M3", "..."),
    ]);

    // A path component Camel would resolve as a directory traversal, and one
    // it would resolve as "the same directory", are both encoded away. Three
    // dots means nothing to a filesystem, so it survives verbatim.
    assert_eq!(paths(&tree), ["%2E", "%2E%2E", "..."]);
}

#[test]
fn a_nul_in_a_name_cannot_truncate_the_path() {
    // The path crosses into C as a string; a NUL in it would either fail to
    // convert or silently name a different folder.
    let tree = tree(&[mailbox("M1", "Inbox\0Sent")]);

    assert_eq!(paths(&tree), ["Inbox%00Sent"]);
}

#[test]
fn siblings_with_the_same_name_still_get_distinct_paths() {
    // Illegal per RFC 8621 §2, but a store that maps two mailboxes onto one
    // path hands back the wrong folder's mail, so it must not be possible.
    let tree = tree(&[mailbox("M1", "Inbox"), mailbox("M2", "Inbox")]);
    let paths = paths(&tree);

    assert_eq!(paths, ["Inbox", "Inbox%23M2"]);
    assert_eq!(
        tree.find("Inbox").map(|f| f.id.clone()),
        Some(Id::new("M1"))
    );
    assert_eq!(
        tree.find("Inbox%23M2").map(|f| f.id.clone()),
        Some(Id::new("M2"))
    );

    // Which of the two keeps the plain path must not depend on the order the
    // server happened to list them in — the path is remembered in the summary
    // cache and in saved filters, and it may not move between sessions.
    let reversed = FolderTree::from_mailboxes(&[mailbox("M2", "Inbox"), mailbox("M1", "Inbox")])
        .expect("a well-formed mailbox list");
    assert_eq!(
        reversed.find("Inbox").map(|f| f.id.clone()),
        Some(Id::new("M1"))
    );
}

#[test]
fn an_orphan_is_kept_as_a_top_level_folder() {
    // Its parent may be a mailbox this account cannot see. Dropping it would
    // be hiding mail; a top-level folder is visible and correct enough.
    let tree = tree(&[mailbox("M1", "Inbox"), child("M2", "Shared", "M99")]);

    assert_eq!(paths(&tree), ["Inbox", "Shared"]);
}

#[test]
fn a_mailbox_that_is_its_own_parent_is_a_root() {
    let tree = tree(&[child("M1", "Inbox", "M1")]);

    assert_eq!(paths(&tree), ["Inbox"]);
    assert!(tree.roots()[0].children.is_empty(), "not its own subfolder");
}

#[test]
fn a_parent_cycle_does_not_hang_and_keeps_every_mailbox() {
    let tree = tree(&[
        mailbox("M1", "Inbox"),
        child("M2", "A", "M3"),
        child("M3", "B", "M2"),
    ]);

    // The cycle is cut at its first member in sibling order, which leaves a
    // forest: nothing is lost, nothing is reachable twice.
    assert_eq!(paths(&tree), ["Inbox", "A", "A/B"]);
    assert_eq!(tree.len(), 3);
}

#[test]
fn counts_come_from_the_mailbox_and_saturate() {
    let tree = tree(&[
        Mailbox {
            total_emails: Some(12),
            unread_emails: Some(3),
            ..mailbox("M1", "Inbox")
        },
        Mailbox {
            // Just past 32 bits: a wrapping cast would report five messages,
            // which is a plausible-looking lie rather than an obvious one.
            total_emails: Some(u64::from(u32::MAX) + 5),
            ..mailbox("M2", "Absurd")
        },
        mailbox("M3", "Quiet"),
    ]);
    let counts: Vec<(u32, u32)> = tree
        .iter()
        .map(|folder| (folder.total, folder.unread))
        .collect();

    // Camel counts are 32-bit; saturating beats wrapping to a small number.
    // "Absurd" sorts before "Inbox" before "Quiet".
    assert_eq!(counts, [(u32::MAX, 0), (12, 3), (0, 0)]);
}

#[test]
fn an_absent_subscription_flag_means_subscribed() {
    // A server that does not model subscriptions must not end up with every
    // folder hidden.
    let tree = tree(&[
        mailbox("M1", "Inbox"),
        Mailbox {
            is_subscribed: Some(false),
            ..mailbox("M2", "Noisy list")
        },
    ]);
    let subscribed: Vec<bool> = tree.iter().map(|folder| folder.subscribed).collect();

    assert_eq!(subscribed, [true, false]);
}

#[test]
fn find_resolves_a_nested_path_and_rejects_an_unknown_one() {
    let tree = tree(&[mailbox("M1", "Projects"), child("M2", "JMAP", "M1")]);

    assert_eq!(
        tree.find("Projects/JMAP").map(|folder| folder.id.clone()),
        Some(Id::new("M2"))
    );
    assert!(tree.find("Projects/Other").is_none());
    assert!(tree.find("").is_none());
}

#[test]
fn an_empty_list_is_an_empty_tree() {
    let tree = tree(&[]);

    assert!(tree.is_empty());
    assert_eq!(tree.len(), 0);
    assert!(tree.roots().is_empty());
}

#[test]
fn a_mailbox_without_an_id_is_a_protocol_error() {
    let nameless = Mailbox {
        id: None,
        name: "Inbox".to_owned(),
        ..Mailbox::default()
    };

    let error = FolderTree::from_mailboxes(&[nameless]).expect_err("an id is not optional");

    assert!(error.to_string().contains("id"), "{error}");
}

#[test]
fn a_repeated_id_is_a_protocol_error() {
    // Two mailboxes with one id make the id-based tie-break above meaningless,
    // and there is no answer that is right for both.
    let error = FolderTree::from_mailboxes(&[mailbox("M1", "Inbox"), mailbox("M1", "Sent")])
        .expect_err("an id identifies one mailbox");

    assert!(error.to_string().contains("twice"), "{error}");
}

#[test]
fn a_mailbox_without_a_name_is_a_protocol_error() {
    let error =
        FolderTree::from_mailboxes(&[mailbox("M1", "")]).expect_err("a name is not optional");

    assert!(error.to_string().contains("name"), "{error}");
}

// ---------------------------------------------------------------------------
// finding the folder a role names

/// What `camel_store_get_inbox_folder_sync` is answered from. The role is a
/// property of the mailbox, not of its name or its place in the tree, so the
/// lookup has to reach every level — a JMAP server is free to put the inbox
/// under another mailbox, and several do.
#[test]
fn a_role_finds_its_folder_wherever_it_sits() {
    let tree = tree(&[
        mailbox("M1", "Accounts"),
        with_role(child("M2", "Inbox", "M1"), role::INBOX),
    ]);

    let inbox = tree.role(FolderRole::Inbox).expect("the account's inbox");
    assert_eq!(inbox.path, "Accounts/Inbox");
    assert_eq!(inbox.id, Id::new("M2"));
}

/// An account whose server assigns no roles at all — legal, since RFC 8621 §2
/// makes `role` nullable on every mailbox. There is no inbox to answer with,
/// and inventing one out of a mailbox called "Inbox" would be this provider
/// guessing where the user's mail arrives.
#[test]
fn a_role_no_mailbox_claims_is_not_found() {
    let tree = tree(&[mailbox("M1", "Inbox"), mailbox("M2", "Sent")]);

    assert!(tree.role(FolderRole::Inbox).is_none());
    assert!(tree.role(FolderRole::Trash).is_none());
}

/// Two mailboxes claiming one role is a server bug, and the tree already
/// settles it by giving the role to the first in sibling order. The lookup must
/// agree with that decision rather than take the first mailbox whose *JMAP*
/// role says inbox, or the folder Camel opens as the inbox would not be the one
/// the folder listing marked `CAMEL_FOLDER_TYPE_INBOX`.
#[test]
fn the_role_is_found_on_the_folder_that_kept_it() {
    let tree = tree(&[
        with_role(
            with_sort_order(mailbox("M1", "Second inbox"), 5),
            role::INBOX,
        ),
        with_role(with_sort_order(mailbox("M2", "Real inbox"), 1), role::INBOX),
    ]);

    let inbox = tree.role(FolderRole::Inbox).expect("the account's inbox");
    assert_eq!(inbox.display_name, "Real inbox");
}

// ---------------------------------------------------------------------------
// the one editable property

/// Subscription is the one thing in a listing the *client* decides, so it is
/// the one thing a tree can be told rather than re-listed for. By id, and at
/// any depth: the walk that finds the folder is the same one `iter` does, and a
/// subscription editor's ticks are mostly on folders well below the roots.
#[test]
fn a_subscription_can_be_recorded_on_a_folder_at_any_depth() {
    let mut tree = tree(&[
        mailbox("M1", "Projects"),
        child("M2", "JMAP", "M1"),
        child("M3", "Drafts", "M2"),
    ]);

    assert!(tree.set_subscribed(&Id::new("M3"), false));

    assert_eq!(
        paths_and_subscriptions(&tree),
        [
            ("Projects", true),
            ("Projects/JMAP", true),
            ("Projects/JMAP/Drafts", false),
        ]
    );
}

/// And a mailbox the tree does not have is reported rather than ignored: the
/// caller is a write that already succeeded against the server, and "the tree
/// predates that folder" is a different situation from "the tree was updated".
#[test]
fn a_subscription_for_a_mailbox_the_tree_does_not_have_changes_nothing() {
    let mut tree = tree(&[mailbox("M1", "Inbox")]);

    assert!(!tree.set_subscribed(&Id::new("M404"), false));

    assert_eq!(paths_and_subscriptions(&tree), [("Inbox", true)]);
}

fn paths_and_subscriptions(tree: &FolderTree) -> Vec<(&str, bool)> {
    tree.iter()
        .map(|folder| (folder.path.as_str(), folder.subscribed))
        .collect()
}

// ---------------------------------------------------------------------------
// the two folders a user adds and takes away

/// A folder made out of nothing, for the tests below: what
/// [`MailSync::create_folder`] answers with, which is where the real ones come
/// from.
fn made(id: &str, path: &str) -> FolderInfo {
    FolderInfo {
        id: Id::new(id),
        path: path.to_owned(),
        display_name: path.rsplit('/').next().unwrap_or(path).to_owned(),
        role: None,
        total: 0,
        unread: 0,
        subscribed: true,
        children: Vec::new(),
    }
}

/// The other half of `create_folder_sync`: the folder the server just made has
/// to join the listing the store is holding, or it is a folder Evolution was
/// told about and cannot open until something refreshes the account.
#[test]
fn a_created_folder_joins_the_roots_when_its_path_has_no_parent_in_it() {
    let mut tree = tree(&[mailbox("M1", "Inbox")]);

    assert!(tree.insert(made("M2", "Projects")));

    assert_eq!(paths(&tree), ["Inbox", "Projects"]);
}

/// And under the folder its path names when it has one — the path is what says
/// where in the tree the folder belongs, because it is built out of the
/// parent's.
#[test]
fn a_created_folder_joins_the_children_of_the_folder_its_path_names() {
    let mut tree = tree(&[mailbox("M1", "Projects"), child("M2", "JMAP", "M1")]);

    assert!(tree.insert(made("M3", "Projects/JMAP/Drafts")));

    assert_eq!(
        paths(&tree),
        ["Projects", "Projects/JMAP", "Projects/JMAP/Drafts"]
    );
}

/// A path whose parent is not in the tree is reported rather than guessed at.
/// The alternative — hanging it off the roots — would be a folder drawn at the
/// top level of the account that the server has somewhere else entirely.
#[test]
fn a_created_folder_whose_parent_is_missing_is_not_inserted() {
    let mut tree = tree(&[mailbox("M1", "Inbox")]);

    assert!(!tree.insert(made("M2", "Projects/JMAP")));

    assert_eq!(paths(&tree), ["Inbox"]);
}

/// The tree may be out of date in the one direction the server cannot correct:
/// it can still hold a folder another client removed, and a create then makes a
/// second folder with that same path. The new one is the one the server just
/// confirmed, so it is the one that stays.
#[test]
fn a_created_folder_replaces_a_stale_namesake() {
    let mut tree = tree(&[mailbox("M1", "Projects")]);

    assert!(tree.insert(made("M2", "Projects")));

    assert_eq!(paths(&tree), ["Projects"]);
    assert_eq!(
        tree.find("Projects").map(|folder| folder.id.as_str()),
        Some("M2")
    );
}

/// The removal half, and by id for [`FolderTree::set_subscribed`]'s reason: the
/// caller is a write that named a mailbox, and a path is this crate's invention.
#[test]
fn a_removed_folder_leaves_the_tree() {
    let mut tree = tree(&[mailbox("M1", "Inbox"), mailbox("M2", "Projects")]);

    assert!(tree.remove(&Id::new("M2")));

    assert_eq!(paths(&tree), ["Inbox"]);
}

/// What hung under it goes with it. RFC 8621 §2.5 makes the server refuse to
/// destroy a mailbox that still has children, so a tree that has some describes
/// folders another client already removed — and leaving them would be folders
/// under a parent that is not there.
#[test]
fn a_removed_folder_takes_its_subtree_with_it() {
    let mut tree = tree(&[
        mailbox("M1", "Projects"),
        child("M2", "JMAP", "M1"),
        child("M3", "Drafts", "M2"),
        mailbox("M4", "Inbox"),
    ]);

    assert!(tree.remove(&Id::new("M2")));

    assert_eq!(paths(&tree), ["Inbox", "Projects"]);
}

/// And a mailbox the tree does not have is reported rather than ignored, the
/// same answer a subscription for one gets.
#[test]
fn removing_a_mailbox_the_tree_does_not_have_changes_nothing() {
    let mut tree = tree(&[mailbox("M1", "Inbox")]);

    assert!(!tree.remove(&Id::new("M404")));

    assert_eq!(paths(&tree), ["Inbox"]);
}

// ---------------------------------------------------------------------------
// the folder a user renames, which is also the one they drag somewhere else

/// The other half of `rename_folder_sync`, and the reason the tree needs an
/// edit of its own rather than a remove and an insert: a folder's path is where
/// it sits, so renaming one moves every path underneath it.
#[test]
fn a_renamed_folder_takes_its_new_name_and_path() {
    let mut tree = tree(&[mailbox("M1", "Inbox"), mailbox("M2", "Projects")]);

    assert!(tree.rename(&Id::new("M2"), "Work", "Work"));

    assert_eq!(paths(&tree), ["Inbox", "Work"]);
    assert_eq!(
        tree.find("Work").map(|folder| folder.display_name.as_str()),
        Some("Work")
    );
}

/// The name is passed beside the path because the path cannot be read back into
/// one: the encoding this crate applies to a mailbox name has no decoder here,
/// and the caller has the name it just sent to the server.
#[test]
fn a_renamed_folder_shows_the_name_rather_than_the_path_component() {
    let mut tree = tree(&[mailbox("M1", "Projects")]);

    assert!(tree.rename(&Id::new("M1"), "and%2For", "and/or"));

    assert_eq!(paths(&tree), ["and%2For"]);
    assert_eq!(
        tree.find("and%2For")
            .map(|folder| folder.display_name.as_str()),
        Some("and/or")
    );
}

/// A move: the new path names some other parent, and the folder hangs under it
/// afterwards — the same reading of a path [`FolderTree::insert`] has.
#[test]
fn a_moved_folder_joins_the_children_of_the_folder_its_new_path_names() {
    let mut tree = tree(&[mailbox("M1", "Notes"), mailbox("M2", "Work")]);

    assert!(tree.rename(&Id::new("M1"), "Work/Notes", "Notes"));

    assert_eq!(paths(&tree), ["Work", "Work/Notes"]);
}

/// And back up to the account's top level, which is the move that has no parent
/// in its path at all. It joins its new siblings at the end of them, as a
/// created folder does: sibling order is the server's — sortOrder, then name —
/// and this side has just been told about one folder, not about the order the
/// account puts it in.
#[test]
fn a_folder_moved_to_the_top_level_joins_the_roots() {
    let mut tree = tree(&[mailbox("M1", "Work"), child("M2", "Notes", "M1")]);

    assert!(tree.rename(&Id::new("M2"), "Notes", "Notes"));

    assert_eq!(paths(&tree), ["Work", "Notes"]);
}

/// What hangs under the folder goes with it, and every one of those paths is
/// rewritten: a descendant left at its old path is a folder Camel would key by
/// a string that names nothing.
#[test]
fn a_moved_folder_brings_its_subtree_to_the_new_path() {
    let mut tree = tree(&[
        mailbox("M1", "Projects"),
        child("M2", "JMAP", "M1"),
        child("M3", "Drafts", "M2"),
        mailbox("M4", "Archive"),
    ]);

    assert!(tree.rename(&Id::new("M2"), "Archive/JMAP", "JMAP"));

    assert_eq!(
        paths(&tree),
        ["Archive", "Archive/JMAP", "Archive/JMAP/Drafts", "Projects"]
    );
}

/// Everything a rename does not touch survives it. The server changed two
/// properties of one mailbox; the counts, the role, the subscription and the id
/// are the same folder's as before.
#[test]
fn a_renamed_folder_keeps_what_the_rename_did_not_change() {
    let mut tree = tree(&[Mailbox {
        total_emails: Some(12),
        unread_emails: Some(3),
        is_subscribed: Some(false),
        ..with_role(mailbox("M1", "Inbox"), role::INBOX)
    }]);

    assert!(tree.rename(&Id::new("M1"), "Post", "Post"));

    let folder = tree.find("Post").expect("the renamed folder");
    assert_eq!(folder.id, Id::new("M1"));
    assert_eq!((folder.total, folder.unread), (12, 3));
    assert!(!folder.subscribed);
    assert_eq!(folder.role, Some(FolderRole::Inbox));
}

/// A new path whose parent is not in the tree is reported rather than guessed
/// at, the answer [`FolderTree::insert`] gives the same question — and here the
/// folder must also still be where it was, because the alternative to guessing
/// is not "lose it".
#[test]
fn a_rename_under_a_parent_the_tree_does_not_have_changes_nothing() {
    let mut tree = tree(&[mailbox("M1", "Notes")]);

    assert!(!tree.rename(&Id::new("M1"), "Work/Notes", "Notes"));

    assert_eq!(paths(&tree), ["Notes"]);
}

/// A folder cannot be moved inside itself. The server refuses it, so this only
/// happens to a tree asked to do something the account never did — and the one
/// thing it must not do is take the subtree out and have nowhere to put it back.
#[test]
fn a_move_into_the_folders_own_subtree_changes_nothing() {
    let mut tree = tree(&[mailbox("M1", "Work"), child("M2", "Notes", "M1")]);

    assert!(!tree.rename(&Id::new("M1"), "Work/Notes/Work", "Work"));

    assert_eq!(paths(&tree), ["Work", "Work/Notes"]);
}

/// The stale namesake a create can find at its destination, which a move can
/// find at its own: of two folders at one path, the one the server has just
/// confirmed is the one that stays.
#[test]
fn a_moved_folder_replaces_a_stale_namesake() {
    let mut tree = tree(&[mailbox("M1", "Notes"), mailbox("M2", "Work")]);

    assert!(tree.rename(&Id::new("M2"), "Notes", "Notes"));

    assert_eq!(paths(&tree), ["Notes"]);
    assert_eq!(
        tree.find("Notes").map(|folder| folder.id.as_str()),
        Some("M2")
    );
}

/// And a mailbox the tree does not have is reported rather than ignored, as
/// every other edit here reports one.
#[test]
fn renaming_a_mailbox_the_tree_does_not_have_changes_nothing() {
    let mut tree = tree(&[mailbox("M1", "Inbox")]);

    assert!(!tree.rename(&Id::new("M404"), "Work", "Work"));

    assert_eq!(paths(&tree), ["Inbox"]);
}
