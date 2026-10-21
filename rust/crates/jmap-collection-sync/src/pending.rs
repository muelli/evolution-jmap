// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The undo window of a collection delete.
//!
//! A destroy cannot be undone: a new collection gets new ids, and the items and
//! shares of other users are lost. Thus a delete does not destroy at once. It
//! disables the child source and records a due time on it. The destroy goes to
//! the server only at the due time. Undo enables the source again, and then
//! nothing goes to the server.
//!
//! The due time is wall-clock seconds since the Unix epoch, so that it stays
//! correct after a restart of the registry. [`decide`] is the only reader.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The time between the delete and the destroy, in which Undo is available.
pub const UNDO_WINDOW: Duration = Duration::from_secs(60);

/// The time at which a pending destroy is due.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Due(i64);

impl Due {
    /// The due time of a delete that starts at `now`.
    ///
    /// A part second rounds up, so the window is never shorter than
    /// [`UNDO_WINDOW`].
    pub fn after_window(now: SystemTime) -> Self {
        let since_epoch = now.duration_since(UNIX_EPOCH).unwrap_or_default() + UNDO_WINDOW;
        let seconds = since_epoch
            .as_secs()
            .saturating_add(u64::from(since_epoch.subsec_nanos() > 0));
        Self(i64::try_from(seconds).unwrap_or(i64::MAX))
    }

    /// Reads a stored stamp. A stamp of zero or less means no pending delete.
    pub fn from_stamp(stamp: i64) -> Option<Self> {
        (stamp > 0).then_some(Self(stamp))
    }

    /// The value to store: seconds since the Unix epoch.
    pub fn stamp(self) -> i64 {
        self.0
    }
}

/// What to do with a child source, from its state and its due time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    /// The source has no due time.
    NotPending,
    /// The source is enabled again, so the user undid the delete. Clear the due
    /// time and send nothing.
    Undone,
    /// Do the check again after this time.
    Wait(Duration),
    /// Destroy the collection on the server, then remove the source.
    Destroy,
}

/// Decides what to do with a child source.
///
/// An enabled source is always [`Pending::Undone`], also after the due time:
/// an enabled source is visible, and a visible collection must not disappear.
/// A wait is never longer than [`UNDO_WINDOW`]. Thus a wall clock that moves
/// back does not keep a delete pending for longer than one window.
pub fn decide(enabled: bool, due: Option<Due>, now: SystemTime) -> Pending {
    let Some(due) = due else {
        return Pending::NotPending;
    };
    if enabled {
        return Pending::Undone;
    }
    let Ok(since_epoch) = now.duration_since(UNIX_EPOCH) else {
        return Pending::Wait(UNDO_WINDOW);
    };
    let due = Duration::from_secs(due.0.unsigned_abs());
    match due.checked_sub(since_epoch) {
        Some(left) if !left.is_zero() => Pending::Wait(left.min(UNDO_WINDOW)),
        _ => Pending::Destroy,
    }
}
