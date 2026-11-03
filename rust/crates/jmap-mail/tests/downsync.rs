// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `get_uncached_uids`: the vfunc "Download messages for offline use" and the
//! per-folder "Copy folder content locally" need to do incremental work.
//!
//! `JmapFolder` derives from `CamelOfflineFolder` (`src/folder.rs`), whose
//! `downsync_sync` is already a full, correct implementation: find the uids
//! to copy, ask `camel_folder_get_uncached_uids` which of them are not yet on
//! disk, and fetch only those (`camel-offline-folder.c`, upstream). This
//! provider never implemented its half of that contract: with no
//! `get_uncached_uids` vfunc installed, `CamelFolder`'s own base
//! implementation answers, which returns every uid it is asked about,
//! unconditionally, as "not cached" (`camel-folder.c`'s `folder_get_uncached_
//! uids`). A downsync would therefore redownload every message in the
//! folder, including ones already read and cached, each time it runs.
//!
//! This test calls the vfunc directly, the same way `tests/message.rs` reaches
//! `get_message_cached`: through the class pointer, with no store connection
//! in play, so what is under test is the cache check alone.

mod common;

use std::ffi::{CStr, CString};
use std::ptr;

use common::Account;
use common::sync_against;
use eds_sys::compat::{folder_dup_uids, folder_free_uids};
use eds_sys::{
    CAMEL_STORE_FOLDER_NONE, CamelFolder, CamelFolderClass, camel_folder_refresh_info_sync,
    camel_store_get_folder_sync,
};
use glib_sys::{GError, GFALSE, GPtrArray, g_ptr_array_add, g_ptr_array_new, g_ptr_array_unref};
use gobject_sys::{g_type_class_ref, g_type_class_unref};
use jmap_mail::folder::folder_type;
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::mail::role;

/// A connected account whose inbox holds two messages, opened the way Camel
/// opens it.
fn with_two_messages() -> (MockServer, Account, *mut CamelFolder) {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Bob", "bob@example.com"),
            "Cached already",
            "This one gets opened before the downsync check.",
            "2026-01-01T09:00:00Z",
        ));
        account.seed_email(EmailSeed::new(
            inbox,
            ("Carol", "carol@example.com"),
            "Never opened",
            "This one has never been fetched.",
            "2026-01-01T09:05:00Z",
        ));
    }

    let account = Account::open();
    account.connect(sync_against(&server));

    let path = CString::new("Inbox").expect("a path with no NUL");
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live store of ours, a NUL-terminated path alive across the
    // call, and an out-parameter that is writable and currently NULL.
    let folder = unsafe {
        camel_store_get_folder_sync(
            account.store,
            path.as_ptr(),
            CAMEL_STORE_FOLDER_NONE,
            ptr::null_mut(),
            &mut error,
        )
    };
    assert!(
        !folder.is_null() && error.is_null(),
        "the folder would not open"
    );
    (server, account, folder)
}

/// The two uids of [`with_two_messages`]'s inbox, in no particular order.
fn both_uids(folder: *mut CamelFolder) -> Vec<CString> {
    // SAFETY: a live folder. The refresh is Camel's own wrapper, and the uid
    // array comes back owned, and is freed with the function Camel documents.
    unsafe {
        let mut error: *mut GError = ptr::null_mut();
        assert_ne!(
            camel_folder_refresh_info_sync(folder, ptr::null_mut(), &mut error),
            GFALSE,
            "the folder would not refresh"
        );
        let array: *mut GPtrArray = folder_dup_uids(folder);
        assert_eq!(
            (*array).len,
            2,
            "the inbox did not hold exactly two messages"
        );
        let uids = (0..(*array).len)
            .map(|index| {
                let uid: *const glib_sys::gchar = (*array).pdata.add(index as usize).read().cast();
                CStr::from_ptr(uid).to_owned()
            })
            .collect();
        folder_free_uids(folder, array);
        uids
    }
}

