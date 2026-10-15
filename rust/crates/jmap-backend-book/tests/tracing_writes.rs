// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Structured tracing tests for `jmap-backend-book`.
//!
//! Asserts structured fields attached to `tracing` events during address book
//! operations.

use std::ffi::{CString, c_void};
use std::ptr;

use eds_sys::{
    E_SOURCE_AUTHENTICATION_ACCEPTED, E_SOURCE_AUTHENTICATION_REJECTED,
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_SECURITY, ESource,
    e_contact_new_from_vcard, e_source_authentication_set_host, e_source_authentication_set_port,
    e_source_get_extension, e_source_new_with_uid, e_source_security_set_secure,
};
use glib_sys::{GError, GFALSE, GSList, GTRUE, g_free, g_slist_free_full, gchar};
use gobject_sys::g_object_unref;
use jmap_backend_book::connect::{self, open_book};
use jmap_backend_book::ops::{self, Outcome};
use jmap_backend_core::source::{ConnectTarget, SourceConfig};
use jmap_client::Credentials;
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_tracing_capture::capture;
use tracing::Level;

const CONTACT_VCARD: &str = "BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
FN:Alice Example\r\n\
N:Example;Alice;;;\r\n\
EMAIL;TYPE=WORK:alice@example.com\r\n\
END:VCARD\r\n";

unsafe extern "C" fn unref_object(ptr: *mut c_void) {
    unsafe { g_object_unref(ptr.cast()) };
}

fn has(captured: &[(Level, String, String)], level: Level, name: &str, value: &str) -> bool {
    captured
        .iter()
        .any(|(l, n, v)| *l == level && n == name && v == value)
}

struct Fixture {
    server: MockServer,
    book_id: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let book_id = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            account.seed_address_book("Personal", true)
        };
        Self { server, book_id }
    }

    fn config(&self) -> SourceConfig {
        SourceConfig {
            target: ConnectTarget::Origin(self.server.origin().to_owned()),
            user: None,
            resource_id: Some(self.book_id.to_string()),
            rebase_urls: false,
        }
    }

    fn sync(&self) -> jmap_book_sync::BookSync {
        capture(|| open_book(&self.config(), Credentials::none()).expect("connected")).0
    }
}

struct TestSource(*mut ESource);

impl TestSource {
    fn new(origin: &str) -> Self {
        let uid = CString::new("jmap-tracing-source").unwrap();
        let mut error = ptr::null_mut();
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null());

        let (host, port) = origin
            .trim_start_matches("http://")
            .split_once(':')
            .expect("the mock origin has a port");
        let host = CString::new(host).unwrap();
        unsafe {
            let auth = e_source_get_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr());
            e_source_authentication_set_host(auth.cast(), host.as_ptr());
            e_source_authentication_set_port(auth.cast(), port.parse().unwrap());
            let sec = e_source_get_extension(source, E_SOURCE_EXTENSION_SECURITY.as_ptr());
            e_source_security_set_secure(sec.cast(), 0);
        }
        Self(source)
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        unsafe { g_object_unref(self.0.cast()) };
    }
}

#[test]
fn open_book_traces_account_id_and_address_book_id() {
    let fixture = Fixture::start();
    let config = fixture.config();

    let (_, captured) = capture(|| {
        let _ = open_book(&config, Credentials::none()).expect("opened");
    });

    assert!(
        has(
            &captured,
            Level::DEBUG,
            "account_id",
            fixture.server.account_id().as_ref()
        ),
        "expected account_id in open_book, got {captured:?}"
    );
    assert!(
        has(
            &captured,
            Level::DEBUG,
            "address_book_id",
            fixture.book_id.as_ref()
        ),
        "expected address_book_id in open_book, got {captured:?}"
    );
}

#[test]
fn list_existing_traces_structured_fields() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let mut sync_tag: *mut gchar = ptr::null_mut();
    let mut objects: *mut GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    let (_, captured) = capture(|| {
        let result = unsafe { ops::list_existing(&sync, &mut sync_tag, &mut objects, &mut error) };
        assert_eq!(result, GTRUE);
    });

    unsafe {
        if !sync_tag.is_null() {
            g_free(sync_tag.cast());
        }
        if !objects.is_null() {
            g_slist_free_full(objects, Some(unref_object));
        }
    }

    assert!(
        has(
            &captured,
            Level::DEBUG,
            "account_id",
            fixture.server.account_id().as_ref()
        ),
        "expected account_id in list_existing, got {captured:?}"
    );
    assert!(
        has(
            &captured,
            Level::DEBUG,
            "address_book_id",
            fixture.book_id.as_ref()
        ),
        "expected address_book_id in list_existing, got {captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|(lvl, name, _)| *lvl == Level::DEBUG && name == "state"),
        "expected state field in list_existing, got {captured:?}"
    );
}

