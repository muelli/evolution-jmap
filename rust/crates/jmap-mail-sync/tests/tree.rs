// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The mapping from a mailbox list to a folder tree, with hand-written lists.
//!
//! The tests use fixtures, not a server, because the important inputs come
//! from no correct server: a name with a `/`, a parent that is not in the
//! list, a cycle. `folders.rs` tests the correct case against `jmap-mock`.

use jmap_mail_sync::{FolderInfo, FolderRole, FolderTree, path};
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

/// The paths of all folders, parents before children.
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
        // No sortOrder sorts as 0, the default of RFC 8621.
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
    // The display name does not change.
    assert_eq!(
        tree.find("and%2For")
            .map(|folder| folder.display_name.clone()),
        Some("and/or".to_owned())
    );
}

#[test]
fn a_path_splits_back_into_its_parents_path_and_its_own_component() {
    let tree = tree(&[
        mailbox("M1", "Projects"),
        child("M2", "and/or", "M1"),
        child("M3", "Drafts", "M2"),
    ]);

    assert_eq!(
        paths(&tree),
        ["Projects", "Projects/and%2For", "Projects/and%2For/Drafts"]
    );
    // A top-level folder has no parent, and its path is its component.
    assert_eq!(path::split("Projects"), (None, "Projects"));
    // The escaped slash is never mistaken for a level.
    assert_eq!(
        path::split("Projects/and%2For"),
        (Some("Projects"), "and%2For")
    );
    // Only the last separator splits.
    assert_eq!(
        path::split("Projects/and%2For/Drafts"),
        (Some("Projects/and%2For"), "Drafts")
    );
}

#[test]
fn a_name_made_of_dots_cannot_escape_the_cache_directory() {
    let tree = tree(&[
        mailbox("M1", ".."),
        mailbox("M2", "."),
        mailbox("M3", "..."),
    ]);

    // Encode the components that Camel reads as "parent directory" or "same
    // directory". Three dots have no meaning to a filesystem, so they stay.
    assert_eq!(paths(&tree), ["%2E", "%2E%2E", "..."]);
}

#[test]
fn a_nul_in_a_name_cannot_truncate_the_path() {
    // The path goes to C as a string. A NUL in it makes the conversion fail, or
    // names a different folder.
    let tree = tree(&[mailbox("M1", "Inbox\0Sent")]);

    assert_eq!(paths(&tree), ["Inbox%00Sent"]);
}

#[test]
fn siblings_with_the_same_name_still_get_distinct_paths() {
    // RFC 8621 §2 forbids this. But if two mailboxes have one path, the store
    // returns the mail of the wrong folder, so this must not occur.
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

    // The listing order must not decide which folder keeps the plain path. The
    // summary cache and saved filters keep the path, so it must stay the same
    // between sessions.
    let reversed = FolderTree::from_mailboxes(&[mailbox("M2", "Inbox"), mailbox("M1", "Inbox")])
        .expect("a well-formed mailbox list");
    assert_eq!(
        reversed.find("Inbox").map(|f| f.id.clone()),
        Some(Id::new("M1"))
    );
}

#[test]
fn an_orphan_is_kept_as_a_top_level_folder() {
    // The parent can be a mailbox that this account cannot see. If the client
    // drops the orphan, it hides mail. A top-level folder is visible.
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

    // The cut is at the first cycle member in sibling order. The result is a
    // forest: no mailbox is lost, and no mailbox occurs twice.
    assert_eq!(paths(&tree), ["Inbox", "A", "A/B"]);
    assert_eq!(tree.len(), 3);
}

