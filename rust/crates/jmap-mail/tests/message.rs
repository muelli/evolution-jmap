// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `get_message_sync`: the vfunc that turns a row into mail the user can read.
//!
//! `refresh_info_sync` filled the folder with rows — a subject, a sender, a
//! date, a size. None of that is the message; it is what a message list draws,
//! and clicking a line of it asks this vfunc for the rest. The previous
//! increment built the fetch half in `jmap-mail-sync` (`MailSync::message_source`,
//! an `Email/get` for the blob id and a download of the blob) and stopped at the
//! bytes. This is the other half: a `CamelMimeMessage` parsed out of them.
//!
//! The assertions are deliberately not "the bytes came back". A test that
//! compared the download against the mock's own rendering would be testing the
//! mock; what has to be true is that *Camel* can read the result, so the message
//! is asked what it is through Camel's own accessors — the subject, the sender,
//! and the body, which is the one that needs the whole message rather than its
//! first few hundred bytes.
//!
//! Two failures matter as much as the success, and they are two different
//! domains. A uid the account no longer holds is
//! `CAMEL_FOLDER_ERROR_INVALID_UID` — the message went away, the account is
//! fine — and a store with no connection is `CAMEL_SERVICE_ERROR_NOT_CONNECTED`,
//! which is what makes Camel reconnect and ask again. Reported the other way
//! round, a single deleted message would take the account offline.

mod common;

use std::ffi::{CStr, CString};
use std::ptr;

