// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `synchronize_sync`: the vfunc that writes the user's flag changes to the
//! server.
//!
//! Earlier parts of the mail provider only read from the server. This part
//! writes. It joins two halves built and tested separately: `jmap-mail-sync`'s
//! `KeywordChange` is the difference between two keyword sets, expressed as
//! the `Email/set` patch that closes it. `CamelJmapMessageInfo` is the summary
//! row that remembers the keywords the last listing found. This file walks
//! between them: it turns each row Camel marked dirty into one patch.
//!
//! ## Why a difference and not the row
//!
//! A `keywords` object holds every keyword any client has put on the message.
//! Sending the row's whole set would speak for keywords this provider has
//! never seen, such as a label from the user's phone or a `$phishing` verdict
//! from the server's own filter. For each of those, the write would say
//! "gone". [`a_keyword_this_client_never_saw_survives_the_change`] tests this
//! rule against a keyword that arrives on the server *after* the folder
//! listed the message. That is the exact case a whole-set write destroys.
//!
//! Holding both ends of the difference makes this possible. That is why
//! [`the_row_remembers_what_it_just_wrote`] matters as much as the write
//! itself: if a row's remembered set is not renewed after a successful write,
//! the before is stale. The next change is then diffed against a set the
//! server stopped holding a synchronisation ago.
//!
//! ## The dirty bit is the queue
//!
//! Camel marks a row `CAMEL_MESSAGE_FOLDER_FLAGGED` when something about it
//! must reach the server. That bit is the whole work list; there is no
//! separate queue. Clearing it is not bookkeeping: a bit left set means the
//! row is retried on every synchronisation forever. A bit cleared without the
//! write happening means the provider silently dropped a change the user
//! made. Three tests check which of those an outcome is.
//!
//! [`changing_a_flag_is_what_puts_the_row_on_the_work_list`] tests something
//! Camel does, not something this provider does. It confirms both halves of
//! an assumption every other test relies on: a row the user changed reaches
//! the work list, and a row a listing built does not. The second half was
//! actually wrong when this file was first written: Camel's column setters
//! and `camel_folder_summary_add` both mark a row as needing to reach the
//! server. So a refresh queued every message it wrote, and this provider
//! would have written the whole mailbox back to the server it had just read
//! it from.

mod common;

use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::ptr;