#[test]
fn get_changes_traces_structured_fields() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    // First, list existing to get the state tag under capture guard
    let mut sync_tag: *mut gchar = ptr::null_mut();
    let mut objects: *mut GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();
    capture(|| unsafe {
        ops::list_existing(&sync, &mut sync_tag, &mut objects, &mut error);
        if !objects.is_null() {
            g_slist_free_full(objects, Some(unref_object));
        }
    });
    assert!(!sync_tag.is_null());

    let mut new_sync_tag: *mut gchar = ptr::null_mut();
    let mut repeat = GFALSE;
    let mut created: *mut GSList = ptr::null_mut();
    let mut modified: *mut GSList = ptr::null_mut();
    let mut removed: *mut GSList = ptr::null_mut();

    let (_, captured) = capture(|| {
        let outcome = unsafe {
            ops::get_changes(
                &sync,
                sync_tag,
                &mut new_sync_tag,
                &mut repeat,
                &mut created,
                &mut modified,
                &mut removed,
                &mut error,
            )
        };
        assert!(matches!(outcome, Outcome::Reported));
    });

    unsafe {
        g_free(sync_tag.cast());
        if !new_sync_tag.is_null() {
            g_free(new_sync_tag.cast());
        }
        if !modified.is_null() {
            g_slist_free_full(modified, Some(unref_object));
        }
        if !removed.is_null() {
            g_slist_free_full(removed, Some(glib_sys::g_free));
        }
    }

    assert!(
        has(
            &captured,
            Level::DEBUG,
            "account_id",
            fixture.server.account_id().as_ref()
        ),
        "expected account_id in get_changes, got {captured:?}"
    );
    assert!(
        has(
            &captured,
            Level::DEBUG,
            "address_book_id",
            fixture.book_id.as_ref()
        ),
        "expected address_book_id in get_changes, got {captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|(lvl, name, _)| *lvl == Level::DEBUG && name == "last_sync_tag"),
        "expected last_sync_tag in get_changes, got {captured:?}"
    );
}

#[test]
fn save_load_and_remove_contact_trace_structured_fields() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let vcard_c = CString::new(CONTACT_VCARD).unwrap();
    let contact = unsafe { e_contact_new_from_vcard(vcard_c.as_ptr()) };
    assert!(!contact.is_null());

    let mut new_uid: *mut gchar = ptr::null_mut();
    let mut extra: *mut gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    let (_, captured_save) = capture(|| {
        let result = unsafe {
            ops::save_contact(&sync, GFALSE, contact, &mut new_uid, &mut extra, &mut error)
        };
        assert_eq!(result, GTRUE);
    });

    assert!(
        has(
            &captured_save,
            Level::DEBUG,
            "account_id",
            fixture.server.account_id().as_ref()
        ),
        "expected account_id in save_contact, got {captured_save:?}"
    );
    assert!(
        has(
            &captured_save,
            Level::DEBUG,
            "address_book_id",
            fixture.book_id.as_ref()
        ),
        "expected address_book_id in save_contact, got {captured_save:?}"
    );
    assert!(
        has(&captured_save, Level::DEBUG, "overwrite_existing", "false"),
        "expected overwrite_existing in save_contact, got {captured_save:?}"
    );

    assert!(!new_uid.is_null());
    let uid_str = unsafe { std::ffi::CStr::from_ptr(new_uid).to_str().unwrap() };

    let mut loaded_contact = ptr::null_mut();
    let (_, captured_load) = capture(|| {
        let result = unsafe {
            ops::load_contact(&sync, new_uid, &mut loaded_contact, &mut extra, &mut error)
        };
        assert_eq!(result, GTRUE);
    });

    assert!(
        has(
            &captured_load,
            Level::DEBUG,
            "account_id",
            fixture.server.account_id().as_ref()
        ),
        "expected account_id in load_contact, got {captured_load:?}"
    );
    assert!(
        has(&captured_load, Level::DEBUG, "uid", uid_str),
        "expected uid in load_contact, got {captured_load:?}"
    );

    let (_, captured_remove) = capture(|| {
        let result = unsafe { ops::remove_contact(&sync, new_uid, &mut error) };
        assert_eq!(result, GTRUE);
    });

    assert!(
        has(
            &captured_remove,
            Level::DEBUG,
            "account_id",
            fixture.server.account_id().as_ref()
        ),
        "expected account_id in remove_contact, got {captured_remove:?}"
    );
    assert!(
        has(&captured_remove, Level::DEBUG, "uid", uid_str),
        "expected uid in remove_contact, got {captured_remove:?}"
    );

    unsafe {
        g_free(new_uid.cast());
        if !loaded_contact.is_null() {
            g_object_unref(loaded_contact.cast());
        }
        g_object_unref(contact.cast());
    }
}

#[test]
fn connect_from_source_traces_structured_fields() {
    let fixture = Fixture::start();
    let source = TestSource::new(fixture.server.origin());
    let mut auth_result = E_SOURCE_AUTHENTICATION_REJECTED;
    let mut error = ptr::null_mut();

    let (_, captured) = capture(|| {
        let sync = unsafe {
            connect::connect(
                source.0,
                ptr::null(),
                ptr::null_mut(),
                &mut auth_result,
                &mut error,
            )
        };
        assert!(sync.is_some());
    });

    assert_eq!(auth_result, E_SOURCE_AUTHENTICATION_ACCEPTED);
    assert!(
        has(
            &captured,
            Level::DEBUG,
            "account_id",
            fixture.server.account_id().as_ref()
        ),
        "expected account_id in connect, got {captured:?}"
    );
    assert!(
        has(
            &captured,
            Level::DEBUG,
            "address_book_id",
            fixture.book_id.as_ref()
        ),
        "expected address_book_id in connect, got {captured:?}"
    );
}
