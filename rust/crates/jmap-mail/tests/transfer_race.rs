// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A transfer out of a folder must not lose to a listing that was already in
//! flight when the transfer started.
//!
//! `camel_folder_refresh_info_sync`, `_synchronize_sync`, and `_expunge_sync`
//! each take `camel_folder_lock` for the whole of their vfunc dispatch, so on
//! one folder those three cannot overlap. But
//! `camel_folder_transfer_messages_to_sync`'s own wrapper takes no such lock
//! on its source folder. So nothing before this crate's fix serialized a
//! transfer against a refresh that was already reconciling the same summary.
//!
//! This test reproduces the race with a transport that wraps a real one
//! against a real [`MockServer`]. The refresh's `Email/query` is allowed to
//! complete for real, so its answer truthfully lists the message, which has
//! not moved yet. But its delivery back to the caller is held until a
//! concurrent transfer out of the same folder has finished.

use std::ffi::CString;
use std::ptr;
use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::thread;
use std::time::Duration;

use eds_sys::{
    CamelFolder, camel_folder_get_folder_summary, camel_folder_refresh_info_sync,
    camel_folder_summary_get, camel_folder_transfer_messages_to_sync,
};
use glib_sys::{GError, GFALSE, GPtrArray, GTRUE, g_error_free, g_free, g_ptr_array_free};
use gobject_sys::g_object_unref;
use jmap_client::{Client, Credentials};
use jmap_mail::folder::new_folder;
use jmap_mail_sync::MailSync;
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::mail::role;

mod common;
use common::Account;
use common::array::StringArray;
use common::race::DelayedDeliveryTransport;
use common::race::Gate;

/// A raw folder pointer, sent to a background thread.
///
/// Sound here for the same reason `folder_listing_race.rs`'s `StorePtr` is:
/// the folder is a real `CamelFolder` GObject, referenced for the rest of
/// this function and freed only after both threads below have been joined.
#[derive(Clone, Copy)]
struct FolderPtr(*mut CamelFolder);
// SAFETY: the folder outlives every use of this wrapper, and every use goes
// through Camel's own thread-safe vfunc dispatch.
unsafe impl Send for FolderPtr {}

