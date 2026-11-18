// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A refresh of the destination folder must not give that folder two adds of
//! one uid.
//!
//! `place_in_destination` asks the destination summary whether it has the uid
//! already, and adds a clone of the source row when it does not. Camel keeps
//! the uid of the clone (`force_keep_uid`), which also tells Camel to skip its
//! own clash test. The two calls are one step only while the summary stays
//! locked between them.
//!
//! A refresh of the same folder adds its rows under that same lock
//! (`apply_delta`). A refresh that lands between the question and the add thus
//! adds the uid first, and the add after it counts the same message a second
//! time. Camel then shows one more message, and one more unread message, than
//! the folder holds, for as long as the summary stays in memory.
//!
//! The test runs the two against each other on one account: the user moves a
//! batch of messages into a folder while offline, and a delta for that same
//! folder arrives at the same time.
//!
//! The delta aims at the gap instead of hoping for it. A queued move marks
//! the row it leaves `CAMEL_MESSAGE_DELETED`, the last step before the move
//! asks the destination about that uid, so the thread that applies the delta
//! waits for that mark and then adds the same uid at once.

use std::ffi::CString;
use std::ptr;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use eds_sys::{
    CAMEL_MESSAGE_DELETED, CAMEL_STORE_FOLDER_NONE, CamelFolder, CamelFolderClass,
    CamelFolderSummary, camel_folder_get_folder_summary, camel_folder_refresh_info_sync,
    camel_folder_summary_count, camel_folder_summary_get_info_flags,
    camel_folder_summary_get_saved_count, camel_folder_summary_get_visible_count,
    camel_store_get_folder_sync,
};
use glib_sys::{GError, GFALSE, GPtrArray, g_clear_error, gboolean};
use gobject_sys::{g_object_unref, g_type_class_ref, g_type_class_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::folder::folder_type;
use jmap_mail::summary::apply_delta;
use jmap_mail_sync::{MailSync, MessageFlags, MessageSummary};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::role;

mod common;
use common::Account;
use common::array::StringArray;

/// How many messages the move carries. The two threads work through the same
/// uids at a similar rate, so a batch gives the refresh many chances to land
/// between the question and the add of one of them.
const BATCH: usize = 120;

/// A raw summary pointer, sent to the refreshing thread.
///
/// Sound for the same reason as the folder pointer of the other race tests:
/// the summary belongs to a folder that is referenced for the rest of this
/// test, and freed only after that thread has been joined. Camel's summary
/// has its own lock, which both threads take.
#[derive(Clone, Copy)]
struct SummaryPtr(*mut CamelFolderSummary);
// SAFETY: the summary outlives every use of this wrapper, and every use goes
// through Camel's own thread-safe summary calls.
unsafe impl Send for SummaryPtr {}

/// One of the account's folders, opened the way Camel opens one.
fn open(account: &Account, path: &str) -> *mut CamelFolder {
    let path = CString::new(path).expect("a path with no NUL");
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live store of ours, a NUL-terminated path alive across the
    // call, and an out-parameter that is writable and currently NULL.
    unsafe {
        let folder = camel_store_get_folder_sync(
            account.store,
            path.as_ptr(),
            CAMEL_STORE_FOLDER_NONE,
            ptr::null_mut(),
            &mut error,
        );
        assert!(
            !folder.is_null() && error.is_null(),
            "the folder would not open"
        );
        folder
    }
}

/// The summary of a folder this test opened.
fn summary_of(folder: *mut CamelFolder) -> *mut CamelFolderSummary {
    // SAFETY: a live folder of ours.
    let summary = unsafe { camel_folder_get_folder_summary(folder) };
    assert!(!summary.is_null(), "the folder has no summary");
    summary
}

/// Moves `uids` through the pointer in the class, skipping the reconnect that
/// Camel's own wrapper makes before it calls a provider offline.
fn move_straight(source: *mut CamelFolder, destination: *mut CamelFolder, uids: &[Id]) {
    let list = StringArray::of(uids);
    let mut transferred: *mut GPtrArray = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: referencing the class runs the class_init that installs the
    // vfunc. Both folders are instances of that class, the uid array is alive
    // across the call, and the two out-parameters are writable and currently
    // NULL.
    let ok = unsafe {
        let class = g_type_class_ref(folder_type()).cast::<CamelFolderClass>();
        let vfunc = (*class)
            .transfer_messages_to_sync
            .expect("the folder cannot transfer messages");
        let ok = vfunc(
            source,
            list.as_ptr(),
            destination,
            gboolean::from(true),
            &mut transferred,
            ptr::null_mut(),
            &mut error,
        );
        g_type_class_unref(class.cast());
        g_clear_error(&mut error);
        ok
    };
    assert_ne!(ok, GFALSE, "the offline move failed");
}

/// One row of a delta for the destination folder, as the server describes a
/// message that is in that mailbox now.
fn listed(uid: &Id) -> MessageSummary {
    MessageSummary {
        uid: uid.clone(),
        blob_id: None,
        thread_id: None,
        flags: MessageFlags::default(),
        tags: Vec::new(),
        size: 0,
        received_at: None,
        sent_at: None,
        subject: None,
        from: Vec::new(),
        to: Vec::new(),
        cc: Vec::new(),
        message_id: None,
        references: Vec::new(),
        preview: None,
    }
}

/// Waits until the move hides the source row of `uid`, the step just before
/// it asks the destination about that same uid.
///
/// The mark is `queue_filing`'s own, and it reaches the flags of the summary
/// through Camel's setter. The wait gives up after a second, so a uid the
/// move never reaches costs one second and not the whole test run.
fn wait_for_queued_move(source: *mut CamelFolderSummary, uid: &CString) {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        // SAFETY: a live summary of ours, and a uid that outlives the call.
        let flags = unsafe { camel_folder_summary_get_info_flags(source, uid.as_ptr()) };
        if flags & CAMEL_MESSAGE_DELETED != 0 || Instant::now() >= deadline {
            return;
        }
        std::hint::spin_loop();
    }
}

