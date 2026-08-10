// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a hostile `Mailbox/get` or `Email/get` can make of what it is read into.
//!
//! `tests/tree.rs` already covers the violations a *broken* server sends —
//! a missing parent, a `parentId` loop, a duplicate id. This file is about the
//! ones that are not a mistake: a `parentId` chain long enough that the tree's
//! own recursive `Drop` runs off the end of the stack, which is an abort of the
//! whole Camel process rather than an error a caller can report (F3), and a
//! date whose year is chosen so that reading it overflows the arithmetic that
//! turns it into a count of seconds (F11).
//!
//! See `docs/AUDIT-FFI.md`, finding F3, and `docs/AUDIT-FFI-20260810.md`, F11.

use jmap_mail_sync::folder::MAX_DEPTH;
use jmap_mail_sync::{FolderTree, MessageSummary};
use jmap_proto::mail::{Email, Mailbox};

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

/// One `Email` as the server sent it, so that what is being tested is the JSON
/// a listing really parses rather than a struct assembled by hand.
fn email(received_at: &str, sent_at: &str) -> Email {
    serde_json::from_value(serde_json::json!({
        "id": "E1",
        "receivedAt": received_at,
        "sentAt": sent_at,
    }))
    .expect("an Email object")
}

/// The years a `receivedAt` can carry are the ones `i64` can parse, and the
/// calendar arithmetic that turns one into a count of seconds is not written for
/// them: `days_from_civil` multiplies the year's era by 146 097 and the answer by
/// 86 400, both of which run past `i64` long before the parse does.
///
/// The server picks the string. A debug build panics — inside a Camel refresh,
/// where the vfunc guard turns it into a failed refresh of a mailbox the user
/// cannot then read — and a release build wraps, which puts a date computed from
/// nothing into the summary. Neither is the answer for text that is not a date a
/// message can carry, which is `None`.
#[test]
fn a_year_no_calendar_can_hold_is_not_a_date_rather_than_an_overflow() {
    for year in [
        // Past `days * 86_400`.
        "300000000000",
        // Past `era * 146_097`.
        "30000000000000000",
        // The largest year the parse itself admits.
        "9223372036854775807",
    ] {
        let stamp = format!("{year}-01-01T00:00:00Z");
        let summary = MessageSummary::from_email(&email(&stamp, &stamp)).expect("a row");
        assert_eq!(summary.received_at, None, "receivedAt {stamp}");
        assert_eq!(summary.sent_at, None, "sentAt {stamp}");
    }
}

/// And the same at the other end, where the year is negative: a `-` where the
/// grammar wants a digit is still text the parse accepts as a year.
#[test]
fn a_year_before_any_calendar_is_not_a_date_either() {
    let stamp = "-9223372036854775808-01-01T00:00:00Z";
    let summary = MessageSummary::from_email(&email(stamp, stamp)).expect("a row");
    assert_eq!(summary.received_at, None);
    assert_eq!(summary.sent_at, None);
}

/// The non-regression half: the dates a real server sends still read as the
/// instants they name, including the ends of the range RFC 3339's four-digit
/// year allows.
#[test]
fn the_dates_a_real_server_sends_are_still_read() {
    let summary =
        MessageSummary::from_email(&email("2026-01-15T09:30:00Z", "2026-01-15T10:30:00+01:00"))
            .expect("a row");
    assert_eq!(summary.received_at, Some(1_768_469_400));
    assert_eq!(summary.sent_at, Some(1_768_469_400));

    let summary =
        MessageSummary::from_email(&email("0001-01-01T00:00:00Z", "9999-12-31T23:59:59Z"))
            .expect("a row");
    assert_eq!(summary.received_at, Some(-62_135_596_800));
    assert_eq!(summary.sent_at, Some(253_402_300_799));
}
