// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests for the undo window of a delete on the EDS side: `defer` on a real
// `ESource`, and `settle` and `watch` on a fake child that records its calls.

use std::ffi::CString;
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime};

use eds_sys::{ESource, e_source_get_enabled, e_source_new_with_uid};
use glib_sys::GFALSE;
use gobject_sys::g_object_unref;
use jmap_backend_collection::deferred_delete::{
    PendingChild, Settled, defer, delete_later, settle, watch,
};
use jmap_backend_collection::delete_resource::DeleteError;
use jmap_collection_sync::DeleteFailure;
use jmap_collection_sync::pending::{Due, UNDO_WINDOW};

mod common;
use common::{with_timeout, with_timeout_duration};

static NEXT: AtomicU32 = AtomicU32::new(0);

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A plain `ESource`, released on drop.
struct Source(*mut ESource);

impl Source {
    fn new() -> Self {
        let uid = CString::new(unique("jmap-deferred")).expect("no NUL in the uid");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a GError
        // out-parameter.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    fn enabled(&self) -> bool {
        // SAFETY: a live source.
        unsafe { e_source_get_enabled(self.0) != GFALSE }
    }

    fn due(&self) -> Option<Due> {
        // SAFETY: a live source.
        unsafe { jmap_config::pending_delete::read(self.0) }
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: this struct holds the only reference.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

#[test]
fn defer_disables_the_child_and_stores_the_due_time() {
    with_timeout(|| {
        let source = Source::new();
        let now = SystemTime::now();
        assert!(source.enabled());

        // SAFETY: a live source.
        let due = unsafe { defer(source.0, now) };

        assert_eq!(due, Due::after_window(now));
        assert_eq!(source.due(), Some(due));
        assert!(!source.enabled());
    });
}

/// What the fake does when `settle` asks it to destroy or remove.
#[derive(Clone, Copy)]
enum Outcome {
    Ok,
    Fail,
}

/// A child with its state in memory. It records each call that changes it.
struct Fake {
    uid: String,
    present: bool,
    enabled: Mutex<bool>,
    due: Mutex<Option<Due>>,
    destroy: Outcome,
    remove: Outcome,
    calls: Mutex<Vec<&'static str>>,
}

impl Fake {
    fn pending(due: Due) -> Self {
        Self {
            uid: unique("fake"),
            present: true,
            enabled: Mutex::new(false),
            due: Mutex::new(Some(due)),
            destroy: Outcome::Ok,
            remove: Outcome::Ok,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }

    fn due_now(&self) -> Option<Due> {
        *self.due.lock().unwrap()
    }

    fn is_enabled(&self) -> bool {
        *self.enabled.lock().unwrap()
    }
}

impl PendingChild for Fake {
    fn uid(&self) -> String {
        self.uid.clone()
    }

    fn present(&self) -> bool {
        self.present
    }

    fn enabled(&self) -> bool {
        *self.enabled.lock().unwrap()
    }

    fn due(&self) -> Option<Due> {
        *self.due.lock().unwrap()
    }

    fn clear_due(&self) {
        self.calls.lock().unwrap().push("clear_due");
        *self.due.lock().unwrap() = None;
    }

    fn enable(&self) {
        self.calls.lock().unwrap().push("enable");
        *self.enabled.lock().unwrap() = true;
    }

    fn destroy_on_server(&self) -> Result<(), DeleteError> {
        self.calls.lock().unwrap().push("destroy_on_server");
        match self.destroy {
            Outcome::Ok => Ok(()),
            Outcome::Fail => Err(DeleteError::Server(DeleteFailure::Client(
                jmap_client::Error::Protocol("refused".to_owned()),
            ))),
        }
    }

    fn remove(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("remove");
        match self.remove {
            Outcome::Ok => Ok(()),
            Outcome::Fail => Err("busy".to_owned()),
        }
    }
}

fn past() -> Due {
    Due::from_stamp(1).expect("a positive stamp")
}

#[test]
fn a_due_child_is_destroyed_on_the_server_and_then_removed() {
    let child = Fake::pending(past());

    assert_eq!(settle(&child, SystemTime::now()), Settled::Destroyed);
    assert_eq!(child.calls(), ["destroy_on_server", "remove"]);
}

#[test]
fn a_child_before_its_due_time_waits_and_nothing_goes_to_the_server() {
    let now = SystemTime::now();
    let child = Fake::pending(Due::after_window(now));

    assert_eq!(settle(&child, now), Settled::Wait(UNDO_WINDOW));
    assert!(child.calls().is_empty());
}

#[test]
fn an_enabled_child_is_undone_and_its_due_time_is_cleared() {
    let child = Fake::pending(past());
    *child.enabled.lock().unwrap() = true;

    assert_eq!(settle(&child, SystemTime::now()), Settled::Undone);
    assert_eq!(child.calls(), ["clear_due"]);
    assert_eq!(child.due_now(), None);
}

#[test]
fn a_child_without_a_due_time_is_not_changed() {
    let child = Fake::pending(past());
    *child.due.lock().unwrap() = None;

    assert_eq!(settle(&child, SystemTime::now()), Settled::NotPending);
    assert!(child.calls().is_empty());
}

#[test]
fn a_child_that_the_registry_no_longer_has_is_not_destroyed() {
    let child = Fake {
        present: false,
        ..Fake::pending(past())
    };

    assert_eq!(settle(&child, SystemTime::now()), Settled::Gone);
    assert!(child.calls().is_empty());
}

/// A collection that is still on the server must not stay hidden. The child
/// is enabled before the due time is cleared: a stop between the two leaves an
/// enabled child with a due time, which the next check undoes.
#[test]
fn a_refused_destroy_restores_the_child_and_removes_nothing() {
    let child = Fake {
        destroy: Outcome::Fail,
        ..Fake::pending(past())
    };

    assert_eq!(settle(&child, SystemTime::now()), Settled::Restored);
    assert_eq!(child.calls(), ["destroy_on_server", "enable", "clear_due"]);
    assert!(child.is_enabled());
    assert_eq!(child.due_now(), None);
}

/// The collection is gone from the server, so the child must not come back.
/// It stays hidden with no due time, and the next populate removes it as
/// obsolete.
#[test]
fn a_failed_removal_after_the_destroy_keeps_the_child_hidden() {
    let child = Fake {
        remove: Outcome::Fail,
        ..Fake::pending(past())
    };

    assert_eq!(settle(&child, SystemTime::now()), Settled::Stale);
    assert_eq!(child.calls(), ["destroy_on_server", "remove", "clear_due"]);
    assert!(!child.is_enabled());
}

#[test]
fn watch_waits_for_the_due_time_then_destroys() {
    with_timeout(|| {
        let start = Instant::now();
        let due = Due::after_window(SystemTime::now() + Duration::from_secs(1) - UNDO_WINDOW);
        let handle = watch(Fake::pending(due)).expect("a first watch of this child");

        assert_eq!(handle.join().expect("no panic"), Settled::Destroyed);
        assert!(start.elapsed() >= Duration::from_millis(900));
    });
}

#[test]
fn a_second_watch_of_the_same_child_is_refused_while_the_first_runs() {
    with_timeout_duration(Duration::from_secs(20), || {
        let due = Due::after_window(SystemTime::now() + Duration::from_secs(2) - UNDO_WINDOW);
        let first = Fake::pending(due);
        let uid = first.uid.clone();
        let handle = watch(first).expect("a first watch of this child");

        let second = Fake {
            uid: uid.clone(),
            ..Fake::pending(due)
        };
        assert!(watch(second).is_none());

        assert_eq!(handle.join().expect("no panic"), Settled::Destroyed);
        let third = Fake {
            uid,
            ..Fake::pending(past())
        };
        let handle = watch(third).expect("the first watch has ended");
        assert_eq!(handle.join().expect("no panic"), Settled::Destroyed);
    });
}

/// A child whose check panics. Its thread never gets to a server call.
struct PanicsOnCheck(String);

impl PendingChild for PanicsOnCheck {
    fn uid(&self) -> String {
        self.0.clone()
    }

    fn present(&self) -> bool {
        panic!("the check of this child fails");
    }

    fn enabled(&self) -> bool {
        unreachable!("present panics first")
    }

    fn due(&self) -> Option<Due> {
        unreachable!("present panics first")
    }

    fn clear_due(&self) {
        unreachable!("present panics first")
    }

    fn enable(&self) {
        unreachable!("present panics first")
    }

    fn destroy_on_server(&self) -> Result<(), DeleteError> {
        unreachable!("present panics first")
    }

    fn remove(&self) -> Result<(), String> {
        unreachable!("present panics first")
    }
}

/// Without this, a child whose thread panics can never be watched again, and
/// its pending delete does not continue until the registry restarts.
#[test]
fn a_watch_thread_that_panics_lets_the_child_be_watched_again() {
    with_timeout(|| {
        let uid = unique("panics");
        let handle = watch(PanicsOnCheck(uid.clone())).expect("a first watch of this child");
        assert!(handle.join().is_err(), "the thread panics");

        let again = Fake {
            uid,
            ..Fake::pending(past())
        };
        let handle = watch(again).expect("the panicked watch has ended");
        assert_eq!(handle.join().expect("no panic"), Settled::Destroyed);
    });
}

/// The vfunc defers and starts the watch in one step, so a check of an older
/// watch cannot run between the two.
#[test]
fn delete_later_defers_the_source_and_watches_the_child() {
    with_timeout(|| {
        let source = Source::new();
        let now = SystemTime::now();
        let child = Fake::pending(Due::after_window(now));
        let uid = child.uid.clone();

        // SAFETY: a live source, and the fake stands for it.
        let due = unsafe { delete_later(source.0, child, now) };

        assert_eq!(source.due(), Some(due));
        assert!(!source.enabled());
        let again = Fake {
            uid,
            ..Fake::pending(due)
        };
        assert!(watch(again).is_none(), "the first thread still waits");
    });
}

/// A child whose due time moves one day later after the first check, as when
/// the wall clock moves back by one day during the wait.
struct ClockMovesBack {
    child: Fake,
    checks: AtomicU32,
}

impl PendingChild for ClockMovesBack {
    fn uid(&self) -> String {
        self.child.uid()
    }

    fn present(&self) -> bool {
        self.child.present()
    }

    fn enabled(&self) -> bool {
        self.child.enabled()
    }

    fn due(&self) -> Option<Due> {
        let due = self.child.due()?;
        if self.checks.fetch_add(1, Ordering::SeqCst) == 0 {
            return Some(due);
        }
        Due::from_stamp(due.stamp() + 86_400)
    }

    fn clear_due(&self) {
        self.child.clear_due();
    }

    fn enable(&self) {
        self.child.enable();
    }

    fn destroy_on_server(&self) -> Result<(), DeleteError> {
        self.child.destroy_on_server()
    }

    fn remove(&self) -> Result<(), String> {
        self.child.remove()
    }
}

/// The info bar closes after at most one window. The destroy must not wait for
/// the wall clock to catch up with the stored due time.
#[test]
fn a_clock_that_moves_back_during_the_wait_does_not_extend_it() {
    with_timeout_duration(Duration::from_secs(20), || {
        let due = Due::after_window(SystemTime::now() + Duration::from_secs(1) - UNDO_WINDOW);
        let child = ClockMovesBack {
            child: Fake::pending(due),
            checks: AtomicU32::new(0),
        };
        let handle = watch(child).expect("a first watch of this child");

        assert_eq!(handle.join().expect("no panic"), Settled::Destroyed);
    });
}