#[test]
fn a_transfer_out_of_a_folder_does_not_lose_to_a_listing_already_in_flight() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let uid = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox_id = account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_mailbox("Archive", None);
        account.seed_email(EmailSeed::new(
            inbox_id,
            ("Bob", "bob@example.com"),
            "Lunch?",
            "One o'clock.",
            "2026-01-15T09:30:00Z",
        ))
    };

    let account = Account::open();

    // A plain connection, only to read the mailbox tree that `new_folder`
    // needs. Opening the folders through `camel_store_get_folder_sync`
    // instead would run this crate's own auto-refresh on first open, and
    // consume the one "never listed" moment this race needs. See
    // `folders.rs`'s `get_folder_sync`.
    let plain = Client::connect(server.origin(), Credentials::none()).expect("connected");
    account.connect(MailSync::new(plain, account_id.clone()));
    let tree = account.jmap().folders(0).expect("initial tree");
    let inbox_info = tree.find("Inbox").expect("the seeded inbox").clone();
    let archive_info = tree.find("Archive").expect("the seeded archive").clone();

    let gate = Arc::new(Gate::default());
    let transport = DelayedDeliveryTransport::new(Arc::clone(&gate), "Email/query");
    let gated = Client::builder()
        .transport(transport)
        .connect(server.origin(), Credentials::none())
        .expect("connected");
    account.connect(MailSync::new(gated, account_id));

    // SAFETY: `account.store` is a live `CamelStore`, and both `FolderInfo`s
    // came from a tree read over a connection to that same store.
    let inbox = FolderPtr(unsafe { new_folder(account.store, &inbox_info) });
    let archive = FolderPtr(unsafe { new_folder(account.store, &archive_info) });
    assert!(
        !inbox.0.is_null() && !archive.0.is_null(),
        "the folders would not construct"
    );

    // Never refreshed, so this is the full listing the race needs. A delta
    // would ask only what changed since a recorded state. "Nothing changed"
    // cannot resurrect a row the way a listing's "still present" can.
    let refresh = thread::spawn(move || {
        // Named again so the closure captures the whole `Send` wrapper
        // rather than just the raw pointer field inside it.
        let inbox = inbox;
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live folder, never refreshed before, and an
        // out-parameter that is writable and currently NULL.
        unsafe { camel_folder_refresh_info_sync(inbox.0, ptr::null_mut(), &mut error) != GFALSE }
    });

    gate.wait_for_start();

    let (done_tx, done_rx) = channel();
    let uid_to_move = uid.clone();
    let transfer = thread::spawn(move || {
        // As above: names the whole `Send` wrappers so the closure does not
        // capture their raw pointer fields directly.
        let (inbox, archive) = (inbox, archive);
        let list = StringArray::of(&[&uid_to_move]);
        let mut transferred: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: two live folders of one store, and an array of
        // NUL-terminated uids alive across the call. Two out-parameters,
        // both writable and currently NULL.
        let ok = unsafe {
            let ok = camel_folder_transfer_messages_to_sync(
                inbox.0,
                list.as_ptr(),
                archive.0,
                GTRUE,
                &mut transferred,
                ptr::null_mut(),
                &mut error,
            );
            // Camel's own callers free the members with `g_free`, then the array.
            if !transferred.is_null() {
                for index in 0..(*transferred).len {
                    g_free(*(*transferred).pdata.add(index as usize));
                }
                g_ptr_array_free(transferred, GTRUE);
            }
            if !error.is_null() {
                g_error_free(error);
            }
            ok != GFALSE
        };
        // The test stops listening once it has answered either way. So a
        // send that fails (the 300ms check below already gave up) is not a
        // bug in this thread.
        let _ = done_tx.send(ok);
    });

    // `camel_folder_transfer_messages_to_sync`'s own wrapper takes no lock.
    // This is confirmed against evolution-data-server 3.52.3's
    // `camel-folder.c`, where the wrappers for `refresh_info_sync`,
    // `synchronize_sync`, and `expunge_sync` all call
    // `camel_folder_lock`/`_unlock` around their vfunc dispatch, and the
    // wrapper for `transfer_messages_to_sync` does not. So the vfunc itself
    // must take the lock, or the transfer above races ahead of the
    // in-flight (and, at this instant, still-parked) listing, instead of
    // waiting for it.
    match done_rx.recv_timeout(Duration::from_millis(300)) {
        Ok(_) => panic!(
            "transfer_messages_to_sync completed while a refresh held the folder's summary; \
             it must take camel_folder_lock the way refresh_info_sync, synchronize_sync and \
             expunge_sync's own Camel wrappers do"
        ),
        Err(RecvTimeoutError::Timeout) => {}
        Err(RecvTimeoutError::Disconnected) => panic!("the transfer thread died without answering"),
    }

    // Lets the parked, now-stale listing land (it still shows the message
    // as present).
    gate.release();

    let transferred_ok = done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the transfer must finish once the refresh releases the folder");
    assert!(transferred_ok, "the transfer failed");
    transfer.join().expect("the transfer thread panicked");
    assert!(
        refresh.join().expect("the refresh thread panicked"),
        "the refresh failed"
    );

    // The message the transfer moved away must not have come back. The
    // listing that named it was answered before the move, and must not be
    // allowed to write its row back in after the move committed.
    let uid_c = CString::new(uid.as_str()).expect("a uid with no NUL");
    // SAFETY: a live folder with a summary, and a NUL-terminated uid.
    let row = unsafe {
        camel_folder_summary_get(camel_folder_get_folder_summary(inbox.0), uid_c.as_ptr())
    };
    assert!(
        row.is_null(),
        "the transferred message's row came back into the folder it was moved out of"
    );
    if !row.is_null() {
        // SAFETY: the one reference `summary_get` handed back.
        unsafe { g_object_unref(row.cast()) };
    }

    // SAFETY: the one reference each construction handed back.
    unsafe {
        g_object_unref(inbox.0.cast());
        g_object_unref(archive.0.cast());
    }
}