/// Opens a message through Camel's own wrapper, the way the preview pane
/// does, so it ends up in the message cache the same way a real read would.
fn open_and_cache(folder: *mut CamelFolder, uid: &CStr) {
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder, a NUL-terminated uid alive across the call, and
    // an out-parameter that is writable and currently NULL.
    let message = unsafe {
        eds_sys::camel_folder_get_message_sync(folder, uid.as_ptr(), ptr::null_mut(), &mut error)
    };
    assert!(
        !message.is_null(),
        "the message would not open to be cached"
    );
    // SAFETY: a live message, the one reference the call above returned.
    unsafe { gobject_sys::g_object_unref(message.cast()) };
}

/// Calls the vfunc directly, the same way `tests/message.rs` reaches
/// `get_message_cached`: through the class pointer, bypassing
/// `camel_folder_get_uncached_uids`'s own locking wrapper, which is not under
/// test here.
fn uncached_uids_of(folder: *mut CamelFolder, uids: &[CString]) -> Vec<String> {
    // SAFETY: a plain array of borrowed pointers into `uids`, which outlives
    // the call. `g_ptr_array_new` makes an array with no free function, so
    // unref below does not touch the strings it was never asked to own.
    unsafe {
        let input = g_ptr_array_new();
        for uid in uids {
            g_ptr_array_add(input, uid.as_ptr().cast_mut().cast());
        }

        let class = g_type_class_ref(folder_type()).cast::<CamelFolderClass>();
        let vfunc = (*class)
            .get_uncached_uids
            .expect("the folder has no get_uncached_uids vfunc");
        let mut error: *mut GError = ptr::null_mut();
        let result = vfunc(folder, input, &mut error);
        g_type_class_unref(class.cast());
        g_ptr_array_unref(input);

        assert!(!result.is_null(), "get_uncached_uids reported an error");
        let uncached = (0..(*result).len)
            .map(|index| {
                let uid: *const glib_sys::gchar = (*result).pdata.add(index as usize).read().cast();
                CStr::from_ptr(uid).to_string_lossy().into_owned()
            })
            .collect();
        folder_free_uids(folder, result);
        uncached
    }
}

/// The whole point of the increment: a message already opened (and so cached)
/// is not reported as needing a download, while one that was never opened
/// is. Without the fix, both come back as uncached, because the base
/// `CamelFolder` implementation this provider inherited answers every uid it
/// is asked about with "not cached", unconditionally.
#[test]
fn an_already_cached_message_is_not_reported_as_uncached() {
    let (_server, _account, folder) = with_two_messages();
    let uids = both_uids(folder);

    open_and_cache(folder, &uids[0]);

    let uncached = uncached_uids_of(folder, &uids);

    let cached_uid = uids[0].to_str().expect("a UTF-8 uid");
    let other_uid = uids[1].to_str().expect("a UTF-8 uid");
    assert!(
        !uncached.iter().any(|uid| uid == cached_uid),
        "an already-cached message was reported as needing a download: {uncached:?}"
    );
    assert!(
        uncached.iter().any(|uid| uid == other_uid),
        "a message that was never opened was not reported as needing a download: {uncached:?}"
    );

    // SAFETY: the one reference this test took from `get_folder_sync`.
    unsafe { gobject_sys::g_object_unref(folder.cast()) };
}

/// Before either message has been opened, both need a download.
#[test]
fn neither_message_is_cached_before_either_is_opened() {
    let (_server, _account, folder) = with_two_messages();
    let uids = both_uids(folder);

    let uncached = uncached_uids_of(folder, &uids);

    assert_eq!(
        uncached.len(),
        2,
        "a message was reported cached before any open: {uncached:?}"
    );

    // SAFETY: the one reference this test took from `get_folder_sync`.
    unsafe { gobject_sys::g_object_unref(folder.cast()) };
}
