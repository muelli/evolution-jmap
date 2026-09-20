// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `transfer_race.rs`'s transfer-vs-in-flight-listing race, proven against a
//! real JMAP server rather than `jmap-mock`.
//!
//! `transfer_race.rs` already proves the fix (`camel_folder_lock` must
//! serialise `transfer_messages_to_sync` against a refresh already
//! reconciling the same summary, or the refresh's stale-but-honest listing
//! can write the just-moved message's row back in after the move commits)
//! with a transport that wraps a real one against a real `jmap_mock::
//! MockServer`: the refresh's `Email/query` is genuinely answered, only its
//! delivery back to the caller is held. `live_server_transfer.rs` already
//! proves the plain-case transfer against real Stalwart. This file reruns
//! the race itself, gate and all, against real Stalwart instead of the mock,
//! with two real folders and one real imported message: the one thing the
//! mock version cannot show is whether a real `Email/query` answer's own
//! shape and timing still exercise the same lock correctly.
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

mod common;

use std::env;
use std::ffi::CString;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use common::Account;
use eds_sys::{
    CamelFolder, camel_folder_get_folder_summary, camel_folder_refresh_info_sync,
    camel_folder_summary_get, camel_folder_transfer_messages_to_sync,
};
use glib_sys::{GError, GFALSE, GPtrArray, GTRUE, g_error_free, g_ptr_array_free, gpointer};
use gobject_sys::g_object_unref;
use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError, UreqTransport};
use jmap_client::{Client, Credentials};
use jmap_mail::folder::new_folder;
use jmap_mail_sync::{Keywords, MailSync};
use jmap_proto::session::CAPABILITY_MAIL;

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-mail/tests/live_server_synchronize.rs::connect_for_write`.
fn connect_for_write() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user, password))
        .expect("could not fetch the session document for the write-test account");
    Some(client)
}

/// Lets the test park the racing request's response and learn when it
/// actually parked, rather than racing a sleep against it. Copy of
/// `transfer_race.rs`'s `Gate`.
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
/// `Email/query` response back until the test releases it — after actually
/// making the request against the real server, so the answer is honest (the
/// message still shows present) for the mailbox's contents at the time it
/// was asked, before the race's transfer moves it away. Copy of
/// `transfer_race.rs`'s `DelayedDeliveryTransport`.
struct DelayedDeliveryTransport {
    inner: UreqTransport,
    gate: Arc<Gate>,
    delayed_once: AtomicBool,
}

impl Transport for DelayedDeliveryTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let is_query = request
            .body
            .map(|body| String::from_utf8_lossy(body).contains("Email/query"))
            .unwrap_or(false);
        let response = self.inner.execute(request)?;
        if is_query && !self.delayed_once.swap(true, Ordering::SeqCst) {
            self.gate.signal_start();
            self.gate.wait_for_release();
        }
        Ok(response)
    }
}

/// A raw folder pointer, sent to a background thread. Copy of
/// `transfer_race.rs`'s `FolderPtr`.
#[derive(Clone, Copy)]
struct FolderPtr(*mut CamelFolder);
// SAFETY: the folder outlives every use of this wrapper, and every use goes
// through Camel's own thread-safe vfunc dispatch.
unsafe impl Send for FolderPtr {}

/// The uid array Camel hands the vfunc, owned for the length of one call.
/// Mirrors `live_server_transfer.rs::UidList`.
struct UidList {
    array: *mut GPtrArray,
    #[allow(dead_code)]
    uid: CString,
}

impl UidList {
    fn of(uid: &str) -> Self {
        let uid = CString::new(uid).expect("a uid with no NUL");
        // SAFETY: a fresh array, filled with one pointer into `uid`, which
        // this value owns and outlives it by.
        let array = unsafe {
            let array = glib_sys::g_ptr_array_new();
            glib_sys::g_ptr_array_add(array, uid.as_ptr() as gpointer);
            array
        };
        Self { array, uid }
    }
}

impl Drop for UidList {
    fn drop(&mut self) {
        // SAFETY: the one array, allocated above; FALSE because the pointer
        // in it belongs to `self.uid`.
        unsafe { g_ptr_array_free(self.array, GFALSE) };
    }
}

/// Mirrors `live_server_folder_listing_race.rs::connect_for_write_with_gate`.
fn connect_for_write_with_gate(gate: Arc<Gate>) -> Client {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER")
        .expect("connect_for_write already confirmed this is set");
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("connect_for_write already confirmed this is set");
    let origin =
        env::var("JMAP_LIVE_SERVER_URL").expect("connect_for_write already confirmed this is set");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let transport = DelayedDeliveryTransport {
        inner: UreqTransport::default(),
        gate,
        delayed_once: AtomicBool::new(false),
    };
    Client::builder()
        .transport(transport)
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user, password))
        .expect("could not fetch the session document for the write-test account")
}

/// Imports a message into a real source folder, then races a transfer of it
/// out against a refresh already in flight on the same folder: the refresh's
/// `Email/query` is genuinely answered by the real server (truthfully: the
/// message is still there) but its delivery is held until the transfer has
/// moved the message away. The transfer must block on the refresh's
/// `camel_folder_lock` rather than complete concurrently, and once the
/// stale-but-honest answer is finally allowed to land, the transferred
/// message's row must not come back into the source folder's local summary.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_transfer_out_of_a_folder_does_not_lose_to_a_listing_already_in_flight_on_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    let sync = MailSync::new(client, account_id.clone());
    let account = Account::open();
    account.connect(sync);

    let suffix = unique_suffix();
    let source_name = format!("agent-mailtransferrace-source-{suffix}");
    let dest_name = format!("agent-mailtransferrace-dest-{suffix}");
    let source_info = account
        .jmap()
        .create_folder(None, &source_name)
        .expect("create_folder failed against the real server for the source");
    let dest_info = account
        .jmap()
        .create_folder(None, &dest_name)
        .expect("create_folder failed against the real server for the destination");

    let subject = format!("agent-mailtransferrace-{suffix}");
    let source_bytes = format!(
        "From: agent-mailtransferrace@example.invalid\r\n\
         To: agent-mailtransferrace@example.invalid\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         Body.\r\n"
    )
    .into_bytes();
    let uid = account
        .jmap()
        .import_message(&source_info.id, source_bytes, &Keywords::default(), None)
        .expect("import_message failed against the real server");

    // Swaps the account's connection for the gated one: source_info/dest_info
    // are plain data from the plain connection above and stay valid.
    let gate = Arc::new(Gate::default());
    let gated = connect_for_write_with_gate(Arc::clone(&gate));
    account.connect(MailSync::new(gated, account_id.clone()));

    // SAFETY: `account.store` is a live `CamelStore`, and both `FolderInfo`s
    // came from a real discovery over that same store's connection.
    let source = FolderPtr(unsafe { new_folder(account.store, &source_info) });
    let dest = FolderPtr(unsafe { new_folder(account.store, &dest_info) });
    assert!(
        !source.0.is_null() && !dest.0.is_null(),
        "the folders would not construct"
    );

    // Never refreshed, so this is the full listing the race needs: a delta
    // would ask only what changed since a recorded state, and "nothing
    // changed" cannot resurrect a row the way a listing's "still present"
    // can.
    let refresh = thread::spawn(move || {
        // Named again so the closure captures the whole `Send` wrapper
        // rather than just the raw pointer field inside it.
        let source = source;
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live folder, never refreshed before, and an
        // out-parameter that is writable and currently NULL.
        unsafe { camel_folder_refresh_info_sync(source.0, ptr::null_mut(), &mut error) != GFALSE }
    });

    gate.wait_for_start();

    let (done_tx, done_rx) = channel();
    let uid_to_move = uid.clone();
    let transfer = thread::spawn(move || {
        // As above: names the whole `Send` wrappers so the closure does not
        // capture their raw pointer fields directly.
        let (source, dest) = (source, dest);
        let list = UidList::of(uid_to_move.as_str());
        let mut transferred: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: two live folders of one store, an array of one
        // NUL-terminated uid alive across the call, and two out-parameters
        // that are writable and currently NULL.
        let ok = unsafe {
            let ok = camel_folder_transfer_messages_to_sync(
                source.0,
                list.array,
                dest.0,
                GTRUE,
                &mut transferred,
                ptr::null_mut(),
                &mut error,
            );
            if !transferred.is_null() {
                g_ptr_array_free(transferred, GTRUE);
            }
            if !error.is_null() {
                g_error_free(error);
            }
            ok != GFALSE
        };
        // The test is done listening once it has answered either way, so a
        // send that fails (the check below already gave up) is not a bug in
        // this thread.
        let _ = done_tx.send(ok);
    });

    // `camel_folder_transfer_messages_to_sync`'s own wrapper takes no lock —
    // see `transfer_race.rs` for the confirmation against evolution-data-
    // server 3.52.3's `camel-folder.c`. So the vfunc itself must take it, or
    // the transfer above races ahead of the in-flight (and, at this instant,
    // still-parked) listing instead of waiting for it. A full second gives
    // real network latency plenty of room without weakening the check: an
    // unlocked transfer would still complete on its own round trip well
    // inside it.
    match done_rx.recv_timeout(Duration::from_secs(1)) {
        Ok(_) => panic!(
            "transfer_messages_to_sync completed while a refresh held the folder's summary; \
             it must take camel_folder_lock the way refresh_info_sync, synchronize_sync and \
             expunge_sync's own Camel wrappers do"
        ),
        Err(RecvTimeoutError::Timeout) => {}
        Err(RecvTimeoutError::Disconnected) => panic!("the transfer thread died without answering"),
    }

    // Lets the parked, now-stale (message still shows present) listing land.
    gate.release();

    let transferred_ok = done_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the transfer must finish once the refresh releases the folder");
    assert!(
        transferred_ok,
        "the transfer failed against the real server"
    );
    transfer.join().expect("the transfer thread panicked");
    assert!(
        refresh.join().expect("the refresh thread panicked"),
        "the refresh failed against the real server"
    );

    // The message the transfer moved away must not have come back: the
    // listing that named it was answered before the move and must not be
    // allowed to write its row back in after the move committed.
    let uid_c = CString::new(uid.as_str()).expect("a uid with no NUL");
    // SAFETY: a live folder with a summary, and a NUL-terminated uid.
    let row = unsafe {
        camel_folder_summary_get(camel_folder_get_folder_summary(source.0), uid_c.as_ptr())
    };
    assert!(
        row.is_null(),
        "the transferred message's row came back into the source folder it was moved out of"
    );
    if !row.is_null() {
        // SAFETY: the one reference `summary_get` handed back.
        unsafe { g_object_unref(row.cast()) };
    }

    // Independent confirmation: a fresh connection, sharing nothing with the
    // ones above, sees the real server agrees.
    let verify_client = connect_for_write().expect("the write-test account is still there");
    let verify_sync = MailSync::new(verify_client, account_id);
    let (_, dest_messages) = verify_sync
        .messages(&dest_info.id)
        .expect("listing the destination folder against the real server failed");
    assert!(
        dest_messages.iter().any(|message| message.uid == uid),
        "the real server should list the message under the destination folder"
    );
    let (_, source_messages) = verify_sync
        .messages(&source_info.id)
        .expect("listing the source folder against the real server failed");
    assert!(
        !source_messages.iter().any(|message| message.uid == uid),
        "the real server should no longer list the message under the source folder"
    );

    // The server refuses to delete a non-empty mailbox, so the message goes
    // first.
    account
        .jmap()
        .expunge_message(&uid, &dest_info.id)
        .expect("expunge_message failed against the real server");
    account
        .jmap()
        .delete_folder(&source_info.id)
        .expect("delete_folder failed against the real server for the source");
    account
        .jmap()
        .delete_folder(&dest_info.id)
        .expect("delete_folder failed against the real server for the destination");

    // SAFETY: the one reference each `new_folder` call returned.
    unsafe {
        g_object_unref(source.0.cast());
        g_object_unref(dest.0.cast());
    }
}
