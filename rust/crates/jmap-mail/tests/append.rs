// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `append_message_sync`: the vfunc that puts a message Camel is holding into
//! a JMAP folder.
//!
//! [`crate::transfer`] files a message the *account already has* into another
//! of its mailboxes, which is one `Email/set`. This is the other kind of
//! arrival: a `CamelMimeMessage` that came from somewhere else entirely — the
//! folder another account's message was dragged out of, the composer saving a
//! draft, a `.eml` file the user dropped on the folder — and the only way a
//! JMAP account takes one of those is `Email/import` over an uploaded blob.
//! The sync half of that was built an increment ago as
//! [`MailSync::import_message`]; what these tests are about is the Camel half.
//!
//! ## The message goes up as the bytes Camel would write out
//!
//! The vfunc's argument is an object, and what `Email/import` takes is a blob,
//! so something has to serialise it. That is Camel's own writer —
//! `camel_data_wrapper_write_to_output_stream_sync` on the message's data
//! wrapper face — for [`crate::message`]'s reason turned around: the parse on
//! the way in has to be Camel's, so the emit on the way out has to be too, or
//! this provider would be a second MIME implementation disagreeing with the
//! first about what the message says.
//!
//! [`the_message_that_went_up_is_the_message_that_comes_back_down`] is that
//! round trip end to end — appended as an object, listed as a row, opened again
//! as an object — because every part of it is a place a byte could be lost.
//!
//! ## The row comes from the listing, not from the append
//!
//! Nothing is added to the folder's summary.
//! [`an_appended_message_does_not_appear_in_the_folder_until_it_is_listed`]
//! pins that, and it is the decision [`crate::transfer`] already made about the
//! folder a message is dragged *into*, made again here for the same reason: what
//! this side holds is a uid, and a row built from a uid alone would be a message
//! list line with no subject, sender or date until a refresh replaced it.
//!
//! ## What Camel knows about the message that the message does not say
//!
//! The `CamelMessageInfo` argument is the folder's answer to "and this is what
//! it was". Two things on it reach the server — the flags and labels, as
//! keywords, and `date_received`, as `receivedAt` — and both matter for the
//! ordinary case of moving mail between two accounts: a transfer that arrived
//! unread when it was read, or dated to the moment of the copy, is a transfer
//! that lost something the user could see.
//!
//! It is also allowed to be absent. `camel_folder_append_message_sync` declares
//! the argument nullable and Camel's own callers pass NULL for a message
//! nothing is known about, so [`a_message_appended_with_no_row_carries_no_keywords`]
//! is a case rather than a defence.
//!
//! [`crate::transfer`]: jmap_mail::transfer
//! [`crate::message`]: jmap_mail::message
//! [`MailSync::import_message`]: jmap_mail_sync::MailSync::import_message

mod common;

use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::ptr;

