// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The disconnect-window bound. A disconnect must not wait out the full
//! network round trip of an in-flight store operation.
//!
//! `JmapStore::drop_connection` takes the connection slot's write lock.
//! Seven of the store's methods (`messages`, `messages_since`,
//! `message_source`, `set_keywords`, `file_message`, `expunge_message`,
//! `import_message`) used to hold that slot's *read* lock across their
//! whole network round trip. A disconnect racing one of them waited for
//! it to finish: up to one refresh, the previously measured worst case.
//! These methods now clone the connection's `Arc` and drop the guard
//! before making the request, so a disconnect no longer waits on any of
//! these seven. `folders` and the four folder-tree-writing methods are
//! deliberately unchanged. They hold the lock across their round trip on
//! purpose, to stop a concurrent `store_connection` call from
//! resurrecting stale data over the listing they are about to write. See
//! the `folders` field's own doc comment in `src/store.rs`.
//!
//! The test uses a fake [`Transport`] that blocks its second call (the
//! first is session discovery) until the test releases it. This lets a
//! `messages` call park mid-flight while `drop_connection` runs
//! concurrently.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use common::race::Gate;
use common::race::StorePtr;
use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError};
use jmap_client::{Client, Credentials};
use jmap_mail::store::JmapStore;
use jmap_mail_sync::MailSync;
use jmap_proto::Id;
use serde_json::json;

/// Answers the first request (session discovery) normally, then parks every
/// later request on `gate` until the test releases it.
struct BlockingTransport {
    gate: Arc<Gate>,
    calls: AtomicUsize,
}

impl Transport for BlockingTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Ok(HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: session_body(),
                final_url: request.url.to_owned(),
            });
        }
        self.gate.signal_start();
        self.gate.wait_for_release();
        Err(TransportError::Failed(
            "test transport released, not a real answer".to_owned(),
        ))
    }
}

fn session_body() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "capabilities": {},
        "accounts": {},
        "primaryAccounts": {},
        "username": "agent@example.com",
        "apiUrl": "https://jmap.example.com/jmap",
        "downloadUrl": "https://jmap.example.com/download/{accountId}/{blobId}/{name}",
        "uploadUrl": "https://jmap.example.com/upload/{accountId}",
        "eventSourceUrl": "",
        "state": "s0",
    }))
    .unwrap()
}

#[test]
fn dropping_the_connection_does_not_wait_out_a_message_listing_in_flight() {
    let gate = Arc::new(Gate::default());
    let transport = BlockingTransport {
        gate: gate.clone(),
        calls: AtomicUsize::new(0),
    };
    let client = Client::builder()
        .transport(transport)
        .connect("https://jmap.example.com", Credentials::none())
        .expect("session discovery should succeed");
    let sync = MailSync::new(client, Id::new("account-1"));

    let store = JmapStore::detached();
    store.store_connection(sync);
    // SAFETY: the test joins both threads before `store` drops.
    let store_ptr = unsafe { StorePtr::new(&store) };

    let mailbox = Id::new("mailbox-1");
    let listing = thread::spawn(move || store_ptr.borrow().messages(&mailbox));

    gate.wait_for_start();

    let (drop_tx, drop_rx) = mpsc::channel();
    let dropping = thread::spawn(move || {
        let start = Instant::now();
        let dropped = store_ptr.borrow().drop_connection();
        let _ = drop_tx.send((dropped, start.elapsed()));
    });

    let received = drop_rx.recv_timeout(Duration::from_secs(2));
    // Releases both threads regardless of whether `drop_connection`
    // answered in time. A failed run must not leave anything blocked
    // past this test.
    gate.release();
    dropping.join().expect("the dropping thread panicked");
    let _ = listing.join().expect("the listing thread panicked");

    let (dropped, elapsed) = received.expect(
        "drop_connection did not return within 2s while a messages call was still in flight \
         — it waited on the read lock instead of cloning the connection out first",
    );
    assert!(dropped, "drop_connection reported nothing to drop");
    assert!(
        elapsed < Duration::from_millis(500),
        "drop_connection took {elapsed:?} while a messages call was in flight; \
         it should not have waited on it at all"
    );
}
