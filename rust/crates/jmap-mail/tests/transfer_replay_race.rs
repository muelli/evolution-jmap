// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The reconnect walk must not discard a filing that another thread queued
//! on the row while the walk's own request was in flight.
//!
//! `replay_filing` reads the row's filing, sends it, and clears it when the
//! server answers. The row is not locked across that request. A move out of
//! the folder the message only passes through rechains the two filings and
//! writes the joined one on this same row (`rewrite_origin_row`). The
//! connection can go at any moment, so the user can queue that second move
//! while the first request is still in flight.
//!
//! The test reproduces the sequence with the gated transport of the other
//! race tests: the server answers the replay's `Email/set` for real, and the
//! answer is held back until the second move has rechained onto the row.

use std::ffi::CString;
use std::ptr;
use std::sync::Arc;
use std::thread;

use eds_sys::{
    CAMEL_STORE_FOLDER_INFO_REFRESH, CAMEL_STORE_FOLDER_NONE, CamelFolder, CamelFolderClass,
    CamelStoreClass, camel_folder_info_free, camel_folder_refresh_info_sync,
    camel_folder_synchronize_sync, camel_store_get_folder_sync,
};
use glib_sys::{GError, GFALSE, GPtrArray, g_clear_error, gboolean};
use gobject_sys::{g_object_unref, g_type_class_ref, g_type_class_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::folder::folder_type;
use jmap_mail::store::store_type;
use jmap_mail_sync::MailSync;
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::role;

mod common;
use common::Account;
use common::array::StringArray;
use common::race::{DelayedDeliveryTransport, Gate};

/// A raw folder pointer, sent to the walking thread.
///
/// Sound for the same reason as `transfer_race.rs`'s own copy: the folder is
/// a real `CamelFolder` GObject, referenced for the rest of this test and
/// freed only after that thread has been joined.
#[derive(Clone, Copy)]
struct FolderPtr(*mut CamelFolder);
// SAFETY: the folder outlives every use of this wrapper, and every use goes
// through Camel's own thread-safe vfunc dispatch.
unsafe impl Send for FolderPtr {}

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

/// Asks the store for its folder tree, the way Evolution does when it shows
/// the folder list. The rechain below needs it: it names the folder of a
/// mailbox id, and the store answers that from the held tree.
fn list_folders(account: &Account) {
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: referencing the class runs the class_init that installs the
    // vfunc. A NULL `top` asks for the whole tree, as Camel does, and the
    // out-parameter is writable and currently NULL. The tree comes back
    // owned, and is freed with the function Camel documents for it.
    unsafe {
        let class = g_type_class_ref(store_type()).cast::<CamelStoreClass>();
        let vfunc = (*class).get_folder_info_sync.expect("a listing vfunc");
        let tree = vfunc(
            account.store,
            ptr::null(),
            CAMEL_STORE_FOLDER_INFO_REFRESH,
            ptr::null_mut(),
            &mut error,
        );
        g_type_class_unref(class.cast());
        assert!(!tree.is_null() && error.is_null(), "the listing failed");
        camel_folder_info_free(tree);
    }
}

/// Moves `uid` through the pointer in the class, skipping the reconnect that
/// Camel's own wrapper makes before it calls a provider offline.
fn move_straight(source: *mut CamelFolder, destination: *mut CamelFolder, uid: &Id) {
    let list = StringArray::of(&[uid]);
    let mut transferred: *mut GPtrArray = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: referencing the class runs the class_init that installs the
    // vfunc. Both folders are instances of that class, the uid array is
    // alive across the call, and the two out-parameters are writable and
    // currently NULL.
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

/// Through Camel's own wrapper, the way Evolution's reconnect flush reaches
/// every open folder.
fn synchronize(folder: *mut CamelFolder) -> bool {
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder of ours, and an out-parameter that is writable
    // and currently NULL.
    unsafe {
        let ok = camel_folder_synchronize_sync(folder, GFALSE, ptr::null_mut(), &mut error);
        g_clear_error(&mut error);
        ok != GFALSE
    }
}

/// The names of the mailboxes the server has the message in now.
fn mailboxes_on_server(server: &MockServer, uid: &Id) -> Vec<String> {
    let account_id = server.account_id();
    let state = server.state();
    let state = state.lock().unwrap();
    let account = state.account(&account_id).unwrap();
    let mailboxes = account
        .emails
        .get(uid)
        .expect("the seeded message")
        .mailbox_ids
        .clone()
        .unwrap_or_default();
    mailboxes
        .iter()
        .filter(|(_, member)| **member)
        .map(|(id, _)| {
            account
                .mailboxes
                .get(id)
                .map(|mailbox| mailbox.name.clone())
                .unwrap_or_else(|| id.as_str().to_owned())
        })
        .collect()
}

/// A move queued while the replay of the earlier move is in flight must
/// still land the message in one mailbox.
///
/// The user moves the message from the inbox to the archive while offline,
/// and from the archive to the projects folder while the replay of the first
/// move is in flight. The message belongs in the projects folder alone: the
/// archive is a waypoint of the chain.
///
/// `replay_filing` cleared the row's filing whatever the row held by then,
/// so the rechained move went with it. The chain's own row then had no
/// record that the first move had landed, and the message stayed in the
/// waypoint as well as in the folder the user chose.
#[test]
fn a_move_queued_during_a_replay_lands_the_message_once() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let uid = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox_id = account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_mailbox("Archive", None);
        account.seed_mailbox("Projects", None);
        account.seed_email(EmailSeed::new(
            inbox_id,
            ("Bob", "bob@example.com"),
            "Lunch?",
            "One o'clock.",
            "2026-01-15T09:30:00Z",
        ))
    };

    let account = Account::open();
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    account.connect(MailSync::new(client, account_id.clone()));

    let inbox = open(&account, "Inbox");
    let archive = open(&account, "Archive");
    let projects = open(&account, "Projects");
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
    // The rechain below names the folder of a mailbox id, and the store
    // answers that from the tree it holds.
    list_folders(&account);

    // The first move, with no connection: it stays on the inbox row.
    assert!(account.jmap().drop_connection());
    move_straight(inbox, archive, &uid);

    // Back online, with a transport that holds back the answer to the
    // replay's own request.
    let gate = Arc::new(Gate::default());
    let gated = Client::builder()
        .transport(DelayedDeliveryTransport::new(
            Arc::clone(&gate),
            "Email/set",
        ))
        .connect(server.origin(), Credentials::none())
        .expect("connected");
    account.connect(MailSync::new(gated, account_id.clone()));
    // A reconnect forgets the tree. The rechain below reads it again while
    // this connection still answers.
    list_folders(&account);

    let walked = FolderPtr(inbox);
    let walk = thread::spawn(move || {
        // Named again so the closure captures the whole `Send` wrapper
        // rather than the raw pointer field inside it.
        let walked = walked;
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live folder of ours, and an out-parameter that is
        // writable and currently NULL.
        let ok = unsafe {
            let ok = camel_folder_synchronize_sync(walked.0, GFALSE, ptr::null_mut(), &mut error);
            g_clear_error(&mut error);
            ok
        };
        ok != GFALSE
    });

    gate.wait_for_start();
    // The connection goes while the request is in flight: the user asked to
    // work offline, or the socket died. The second move is queued, and it
    // rechains onto the row the walk is working on.
    assert!(account.jmap().drop_connection());
    move_straight(archive, projects, &uid);
    gate.release();
    assert!(walk.join().expect("the walking thread panicked"));

    // Online again, and both folders of the chain walked. The order of the
    // two walks must not change where the message ends up.
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    account.connect(MailSync::new(client, account_id));
    assert!(synchronize(inbox), "the inbox would not synchronize");
    assert!(
        synchronize(projects),
        "the projects folder would not synchronize"
    );

    let landed = mailboxes_on_server(&server, &uid);
    // SAFETY: the one reference each `open` handed back.
    unsafe {
        g_object_unref(inbox.cast());
        g_object_unref(archive.cast());
        g_object_unref(projects.cast());
    }
    assert_eq!(
        landed,
        vec!["Projects".to_owned()],
        "the message must land in the folder the user chose and nowhere else"
    );
}