use common::Account;
use eds_sys::{
    CAMEL_MESSAGE_FLAGGED, CAMEL_MESSAGE_SEEN, CAMEL_SERVICE_ERROR_NOT_CONNECTED,
    CAMEL_STORE_FOLDER_NONE, CamelDataWrapper, CamelFolder, CamelFolderClass, CamelMessageInfo,
    CamelMimeMessage, camel_data_wrapper_construct_from_data_sync,
    camel_folder_append_message_sync, camel_folder_free_uids, camel_folder_get_message_sync,
    camel_folder_get_uids, camel_folder_refresh_info_sync, camel_message_info_new,
    camel_message_info_set_date_received, camel_message_info_set_flags,
    camel_message_info_set_user_flag, camel_mime_message_get_subject, camel_service_error_quark,
    camel_store_get_folder_sync,
};
use glib_sys::{GError, GFALSE, GPtrArray, g_free, gboolean, gssize};
use gobject_sys::{g_object_unref, g_type_class_ref, g_type_class_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::folder::folder_type;
use jmap_mail_sync::{MailSync, MessageSummary};
use jmap_mock::MockServer;
use jmap_proto::Id;

/// The RFC 5322 bytes of the message being appended — CRLF line endings, a
/// header block, a blank line, a body.
const MESSAGE: &[u8] = b"From: Bob <bob@example.com>\r\n\
To: Alice <alice@example.com>\r\n\
Subject: Lunch?\r\n\
Message-ID: <lunch@example.com>\r\n\
Date: Thu, 15 Jan 2026 09:30:00 +0000\r\n\
\r\n\
One o'clock at the usual place.\r\n";

/// One connected account with one empty folder open, which is the state every
/// append starts from — a folder a message arrives in from outside is one
/// nothing has necessarily listed.
struct Fixture {
    server: MockServer,
    account: Account,
    archive: *mut CamelFolder,
    archive_id: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let archive_id = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            account.seed_mailbox("Archive", None)
        };

        let account = Account::open();
        let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
        account.connect(MailSync::new(client, account_id));

        let archive = open(&account, "Archive");

        Self {
            server,
            account,
            archive,
            archive_id,
        }
    }

    /// Through Camel's own wrapper, which is what Evolution calls when a
    /// message arrives in the folder from outside the account.
    fn append(&self, message: &Message, info: *mut CamelMessageInfo) -> Appended {
        let mut uid: *mut glib_sys::gchar = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live folder of ours, a live message, an info that is NULL
        // or a live row, and two out-parameters that are writable and currently
        // NULL.
        let ok = unsafe {
            camel_folder_append_message_sync(
                self.archive,
                message.0,
                info,
                &mut uid,
                ptr::null_mut(),
                &mut error,
            )
        };
        Appended::new(ok, uid, error)
    }

    /// Through the pointer in the class, skipping what Camel's wrapper does on
    /// the way in.
    ///
    /// That is not a shortcut here, it is the only way to ask the question:
    /// `camel_folder_append_message_sync` connects the service before it
    /// dispatches, so a store whose connection was dropped is one the wrapper
    /// reconnects — and against these settings, which name no server, the
    /// failure that comes back is the *reconnection's* (`URL_INVALID`) rather
    /// than the vfunc's. [`crate::transfer`]'s tests reach past the wrapper for
    /// the same kind of reason.
    fn append_straight(&self, message: &Message, info: *mut CamelMessageInfo) -> Appended {
        let mut uid: *mut glib_sys::gchar = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: referencing the class runs the class_init that installs the
        // vfunc; the folder is an instance of that class, and the two
        // out-parameters are writable and currently NULL.
        let ok = unsafe {
            let class = g_type_class_ref(folder_type()).cast::<CamelFolderClass>();
            let vfunc = (*class)
                .append_message_sync
                .expect("the folder cannot append messages");
            let ok = vfunc(
                self.archive,
                message.0,
                info,
                &mut uid,
                ptr::null_mut(),
                &mut error,
            );
            g_type_class_unref(class.cast());
            ok
        };
        Appended::new(ok, uid, error)
    }

    /// What the mailbox holds now, as a folder refresh would find it — asked
    /// of the server rather than of the summary, because the summary is
    /// deliberately not written by an append.
    fn listing(&self) -> Vec<MessageSummary> {
        let client =
            Client::connect(self.server.origin(), Credentials::none()).expect("connected again");
        let sync = MailSync::new(client, self.server.account_id());
        let (_, messages) = sync.messages(&self.archive_id).expect("the mailbox lists");
        messages
    }

    /// The one row the mailbox holds, or a failure naming how many it holds
    /// instead.
    fn only(&self) -> MessageSummary {
        let messages = self.listing();
        assert_eq!(messages.len(), 1, "one appended message");
        messages.into_iter().next().expect("the row just counted")
    }

    /// And what the server holds for its keywords.
    fn keywords_on_server(&self, uid: &Id) -> BTreeMap<String, bool> {
        let account_id = self.server.account_id();
        let state = self.server.state();
        let state = state.lock().unwrap();
        let account = state.account(&account_id).unwrap();
        account
            .emails
            .get(uid)
            .expect("the appended message")
            .keywords
            .clone()
            .unwrap_or_default()
    }

    /// Removes the mailbox the way another client would.
    fn removed_elsewhere(&self) {
        let account_id = self.server.account_id();
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let mailbox = self.archive_id.clone();
        account.mailboxes.transaction(|mailboxes| {
            assert!(
                mailboxes.destroy(&mailbox),
                "the seeded mailbox was not there"
            );
        });
    }

    /// Lists the folder the way Evolution's message list does.
    fn refresh(&self) {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live folder of ours, and an out-parameter that is writable
        // and currently NULL.
        unsafe {
            assert_ne!(
                camel_folder_refresh_info_sync(self.archive, ptr::null_mut(), &mut error),
                GFALSE,
                "the folder would not refresh"
            );
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // SAFETY: the reference this fixture took from `get_folder_sync`,
        // released before the store it hangs off.
        unsafe { g_object_unref(self.archive.cast()) };
    }
}

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

