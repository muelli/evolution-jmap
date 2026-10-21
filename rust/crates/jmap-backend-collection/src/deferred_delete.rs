// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The undo window of a delete, on the EDS side.
//!
//! `delete_resource_sync` does not destroy at once. [`defer`] disables the child
//! and stores a due time on it. [`watch`] waits on a thread until the due time,
//! then does what [`settle`] decides. `child_added` also calls [`watch`] for a
//! child with a due time, so a pending delete continues after a restart of the
//! registry. [`jmap_collection_sync::pending`] has the rules.
//!
//! A destroy that the server refuses restores the child. A hidden collection
//! that stays on the server is the worst result, because the user cannot see
//! or delete it again.

use std::collections::BTreeSet;
use std::sync::{Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use eds_sys::{ESource, e_source_set_enabled};
use glib_sys::GFALSE;
use jmap_backend_core::trampoline::log_critical_for_resource;
use jmap_collection_sync::pending::{Due, Pending, decide};

use crate::delete_resource::DeleteError;

/// Disables `child` and stores the due time of a delete that starts at `now`.
///
/// The due time is written first. A stop between the two writes then leaves an
/// enabled child with a due time, which the next check undoes. The opposite
/// order can leave a disabled child with no due time, and nothing shows it again.
///
/// # Safety
///
/// `child` must be a valid `ESource`.
pub unsafe fn defer(child: *mut ESource, now: SystemTime) -> Due {
    let due = Due::after_window(now);
    // SAFETY: a valid source by the contract of this function.
    unsafe {
        jmap_config::pending_delete::write(child, Some(due));
        e_source_set_enabled(child, GFALSE);
    }
    due
}

/// A child source with a pending delete, as [`settle`] sees it.
pub trait PendingChild {
    /// The uid of the source. [`watch`] uses it to watch each child only once.
    fn uid(&self) -> String;
    /// `false` if the registry no longer has the source.
    fn present(&self) -> bool;
    /// The `enabled` property of the source.
    fn enabled(&self) -> bool;
    /// The stored due time, or `None` for no pending delete.
    fn due(&self) -> Option<Due>;
    /// Removes the stored due time.
    fn clear_due(&self);
    /// Sets the `enabled` property of the source.
    fn enable(&self);
    /// Destroys the collection on the server.
    fn destroy_on_server(&self) -> Result<(), DeleteError>;
    /// Removes the source from the registry. The `Err` is the reason from EDS.
    fn remove(&self) -> Result<(), String>;
}

/// The result of one [`settle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    /// The source has no due time.
    NotPending,
    /// The registry no longer has the source. Nothing goes to the server.
    Gone,
    /// The source is enabled again. Its due time is cleared.
    Undone,
    /// The due time is in the future. Check again after this time.
    Wait(Duration),
    /// The collection is destroyed and the source is removed.
    Destroyed,
    /// The server refused the destroy. The source is enabled again.
    Restored,
    /// The collection is destroyed, but EDS did not remove the source. The source
    /// stays disabled with no due time. The next populate removes it.
    Stale,
}

/// Checks `child` once at `now`, and does what [`decide`] tells.
pub fn settle(child: &impl PendingChild, now: SystemTime) -> Settled {
    match check(child, now) {
        Checked::Due => destroy(child),
        Checked::Done(settled) => settled,
    }
}

/// The part of a [`settle`] that sends nothing to the server.
enum Checked {
    Done(Settled),
    Due,
}

fn check(child: &impl PendingChild, now: SystemTime) -> Checked {
    if !child.present() {
        return Checked::Done(Settled::Gone);
    }
    match decide(child.enabled(), child.due(), now) {
        Pending::NotPending => Checked::Done(Settled::NotPending),
        Pending::Undone => {
            child.clear_due();
            Checked::Done(Settled::Undone)
        }
        Pending::Wait(left) => Checked::Done(Settled::Wait(left)),
        Pending::Destroy => Checked::Due,
    }
}

fn destroy(child: &impl PendingChild) -> Settled {
    if let Err(failure) = child.destroy_on_server() {
        log_critical_for_resource(
            &child.uid(),
            &format!(
                "pending delete: the server refused the destroy, so the source is shown again: {failure}"
            ),
        );
        // Enable first, for the reason in `defer`.
        child.enable();
        child.clear_due();
        return Settled::Restored;
    }
    if let Err(reason) = child.remove() {
        log_critical_for_resource(
            &child.uid(),
            &format!(
                "pending delete: the collection is deleted, but the source could not be removed: {reason}"
            ),
        );
        child.clear_due();
        return Settled::Stale;
    }
    Settled::Destroyed
}

/// The uids that a [`watch`] thread watches now.
///
/// The lock also covers each check of a thread and each [`delete_later`].
/// Without it, a check can clear the due time of a delete that starts after the
/// check read the source, and that delete then has no thread.
static WATCHED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

fn watched() -> MutexGuard<'static, BTreeSet<String>> {
    WATCHED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Removes a uid from [`WATCHED`] if its thread stops on a panic.
struct Unwatch(Option<String>);

impl Drop for Unwatch {
    fn drop(&mut self) {
        if let Some(uid) = self.0.take() {
            watched().remove(&uid);
        }
    }
}

/// Defers the delete of `source` (refer to [`defer`]) and starts a [`watch`]
/// of `child`, under one lock.
///
/// # Safety
///
/// `source` must be a valid `ESource`, and `child` must stand for it.
pub unsafe fn delete_later<C: PendingChild + Send + 'static>(
    source: *mut ESource,
    child: C,
    now: SystemTime,
) -> Due {
    let mut watched = watched();
    // SAFETY: the contract of this function.
    let due = unsafe { defer(source, now) };
    let _ = start(&mut watched, child);
    due
}

/// Starts a thread that runs [`settle`] on `child` until the result is not a
/// wait. Returns `None` if a thread already watches this child, so two
/// threads cannot send two destroys.
///
/// The thread holds `child` for at most the undo window. A stop of the process
/// stops the thread, and the stored due time continues the delete after the
/// next start.
pub fn watch<C: PendingChild + Send + 'static>(child: C) -> Option<JoinHandle<Settled>> {
    start(&mut watched(), child)
}

fn start<C: PendingChild + Send + 'static>(
    uids: &mut BTreeSet<String>,
    child: C,
) -> Option<JoinHandle<Settled>> {
    let uid = child.uid();
    if !uids.insert(uid.clone()) {
        return None;
    }
    let spawned = thread::Builder::new()
        .name("jmap-pending-delete".to_owned())
        .spawn(move || {
            let mut unwatch = Unwatch(Some(child.uid()));
            loop {
                let mut watched = watched();
                let checked = check(&child, SystemTime::now());
                if let Checked::Done(Settled::Wait(left)) = checked {
                    drop(watched);
                    thread::sleep(left);
                    continue;
                }
                // The thread ends here, so remove the uid in the same lock
                // as the check.
                if let Some(uid) = unwatch.0.take() {
                    watched.remove(&uid);
                }
                drop(watched);
                return match checked {
                    Checked::Due => destroy(&child),
                    Checked::Done(settled) => settled,
                };
            }
        });
    match spawned {
        Ok(handle) => Some(handle),
        Err(error) => {
            uids.remove(&uid);
            log_critical_for_resource(
                &uid,
                &format!("pending delete: no thread to wait for the due time: {error}"),
            );
            None
        }
    }
}
