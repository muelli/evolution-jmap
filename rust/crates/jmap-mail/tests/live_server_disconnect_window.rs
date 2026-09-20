// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `disconnect_window.rs`'s disconnect-does-not-wait-out-an-in-flight-call
//! bound, proven against a real JMAP server rather than a fake blocking
//! [`Transport`].
//!
//! `disconnect_window.rs` proves that `JmapStore::drop_connection` no longer
//! waits out a `messages` call's whole network round trip, using a fake
//! transport that blocks the request itself before it is ever sent. That
//! shows the store clones the connection's `Arc` and drops its read guard
//! before making the request, but it cannot show what happens when a real
//! network round trip to a real server is genuinely in flight while the
//! drop runs. This file reruns the same race with the delayed-delivery
//! technique `live_server_folder_listing_race.rs` already uses: a real
//! request is genuinely answered by Stalwart, only its delivery back to the
//! caller is held, so `drop_connection` races an in-flight real round trip
//! rather than a request that never left the process.
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
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

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

/// Lets the test park a transport call and learn when it actually parked,
/// rather than racing a sleep against it. Copy of `disconnect_window.rs`'s
/// `Gate`.
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

/// Forwards every request to a real transport, but holds back the delivery
/// of the first `Email/get` request until the test releases it — after the
/// real server has already answered it. `messages()` makes exactly this
/// call (fetching the account's `Email` state) right after cloning the
/// connection's `Arc` and dropping the read guard, so holding its delivery
/// is exactly the window `drop_connection` needs to race. Matched on the
/// request body rather than a raw call index: `create_folder` (`Mailbox/set`)
/// runs on the same client and transport before the race even starts, so a
/// plain call counter would arm on one of *its* requests instead and
/// deadlock the calling thread against a gate nothing else will ever
/// release.
struct DelayedDeliveryTransport {
    inner: UreqTransport,
    gate: Arc<Gate>,
    delayed_once: AtomicBool,
}

impl Transport for DelayedDeliveryTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let is_email_get = request
            .body
            .map(|body| String::from_utf8_lossy(body).contains("Email/get"))
            .unwrap_or(false);
        let response = self.inner.execute(request)?;
        if is_email_get && !self.delayed_once.swap(true, Ordering::SeqCst) {
            self.gate.signal_start();
            self.gate.wait_for_release();
        }
        Ok(response)
    }
}

/// `JmapStore::detached()` is not a GObject, so it cannot be shared across a
/// real thread boundary the way Rust's `Sync` would want proof of; see
/// `disconnect_window.rs`'s copy of the same wrapper for why that is sound
/// here too.
#[derive(Clone, Copy)]
struct StorePtr(*const JmapStore);
// SAFETY: every use below only dereferences this while the `Box<JmapStore>`
// it was taken from is still alive on the test's main thread, and the test
// joins both threads before that box is dropped.
unsafe impl Send for StorePtr {}

impl StorePtr {
    fn borrow<'a>(self) -> &'a JmapStore {
        // SAFETY: see the type's own doc comment.
        unsafe { &*self.0 }
    }
}

/// Mirrors `live_server_folder_listing_race.rs`'s `connect_for_write_with_gate`.
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

/// Same scenario as `disconnect_window.rs`, against real Stalwart: a
/// `messages` listing's first request is genuinely in flight (answered by
/// the server, held before delivery) when `drop_connection` runs
/// concurrently; the drop must not wait out the round trip.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn dropping_the_connection_does_not_wait_out_a_message_listing_in_flight_on_the_real_server() {
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

    let name = format!("agent-disconnectwindow-{}", unique_suffix());
    let created = store
        .create_folder(None, &name)
        .expect("create_folder should succeed against the real server");

    let store_ptr = StorePtr(&*store as *const JmapStore);
    let mailbox = created.id.clone();
    let listing = thread::spawn(move || store_ptr.borrow().messages(&mailbox));

    // The listing's first request (the account's `Email` state) has now
    // been genuinely answered by the real server and is being held back
    // from the caller.
    gate.wait_for_start();

    let (drop_tx, drop_rx) = mpsc::channel();
    let dropping = thread::spawn(move || {
        let start = Instant::now();
        let dropped = store_ptr.borrow().drop_connection();
        let _ = drop_tx.send((dropped, start.elapsed()));
    });

    let received = drop_rx.recv_timeout(Duration::from_secs(5));
    // Unparks both threads regardless of whether `drop_connection` answered
    // in time, so a red run cannot leave anything blocked past this test.
    gate.release();
    dropping.join().expect("the dropping thread panicked");
    let listed = listing
        .join()
        .expect("the listing thread panicked")
        .expect("the listing should still complete against the real server");

    let (dropped, elapsed) = received.expect(
        "drop_connection did not return within 5s while a messages call was still in flight \
         against the real server — it waited on the read lock instead of cloning the \
         connection out first",
    );
    assert!(dropped, "drop_connection reported nothing to drop");
    assert!(
        elapsed < Duration::from_millis(500),
        "drop_connection took {elapsed:?} while a real messages call was in flight; \
         it should not have waited on it at all"
    );
    assert!(
        listed.1.is_empty(),
        "the freshly created folder should have no messages yet"
    );

    // Cleanup needs its own connection: `drop_connection` above already took
    // this store's.
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");
    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(
            &env::var("JMAP_LIVE_SERVER_URL").unwrap(),
            Credentials::basic(
                env::var("JMAP_LIVE_SERVER_WRITE_USER").unwrap(),
                env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD").unwrap(),
            ),
        )
        .expect("could not reconnect for cleanup");
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let cleanup_sync = MailSync::new(client, account_id);
    let cleanup_store = JmapStore::detached();
    cleanup_store.store_connection(cleanup_sync);
    cleanup_store
        .delete_folder(&created.id)
        .expect("cleanup: delete_folder failed against the real server");
}
