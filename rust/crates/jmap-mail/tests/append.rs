// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `append_message_sync`: the vfunc that puts a message Camel holds into a
//! JMAP folder.
//!
//! [`crate::transfer`] moves a message the *account already has* into another
//! of its mailboxes. That is one `Email/set` call. This file covers a
//! different arrival: a `CamelMimeMessage` from an outside source. Sources
//! include the folder another account's message was dragged out of, the
//! composer saving a draft, and a `.eml` file the user dropped on the folder.
//! A JMAP account takes such a message only through `Email/import` over an
//! uploaded blob. [`MailSync::import_message`] built the sync half of that
//! earlier. These tests cover the Camel half.
//!
//! ## The message goes up as the bytes Camel would write out
//!
//! The vfunc argument is an object, but `Email/import` takes a blob, so
//! something must serialize it. [`jmap_mail::mime`] does this, using Camel's
//! own writer. [`crate::message`] parses incoming mail with Camel's parser for
//! the same reason: the emit on the way out must also be Camel's. Otherwise
//! this provider would be a second MIME implementation that disagrees with
//! the first about the message content. [`jmap_mail::mime`] has its own
//! module and its own tests, because sending needs the same bytes. These
//! tests check what an append does with the serialized bytes.
//!
//! [`the_message_that_went_up_is_the_message_that_comes_back_down`] checks
//! that round trip end to end: append as an object, list as a row, open again
//! as an object. Every step is a place a byte could be lost.
//!
//! ## The row comes from the listing, not from the append
//!
//! An append adds nothing to the folder's summary.
//! [`an_appended_message_does_not_appear_in_the_folder_until_it_is_listed`]
//! checks this. [`crate::transfer`] made the same decision for the folder a
//! message is dragged *into*, for the same reason: this side holds only a
//! uid. A row built from a uid alone would be a message list line with no
//! subject, sender, or date, until a refresh replaces it.
//!
//! ## What Camel knows about the message that the message does not say
//!
//! The `CamelMessageInfo` argument is the folder's answer to "and this is
//! what it was". Two parts of it reach the server: the flags and labels, as
//! keywords, and `date_received`, as `receivedAt`. Both matter for the
//! ordinary case of moving mail between two accounts. A transfer that
//! arrives unread when it was read, or dated to the moment of the copy,
//! loses something the user could see.
//!
//! The argument may also be absent. `camel_folder_append_message_sync`
//! declares it nullable, and Camel's own callers pass NULL for a message
//! with no known history. [`a_message_appended_with_no_row_carries_no_keywords`]
//! checks this normal case, not a defensive one.
//!
//! ## Offline
//!
//! [`appending_to_a_folder_of_a_disconnected_account_is_queued_and_shown_locally`]
//! covers ROADMAP item 176(b)'s pending append: with no connection, the
//! append does not fail, and unlike the connected case above, a row for the
//! message appears in the folder right away, because there is no listing to
//! wait for.
//!
//! [`crate::transfer`]: jmap_mail::transfer
//! [`crate::message`]: jmap_mail::message
//! [`MailSync::import_message`]: jmap_mail_sync::MailSync::import_message

mod common;

use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::ptr;

