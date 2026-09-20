// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A refresh's response can arrive late, after a local edit the connection
//! made in the meantime — and `JmapStore::folders` must not let the late
//! answer erase the edit.
//!
//! `folders(REFRESH)` snapshots the listing, makes a `Mailbox/changes` round
//! trip, then writes the result back unconditionally. `create_folder` (and
//! its three siblings) edit the *current* listing in place, under the same
//! lock, as soon as the server confirms the write — with no ordering against
//! a refresh that started earlier and is still waiting on its own response.
//! A refresh's `Mailbox/changes` answer that was computed before the create
//! (so it truthfully says nothing changed) but *delivered* after it overwrites
//! the listing with the pre-create tree, silently losing the folder Camel was
//! just told exists.
//!
//! Proven with a transport that wraps a real one against a real
//! [`MockServer`]: the `Mailbox/changes` request is allowed to complete for
//! real (so its answer is the server's honest one for the state at the time),
//! but its delivery back to the caller is held until a concurrent
//! `create_folder` has finished.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError, UreqTransport};
use jmap_client::{Client, Credentials};
use jmap_mail::store::JmapStore;
use jmap_mail_sync::MailSync;
use jmap_mock::MockServer;

/// Lets the test park the `Mailbox/changes` response and learn when it
/// actually parked, rather than racing a sleep against it.
#[derive(Default)]
struct Gate {
    started: Mutex<bool>,
    started_cond: Condvar,
    released: Mutex<bool>,
    released_cond: Condvar,
}

impl Gate {
    fn wait_for_start(&self) {
        let mut started = self.started.lock().unwrap();
        while !*started {
            started = self.started_cond.wait(started).unwrap();
        }
    }

    fn signal_start(&self) {
        *self.started.lock().unwrap() = true;
        self.started_cond.notify_all();
    }

    fn wait_for_release(&self) {
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.released_cond.wait(released).unwrap();
        }
    }

    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.released_cond.notify_all();
    }
}

/// Forwards every request to a real transport, but holds the *first*
/// `Mailbox/changes` response back until the test releases it — after
/// actually making the request, so the answer is the server's honest one for
/// the account state at the time it was asked.
struct DelayedDeliveryTransport {
    inner: UreqTransport,
    gate: Arc<Gate>,
    delayed_once: AtomicBool,
}

impl Transport for DelayedDeliveryTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let is_changes_call = request
            .body
            .map(|body| String::from_utf8_lossy(body).contains("Mailbox/changes"))
            .unwrap_or(false);
        let response = self.inner.execute(request)?;
        if is_changes_call && !self.delayed_once.swap(true, Ordering::SeqCst) {
            self.gate.signal_start();
            self.gate.wait_for_release();
        }
        Ok(response)
    }
}

/// `JmapStore::detached()` is not a GObject, so it cannot be shared across a
/// real thread boundary the way Rust's `Sync` would want proof of — see
/// `disconnect_window.rs`'s copy of the same wrapper for why that is sound
/// here too.
#[derive(Clone, Copy)]
struct StorePtr(*const JmapStore);
// SAFETY: every use below only dereferences this while the `Box<JmapStore>`
// it was taken from is still alive on the test's main thread, and the test
// joins the background thread before that box is dropped.
unsafe impl Send for StorePtr {}

impl StorePtr {
    fn borrow<'a>(self) -> &'a JmapStore {
        // SAFETY: see the type's own doc comment.
        unsafe { &*self.0 }
    }
}

#[test]
fn a_folder_created_while_a_refresh_is_in_flight_is_not_lost_to_a_stale_answer() {
    let server = MockServer::builder().start();
    let gate = Arc::new(Gate::default());
    let transport = DelayedDeliveryTransport {
        inner: UreqTransport::default(),
        gate: gate.clone(),
        delayed_once: AtomicBool::new(false),
    };
    let client = Client::builder()
        .transport(transport)
        .connect(server.origin(), Credentials::none())
        .expect("connected");
    let sync = MailSync::new(client, server.account_id());

    let store = JmapStore::detached();
    store.store_connection(sync);
    // Primes the listing with the account's current (empty) tree, over a
    // plain request the gate does not touch.
    store.folders(0).expect("initial listing");

    let store_ptr = StorePtr(&*store as *const JmapStore);
    let refresh = thread::spawn(move || {
        store_ptr
            .borrow()
            .folders(eds_sys::CAMEL_STORE_FOLDER_INFO_REFRESH)
    });

    // The refresh's `Mailbox/changes` request has now been answered by the
    // server (truthfully: nothing has changed yet) and is being held back.
    gate.wait_for_start();

    let created = store
        .create_folder(None, "New")
        .expect("create_folder should succeed while the refresh is parked");
    assert_eq!(created.display_name, "New");

    // Lets the parked, now-stale "nothing changed" answer land.
    gate.release();
    let refreshed = refresh
        .join()
        .expect("the refresh thread panicked")
        .expect("refresh should still succeed");

    let paths: Vec<&str> = refreshed
        .iter()
        .map(|folder| folder.path.as_str())
        .collect();
    assert!(
        paths.contains(&"New"),
        "the folder created while the refresh was in flight was lost to the refresh's \
         stale (pre-create) answer landing after it; store's tree has {paths:?}"
    );

    // And the store's own held listing agrees — the loss this test guards
    // against is exactly the refresh's write silently replacing it.
    let held = store.folders(0).expect("cached listing");
    let held_paths: Vec<&str> = held.iter().map(|folder| folder.path.as_str()).collect();
    assert!(
        held_paths.contains(&"New"),
        "store's cached listing lost the created folder; has {held_paths:?}"
    );
}