#[test]
fn a_mailbox_hanging_off_a_cycle_stays_under_its_parent() {
    let tree = tree(&[
        mailbox("M1", "Inbox"),
        child("M2", "A", "M3"),
        child("M3", "B", "M2"),
        child("M4", "C", "M3"),
    ]);

    // The cut removes only one link, from A. B is still the parent of C, so
    // C must not become a second top-level folder.
    assert_eq!(paths(&tree), ["Inbox", "A", "A/B", "A/B/C"]);
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
            // Just more than 32 bits. A wrapping cast gives five messages,
            // which is wrong but looks correct.
            total_emails: Some(u64::from(u32::MAX) + 5),
            ..mailbox("M2", "Absurd")
        },
        mailbox("M3", "Quiet"),
    ]);
    let counts: Vec<(u32, u32)> = tree
        .iter()
        .map(|folder| (folder.total, folder.unread))
        .collect();

    // Camel counts are 32-bit. Saturation is better than a wrap to a small
    // number. The order is "Absurd", "Inbox", "Quiet".
    assert_eq!(counts, [(u32::MAX, 0), (12, 3), (0, 0)]);
}

#[test]
fn an_absent_subscription_flag_means_subscribed() {
    // If a server has no subscriptions, the client must not hide all folders.
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
fn a_tree_with_one_mailbox_is_not_empty() {
    let tree = tree(&[mailbox("M1", "Inbox")]);

    assert!(!tree.is_empty());
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
    // Two mailboxes with one id break the id tie-break above, and no result is
    // correct for both.
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
// role lookup

/// The source for `camel_store_get_inbox_folder_sync`. The role is a property
/// of the mailbox, not of its name or position. Thus the lookup must search all
/// levels: a JMAP server can put the inbox below a different mailbox.
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

/// A server that gives no roles. This is valid: RFC 8621 §2 makes `role`
/// nullable. There is no inbox. A mailbox called "Inbox" is only a guess about
/// where mail arrives, so the provider does not use it.
#[test]
fn a_role_no_mailbox_claims_is_not_found() {
    let tree = tree(&[mailbox("M1", "Inbox"), mailbox("M2", "Sent")]);

    assert!(tree.role(FolderRole::Inbox).is_none());
    assert!(tree.role(FolderRole::Trash).is_none());
}

/// Two mailboxes with one role are a server bug. The tree gives the role to the
/// first in sibling order, and the lookup must agree. Else, Camel opens a
/// different inbox than the folder marked `CAMEL_FOLDER_TYPE_INBOX`.
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

/// The reverse direction: the JMAP name of a role. A caller puts it in its
/// error when the account has no such mailbox. The name must go back through
/// [`FolderRole::from_jmap`], else this crate does not recognise it. The test
/// compares with the `jmap_proto` constants, not literals, because the code
/// compares a server `role` with these constants.
#[test]
fn every_role_is_named_by_the_jmap_role_it_is_read_from() {
    let roles = [
        (FolderRole::Inbox, role::INBOX),
        (FolderRole::Drafts, role::DRAFTS),
        (FolderRole::Sent, role::SENT),
        (FolderRole::Trash, role::TRASH),
        (FolderRole::Junk, role::JUNK),
        (FolderRole::Archive, role::ARCHIVE),
    ];

    for (role, name) in roles {
        assert_eq!(role.as_jmap(), name, "{role:?}");
        assert_eq!(
            FolderRole::from_jmap(role.as_jmap()),
            Some(role),
            "{role:?}"
        );
    }
}

/// [`FolderRole::from_jmap`] maps an unknown mailbox role to `None`. Examples
/// are `"snoozed"` from `draft-ietf-extra-email-snooze`, other roles from the
/// RFC 8457 registry (`\Important`, `\Flagged`, …), and vendor extensions.
///
/// In Evolution, `role: None` maps to `CAMEL_FOLDER_TYPE_NORMAL`. Thus a
/// mailbox with snoozed messages becomes a normal folder, and the
/// `Mailbox/get` response does NOT fail.
#[test]
fn unrecognised_and_snoozed_mailbox_roles_degrade_to_normal_folders() {
    let unrecognised_roles = [
        "snoozed",
        "Snoozed",
        "SNOOZED",
        "important",
        "flagged",
        "starred",
        "promotions",
        "social",
        "updates",
        "forums",
        "quarantine",
        "custom_vendor_role",
        "",
    ];

    for role_name in unrecognised_roles {
        assert_eq!(
            FolderRole::from_jmap(role_name),
            None,
            "role '{role_name}' should map to None (normal folder)"
        );
    }

    // Deserialize a full Mailbox/get payload with standard mailboxes and a
    // snoozed mailbox with extensions (RFC 8621, draft-ietf-extra-email-snooze).
    let payload = serde_json::json!({
        "accountId": "acc_main",
        "state": "state_123",
        "list": [
            {
                "id": "M_inbox",
                "name": "Inbox",
                "role": "inbox",
                "sortOrder": 1,
                "totalEmails": 42,
                "unreadEmails": 5
            },
            {
                "id": "M_snoozed",
                "name": "Snoozed",
                "role": "snoozed",
                "sortOrder": 2,
                "totalEmails": 3,
                "unreadEmails": 0,
                "snoozeInfo": {
                    "activeUntil": "2026-09-01T12:00:00Z"
                }
            },
            {
                "id": "M_sent",
                "name": "Sent",
                "role": "sent",
                "sortOrder": 3
            },
            {
                "id": "M_custom",
                "name": "Important Topics",
                "role": "important",
                "sortOrder": 4
            }
        ]
    });

    let response: jmap_proto::methods::GetResponse<Mailbox> =
        serde_json::from_value(payload).expect("GetResponse<Mailbox> deserializes");
    assert_eq!(response.list.len(), 4);

    let tree = FolderTree::from_mailboxes(&response.list).expect("builds FolderTree");
    assert_eq!(tree.len(), 4);

    // Standard folders keep their roles.
    assert_eq!(
        tree.role(FolderRole::Inbox).map(|f| f.id.as_str()),
        Some("M_inbox")
    );
    assert_eq!(
        tree.role(FolderRole::Sent).map(|f| f.id.as_str()),
        Some("M_sent")
    );

    // Snoozed and important folders become normal folders (role: None).
    let snoozed_folder = tree.find("Snoozed").expect("snoozed folder exists in tree");
    assert_eq!(snoozed_folder.id.as_str(), "M_snoozed");
    assert_eq!(snoozed_folder.role, None);
    assert_eq!(snoozed_folder.total, 3);
    assert_eq!(snoozed_folder.unread, 0);

    let important_folder = tree
        .find("Important Topics")
        .expect("important folder exists");
    assert_eq!(important_folder.id.as_str(), "M_custom");
    assert_eq!(important_folder.role, None);
}

// ---------------------------------------------------------------------------
// subscription, the only editable property

/// The *client* decides the subscription, so the tree can record it without a
/// new listing. The lookup is by id, at all depths, because a subscription
/// editor usually changes folders far below the roots.
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

/// The tree reports a missing mailbox, and does not ignore it. The server write
/// already succeeded, and "the tree is older than the folder" is different
/// from "the tree changed".
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
// insert and remove

/// A new folder for the tests below, as [`MailSync::create_folder`] returns it.
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

/// The second part of `create_folder_sync`: the new folder must go into the
/// listing of the store. Else, Evolution cannot open it until a refresh.
#[test]
fn a_created_folder_joins_the_roots_when_its_path_has_no_parent_in_it() {
    let mut tree = tree(&[mailbox("M1", "Inbox")]);

    assert!(tree.insert(made("M2", "Projects")));

    assert_eq!(paths(&tree), ["Inbox", "Projects"]);
}

/// A path with a parent puts the folder below that parent. The path gives the
/// tree position, because the code makes it from the parent path.
#[test]
fn a_created_folder_joins_the_children_of_the_folder_its_path_names() {
    let mut tree = tree(&[mailbox("M1", "Projects"), child("M2", "JMAP", "M1")]);

    assert!(tree.insert(made("M3", "Projects/JMAP/Drafts")));

    assert_eq!(
        paths(&tree),
        ["Projects", "Projects/JMAP", "Projects/JMAP/Drafts"]
    );
}

/// The tree reports a path with a missing parent, and does not guess. As a
/// root, the folder shows at the top level, but the server has it elsewhere.
#[test]
fn a_created_folder_whose_parent_is_missing_is_not_inserted() {
    let mut tree = tree(&[mailbox("M1", "Inbox")]);

    assert!(!tree.insert(made("M2", "Projects/JMAP")));

    assert_eq!(paths(&tree), ["Inbox"]);
}

/// The tree can keep a folder that a different client removed. A create then
/// makes a second folder with the same path. The server just confirmed the new
/// folder, so the new folder stays.
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

/// Removal is by id, as for [`FolderTree::set_subscribed`]: the caller write
/// names a mailbox id, and this crate makes the paths.
#[test]
fn a_removed_folder_leaves_the_tree() {
    let mut tree = tree(&[mailbox("M1", "Inbox"), mailbox("M2", "Projects")]);

    assert!(tree.remove(&Id::new("M2")));

    assert_eq!(paths(&tree), ["Inbox"]);
}

/// The subtree goes too. RFC 8621 §2.5 makes the server refuse to destroy a
/// mailbox with children, so a different client already removed these
/// children. If they stay, their parent is missing.
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

/// The tree reports a missing mailbox, as for a subscription.
#[test]
fn removing_a_mailbox_the_tree_does_not_have_changes_nothing() {
    let mut tree = tree(&[mailbox("M1", "Inbox")]);

    assert!(!tree.remove(&Id::new("M404")));

    assert_eq!(paths(&tree), ["Inbox"]);
}

// ---------------------------------------------------------------------------
// rename (which is also move)

/// The second part of `rename_folder_sync`. The tree needs a special edit, not
/// a remove and an insert: a rename changes all paths below the folder.
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

/// The caller gives the name with the path, because this crate has no decoder
/// for the name encoding. The caller has the name that it sent to the server.
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

/// A move: the new path names a different parent, and the folder goes below
/// it. [`FolderTree::insert`] reads a path the same way.
#[test]
fn a_moved_folder_joins_the_children_of_the_folder_its_new_path_names() {
    let mut tree = tree(&[mailbox("M1", "Notes"), mailbox("M2", "Work")]);

    assert!(tree.rename(&Id::new("M1"), "Work/Notes", "Notes"));

    assert_eq!(paths(&tree), ["Work", "Work/Notes"]);
}

/// A move to the top level, with no parent in the path. The folder goes after
/// its new siblings, as a created folder does. The server decides sibling
/// order (sortOrder, then name), and the client does not know that order.
#[test]
fn a_folder_moved_to_the_top_level_joins_the_roots() {
    let mut tree = tree(&[mailbox("M1", "Work"), child("M2", "Notes", "M1")]);

    assert!(tree.rename(&Id::new("M2"), "Notes", "Notes"));

    assert_eq!(paths(&tree), ["Work", "Notes"]);
}

/// The subtree moves with the folder, and all its paths change. A descendant
/// with its old path has a Camel key that names no folder.
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

/// A rename keeps all other data. The server changed two properties of one
/// mailbox. The counts, the role, the subscription and the id do not change.
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

/// The tree reports a new path with a missing parent, as
/// [`FolderTree::insert`] does. The folder must also stay at its old position:
/// it must not be lost.
#[test]
fn a_rename_under_a_parent_the_tree_does_not_have_changes_nothing() {
    let mut tree = tree(&[mailbox("M1", "Notes")]);

    assert!(!tree.rename(&Id::new("M1"), "Work/Notes", "Notes"));

    assert_eq!(paths(&tree), ["Notes"]);
}

/// A folder cannot move into itself. The server refuses this, so it occurs
/// only if the tree gets a false request. The tree must not remove the subtree
/// without a position for it.
#[test]
fn a_move_into_the_folders_own_subtree_changes_nothing() {
    let mut tree = tree(&[mailbox("M1", "Work"), child("M2", "Notes", "M1")]);

    assert!(!tree.rename(&Id::new("M1"), "Work/Notes/Work", "Work"));

    assert_eq!(paths(&tree), ["Work", "Work/Notes"]);
}

/// A move, as a create, can find an old folder at its destination path. Of the
/// two folders, the one that the server just confirmed stays.
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

/// The tree reports a missing mailbox, as all other edits do.
#[test]
fn renaming_a_mailbox_the_tree_does_not_have_changes_nothing() {
    let mut tree = tree(&[mailbox("M1", "Inbox")]);

    assert!(!tree.rename(&Id::new("M404"), "Work", "Work"));

    assert_eq!(paths(&tree), ["Inbox"]);
}