use common::Account;
use eds_sys::compat::{folder_dup_uids, folder_free_uids};
use eds_sys::{
    CAMEL_FOLDER_ERROR_INSUFFICIENT_PERMISSION, CAMEL_FOLDER_ERROR_INVALID, CAMEL_MESSAGE_FLAGGED,
    CAMEL_MESSAGE_SEEN, CAMEL_STORE_FOLDER_NONE, CamelDataWrapper, CamelFolder, CamelFolderClass,
    CamelMessageInfo, CamelMimeMessage, camel_data_wrapper_construct_from_data_sync,
    camel_folder_append_message_sync, camel_folder_error_quark, camel_folder_get_folder_summary,
    camel_folder_get_message_sync, camel_folder_refresh_info_sync, camel_folder_summary_get,
    camel_folder_synchronize_sync, camel_message_info_new, camel_message_info_set_date_received,
    camel_message_info_set_flags, camel_message_info_set_user_flag, camel_mime_message_get_subject,
    camel_store_get_folder_sync,
};
use glib_sys::{GError, GFALSE, GPtrArray, g_free, gboolean, gssize};
use gobject_sys::{g_object_unref, g_type_class_ref, g_type_class_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::folder::folder_type;
use jmap_mail::message_info::pending_append;
use jmap_mail::summary::set_summary_state;
use jmap_mail_sync::{MailSync, MessageSummary};
use jmap_mock::MockServer;
use jmap_proto::mail::MailboxRights;
use jmap_proto::{Id, State};

/// The RFC 5322 bytes of the message to append: CRLF line endings, a header
/// block, a blank line, and a body.
const MESSAGE: &[u8] = b"From: Bob <bob@example.com>\r\n\
To: Alice <alice@example.com>\r\n\
Subject: Lunch?\r\n\
Message-ID: <lunch@example.com>\r\n\
Date: Thu, 15 Jan 2026 09:30:00 +0000\r\n\
\r\n\
One o'clock at the usual place.\r\n";

/// One connected account with one empty folder open. This is the state every
/// append starts from: a folder receiving an outside message may not yet be
/// listed.
struct Fixture {
    server: MockServer,
    account: Account,
    archive: *mut CamelFolder,
    archive_id: Id,
}

impl Fixture {
    fn start() -> Self {
        Self::started_with(MockServer::builder())
    }

    fn started_with(builder: jmap_mock::MockServerBuilder) -> Self {
        let server = builder.start();
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

    /// The same single-folder account, with myRights already set on its one
    /// mailbox before the first listing. The mailbox has every right except
    /// mayAddItems, the shape a read-only share leaves.
    fn started_without_add_rights() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let archive_id = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            let id = account.seed_mailbox("Archive", None);
            account.mailboxes.transaction(|mailboxes| {
                let mailbox = mailboxes.get(&id).expect("the seeded mailbox").clone();
                let denied = mailbox.with_my_rights(MailboxRights::new().with_may_add_items(false));
                assert!(
                    mailboxes.update(&id, denied),
                    "the seeded mailbox was not there"
                );
            });
            id
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

    /// Appends through Camel's own wrapper, the function Evolution calls when
    /// a message arrives in the folder from outside the account.
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

    /// Appends through the pointer in the class, skipping what Camel's
    /// wrapper does on the way in.
    ///
    /// This is not a shortcut: it is the only way to test the vfunc directly.
    /// `camel_folder_append_message_sync` connects the service before it
    /// dispatches, so the wrapper reconnects a store whose connection was
    /// dropped. Against these settings, which name no server, the
    /// reconnection fails first, so the reported failure is the
    /// reconnection's (`URL_INVALID`), not the vfunc's. [`crate::transfer`]'s
    /// tests reach past the wrapper for the same reason.
    fn append_straight(&self, message: &Message, info: *mut CamelMessageInfo) -> Appended {
        let mut uid: *mut glib_sys::gchar = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: referencing the class runs class_init, which installs the
        // vfunc. The folder is an instance of that class. Both
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

    /// What the mailbox holds now, as a folder refresh would find it. This
    /// asks the server, not the summary, because an append does not write
    /// the summary.
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

    /// The keywords the server holds for the message.
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

    /// The mailbox a local row's pending append is queued for, or `None` if
    /// the row has none (including no row at all).
    fn pending_append(&self, uid: &str) -> Option<Id> {
        let uid = CString::new(uid).expect("a uid with no NUL");
        // SAFETY: a live folder with a summary, a NUL-terminated uid alive
        // across the call, and one reference to the row, released here.
        unsafe {
            let summary = camel_folder_get_folder_summary(self.archive);
            assert!(!summary.is_null(), "the folder has no summary");
            let info: *mut CamelMessageInfo = camel_folder_summary_get(summary, uid.as_ptr());
            if info.is_null() {
                return None;
            }
            let append = pending_append(info);
            g_object_unref(info.cast());
            append.map(|append| append.mailbox().clone())
        }
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

    /// Through Camel's own wrapper, the way Evolution's generic reconnect
    /// flush calls it on every open folder.
    fn synchronize(&self) -> (bool, *mut GError) {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live folder of ours, and an out-parameter that is writable
        // and currently NULL.
        let ok = unsafe {
            camel_folder_synchronize_sync(self.archive, GFALSE, ptr::null_mut(), &mut error)
        };
        (ok != GFALSE, error)
    }

    /// Through the pointer in the class, skipping what Camel's wrapper does
    /// on the way in: the wrapper reconnects the service before it
    /// dispatches, so a call through it while genuinely offline fails at
    /// that reconnection (these settings name no server), not in the vfunc
    /// this crate installed. `tests/synchronize.rs`'s own
    /// `synchronize_straight` is the same shortcut, for the same reason.
    fn synchronize_straight(&self) -> (bool, *mut GError) {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: referencing the class runs class_init, which installs the
        // vfunc. The folder is an instance of that class, and the
        // out-parameter is writable and currently NULL.
        let ok = unsafe {
            let class = g_type_class_ref(folder_type()).cast::<CamelFolderClass>();
            let vfunc = (*class)
                .synchronize_sync
                .expect("the folder cannot be synchronised");
            let ok = vfunc(self.archive, GFALSE, ptr::null_mut(), &mut error);
            g_type_class_unref(class.cast());
            ok
        };
        (ok != GFALSE, error)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // SAFETY: releases the reference this fixture took from
        // `get_folder_sync`. The store this folder depends on is released
        // after this.
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
        let array: *mut GPtrArray = folder_dup_uids(folder);
        let uids = (0..(*array).len)
            .map(|index| {
                let uid = *(*array).pdata.add(index as usize);
                CStr::from_ptr(uid.cast()).to_string_lossy().into_owned()
            })
            .collect();
        folder_free_uids(folder, array);
        uids
    }
}

/// The object Camel hands the vfunc: a `CamelMimeMessage` parsed from bytes.
/// This is exactly how the message reaches the vfunc in Camel's own
/// cross-store transfer, through `get_message_sync` on the source folder.
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

    // Camel declares the argument nullable. An append that invents a keyword
    // for an unknown message would put a label on it that every other
    // client would show.
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
    // A row Camel built and never dated: `date_received` is zero. This does
    // not mean 1970, it means "nothing known". Sending it as a date would
    // file the message at the epoch forever.
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

/// ROADMAP item 176(b): an append made with no connection must not fail
/// outright. `append_message_sync` queues the message on a local row and
/// reports success, the same way an offline move or an offline Expunge
/// already succeeds locally ([`crate::transfer`], [`crate::expunge`]). Unlike
/// a connected append, the row appears at once: there is no listing to wait
/// for while offline.
#[test]
fn appending_to_a_folder_of_a_disconnected_account_is_queued_and_shown_locally() {
    let fixture = Fixture::start();
    fixture.account.jmap().drop_connection();

    let appended = fixture.append_straight(&Message::parsed(MESSAGE), ptr::null_mut());
    appended.expect_ok();

    let uid = appended.reported().expect("a queued append reports a uid");
    assert!(
        uid.starts_with("jmap-local-"),
        "a queued append's uid must be tagged as synthetic: {uid}"
    );
    assert_eq!(
        listed(fixture.archive),
        vec![uid.clone()],
        "a queued append must show in the folder right away, with no listing"
    );

    let state = fixture.server.state();
    let state = state.lock().unwrap();
    assert!(
        state
            .account(&fixture.server.account_id())
            .unwrap()
            .blobs
            .is_empty(),
        "a queued append must not reach the server at all"
    );
    drop(state);

    assert_eq!(
        fixture.pending_append(&uid),
        Some(fixture.archive_id.clone()),
        "the queued row must remember which mailbox the append is bound for"
    );
}

/// ROADMAP item 176(b)'s reconnect walk: synchronising a folder that still
/// holds an unsent append must not fail. Before the walk learned to skip a
/// pending-append row, `push_row` read the row's own flags and labels as a
/// keyword change against its synthetic uid and tried to send it, which
/// only `Email/set` on a real uid can ever answer.
#[test]
fn synchronising_a_folder_with_a_queued_append_still_offline_does_not_fail() {
    let fixture = Fixture::start();
    fixture.account.jmap().drop_connection();

    let row = Row::blank()
        .flags(CAMEL_MESSAGE_SEEN | CAMEL_MESSAGE_FLAGGED)
        .label(c"Work");
    let appended = fixture.append_straight(&Message::parsed(MESSAGE), row.0);
    appended.expect_ok();
    let local_uid = appended.reported().expect("a queued append reports a uid");

    let (ok, error) = fixture.synchronize_straight();
    assert!(
        ok,
        "a still-offline synchronise with a queued append must not fail: {}",
        if error.is_null() {
            "no error".to_owned()
        } else {
            // SAFETY: a live GError from the call above.
            unsafe {
                CStr::from_ptr((*error).message)
                    .to_string_lossy()
                    .into_owned()
            }
        }
    );
    // SAFETY: NULL or a GError this function owns.
    unsafe {
        if !error.is_null() {
            glib_sys::g_error_free(error);
        }
    }
    assert_eq!(
        listed(fixture.archive),
        vec![local_uid],
        "the queued row must stay exactly as it was"
    );
}

/// The queued append reaches the server once the store reconnects, with no
/// explicit `Email/import` call from the client beyond the generic
/// `CamelOfflineStore` reconnect flush: the same flush `transfer.rs`'s own
/// queued-move test already proves exists for a pending filing.
#[test]
fn a_queued_append_lands_once_the_store_reconnects() {
    let fixture = Fixture::start();
    fixture.account.jmap().drop_connection();

    let row = Row::blank()
        .flags(CAMEL_MESSAGE_SEEN | CAMEL_MESSAGE_FLAGGED)
        .label(c"Work");
    let appended = fixture.append_straight(&Message::parsed(MESSAGE), row.0);
    appended.expect_ok();
    let local_uid = appended.reported().expect("a queued append reports a uid");

    let client =
        Client::connect(fixture.server.origin(), Credentials::none()).expect("connected again");
    fixture
        .account
        .connect(MailSync::new(client, fixture.server.account_id()));

    let (ok, error) = fixture.synchronize();
    assert!(
        ok,
        "the reconnect synchronise failed: {}",
        if error.is_null() {
            "no error".to_owned()
        } else {
            // SAFETY: a live GError from the call above.
            unsafe {
                CStr::from_ptr((*error).message)
                    .to_string_lossy()
                    .into_owned()
            }
        }
    );
    // SAFETY: NULL or a GError this function owns.
    unsafe {
        if !error.is_null() {
            glib_sys::g_error_free(error);
        }
    }

    assert!(
        fixture.pending_append(&local_uid).is_none(),
        "the synthetic row must be gone after the append lands"
    );
    let remaining = listed(fixture.archive);
    assert_eq!(
        remaining.len(),
        1,
        "exactly the one landed row must remain: {remaining:?}"
    );
    let new_uid = &remaining[0];
    assert!(
        *new_uid != local_uid && !new_uid.starts_with("jmap-local-"),
        "the landed row must carry the server's own uid, not the synthetic one: {new_uid}"
    );

    let message = fixture.only();
    assert_eq!(message.uid.as_str(), new_uid);
    assert!(message.flags.seen, "the landed append lost its read flag");
    assert!(message.flags.flagged, "the landed append lost its star");
    assert_eq!(message.tags, vec!["Work".to_owned()]);
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

/// A shared mailbox whose `myRights.mayAddItems` is `false` (RFC 8621 §2) has
/// the shape a read-only share leaves. The append is refused locally, from
/// the last listing, instead of being uploaded for the server to reject.
#[test]
fn appending_to_a_mailbox_without_add_rights_is_refused_locally() {
    let fixture = Fixture::started_without_add_rights();

    let appended = fixture.append(&Message::parsed(MESSAGE), ptr::null_mut());

    // SAFETY: no arguments, and the quark registers itself.
    let quark = unsafe { camel_folder_error_quark() };
    assert_eq!(
        appended.failure(),
        (quark, CAMEL_FOLDER_ERROR_INSUFFICIENT_PERMISSION as i32),
        "{}",
        appended.message()
    );
    assert_eq!(
        appended.reported(),
        None,
        "an append that failed reported a uid"
    );

    // Nothing reached the server: the refusal is local, before the upload.
    let state = fixture.server.state();
    let state = state.lock().unwrap();
    assert!(
        state
            .account(&fixture.server.account_id())
            .unwrap()
            .blobs
            .is_empty(),
        "a locally refused append still uploaded the message"
    );
}

/// A message larger than the account's `maxSizeUpload` (RFC 8620 §6.1) is
/// refused before a byte of it is sent. The folder states the reason.
///
/// The size limit is the one refusal that is knowable in advance: it is in
/// the session document, and the message is already in hand. Sending the
/// message anyway would cost the user the whole upload. For a large
/// attachment on a slow connection, that can mean minutes of a progress bar.
/// The user would learn at the end what the folder could have said at the
/// start.
///
/// The test reports this in `CAMEL_FOLDER_ERROR`, not `CAMEL_SERVICE_ERROR`.
/// Nothing is wrong with the account. Evolution reads a service error as a
/// sign that the account is unusable. This matches the judgement the vfunc
/// makes about a message that Camel's writer refuses: the message, not the
/// account, could not be used.
#[test]
fn appending_a_message_over_the_accounts_upload_limit_fails_before_it_is_sent() {
    let fixture = Fixture::started_with(MockServer::builder().size_upload(64));
    assert!(MESSAGE.len() > 64, "the test message is not over the limit");

    let appended = fixture.append(&Message::parsed(MESSAGE), ptr::null_mut());

    // SAFETY: no arguments, and the quark registers itself.
    let quark = unsafe { camel_folder_error_quark() };
    assert_eq!(
        appended.failure(),
        (quark, CAMEL_FOLDER_ERROR_INVALID as i32),
        "{}",
        appended.message()
    );
    // The limit is in the sentence, because "too large" without the number is
    // not something a user can act on.
    assert!(appended.message().contains("64"), "{}", appended.message());
    assert_eq!(
        appended.reported(),
        None,
        "an append that failed reported a uid"
    );

    // And the mailbox is as it was: no blob, no message.
    assert!(fixture.listing().is_empty());
    let state = fixture.server.state();
    let state = state.lock().unwrap();
    assert!(
        state
            .account(&fixture.server.account_id())
            .unwrap()
            .blobs
            .is_empty(),
        "a refused append still uploaded the message"
    );
}

/// ROADMAP item 167(a): the server's permanent refusal of a queued append
/// (the message is over `maxSizeUpload`) must not wedge this folder's
/// synchronise forever. The message cannot shrink between retries, so
/// retrying it on every reconnect can never succeed.
#[test]
fn a_queued_append_too_large_for_the_server_gives_up_instead_of_wedging_synchronise() {
    let fixture = Fixture::started_with(MockServer::builder().size_upload(64));
    assert!(MESSAGE.len() > 64, "the test message is not over the limit");
    fixture.account.jmap().drop_connection();

    let appended = fixture.append_straight(&Message::parsed(MESSAGE), ptr::null_mut());
    appended.expect_ok();
    let local_uid = appended.reported().expect("a queued append reports a uid");

    let client =
        Client::connect(fixture.server.origin(), Credentials::none()).expect("connected again");
    fixture
        .account
        .connect(MailSync::new(client, fixture.server.account_id()));

    let (ok, error) = fixture.synchronize();
    assert!(
        ok,
        "a permanently refused append must not fail the whole synchronise: {}",
        if error.is_null() {
            "no error".to_owned()
        } else {
            // SAFETY: a live GError from the call above.
            unsafe {
                CStr::from_ptr((*error).message)
                    .to_string_lossy()
                    .into_owned()
            }
        }
    );
    // SAFETY: NULL or a GError this function owns.
    unsafe {
        if !error.is_null() {
            glib_sys::g_error_free(error);
        }
    }

    assert!(
        fixture.pending_append(&local_uid).is_none(),
        "a permanently refused append must give up, not retry forever"
    );

    // A second synchronise must stay clean: nothing is left to retry.
    let (ok_again, error_again) = fixture.synchronize();
    assert!(ok_again, "synchronise must stay clean after giving up");
    // SAFETY: NULL or a GError this function owns.
    unsafe {
        if !error_again.is_null() {
            glib_sys::g_error_free(error_again);
        }
    }
}

/// A listing that runs before the reconnect flush has replayed a queued
/// append must not remove the queued row: it never names the synthetic uid,
/// since the server does not have the message yet. This can happen for
/// real: a push, or `MailSync`'s own delta-cost rule, can trigger a
/// listing-based refresh with no guarantee it runs after
/// `synchronize_sync`'s reconnect flush has uploaded the append.
#[test]
fn a_listing_that_runs_before_a_queued_append_lands_does_not_remove_its_row() {
    let fixture = Fixture::start();
    fixture.account.jmap().drop_connection();

    let appended = fixture.append_straight(&Message::parsed(MESSAGE), ptr::null_mut());
    appended.expect_ok();
    let local_uid = appended.reported().expect("a queued append reports a uid");

    let client =
        Client::connect(fixture.server.origin(), Credentials::none()).expect("connected again");
    fixture
        .account
        .connect(MailSync::new(client, fixture.server.account_id()));

    // Force the next refresh to be a full relist rather than a delta
    // (`tests/refresh.rs`'s own
    // `a_refresh_from_a_state_the_server_will_not_calculate_from_lists_again`
    // uses the same trick): a state the mock does not recognize answers
    // `Email/changes` with `cannotCalculateChanges`, so `messages_since`
    // falls back to `relist`, the one path that calls `apply_listing` and so
    // `remove_absent`. The folder's one real message is empty right now
    // (the queued append never reached the server), so the listing names
    // nothing, and only the synthetic row is at risk of being swept away.
    unsafe {
        set_summary_state(
            camel_folder_get_folder_summary(fixture.archive),
            State::new("a state from some other server"),
        );
    }

    fixture.refresh();

    assert_eq!(
        listed(fixture.archive),
        vec![local_uid.clone()],
        "a relisting ahead of the reconnect flush must not drop the queued row"
    );
    assert_eq!(
        fixture.pending_append(&local_uid),
        Some(fixture.archive_id.clone()),
        "the row must still remember which mailbox the append is bound for"
    );
}