/// The uids Camel would draw a message list from, asked of the folder the way
/// Evolution asks.
fn listed(folder: *mut CamelFolder) -> Vec<String> {
    // SAFETY: a live folder; the array comes back owned and is freed with the
    // function Camel documents for it.
    unsafe {
        let array: *mut GPtrArray = camel_folder_get_uids(folder);
        let uids = (0..(*array).len)
            .map(|index| {
                let uid = *(*array).pdata.add(index as usize);
                CStr::from_ptr(uid.cast()).to_string_lossy().into_owned()
            })
            .collect();
        camel_folder_free_uids(folder, array);
        uids
    }
}

/// The object Camel hands the vfunc: a `CamelMimeMessage` parsed out of bytes,
/// which is exactly how the message reaches the vfunc in Camel's own
/// cross-store transfer — `get_message_sync` on the source folder.
struct Message(*mut CamelMimeMessage);

impl Message {
    fn parsed(source: &[u8]) -> Self {
        // SAFETY: a fresh message is a valid `CamelDataWrapper`, `source` is a
        // live buffer of the length given, and the error out-parameter is a
        // local that starts NULL.
        unsafe {
            let message = eds_sys::camel_mime_message_new();
            let mut error: *mut GError = ptr::null_mut();
            let parsed = camel_data_wrapper_construct_from_data_sync(
                message.cast::<CamelDataWrapper>(),
                source.as_ptr().cast(),
                source.len() as gssize,
                ptr::null_mut(),
                &mut error,
            );
            assert_ne!(parsed, GFALSE, "the fixture message would not parse");
            Self(message)
        }
    }
}

