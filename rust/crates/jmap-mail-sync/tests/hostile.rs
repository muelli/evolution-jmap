// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Hostile `Mailbox/get` and `Email/get` answers.
//!
//! `tests/tree.rs` covers the errors of a *broken* server (a missing parent, a
//! `parentId` loop, a duplicate id). This file covers intentional attacks:
//!
//! - A `parentId` chain so long that the recursive `Drop` of the tree overflows
//!   the stack. This aborts the full Camel process, and no caller can report it.
//! - A date with a year that overflows the conversion to a count of seconds.

use jmap_mail_sync::folder::MAX_DEPTH;
use jmap_mail_sync::{FolderTree, MessageSummary};
use jmap_proto::mail::{Email, Mailbox};

/// `count` mailboxes, each the child of the one before it.
fn chain(count: usize) -> Vec<Mailbox> {
    (0..count)
        .map(|index| {
            let mut mailbox = Mailbox::new(format!("f{index}")).with_id(format!("M{index}"));
            if index > 0 {
                mailbox = mailbox.with_parent_id(format!("M{}", index - 1));
            }
            mailbox
        })
        .collect()
}

/// The deepest path in `tree`, counting the root as one.
fn depth(tree: &FolderTree) -> usize {
    // Iterative, because the server sets the depth of the tree.
    let mut deepest = 0;
    let mut stack: Vec<(&jmap_mail_sync::FolderInfo, usize)> =
        tree.roots().iter().map(|folder| (folder, 1)).collect();
    while let Some((folder, level)) = stack.pop() {
        deepest = deepest.max(level);
        stack.extend(folder.children.iter().map(|child| (child, level + 1)));
    }
    deepest
}

/// A chain within the cap keeps its shape. The cap must not change a tree
/// that a real server can send.
#[test]
fn a_chain_within_the_limit_stays_one_chain() {
    let tree = FolderTree::from_mailboxes(&chain(MAX_DEPTH)).expect("a well-formed chain");

    assert_eq!(tree.len(), MAX_DEPTH);
    assert_eq!(tree.roots().len(), 1);
    assert_eq!(depth(&tree), MAX_DEPTH);
}

/// Past the cap, the mailbox at level 65 becomes a root, and its descendants
/// stay below it. No mailbox is lost, because a lost mailbox hides its mail.
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

/// The regression that the cap prevents. Without the cap, the recursive `Drop`
/// of the tree aborts the test binary, as it aborts the Camel provider.
#[test]
fn a_pathologically_deep_chain_neither_reshapes_into_a_stack_overflow_nor_hangs() {
    let count = 100_000;
    let tree = FolderTree::from_mailboxes(&chain(count)).expect("a very deep chain");

    assert_eq!(tree.len(), count);
    assert!(depth(&tree) <= MAX_DEPTH, "depth {}", depth(&tree));
    // The drop at the end of this scope is the step that aborted before.
}

/// One `Email` parsed from JSON, as a listing parses it, not a struct made
/// by hand.
fn email(received_at: &str, sent_at: &str) -> Email {
    serde_json::from_value(serde_json::json!({
        "id": "E1",
        "receivedAt": received_at,
        "sentAt": sent_at,
    }))
    .expect("an Email object")
}

/// A `receivedAt` year can be any value that `i64` can parse. The conversion to
/// seconds overflows `i64` much earlier: `days_from_civil` multiplies the era
/// by 146 097, and its caller multiplies the day count by 86 400.
///
/// The server sets the string. Without a bound, a debug build panics in a Camel
/// refresh, and the vfunc guard makes the refresh fail, so the user cannot read
/// the mailbox. A release build wraps and puts a false date in the summary.
/// The correct result for such text is `None`.
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

/// The same check for a negative year. The parse accepts a `-` sign as part of
/// the year, although the grammar requires a digit.
#[test]
fn a_year_before_any_calendar_is_not_a_date_either() {
    let stamp = "-9223372036854775808-01-01T00:00:00Z";
    let summary = MessageSummary::from_email(&email(stamp, stamp)).expect("a row");
    assert_eq!(summary.received_at, None);
    assert_eq!(summary.sent_at, None);
}

/// Non-regression: dates from a real server still give the correct instants,
/// also at the two ends of the RFC 3339 four-digit year range.
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
