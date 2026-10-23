// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A refresh's response can arrive late, after a local edit made by the
//! connection in the meantime. `JmapStore::folders` must not let the late
//! answer erase the edit.
//!
//! `folders(REFRESH)` snapshots the listing, makes a `Mailbox/changes` round
//! trip, then writes the result back unconditionally. `create_folder` and
//! its three siblings edit the *current* listing in place, under the same
//! lock, as soon as the server confirms the write. This edit has no ordering
//! against a refresh that started earlier and is still waiting on its own
//! response. A refresh's `Mailbox/changes` answer can be computed before the
//! create, so it truthfully says nothing changed. If that answer is
//! *delivered* after the create, it overwrites the listing with the
//! pre-create tree. This silently loses the folder Camel was just told
//! exists.
//!
//! This is proven with a transport that wraps a real one against a real
//! [`MockServer`]. The `Mailbox/changes` request completes for real, so its
//! answer is the server's honest one for the state at the time. Its delivery
//! back to the caller is held until a concurrent `create_folder` has
//! finished.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use common::race::Gate;
use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError, UreqTransport};
use jmap_client::{Client, Credentials};
use jmap_mail::store::JmapStore;
use jmap_mail_sync::MailSync;
use jmap_mock::MockServer;

/// Forwards every request to a real transport. Holds back the *first*
/// `Mailbox/changes` response until the test releases it. The request is
/// made first, so the answer is the server's honest one for the account
/// state at the time it was asked.
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
// SAFETY: every use below dereferences this only while the `Box<JmapStore>`
// it came from is still alive on the test's main thread. The test joins the
// background thread before that box drops.
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
    // Primes the listing with the account's current (empty) tree. Uses a
    // plain request the gate does not touch.
    store.folders(0).expect("initial listing");

    let store_ptr = StorePtr(&*store as *const JmapStore);
    let refresh = thread::spawn(move || {
        store_ptr
            .borrow()
            .folders(eds_sys::CAMEL_STORE_FOLDER_INFO_REFRESH)
    });

    // The server has now answered the refresh's `Mailbox/changes` request.
    // The answer truthfully says nothing has changed yet, and the gate
    // holds it back.
    gate.wait_for_start();

    let created = store
        .create_folder(None, "New")
        .expect("create_folder should succeed while the refresh is parked");
    assert_eq!(created.display_name, "New");

    // Releases the gate. The stale "nothing changed" answer lands now.
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

    // The store's own held listing agrees. The loss this test guards
    // against is the refresh's write silently replacing it.
    let held = store.folders(0).expect("cached listing");
    let held_paths: Vec<&str> = held.iter().map(|folder| folder.path.as_str()).collect();
    assert!(
        held_paths.contains(&"New"),
        "store's cached listing lost the created folder; has {held_paths:?}"
    );
}
