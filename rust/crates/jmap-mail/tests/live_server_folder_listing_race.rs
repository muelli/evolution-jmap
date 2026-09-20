// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `folder_listing_race.rs`'s stale-refresh-vs-concurrent-create race,
//! proven against a real JMAP server rather than `jmap-mock`.
//!
//! `folder_listing_race.rs` already proves the fix (`JmapStore::folders`
//! must not let a `Mailbox/changes` answer that was computed before a
//! concurrent `create_folder` but delivered after it erase that folder from
//! the cache) with a transport that wraps a real one against a real
//! `jmap_mock::MockServer`: the request is genuinely answered by the
//! server, only its delivery back to the caller is held. That technique
//! does not care which server answered the request, so this file reruns the
//! identical race, gate and all, against real Stalwart instead of the mock:
//! the one thing the mock version cannot show is whether a real
//! `Mailbox/changes` answer's own shape still round-trips through the same
//! decision correctly under the same timing.
//!
//! ## Running it
//!
//! Same environment as the other live-server tests — see
//! `docs/manual-test-live-server.md`:
//!
//! ```console
//! $ cargo test -p jmap-mail --features testing -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError, UreqTransport};
use jmap_client::{Client, Credentials};
use jmap_mail::store::JmapStore;
use jmap_mail_sync::MailSync;
use jmap_proto::session::CAPABILITY_MAIL;

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Lets the test park the `Mailbox/changes` response and learn when it
/// actually parked, rather than racing a sleep against it. Copy of
/// `folder_listing_race.rs`'s `Gate`.
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
/// `Mailbox/changes` response back until the test releases it, after
/// actually making the request against the real server. Copy of
/// `folder_listing_race.rs`'s `DelayedDeliveryTransport`.
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
/// real thread boundary the way Rust's `Sync` would want proof of; see
/// `folder_listing_race.rs`'s copy of the same wrapper for why that is sound
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

/// Mirrors `live_server_folder.rs::connect_for_write`, but installs the
/// delayed-delivery transport rather than the default one.
fn connect_for_write_with_gate(gate: Arc<Gate>) -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let transport = DelayedDeliveryTransport {
        inner: UreqTransport::default(),
        gate,
        delayed_once: AtomicBool::new(false),
    };
    let client = Client::builder()
        .transport(transport)
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user, password))
        .expect("could not fetch the session document for the write-test account");
    Some(client)
}

/// Same scenario as `folder_listing_race.rs`, against real Stalwart: a
/// refresh's `Mailbox/changes` request is genuinely answered (truthfully:
/// nothing has changed yet) but its delivery is held until a concurrent
/// `create_folder` has finished, so the stale-but-honest answer must not
/// erase the folder the create just added.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_folder_created_while_a_refresh_is_in_flight_is_not_lost_to_a_stale_answer_on_the_real_server()
{
    let gate = Arc::new(Gate::default());
    let Some(client) = connect_for_write_with_gate(gate.clone()) else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let sync = MailSync::new(client, account_id);

    let store = JmapStore::detached();
    store.store_connection(sync);
    // Primes the cache with the account's current tree, over a plain
    // request the gate does not touch (it only holds `Mailbox/changes`).
    store.folders(0).expect("initial listing failed");

    let store_ptr = StorePtr(&*store as *const JmapStore);
    let refresh = thread::spawn(move || {
        store_ptr
            .borrow()
            .folders(eds_sys::CAMEL_STORE_FOLDER_INFO_REFRESH)
    });

    // The refresh's `Mailbox/changes` request has now been genuinely
    // answered by the real server and is being held back from the caller.
    gate.wait_for_start();

    let name = format!("agent-folderrace-{}", unique_suffix());
    let created = store
        .create_folder(None, &name)
        .expect("create_folder should succeed while the refresh is parked");
    assert_eq!(created.display_name, name);

    // Lets the parked, now-stale (pre-create) answer land.
    gate.release();
    let refreshed = refresh
        .join()
        .expect("the refresh thread panicked")
        .expect("refresh should still succeed against the real server");

    let paths: Vec<&str> = refreshed
        .iter()
        .map(|folder| folder.path.as_str())
        .collect();
    assert!(
        paths.contains(&name.as_str()),
        "the folder created while the refresh was in flight was lost to the refresh's \
         stale (pre-create) answer landing after it; store's tree has {paths:?}"
    );

    // And the store's own held listing agrees.
    let held = store.folders(0).expect("cached listing");
    let held_paths: Vec<&str> = held.iter().map(|folder| folder.path.as_str()).collect();
    assert!(
        held_paths.contains(&name.as_str()),
        "store's cached listing lost the created folder; has {held_paths:?}"
    );

    store
        .delete_folder(&created.id)
        .expect("cleanup: delete_folder failed against the real server");
}
