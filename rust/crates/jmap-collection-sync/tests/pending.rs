// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The undo window of a collection delete: when the destroy is due, and when
//! the user has undone it.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jmap_collection_sync::pending::{Due, Pending, UNDO_WINDOW, decide};

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

#[test]
fn the_window_is_one_minute() {
    assert_eq!(UNDO_WINDOW, Duration::from_secs(60));
}

#[test]
fn a_new_delete_is_due_one_window_after_now() {
    let due = Due::after_window(at(1_000));
    assert_eq!(due.stamp(), 1_060);
}

#[test]
fn a_part_second_rounds_the_due_time_up() {
    let due = Due::after_window(at(1_000) + Duration::from_millis(1));
    assert_eq!(due.stamp(), 1_061);
}

#[test]
fn a_stamp_reads_back_as_the_same_due_time() {
    let due = Due::after_window(at(5_000));
    assert_eq!(Due::from_stamp(due.stamp()), Some(due));
}

#[test]
fn a_stamp_of_zero_or_less_is_no_pending_delete() {
    assert_eq!(Due::from_stamp(0), None);
    assert_eq!(Due::from_stamp(-1), None);
    assert_eq!(Due::from_stamp(i64::MIN), None);
}

#[test]
fn a_source_without_a_due_time_is_not_pending() {
    assert_eq!(decide(false, None, at(1_000)), Pending::NotPending);
    assert_eq!(decide(true, None, at(1_000)), Pending::NotPending);
}

#[test]
fn a_disabled_source_waits_until_its_due_time() {
    let due = Due::from_stamp(1_060);
    assert_eq!(
        decide(false, due, at(1_000)),
        Pending::Wait(Duration::from_secs(60))
    );
    assert_eq!(
        decide(false, due, at(1_059) + Duration::from_millis(500)),
        Pending::Wait(Duration::from_millis(500))
    );
}

#[test]
fn a_disabled_source_is_destroyed_at_and_after_its_due_time() {
    let due = Due::from_stamp(1_060);
    assert_eq!(decide(false, due, at(1_060)), Pending::Destroy);
    assert_eq!(decide(false, due, at(9_999_999)), Pending::Destroy);
}

#[test]
fn an_enabled_source_with_a_due_time_is_undone_at_any_time() {
    // Undo enables the source again. The same is true when the user enables it
    // in the account preferences, also after the due time.
    let due = Due::from_stamp(1_060);
    assert_eq!(decide(true, due, at(1_000)), Pending::Undone);
    assert_eq!(decide(true, due, at(1_060)), Pending::Undone);
    assert_eq!(decide(true, due, at(9_999_999)), Pending::Undone);
}

#[test]
fn a_clock_that_moves_back_never_extends_the_wait_past_one_window() {
    // The stamp is wall-clock time. If the clock moves back by a day, the
    // destroy must not wait for one more day.
    let due = Due::after_window(at(100_000));
    assert_eq!(
        decide(false, Some(due), at(13_600)),
        Pending::Wait(UNDO_WINDOW)
    );
}

#[test]
fn a_clock_before_the_epoch_waits_one_window() {
    let due = Due::from_stamp(1_060);
    let before_epoch = UNIX_EPOCH - Duration::from_secs(10);
    assert_eq!(decide(false, due, before_epoch), Pending::Wait(UNDO_WINDOW));
}

#[test]
fn a_due_time_that_does_not_fit_the_stamp_saturates() {
    let far = UNIX_EPOCH + Duration::from_secs(i64::MAX as u64 - 10);
    assert_eq!(Due::after_window(far).stamp(), i64::MAX);
}