use common::Account;
use eds_sys::{
    CAMEL_MESSAGE_DELETED, CAMEL_MESSAGE_SEEN, CAMEL_SERVICE_ERROR_NOT_CONNECTED,
    CAMEL_STORE_FOLDER_NONE, CamelFolder, CamelFolderClass, CamelMessageInfo,
    camel_folder_get_folder_summary, camel_folder_refresh_info_sync, camel_folder_summary_add,
    camel_folder_summary_get, camel_folder_synchronize_sync, camel_message_info_get_folder_flagged,
    camel_message_info_set_flags, camel_message_info_set_uid, camel_service_error_quark,
    camel_store_get_folder_sync,
};
use glib_sys::{GError, GFALSE, GTRUE, gboolean};
use gobject_sys::{g_object_unref, g_type_class_ref, g_type_class_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::folder::folder_type;
use jmap_mail::message_info::new_message_info;
use jmap_mail_sync::{MailSync, MessageFlags, MessageSummary};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::role;

/// One connected account with one message in its inbox. The inbox is open
/// and refreshed. Every synchronisation starts from this state, because a
/// row can only be dirty if it exists.
struct Fixture {
    server: MockServer,
    account: Account,
    folder: *mut CamelFolder,
    uid: Id,
}

impl Fixture {
    /// The message is seeded with `keywords`, then the folder is refreshed.
    /// The row's remembered set then matches exactly what the server holds.
    /// This is the state a user's next click starts from.
    fn start(keywords: &[&str]) -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let uid = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            let mailbox = account.seed_mailbox("Inbox", Some(role::INBOX));
            let mut seed = EmailSeed::new(
                mailbox,
                ("Bob", "bob@example.com"),
                "Lunch?",
                "One o'clock.",
                "2026-01-15T09:30:00Z",
            );
            for keyword in keywords {
                seed = seed.keyword(keyword);
            }
            account.seed_email(seed)
        };

        let account = Account::open();
        let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
        account.connect(MailSync::new(client, account_id));

        let path = CString::new("Inbox").expect("a path with no NUL");
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live store of ours, a NUL-terminated path alive across the
        // call, and out-parameters that are writable and currently NULL.
        let folder = unsafe {
            let folder = camel_store_get_folder_sync(
                account.store,
                path.as_ptr(),
                CAMEL_STORE_FOLDER_NONE,
                ptr::null_mut(),
                &mut error,
            );
            assert!(
                !folder.is_null() && error.is_null(),
                "the inbox would not open"
            );
            assert_ne!(
                camel_folder_refresh_info_sync(folder, ptr::null_mut(), &mut error),
                GFALSE,
                "the inbox would not refresh"
            );
            folder
        };

        Self {
            server,
            account,
            folder,
            uid,
        }
    }

    /// Another listing of the mailbox, the kind Evolution's refresh timer
    /// requests. Three tests in this file request one between the user's
    /// click and the write, which is the worst possible moment.
    fn refresh(&self) {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live folder of ours, and an out-parameter that is writable
        // and currently NULL.
        unsafe {
            assert_ne!(
                camel_folder_refresh_info_sync(self.folder, ptr::null_mut(), &mut error),
                GFALSE,
                "the inbox would not refresh"
            );
        }
    }

    /// The summary row for the seeded message. The caller owns the returned
    /// reference.
    fn row(&self) -> *mut CamelMessageInfo {
        let uid = CString::new(self.uid.as_str()).expect("a uid with no NUL");
        // SAFETY: a live folder that has a summary, and a NUL-terminated uid
        // alive across the call.
        unsafe {
            let summary = camel_folder_get_folder_summary(self.folder);
            assert!(!summary.is_null(), "the folder has no summary");
            let info = camel_folder_summary_get(summary, uid.as_ptr());
            assert!(!info.is_null(), "the refresh left no row for the message");
            info
        }
    }

    /// Changes the row's flags the way Evolution's message list does.
    fn set_flags(&self, mask: u32, set: u32) {
        // SAFETY: the row is a live message info this call owns a reference to.
        unsafe {
            let info = self.row();
            camel_message_info_set_flags(info, mask, set);
            g_object_unref(info.cast());
        }
    }

    /// Whether Camel still holds the row as needing to reach the server.
    fn is_dirty(&self) -> bool {
        // SAFETY: as above.
        unsafe {
            let info = self.row();
            let dirty = camel_message_info_get_folder_flagged(info) != GFALSE;
            g_object_unref(info.cast());
            dirty
        }
    }

    /// What the server holds for the message now.
    fn keywords_on_server(&self) -> BTreeMap<String, bool> {
        let account_id = self.server.account_id();
        let state = self.server.state();
        let state = state.lock().unwrap();
        let account = state.account(&account_id).unwrap();
        account
            .emails
            .get(&self.uid)
            .expect("the seeded message")
            .keywords
            .clone()
            .unwrap_or_default()
    }

    /// Puts a keyword on the message, as another client would: without the
    /// folder's knowledge, and with no listing in between.
    fn keyword_from_elsewhere(&self, name: &str) {
        let account_id = self.server.account_id();
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let uid = self.uid.clone();
        account.emails.transaction(|emails| {
            let mut email = emails.get(&uid).expect("the seeded message").clone();
            email
                .keywords
                .get_or_insert_with(BTreeMap::new)
                .insert(name.to_owned(), true);
            emails.update(&uid, email);
        });
    }

    /// Destroys the message the way another client would.
    fn destroyed_elsewhere(&self) {
        let account_id = self.server.account_id();
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let uid = self.uid.clone();
        account.emails.transaction(|emails| {
            assert!(emails.destroy(&uid), "the seeded message was not there");
        });
    }

    /// Through Camel's own wrapper. Evolution calls this wrapper when a
    /// folder closes, or when the user saves changes.
    fn synchronize(&self) -> Synchronised {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live folder, and an out-parameter that is writable and
        // currently NULL. FALSE for `expunge`: deleting mail is a mailbox
        // change this provider does not make yet.
        let ok = unsafe {
            camel_folder_synchronize_sync(self.folder, GFALSE, ptr::null_mut(), &mut error)
        };
        Synchronised::new(ok, error)
    }

    /// Through the pointer in the class. This skips whatever the wrapper does
    /// on the way in. See [`a_folder_whose_store_has_no_connection_reports_it`].
    fn synchronize_straight(&self) -> Synchronised {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: referencing the class runs the class_init that installs the
        // vfunc. `folder` is an instance of that class, and `error` is writable
        // and currently NULL.
        let ok = unsafe {
            let class = g_type_class_ref(folder_type()).cast::<CamelFolderClass>();
            let vfunc = (*class)
                .synchronize_sync
                .expect("the folder cannot be synchronised");
            let ok = vfunc(self.folder, GFALSE, ptr::null_mut(), &mut error);
            g_type_class_unref(class.cast());
            ok
        };
        Synchronised::new(ok, error)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // SAFETY: releases the one reference this fixture took from
        // `get_folder_sync`, before releasing the store it depends on.
        unsafe { g_object_unref(self.folder.cast()) };
    }
}