use common::Account;
use eds_sys::{
    CAMEL_FOLDER_ERROR_INVALID_UID, CAMEL_SERVICE_ERROR_NOT_CONNECTED, CAMEL_STORE_FOLDER_NONE,
    CamelAddress, CamelDataWrapper, CamelFolder, CamelFolderClass, CamelMedium, CamelMimeMessage,
    camel_address_format, camel_data_wrapper_decode_to_output_stream_sync,
    camel_folder_error_quark, camel_folder_free_uids, camel_folder_get_message_sync,
    camel_folder_get_uids, camel_folder_refresh_info_sync, camel_medium_get_content,
    camel_mime_message_get_from, camel_mime_message_get_subject, camel_service_error_quark,
    camel_store_get_folder_sync,
};
use gio_sys::{
    GMemoryOutputStream, GOutputStream, g_memory_output_stream_get_data,
    g_memory_output_stream_get_data_size, g_memory_output_stream_new_resizable,
};
use glib_sys::{GError, GPtrArray, g_free};
use gobject_sys::{g_object_unref, g_type_class_ref, g_type_class_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::folder::folder_type;
use jmap_mail_sync::MailSync;
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::mail::role;

/// The one message every test here opens. Named once so the assertions and the
/// seed cannot drift apart.
const SUBJECT: &str = "The only message";
const BODY: &str = "Two lines, so the body is more than a header value.\nAnd the second one.";
const SENDER: &str = "bob@example.com";

fn sync_against(server: &MockServer) -> MailSync {
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    MailSync::new(client, server.account_id())
}

/// A connected account whose inbox holds exactly one message, and that inbox
/// opened the way Camel opens it.
fn with_one_message() -> (MockServer, Account, *mut CamelFolder) {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_email(EmailSeed::new(
            inbox,
            ("Bob", SENDER),
            SUBJECT,
            BODY,
            "2026-01-01T09:00:00Z",
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

/// The uid Evolution would pass to `get_message_sync`: one it read out of the
/// folder after a refresh, rather than one the test invented.
fn the_one_uid(folder: *mut CamelFolder) -> CString {
    // SAFETY: a live folder; the refresh is Camel's own wrapper, and the uid
    // array comes back owned and is freed with the function Camel documents.
    unsafe {
        let mut error: *mut GError = ptr::null_mut();
        assert_ne!(
            camel_folder_refresh_info_sync(folder, ptr::null_mut(), &mut error),
            glib_sys::GFALSE,
            "the folder would not refresh"
        );
        let array: *mut GPtrArray = camel_folder_get_uids(folder);
        assert_eq!(
            (*array).len,
            1,
            "the inbox did not hold exactly one message"
        );
        let uid = CStr::from_ptr((*array).pdata.read().cast()).to_owned();
        camel_folder_free_uids(folder, array);
        uid
    }
}

/// One call, and the two things it can produce.
struct Opened {
    message: *mut CamelMimeMessage,
    error: *mut GError,
}

impl Opened {
    /// Through Camel's own wrapper, which is what Evolution's preview pane
    /// calls.
    fn of(folder: *mut CamelFolder, uid: &CStr) -> Self {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live folder, a NUL-terminated uid alive across the call,
        // and an out-parameter that is writable and currently NULL.
        let message = unsafe {
            camel_folder_get_message_sync(folder, uid.as_ptr(), ptr::null_mut(), &mut error)
        };
        Self { message, error }
    }

    /// Through the pointer in the class, skipping whatever the wrapper does
    /// first — the same reason tests/refresh.rs has this pair: a store Camel
    /// would have reconnected on the way in is not the state under test.
    fn straight(folder: *mut CamelFolder, uid: &CStr) -> Self {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: referencing the class runs the class_init that installs the
        // vfunc; `folder` is an instance of that class, and `error` is writable
        // and currently NULL.
        let message = unsafe {
            let class = g_type_class_ref(folder_type()).cast::<CamelFolderClass>();
            let vfunc = (*class)
                .get_message_sync
                .expect("the folder cannot open a message");
            let message = vfunc(folder, uid.as_ptr(), ptr::null_mut(), &mut error);
            g_type_class_unref(class.cast());
            message
        };
        Self { message, error }
    }

    fn expect_message(&self) -> *mut CamelMimeMessage {
        assert!(
            !self.message.is_null(),
            "the message would not open: {}",
            self.message_text()
        );
        assert!(self.error.is_null(), "an open that worked set an error");
        self.message
    }

    fn message_text(&self) -> String {
        if self.error.is_null() {
            return "no error".to_owned();
        }
        // SAFETY: a live GError whose message is NUL-terminated.
        unsafe {
            CStr::from_ptr((*self.error).message)
                .to_string_lossy()
                .into_owned()
        }
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        // SAFETY: one reference each, taken by the call above.
        unsafe {
            if !self.message.is_null() {
                g_object_unref(self.message.cast());
            }
            if !self.error.is_null() {
                glib_sys::g_error_free(self.error);
            }
        }
    }
}

/// A NUL-terminated string Camel owns, as a `String`.
///
/// # Safety
///
/// `s` must be NULL or a live NUL-terminated string.
unsafe fn borrowed(s: *const glib_sys::gchar) -> Option<String> {
    if s.is_null() {
        return None;
    }
    // SAFETY: the contract above.
    Some(unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned())
}

/// The message's body, decoded the way a renderer decodes it.
///
/// Through `camel_medium_get_content` and the data wrapper's own decoder rather
/// than by re-reading the download: what is being asked is whether the bytes
/// reached the part of the object Camel renders from, and only Camel's own path
/// through the message answers that.
fn body_of(message: *mut CamelMimeMessage) -> String {
    // SAFETY: a live message of ours; the content is borrowed from it, the
    // stream is owned here and unreffed below, and the error out-parameter is
    // writable and currently NULL.
    unsafe {
        let content: *mut CamelDataWrapper =
            camel_medium_get_content(message.cast::<CamelMedium>());
        assert!(!content.is_null(), "the message has no content");

        let stream: *mut GOutputStream = g_memory_output_stream_new_resizable();
        let mut error: *mut GError = ptr::null_mut();
        let written = camel_data_wrapper_decode_to_output_stream_sync(
            content,
            stream,
            ptr::null_mut(),
            &mut error,
        );
        assert!(written >= 0 && error.is_null(), "the body would not decode");

        let memory = stream.cast::<GMemoryOutputStream>();
        let size = g_memory_output_stream_get_data_size(memory);
        let data = g_memory_output_stream_get_data(memory).cast::<u8>();
        // A resizable memory stream nothing was written to has no buffer at
        // all, and `from_raw_parts` on its NULL is undefined behaviour rather
        // than an empty slice — which turns a test that should fail with "the
        // body decoded to \"\"" into an abort with no assertion in it.
        let body = if data.is_null() || size == 0 {
            String::new()
        } else {
            String::from_utf8_lossy(std::slice::from_raw_parts(data, size)).into_owned()
        };
        g_object_unref(stream.cast());
        body
    }
}

/// The whole point of the increment: a uid out of the message list becomes a
/// message Camel can read, headers and body alike.
#[test]
fn an_opened_uid_becomes_the_message_the_server_holds() {
    let (_server, _account, folder) = with_one_message();
    let uid = the_one_uid(folder);

    let opened = Opened::of(folder, &uid);
    let message = opened.expect_message();

    // SAFETY: a live message; both accessors borrow from it.
    unsafe {
        assert_eq!(
            borrowed(camel_mime_message_get_subject(message)).as_deref(),
            Some(SUBJECT)
        );
        let from = camel_mime_message_get_from(message);
        assert!(!from.is_null(), "the message has no From");
        let formatted = camel_address_format(from.cast::<CamelAddress>());
        let sender = borrowed(formatted).unwrap_or_default();
        g_free(formatted.cast());
        assert!(sender.contains(SENDER), "the message came from {sender:?}");
    }

    let body = body_of(message);
    assert!(
        body.contains("And the second one."),
        "the body decoded to {body:?}"
    );

    drop(opened);
    // SAFETY: the one reference this test took from `get_folder_sync`.
    unsafe { g_object_unref(folder.cast()) };
}

/// A uid is a claim about the last listing, and another client deleting the
/// message since is ordinary. `CAMEL_FOLDER_ERROR_INVALID_UID` is how Evolution
/// hears "that one is gone" instead of "this account is broken".
#[test]
fn a_uid_the_account_does_not_hold_is_an_invalid_uid() {
    let (_server, _account, folder) = with_one_message();
    let uid = CString::new("no-such-message").expect("a uid with no NUL");

    let opened = Opened::of(folder, &uid);

    assert!(opened.message.is_null(), "a made-up uid opened a message");
    assert!(!opened.error.is_null(), "it failed without saying why");
    // SAFETY: a live GError, and the quark accessor takes no arguments.
    unsafe {
        assert_eq!((*opened.error).domain, camel_folder_error_quark());
        assert_eq!(
            (*opened.error).code,
            CAMEL_FOLDER_ERROR_INVALID_UID as i32,
            "reported as {}",
            opened.message_text()
        );
    }

    drop(opened);
    // SAFETY: the one reference this test took.
    unsafe { g_object_unref(folder.cast()) };
}

/// And the other domain. The message is fetched over the store's connection, so
/// a store that has none cannot answer — and `NOT_CONNECTED` is what makes Camel
/// connect and ask again rather than show the account as broken.
#[test]
fn a_folder_whose_store_has_no_connection_reports_it() {
    let (_server, account, folder) = with_one_message();
    let uid = the_one_uid(folder);
    assert!(account.jmap().drop_connection());

    let opened = Opened::straight(folder, &uid);

    assert!(opened.message.is_null(), "a disconnected folder answered");
    assert!(!opened.error.is_null(), "it failed without saying why");
    // SAFETY: a live GError, and the quark accessor takes no arguments.
    unsafe {
        assert_eq!((*opened.error).domain, camel_service_error_quark());
        assert_eq!(
            (*opened.error).code,
            CAMEL_SERVICE_ERROR_NOT_CONNECTED as i32
        );
    }

    drop(opened);
    // SAFETY: the one reference this test took.
    unsafe { g_object_unref(folder.cast()) };
}