/// A delta for the destination folder, arriving while the move puts its own
/// rows there, must leave the folder with one row and one count per message.
///
/// `place_in_destination` asked `camel_folder_summary_check_uid` and then
/// added with `force_keep_uid`, which skips Camel's own clash test. Nothing
/// held the summary between the two, so a delta that added the uid in that
/// gap got a second add of the same message: the uid list stayed as it was,
/// and the message counts of the folder went one higher for each such add.
#[test]
fn a_delta_for_the_destination_does_not_double_count_a_queued_arrival() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let uids = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox_id = account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_mailbox("Archive", None);
        (0..BATCH)
            .map(|index| {
                account.seed_email(EmailSeed::new(
                    inbox_id.clone(),
                    ("Bob", "bob@example.com"),
                    &format!("Lunch? {index}"),
                    "One o'clock.",
                    "2026-01-15T09:30:00Z",
                ))
            })
            .collect::<Vec<_>>()
    };

    let account = Account::open();
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    account.connect(MailSync::new(client, account_id));

    let inbox = open(&account, "Inbox");
    let archive = open(&account, "Archive");
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder of ours, and an out-parameter that is writable
    // and currently NULL.
    unsafe {
        assert_ne!(
            camel_folder_refresh_info_sync(inbox, ptr::null_mut(), &mut error),
            GFALSE,
            "the inbox would not refresh"
        );
    }

    // The move needs no connection: it queues a filing on each row.
    assert!(account.jmap().drop_connection());

    let target = SummaryPtr(summary_of(archive));
    let left = SummaryPtr(summary_of(inbox));
    let delta: Vec<MessageSummary> = uids.iter().map(listed).collect();
    let keys: Vec<CString> = uids
        .iter()
        .map(|uid| CString::new(uid.as_str()).expect("a uid with no NUL"))
        .collect();
    let start = Arc::new(Barrier::new(2));
    let refresh_start = Arc::clone(&start);
    let refresh = thread::spawn(move || {
        // Named again so the closure captures the whole `Send` wrapper rather
        // than the raw pointer field inside it.
        let target = target;
        let left = left;
        refresh_start.wait();
        for (message, key) in delta.iter().zip(&keys) {
            wait_for_queued_move(left.0, key);
            // SAFETY: a live summary of ours, alive until this thread is
            // joined below.
            unsafe { apply_delta(target.0, std::slice::from_ref(message), &[]) };
        }
    });

    start.wait();
    move_straight(inbox, archive, &uids);
    refresh.join().expect("the refreshing thread panicked");

    // SAFETY: a live summary of ours.
    let (rows, saved, visible) = unsafe {
        (
            camel_folder_summary_count(target.0),
            camel_folder_summary_get_saved_count(target.0),
            camel_folder_summary_get_visible_count(target.0),
        )
    };
    // SAFETY: the one reference each `open` handed back.
    unsafe {
        g_object_unref(inbox.cast());
        g_object_unref(archive.cast());
    }
    assert_eq!(
        rows, BATCH as u32,
        "the destination must hold every message"
    );
    assert_eq!(
        saved, rows,
        "the destination must count each of its rows once"
    );
    assert_eq!(
        visible, rows,
        "the destination must show each of its rows once"
    );
}
