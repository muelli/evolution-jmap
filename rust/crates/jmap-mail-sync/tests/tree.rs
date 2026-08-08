// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The mailbox-list-to-folder-tree mapping, on hand-written mailbox lists.
//!
//! Fixtures rather than a live server, because the interesting inputs here are
//! the ones no correct server produces: a name with a `/` in it, a parent that
//! is not in the list, a cycle. `folders.rs` covers the well-behaved case
//! against `jmap-mockd`.

use jmap_mail_sync::{FolderRole, FolderTree};
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