impl Drop for Message {
    fn drop(&mut self) {
        // SAFETY: the one reference, taken at construction.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// A summary row built the way Camel builds one for a message it is about to
/// append: detached from any summary, carrying whatever the source folder knew.
struct Row(*mut CamelMessageInfo);

impl Row {
    fn blank() -> Self {
        // SAFETY: NULL for the summary is the documented way to build a row
        // that belongs to none, which is what an append's argument is.
        Self(unsafe { camel_message_info_new(ptr::null_mut()) })
    }

    fn flags(self, flags: u32) -> Self {
        // SAFETY: a live row this value owns; the mask and the value are the
        // same word, which sets exactly the named bits.
        unsafe { camel_message_info_set_flags(self.0, flags, flags) };
        self
    }

    fn label(self, name: &CStr) -> Self {
        // SAFETY: as above, and a NUL-terminated name Camel copies.
        unsafe { camel_message_info_set_user_flag(self.0, name.as_ptr(), 1) };
        self
    }

    fn received_at(self, seconds: i64) -> Self {
        // SAFETY: as above.
        unsafe { camel_message_info_set_date_received(self.0, seconds) };
        self
    }
}

impl Drop for Row {
    fn drop(&mut self) {
        // SAFETY: the one reference, taken at construction.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// One append and both of its answers, owned the way Camel owns them.
struct Appended {
    ok: bool,
    uid: *mut glib_sys::gchar,
    error: *mut GError,
}

impl Appended {
    fn new(ok: gboolean, uid: *mut glib_sys::gchar, error: *mut GError) -> Self {
        Self {
            ok: ok != GFALSE,
            uid,
            error,
        }
    }

    fn expect_ok(&self) {
        assert!(self.ok, "the append failed: {}", self.message());
        assert!(self.error.is_null(), "an append that worked set an error");
    }

    /// The uid the message has in the folder, as reported.
    fn reported(&self) -> Option<String> {
        if self.uid.is_null() {
            return None;
        }
        // SAFETY: a NUL-terminated string this value owns.
        Some(unsafe { CStr::from_ptr(self.uid).to_string_lossy().into_owned() })
    }

    fn message(&self) -> String {
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

    /// The domain and code Camel reports, for the tests about a refusal.
    fn failure(&self) -> (glib_sys::GQuark, i32) {
        assert!(!self.ok, "the append was expected to fail");
        assert!(!self.error.is_null(), "a failed append set no error");
        // SAFETY: a live GError.
        unsafe { ((*self.error).domain, (*self.error).code) }
    }
}

impl Drop for Appended {
    fn drop(&mut self) {
        // SAFETY: the string and the error are both the caller's to free, and
        // this value is the caller.
        unsafe {
            if !self.uid.is_null() {
                g_free(self.uid.cast());
            }
            if !self.error.is_null() {
                glib_sys::g_error_free(self.error);
            }
        }
    }
}

#[test]
fn an_appended_message_is_a_message_of_the_folder_it_was_appended_to() {
    let fixture = Fixture::start();
    let appended = fixture.append(&Message::parsed(MESSAGE), ptr::null_mut());
    appended.expect_ok();

    // The uid the vfunc answered with is the uid the mailbox lists it under:
    // what Camel records for the message has to be what it will next find the
    // message by.
    let row = fixture.only();
    assert_eq!(appended.reported().as_deref(), Some(row.uid.as_str()));
}

#[test]
fn the_message_that_went_up_is_the_message_that_comes_back_down() {
    let fixture = Fixture::start();
    fixture
        .append(&Message::parsed(MESSAGE), ptr::null_mut())
        .expect_ok();

    // Out as an object and back in as one, through Camel's own writer and
    // Camel's own parser, with an upload and a download in between. Every
    // property below is one a serialisation that dropped a header would lose.
    let row = fixture.only();
    assert_eq!(row.subject.as_deref(), Some("Lunch?"));
    assert_eq!(
        row.from.first().map(|from| from.email.as_str()),
        Some("bob@example.com")
    );
    assert_eq!(row.message_id.as_deref(), Some("lunch@example.com"));

    fixture.refresh();
    let uid = CString::new(row.uid.as_str()).expect("a uid with no NUL");
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder of ours, a NUL-terminated uid alive across the
    // call, and an out-parameter that is writable and currently NULL.
    unsafe {
        let message = camel_folder_get_message_sync(
            fixture.archive,
            uid.as_ptr(),
            ptr::null_mut(),
            &mut error,
        );
        assert!(!message.is_null(), "the appended message would not open");
        let subject = camel_mime_message_get_subject(message);
        assert!(!subject.is_null(), "the reopened message lost its subject");
        assert_eq!(
            CStr::from_ptr(subject).to_string_lossy(),
            "Lunch?",
            "the reopened message is not the one that went up"
        );
        g_object_unref(message.cast());
    }
}

#[test]
fn the_flags_and_labels_a_row_carries_go_up_as_keywords() {
    let fixture = Fixture::start();
    let row = Row::blank()
        .flags(CAMEL_MESSAGE_SEEN | CAMEL_MESSAGE_FLAGGED)
        .label(c"Work");

    fixture.append(&Message::parsed(MESSAGE), row.0).expect_ok();

    // A message dragged in from another account arrives read if it was read:
    // the whole round trip through the keyword mapping, read back as the
    // server holds it.
    let listed = fixture.only();
    assert!(listed.flags.seen, "the appended message arrived unread");
    assert!(listed.flags.flagged, "the appended message lost its star");
    assert_eq!(listed.tags, vec!["Work".to_owned()]);
    assert_eq!(
        fixture.keywords_on_server(&listed.uid).keys().count(),
        3,
        "the server holds keywords the row did not carry"
    );
}

#[test]
fn a_message_appended_with_no_row_carries_no_keywords() {
    let fixture = Fixture::start();
    fixture
        .append(&Message::parsed(MESSAGE), ptr::null_mut())
        .expect_ok();

    // Camel declares the argument nullable, and an append that invented a
    // keyword for a message nothing is known about would put a label on it
    // that every other client would then show.
    let listed = fixture.only();
    assert!(!listed.flags.seen, "an unknown message arrived read");
    assert!(listed.tags.is_empty(), "{:?}", listed.tags);
}

#[test]
fn the_moment_a_row_says_the_message_arrived_is_the_moment_it_keeps() {
    let fixture = Fixture::start();
    // 2026-01-15T09:30:00Z, which is what Camel parsed out of the message when
    // the folder it came from listed it.
    let received_at = 1_768_469_400;
    let row = Row::blank().received_at(received_at);

    fixture.append(&Message::parsed(MESSAGE), row.0).expect_ok();

    // Left to the server it would be the moment of the copy, which sorts a
    // message moved between accounts to the wrong end of the folder.
    assert_eq!(fixture.only().received_at, Some(received_at));
}

#[test]
fn a_message_appended_with_no_moment_is_dated_by_the_server() {
    let fixture = Fixture::start();
    // A row Camel built and never dated: `date_received` is zero, which is not
    // 1970 — it is "nothing known", and sending it as a date would file the
    // message at the epoch forever.
    let row = Row::blank();

    fixture.append(&Message::parsed(MESSAGE), row.0).expect_ok();

    let received_at = fixture.only().received_at.expect("the server dated it");
    assert!(received_at > 0, "{received_at}");
}

#[test]
fn an_appended_message_does_not_appear_in_the_folder_until_it_is_listed() {
    let fixture = Fixture::start();
    let appended = fixture.append(&Message::parsed(MESSAGE), ptr::null_mut());
    appended.expect_ok();

    // The row is the listing's to write, for [`crate::transfer`]'s reason: a
    // row built from a uid alone would be a message list line with no subject,
    // sender or date.
    assert!(
        listed(fixture.archive).is_empty(),
        "the append wrote a row of its own"
    );

    fixture.refresh();
    assert_eq!(
        listed(fixture.archive),
        vec![appended.reported().expect("a uid was reported")]
    );
}

#[test]
fn appending_to_a_folder_of_a_disconnected_account_fails() {
    let fixture = Fixture::start();
    fixture.account.jmap().drop_connection();

    let appended = fixture.append_straight(&Message::parsed(MESSAGE), ptr::null_mut());
    // SAFETY: no arguments, and the quark registers itself.
    let quark = unsafe { camel_service_error_quark() };
    assert_eq!(
        appended.failure(),
        (quark, CAMEL_SERVICE_ERROR_NOT_CONNECTED as i32),
        "{}",
        appended.message()
    );
    assert_eq!(
        appended.reported(),
        None,
        "an append that failed reported a uid"
    );
}

#[test]
fn appending_to_a_mailbox_the_account_no_longer_has_fails() {
    let fixture = Fixture::start();
    fixture.removed_elsewhere();

    let appended = fixture.append(&Message::parsed(MESSAGE), ptr::null_mut());
    assert!(
        !appended.ok && !appended.error.is_null(),
        "the message was appended to a mailbox that is gone"
    );
    assert_eq!(
        appended.reported(),
        None,
        "an append that failed reported a uid"
    );
}
