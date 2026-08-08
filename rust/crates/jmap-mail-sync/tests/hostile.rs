// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a hostile `Mailbox/get` can make of the folder tree.
//!
//! `tests/tree.rs` already covers the violations a *broken* server sends —
//! a missing parent, a `parentId` loop, a duplicate id. This file is about the
//! one that is not a mistake: a `parentId` chain long enough that the tree's
//! own recursive `Drop` runs off the end of the stack, which is an abort of the
//! whole Camel process rather than an error a caller can report.
//!
//! See `docs/AUDIT-FFI.md`, finding F3.

use jmap_mail_sync::FolderTree;
use jmap_mail_sync::folder::MAX_DEPTH;
use jmap_proto::mail::Mailbox;

/// `count` mailboxes, each the child of the one before it.
fn chain(count: usize) -> Vec<Mailbox> {
    (0..count)
        .map(|index| Mailbox {
            id: Some(format!("M{index}").into()),
            name: format!("f{index}"),
            parent_id: (index > 0).then(|| format!("M{}", index - 1).into()),
            ..Mailbox::default()
        })
        .collect()
}

/// The deepest path in `tree`, counting the root as one.
fn depth(tree: &FolderTree) -> usize {
    // Iteratively, for the same reason the tree is built that way: the point of
    // this file is a stack a server chose the size of.
    let mut deepest = 0;
    let mut stack: Vec<(&jmap_mail_sync::FolderInfo, usize)> =
        tree.roots().iter().map(|folder| (folder, 1)).collect();
    while let Some((folder, level)) = stack.pop() {
        deepest = deepest.max(level);
        stack.extend(folder.children.iter().map(|child| (child, level + 1)));
    }
    deepest
}

/// A chain that fits keeps its shape: the cap must not reshape a tree any real
/// server could produce.
#[test]
fn a_chain_within_the_limit_stays_one_chain() {
    let tree = FolderTree::from_mailboxes(&chain(MAX_DEPTH)).expect("a well-formed chain");

    assert_eq!(tree.len(), MAX_DEPTH);
    assert_eq!(tree.roots().len(), 1);
    assert_eq!(depth(&tree), MAX_DEPTH);
}

/// One past the cap: the mailbox that would have been level 65 becomes a root
/// of its own, and its descendants hang off it. No mailbox is lost — losing one
/// is mail the user cannot reach.
#[test]
fn a_chain_past_the_limit_is_cut_rather_than_dropped_or_refused() {
    let count = MAX_DEPTH + 3;
    let tree = FolderTree::from_mailboxes(&chain(count)).expect("a deep chain is not an error");

    assert_eq!(tree.len(), count, "a mailbox went missing");
    assert_eq!(tree.roots().len(), 2, "the chain was not cut exactly once");
    assert!(
        depth(&tree) <= MAX_DEPTH,
        "depth {} exceeds the cap",
        depth(&tree)
    );

    // Every mailbox is still reachable by name.
    for index in 0..count {
        assert!(
            tree.iter()
                .any(|folder| folder.display_name == format!("f{index}")),
            "f{index} is not in the tree"
        );
    }
}

/// The regression this cap exists for. Without it the recursive `Drop` of the
/// returned tree aborts the test binary instead of failing an assertion, which
/// is exactly the symptom in `evolution-jmap`'s Camel provider.
#[test]
fn a_pathologically_deep_chain_neither_reshapes_into_a_stack_overflow_nor_hangs() {
    let count = 100_000;
    let tree = FolderTree::from_mailboxes(&chain(count)).expect("a very deep chain");

    assert_eq!(tree.len(), count);
    assert!(depth(&tree) <= MAX_DEPTH, "depth {}", depth(&tree));
    // The drop at the end of this scope is the part that used to abort.
}
