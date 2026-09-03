// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `get_quota_info_sync`, through Camel's own wrapper — what Evolution's
//! folder-properties dialog would see.
//!
//! Unlike `refresh_info_sync`, the wrapper this file drives
//! (`camel_folder_get_quota_info_sync`) does not reconnect the store first —
//! `camel-folder.c`'s own source has no such call, only the class dispatch —
//! so the disconnected case below needs no bypass through the class pointer
//! the way `refresh.rs`'s does.

mod common;

use std::ffi::{CStr, CString};
use std::ptr;

use common::Account;
use eds_sys::{
    CAMEL_SERVICE_ERROR_NOT_CONNECTED, CAMEL_STORE_FOLDER_NONE, CamelFolder,
    camel_folder_get_quota_info_sync, camel_service_error_quark, camel_store_get_folder_sync,
};
use gio_sys::{G_IO_ERROR_NOT_SUPPORTED, g_io_error_quark};
use glib_sys::GError;
use gobject_sys::g_object_unref;
use jmap_client::{Client, Credentials};
use jmap_mail_sync::MailSync;
use jmap_mock::{MockServer, MockServerBuilder};
use jmap_proto::Id;
use jmap_proto::mail::role;
use jmap_proto::quota::{Quota, quota_data_type, quota_resource_type, quota_scope};
use jmap_proto::session::CAPABILITY_QUOTA;

fn sync_against(server: &MockServer) -> MailSync {
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    MailSync::new(client, server.account_id())
}

/// A connected account with one mailbox, opened the way Camel opens it —
/// every fresh mock account already carries the one Mail/octets/account
/// quota `jmap-mock`'s `AccountState::new` seeds, so no quota seeding of its
/// own is needed for the success case.
fn with_inbox() -> (MockServer, Account, *mut CamelFolder) {
    with_inbox_on(MockServer::builder())
}

/// [`with_inbox`], parameterised on the builder, so a test can shape the
/// server (e.g. leave a capability out) before the folder opens.
fn with_inbox_on(builder: MockServerBuilder) -> (MockServer, Account, *mut CamelFolder) {
    let server = builder.start();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&server.account_id())
            .unwrap()
            .seed_mailbox("Inbox", Some(role::INBOX));
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

#[test]
fn a_folders_quota_is_the_accounts_mail_quota() {
    let (_server, _account, folder) = with_inbox();

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder, and an out-parameter that is writable and
    // currently NULL.
    let info = unsafe { camel_folder_get_quota_info_sync(folder, ptr::null_mut(), &mut error) };

    assert!(
        !info.is_null(),
        "the account's own default quota went missing"
    );
    assert!(error.is_null(), "a successful answer set an error too");
    // SAFETY: a live `CamelFolderQuotaInfo`, its `name` a NUL-terminated
    // string Camel owns.
    unsafe {
        assert_eq!(
            CStr::from_ptr((*info).name).to_string_lossy(),
            "Mail",
            "jmap-mock seeds the default quota's name as \"Mail\""
        );
        assert_eq!((*info).used, 0);
        assert_eq!((*info).total, 1_073_741_824);
        assert!(
            (*info).next.is_null(),
            "only one quota was seeded, so the chain should have one node"
        );
        eds_sys::camel_folder_quota_info_free(info);
        g_object_unref(folder.cast());
    }
}

#[test]
fn a_folder_whose_store_has_no_connection_reports_it() {
    let (_server, account, folder) = with_inbox();
    assert!(account.jmap().drop_connection());

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder, and an out-parameter that is writable and
    // currently NULL. The wrapper does not reconnect (see this file's own
    // header), so this reaches the vfunc with the connection already gone.
    let info = unsafe { camel_folder_get_quota_info_sync(folder, ptr::null_mut(), &mut error) };

    assert!(info.is_null(), "a disconnected folder answered anyway");
    assert!(!error.is_null(), "it failed without saying why");
    // SAFETY: a live GError, and the quark accessor takes no arguments.
    unsafe {
        assert_eq!((*error).domain, camel_service_error_quark());
        assert_eq!((*error).code, CAMEL_SERVICE_ERROR_NOT_CONNECTED as i32);
        glib_sys::g_error_free(error);
        g_object_unref(folder.cast());
    }
}

#[test]
fn a_quota_that_says_nothing_about_mail_is_reported_as_unsupported() {
    let (server, _account, folder) = with_inbox();
    {
        // Rewritten in place rather than removed: `Store<T>` has no removal
        // method (nothing in the mock ever needed one), and overwriting the
        // account's one seeded quota with a Contacts-only one is the same
        // test either way — the point is that nothing left applies to Mail.
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&server.account_id()).unwrap();
        *account.quotas.get_mut(&Id::from("Q1")).unwrap() = Quota::new(
            "Q1",
            "Contacts",
            quota_resource_type::OCTETS,
            0,
            1_073_741_824,
            quota_scope::ACCOUNT,
            [quota_data_type::CONTACTS],
        );
    }

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder, and an out-parameter that is writable and
    // currently NULL.
    let info = unsafe { camel_folder_get_quota_info_sync(folder, ptr::null_mut(), &mut error) };

    assert!(
        info.is_null(),
        "a quota that only covers Contacts is not a Mail quota"
    );
    assert!(!error.is_null(), "it failed without saying why");
    // SAFETY: a live GError, and the quark accessor takes no arguments.
    unsafe {
        assert_eq!((*error).domain, g_io_error_quark());
        assert_eq!((*error).code, G_IO_ERROR_NOT_SUPPORTED);
        glib_sys::g_error_free(error);
        g_object_unref(folder.cast());
    }
}

/// A server that never advertises `urn:ietf:params:jmap:quota` (Fastmail, in
/// practice) is reported exactly like one that advertises it with nothing
/// scoped to Mail: `Client::quotas` answers an empty list without a request,
/// which chains to the same `NoQuota`/`G_IO_ERROR_NOT_SUPPORTED` this vfunc
/// already returns for [`a_quota_that_says_nothing_about_mail_is_reported_as_unsupported`].
#[test]
fn an_account_with_no_quota_capability_is_reported_as_unsupported() {
    let (_server, _account, folder) =
        with_inbox_on(MockServer::builder().without_capability(CAPABILITY_QUOTA));

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder, and an out-parameter that is writable and
    // currently NULL.
    let info = unsafe { camel_folder_get_quota_info_sync(folder, ptr::null_mut(), &mut error) };

    assert!(
        info.is_null(),
        "an account with no quota capability answered anyway"
    );
    assert!(!error.is_null(), "it failed without saying why");
    // SAFETY: a live GError, and the quark accessor takes no arguments.
    unsafe {
        assert_eq!((*error).domain, g_io_error_quark());
        assert_eq!((*error).code, G_IO_ERROR_NOT_SUPPORTED);
        glib_sys::g_error_free(error);
        g_object_unref(folder.cast());
    }
}
