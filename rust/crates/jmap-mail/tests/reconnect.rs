// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A store that Evolution disconnected opens its folders again.
//!
//! The account editor of Evolution disconnects the store after each save
//! (`mail_shell_backend_changes_committed_cb`, 3.52). The session stays
//! online, and Evolution does not connect the store again. Camel connects
//! again before a folder listing and before each folder operation, but not
//! before `camel_store_get_folder_sync`. Thus the store must connect itself
//! when it opens a folder.

mod common;

use std::ffi::{CStr, CString};
use std::ptr;

use common::Account;
use eds_sys::{
    CAMEL_NETWORK_SECURITY_METHOD_NONE, CamelFolder, CamelNetworkSettings, CamelService,
    camel_network_settings_set_host, camel_network_settings_set_port,
    camel_network_settings_set_security_method, camel_service_connect_sync,
    camel_service_disconnect_sync, camel_service_ref_session, camel_service_ref_settings,
    camel_session_set_online, camel_store_get_folder_sync, camel_store_get_inbox_folder_sync,
};
use glib_sys::{GError, GTRUE, g_error_free};
use gobject_sys::g_object_unref;
use jmap_mock::MockServer;
use jmap_proto::mail::role;

/// A mock with an inbox, and an online account that has connected to it
/// through Camel and then disconnected, as after a save in the account editor.
fn disconnected_account() -> (MockServer, Account) {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    server
        .state()
        .lock()
        .unwrap()
        .account_mut(&account_id)
        .unwrap()
        .seed_mailbox("Inbox", Some(role::INBOX));
    let port = server.origin().rsplit(':').next().unwrap().parse().unwrap();

    let account = Account::open();
    let service = account.store.cast::<CamelService>();
    // SAFETY: a live store. Its settings are a `CamelNetworkSettings`; the
    // setters copy the host. Each reference that this block takes, it
    // releases. The default session authenticates with no password, and the
    // mock asks for none.
    unsafe {
        let settings = camel_service_ref_settings(service);
        let network = settings.cast::<CamelNetworkSettings>();
        let host = CString::new("127.0.0.1").unwrap();
        camel_network_settings_set_host(network, host.as_ptr());
        camel_network_settings_set_port(network, port);
        camel_network_settings_set_security_method(network, CAMEL_NETWORK_SECURITY_METHOD_NONE);
        g_object_unref(settings.cast());

        let session = camel_service_ref_session(service);
        camel_session_set_online(session, GTRUE);
        g_object_unref(session.cast());

        let mut error: *mut GError = ptr::null_mut();
        let connected = camel_service_connect_sync(service, ptr::null_mut(), &mut error);
        assert_eq!(connected, GTRUE, "connect: {}", message(error));
        let disconnected =
            camel_service_disconnect_sync(service, GTRUE, ptr::null_mut(), &mut error);
        assert_eq!(disconnected, GTRUE, "disconnect: {}", message(error));
    }
    assert!(!account.jmap().is_connected());
    (server, account)
}

/// The message of `error`, or an empty string. Frees the error.
fn message(error: *mut GError) -> String {
    if error.is_null() {
        return String::new();
    }
    // SAFETY: an owned, live GError with a NUL-terminated message.
    unsafe {
        let text = CStr::from_ptr((*error).message)
            .to_string_lossy()
            .into_owned();
        g_error_free(error);
        text
    }
}

/// Fails with the error unless `folder` is a folder. Releases the folder.
fn expect_folder(folder: *mut CamelFolder, error: *mut GError) {
    assert!(!folder.is_null(), "no folder: {}", message(error));
    // SAFETY: a transfer-full reference from Camel.
    unsafe { g_object_unref(folder.cast()) };
}

#[test]
fn a_folder_opens_after_a_disconnect() {
    let (_server, account) = disconnected_account();

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live store and a NUL-terminated path.
    let folder = unsafe {
        camel_store_get_folder_sync(
            account.store,
            c"Inbox".as_ptr(),
            0,
            ptr::null_mut(),
            &mut error,
        )
    };

    expect_folder(folder, error);
    assert!(account.jmap().is_connected());
}

#[test]
fn the_inbox_opens_by_purpose_after_a_disconnect() {
    let (_server, account) = disconnected_account();

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live store.
    let folder =
        unsafe { camel_store_get_inbox_folder_sync(account.store, ptr::null_mut(), &mut error) };

    expect_folder(folder, error);
    assert!(account.jmap().is_connected());
}

#[test]
fn a_store_that_cannot_connect_opens_no_folder_and_says_why() {
    // Online, but the account names no server.
    let account = Account::open();

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live store and a NUL-terminated path.
    let folder = unsafe {
        camel_store_get_folder_sync(
            account.store,
            c"Inbox".as_ptr(),
            0,
            ptr::null_mut(),
            &mut error,
        )
    };

    assert!(folder.is_null(), "a store with no server opened a folder");
    assert!(!message(error).is_empty(), "no reason given");
    assert!(!account.jmap().is_connected());
}
