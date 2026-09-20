// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `transfer_messages_to_sync` against a real JMAP server.
//!
//! `jmap-mail-sync/tests/live_server_filing.rs` already proves
//! `MailSync::file_message`, the function a transfer's patch is built from,
//! against real Stalwart. What that file cannot reach is this crate's own
//! vfunc on top of it: the folder lock, the per-uid walk, the immediate
//! removal of a moved row from the source folder's local summary and the
//! `transferred_uids` array Camel reads back. Every test of that layer so far
//! (`tests/transfer_race.rs`) has run against `jmap-mockd`, and only in its
//! race variant — there is no plain-case test even against the mock. This is
//! the live-server counterpart.
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

use common::Account;
use eds_sys::{
    camel_folder_get_folder_summary, camel_folder_refresh_info_sync, camel_folder_summary_get,
    camel_folder_transfer_messages_to_sync,
};
use glib_sys::{GError, GFALSE, GPtrArray, GTRUE, g_error_free, g_ptr_array_free, gpointer};
use gobject_sys::g_object_unref;
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

/// The uid array Camel hands the vfunc, owned for the length of one call.
///
/// Mirrors `tests/transfer_race.rs::UidList`.
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

/// Imports a message into a real source folder, opens real `CamelFolder`s on
/// it and a second, empty destination folder, refreshes the source (a real
/// listing builds its summary row), then drives a real
/// `camel_folder_transfer_messages_to_sync` moving the message across.
/// Confirms the source folder's row is gone immediately, with no further
/// refresh, and that an independent connection sees the message filed under
/// the destination on the real server. Cleans up by expunging from the
/// destination (the server refuses to delete a non-empty mailbox) and
/// deleting both folders.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn transfer_messages_to_sync_moves_a_message_on_the_real_server() {
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
    let source_name = format!("agent-mailtransfer-source-{suffix}");
    let dest_name = format!("agent-mailtransfer-dest-{suffix}");
    let source_info = account
        .jmap()
        .create_folder(None, &source_name)
        .expect("create_folder failed against the real server for the source");
    let dest_info = account
        .jmap()
        .create_folder(None, &dest_name)
        .expect("create_folder failed against the real server for the destination");

    let subject = format!("agent-mailtransfer-{suffix}");
    let source_bytes = format!(
        "From: agent-mailtransfer@example.invalid\r\n\
         To: agent-mailtransfer@example.invalid\r\n\
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

    // SAFETY: `account.store` is a live `CamelStore`, and both `FolderInfo`s
    // are a real discovery over that same store's connection.
    let source_folder = unsafe { new_folder(account.store, &source_info) };
    let dest_folder = unsafe { new_folder(account.store, &dest_info) };
    assert!(
        !source_folder.is_null() && !dest_folder.is_null(),
        "the folders would not construct"
    );

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder, never refreshed before, and a writable,
    // currently-NULL out-parameter.
    unsafe {
        assert_ne!(
            camel_folder_refresh_info_sync(source_folder, ptr::null_mut(), &mut error),
            GFALSE,
            "the source folder would not refresh"
        );
    }

    let uid_c = CString::new(uid.as_str()).expect("a uid with no NUL");
    // SAFETY: the source folder has a summary once refreshed.
    unsafe {
        let summary = camel_folder_get_folder_summary(source_folder);
        assert!(!summary.is_null(), "the source folder has no summary");
        let row = camel_folder_summary_get(summary, uid_c.as_ptr());
        assert!(
            !row.is_null(),
            "the refresh left no row for the imported message"
        );
        g_object_unref(row.cast());
    }

    let list = UidList::of(uid.as_str());
    let mut transferred: *mut GPtrArray = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: two live folders of one store, an array of one NUL-terminated
    // uid alive across the call, and two writable, currently-NULL
    // out-parameters. GTRUE: this is a move, not a copy.
    let ok = unsafe {
        camel_folder_transfer_messages_to_sync(
            source_folder,
            list.array,
            dest_folder,
            GTRUE,
            &mut transferred,
            ptr::null_mut(),
            &mut error,
        )
    };
    assert_ne!(
        ok, GFALSE,
        "transfer_messages_to_sync failed against the real server"
    );
    assert!(
        error.is_null(),
        "a transfer that worked should not have set an error"
    );
    // SAFETY: `transferred`, if non-NULL, is the array Camel allocated for
    // this call's out-parameter, owned by this test from here.
    unsafe {
        if !transferred.is_null() {
            g_ptr_array_free(transferred, GTRUE);
        }
        if !error.is_null() {
            g_error_free(error);
        }
    }

    // The row must be gone from the source folder's local summary right
    // away, with no further refresh: that is what lets the message list
    // already on screen stop showing it the moment the drag completes.
    // SAFETY: as above.
    unsafe {
        let summary = camel_folder_get_folder_summary(source_folder);
        let row = camel_folder_summary_get(summary, uid_c.as_ptr());
        assert!(
            row.is_null(),
            "the moved message's row should be gone from the source folder immediately"
        );
    }

    // Independent confirmation: a fresh connection, sharing nothing with the
    // one the transfer used, sees the message filed under the destination on
    // the real server.
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
        g_object_unref(source_folder.cast());
        g_object_unref(dest_folder.cast());
    }
}