/// One synchronisation, and both of its answers. Owned the same way Camel
/// owns them.
struct Synchronised {
    ok: bool,
    error: *mut GError,
}

impl Synchronised {
    fn new(ok: gboolean, error: *mut GError) -> Self {
        Self {
            ok: ok != GFALSE,
            error,
        }
    }

    fn expect_ok(self) {
        assert!(self.ok, "the synchronisation failed: {}", self.message());
        assert!(
            self.error.is_null(),
            "a synchronisation that worked set an error"
        );
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
}

impl Drop for Synchronised {
    fn drop(&mut self) {
        if !self.error.is_null() {
            // SAFETY: the one reference, taken by the call above.
            unsafe { glib_sys::g_error_free(self.error) };
        }
    }
}

/// The dirty bit is the work list, and Camel fills it, not this provider.
/// If marking a message read did not set this bit, every test in this file
/// would need to mark the folder dirty by hand before synchronising it.
/// That would test the walk against a queue Evolution never fills.
#[test]
fn changing_a_flag_is_what_puts_the_row_on_the_work_list() {
    let fixture = Fixture::start(&[]);
    assert!(
        !fixture.is_dirty(),
        "a freshly listed row was already dirty"
    );

    fixture.set_flags(CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);

    assert!(
        fixture.is_dirty(),
        "the row Camel changed is not on the list"
    );
}

/// The whole increment: the user marks a message read and the server agrees.
#[test]
fn a_flag_the_user_changed_reaches_the_server() {
    let fixture = Fixture::start(&[]);
    fixture.set_flags(CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);

    fixture.synchronize().expect_ok();

    assert_eq!(
        fixture.keywords_on_server(),
        BTreeMap::from([("$seen".to_owned(), true)])
    );
}

/// The row also leaves the work list. Otherwise it would be written again
/// on every synchronisation for as long as the folder stays open.
#[test]
fn a_row_that_reached_the_server_leaves_the_work_list() {
    let fixture = Fixture::start(&[]);
    fixture.set_flags(CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);

    fixture.synchronize().expect_ok();

    assert!(
        !fixture.is_dirty(),
        "the row is still queued for the server"
    );
}

/// The reason a flag change is a difference, not a full state. `Urgent`
/// arrives on the server after the folder listed the message, so nothing
/// on this side knows about it. A write that sent the row's whole keyword
/// set would remove it.
#[test]
fn a_keyword_this_client_never_saw_survives_the_change() {
    let fixture = Fixture::start(&["$seen"]);
    fixture.keyword_from_elsewhere("Urgent");

    fixture.set_flags(CAMEL_MESSAGE_SEEN, 0);
    fixture.synchronize().expect_ok();

    assert_eq!(
        fixture.keywords_on_server(),
        BTreeMap::from([("Urgent".to_owned(), true)]),
        "the write spoke for a keyword it had never seen"
    );
}

/// A difference needs a *before*. The before for the next change is what
/// this change just wrote. A row that did not renew it would diff the
/// user's next click against the keywords the last listing found. Then
/// unmarking a message read would produce no change, and the flag would
/// never come off.
#[test]
fn the_row_remembers_what_it_just_wrote() {
    let fixture = Fixture::start(&[]);
    fixture.set_flags(CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);
    fixture.synchronize().expect_ok();

    fixture.set_flags(CAMEL_MESSAGE_SEEN, 0);
    fixture.synchronize().expect_ok();

    assert!(
        fixture.keywords_on_server().is_empty(),
        "the second change was diffed against a stale set: {:?}",
        fixture.keywords_on_server()
    );
}

/// Camel marks a row dirty for reasons that are not keywords at all.
/// `CAMEL_MESSAGE_DELETED` is a local mark with no matching JMAP keyword.
/// Such a row must leave the server unchanged, and must still leave the
/// work list. A bit nothing can clear means the row is retried on every
/// synchronisation forever.
#[test]
fn a_row_dirty_for_something_that_is_not_a_keyword_leaves_the_server_alone() {
    let fixture = Fixture::start(&["$seen"]);
    fixture.set_flags(CAMEL_MESSAGE_DELETED, CAMEL_MESSAGE_DELETED);

    fixture.synchronize().expect_ok();

    assert_eq!(
        fixture.keywords_on_server(),
        BTreeMap::from([("$seen".to_owned(), true)])
    );
    assert!(!fixture.is_dirty(), "the row will be retried forever");
}

/// A uid in a summary is a claim about the last listing. Another client
/// destroying the message since then is ordinary. The flag change is moot,
/// not failed: reporting it would alert the user about a message that is
/// not there. Leaving the row queued would retry a write that can never
/// succeed.
#[test]
fn a_message_another_client_deleted_does_not_fail_the_synchronisation() {
    let fixture = Fixture::start(&[]);
    fixture.set_flags(CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);
    fixture.destroyed_elsewhere();

    fixture.synchronize().expect_ok();

    assert!(!fixture.is_dirty(), "the row will be retried forever");
}

/// The race between the two directions, end to end. Evolution refreshes a
/// folder on a timer and synchronises it when the folder closes. A listing
/// started *before* the user's click often arrives *after* it. Written over
/// the row whole, that listing would undo the click on screen and leave the
/// row claiming exactly what it remembers the server holding. The diff
/// would then be empty, so the change would be lost silently, with the row
/// still on the work list and nothing left to send.
#[test]
fn a_refresh_between_the_click_and_the_write_does_not_lose_the_click() {
    let fixture = Fixture::start(&[]);
    fixture.set_flags(CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);

    fixture.refresh();
    fixture.synchronize().expect_ok();

    assert_eq!(
        fixture.keywords_on_server(),
        BTreeMap::from([("$seen".to_owned(), true)]),
        "the refresh undid the user's unsaved change"
    );
    assert!(
        !fixture.is_dirty(),
        "the row is still queued for the server"
    );
}

/// The listing is still applied. What another client did between the click
/// and the refresh is news the user's outstanding change says nothing about.
/// The row ends up carrying both changes, and the write that follows sends
/// only the half that is this folder's to send.
#[test]
fn a_refresh_leaves_a_queued_row_carrying_both_changes() {
    let fixture = Fixture::start(&[]);
    fixture.set_flags(CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);
    fixture.keyword_from_elsewhere("Urgent");

    fixture.refresh();
    fixture.synchronize().expect_ok();

    assert_eq!(
        fixture.keywords_on_server(),
        BTreeMap::from([("$seen".to_owned(), true), ("Urgent".to_owned(), true)])
    );
}

/// The other half of the same rule. The listing is renewed onto the row
/// rather than keeping the row's own claim, because a server already told
/// (by this folder's own earlier write, or by the user's phone) leaves
/// nothing to send. The row then settles, instead of writing the same
/// keyword on every synchronisation for as long as the folder stays open.
#[test]
fn a_change_the_server_already_made_itself_settles_the_row() {
    let fixture = Fixture::start(&[]);
    fixture.set_flags(CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);
    fixture.keyword_from_elsewhere("$seen");

    fixture.refresh();
    fixture.synchronize().expect_ok();

    assert_eq!(
        fixture.keywords_on_server(),
        BTreeMap::from([("$seen".to_owned(), true)])
    );
    assert!(!fixture.is_dirty(), "the row will be retried forever");
}

/// A folder with nothing queued asks the server nothing. It succeeds even
/// with no server to ask, which proves no request was made. Camel
/// synchronises a folder every time it closes, and a provider that made a
/// round trip per close would pay for one on every folder the user clicks
/// through.
#[test]
fn a_folder_with_nothing_queued_needs_no_server() {
    let fixture = Fixture::start(&[]);
    assert!(fixture.account.jmap().drop_connection());

    fixture.synchronize_straight().expect_ok();
}

/// The folder is asked, and the connection belongs to the store.
/// `NOT_CONNECTED` is the code that makes Camel connect and ask again,
/// instead of showing the account as broken. `refresh_info_sync` follows
/// the same rule.
///
/// This test calls through the class rather than through the wrapper, as
/// `tests/refresh.rs` does. It tests the window after Camel confirmed a
/// connection existed, not a state Camel would have fixed on the way in.
#[test]
fn a_folder_whose_store_has_no_connection_reports_it() {
    let fixture = Fixture::start(&[]);
    fixture.set_flags(CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);
    assert!(fixture.account.jmap().drop_connection());

    let synchronised = fixture.synchronize_straight();

    assert!(
        !synchronised.ok,
        "a disconnected folder synchronised anyway"
    );
    assert!(
        !synchronised.error.is_null(),
        "it failed without saying why"
    );
    // SAFETY: a live GError, and the quark accessor takes no arguments.
    unsafe {
        assert_eq!((*synchronised.error).domain, camel_service_error_quark());
        assert_eq!(
            (*synchronised.error).code,
            CAMEL_SERVICE_ERROR_NOT_CONNECTED as i32
        );
    }
    // The change also stays queued: a write that never happened is one the
    // next synchronisation must make.
    assert!(fixture.is_dirty(), "the unsent change was forgotten");
}

/// A row with nothing set except the one field a row cannot be without.
/// This test overwrites the uid right after. See
/// [`a_row_whose_uid_is_not_text_stays_queued`].
fn blank_message(uid: &str) -> MessageSummary {
    MessageSummary {
        uid: Id::new(uid),
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

/// A uid Camel stored that cannot be read back as text cannot be asked
/// about on the server either. So `push_row` leaves it queued, instead of
/// settling a row that nothing was sent for. Such a uid signals a corrupted
/// or foreign summary database; this provider never writes one itself, but
/// Camel's own storage does not enforce UTF-8 on a uid. The generic
/// settling step used to run for it anyway, because an unreadable uid
/// produced the same `Ok(())` as a nothing-to-send row: it marked the row
/// as if the write had happened when no request was ever sent.
#[test]
fn a_row_whose_uid_is_not_text_stays_queued() {
    let fixture = Fixture::start(&[]);
    // SAFETY: `fixture.folder` is a live folder for the length of the test.
    let summary = unsafe { camel_folder_get_folder_summary(fixture.folder) };

    let raw_uid = CString::new(vec![0xFF, 0xFE]).expect("no embedded NUL");
    assert!(
        raw_uid.to_str().is_err(),
        "the uid needs to be unreadable as text for this test to mean anything"
    );

    // SAFETY: `summary` is the live summary above. `raw_uid` outlives every
    // call it is passed to. `info` is a fresh row this scope owns, until it
    // hands the row to the summary, which takes its own reference.
    let info = unsafe {
        let info = new_message_info(&blank_message("placeholder"));
        camel_message_info_set_uid(info, raw_uid.as_ptr());
        camel_folder_summary_add(summary, info, GTRUE);
        // Nothing here can ever send this change. See the doc comment above.
        camel_message_info_set_flags(info, CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);
        info
    };
    assert!(
        unsafe { camel_message_info_get_folder_flagged(info) } != GFALSE,
        "the row needs to start out queued for this test to mean anything"
    );

    fixture.synchronize().expect_ok();

    assert!(
        unsafe { camel_message_info_get_folder_flagged(info) } != GFALSE,
        "a change nothing could send was dequeued as though it had been"
    );
    // SAFETY: the one reference taken above.
    unsafe { g_object_unref(info.cast()) };
}
