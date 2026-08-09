// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `synchronize_sync`: the vfunc that puts the user's flag changes on the
//! server.
//!
//! Every earlier increment of the mail provider reads. This is the one that
//! writes, and the two halves it joins have been built separately and tested
//! separately: `jmap-mail-sync`'s `KeywordChange` is the difference between two
//! keyword sets as the `Email/set` patch that closes it, and
//! `CamelJmapMessageInfo` is the summary row that remembers the keywords the
//! last listing found. This is the walk between them — the rows Camel marked
//! dirty, turned into one patch each.
//!
//! ## Why a difference and not the row
//!
//! A `keywords` object holds everything every client ever put on the message.
//! Sending the row's whole set would speak for keywords this provider has never
//! heard of — a label from the user's phone, a `$phishing` verdict from the
//! server's own filter — and what it would say about each of them is "gone".
//! [`a_keyword_this_client_never_saw_survives_the_change`] is that rule, tested
//! against a keyword that arrives on the server *after* the folder listed the
//! message, which is precisely the case a whole-set write destroys.
//!
//! Holding both ends of the difference is what makes it possible, and it is why
//! [`the_row_remembers_what_it_just_wrote`] matters as much as the write itself:
//! a row whose remembered set was not renewed after a successful write has a
//! stale *before*, and the next change is diffed against a set the server
//! stopped holding a synchronisation ago.
//!
//! ## The dirty bit is the queue
//!
//! Camel marks a row `CAMEL_MESSAGE_FOLDER_FLAGGED` when something about it has
//! to reach the server, and that bit is the whole work list — there is no
//! separate queue. So clearing it is not bookkeeping: a bit left set is a row
//! retried on every synchronisation forever, and a bit cleared without the
//! write having happened is a change the user made and this provider silently
//! dropped. Three tests are about which of those a given outcome is.
//!
//! Which is why [`changing_a_flag_is_what_puts_the_row_on_the_work_list`] is
//! here at all, testing something Camel does rather than something this provider
//! does. It fixes both ends of the assumption every other test rests on: that a
//! row the user changed arrives on the list, and — the half that was actually
//! wrong when this file was first written — that a row a *listing* built does
//! not. Camel's column setters and `camel_folder_summary_add` both mark a row as
//! having to reach the server, so a refresh queued every message it wrote, and
//! this provider would have written the whole mailbox straight back to the
//! server it had just read it from.

mod common;

use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::ptr;

use common::Account;
use eds_sys::{
    CAMEL_MESSAGE_DELETED, CAMEL_MESSAGE_SEEN, CAMEL_SERVICE_ERROR_NOT_CONNECTED,
    CAMEL_STORE_FOLDER_NONE, CamelFolder, CamelFolderClass, CamelMessageInfo,
    camel_folder_get_folder_summary, camel_folder_refresh_info_sync, camel_folder_summary_get,
    camel_folder_synchronize_sync, camel_message_info_get_folder_flagged,
    camel_message_info_set_flags, camel_service_error_quark, camel_store_get_folder_sync,
};
use glib_sys::{GError, GFALSE, gboolean};
use gobject_sys::{g_object_unref, g_type_class_ref, g_type_class_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::folder::folder_type;
use jmap_mail_sync::MailSync;
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::role;

/// One connected account, one message in its inbox, and that inbox opened and
/// refreshed — which is the state every synchronisation starts from, because a
/// row can only be dirty if it is there.
struct Fixture {
    server: MockServer,
    account: Account,
    folder: *mut CamelFolder,
    uid: Id,
}

impl Fixture {
    /// The message is seeded with `keywords` and the folder is then refreshed,
    /// so the row's remembered set is exactly what the server holds — the state
    /// a user's next click starts from.
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

    /// Another listing of the mailbox, the way Evolution's refresh timer asks
    /// for one — including at the worst possible moment, which is what the two
    /// tests at the end of this file are about.
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

    /// The summary row for the seeded message, as a reference the caller owns.
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

    /// Puts a keyword on the message the way another client would — behind the
    /// folder's back, with no listing in between.
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

    /// Through Camel's own wrapper, which is what Evolution calls when a folder
    /// is closed or the user's changes are saved.
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

    /// Through the pointer in the class, skipping whatever the wrapper does on
    /// the way in — see [`a_folder_whose_store_has_no_connection_reports_it`].
    fn synchronize_straight(&self) -> Synchronised {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: referencing the class runs the class_init that installs the
        // vfunc; `folder` is an instance of that class, and `error` is writable
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
        // SAFETY: the one reference this fixture took from `get_folder_sync`,
        // released before the store it hangs off.
        unsafe { g_object_unref(self.folder.cast()) };
    }
}

/// One synchronisation and both of its answers, owned the way Camel owns them.
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

/// The dirty bit is the work list, and Camel is the one who fills it. If
/// marking a message read did not set it, every test in this file would be
/// synchronising a folder it had marked dirty by hand — testing the walk
/// against a queue Evolution never fills.
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

/// And the row leaves the work list, or it is written again on every
/// synchronisation for as long as the folder is open.
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

/// The reason a flag change is a difference and not a state. `Urgent` arrives
/// on the server after the folder listed the message, so nothing on this side
/// has ever heard of it — and a write that sent the row's whole keyword set
/// would take it off.
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

/// A difference needs a *before*, and the before of the next change is what
/// this one just wrote. A row that did not renew it would diff the user's next
/// click against the keywords the last listing found — so unmarking a message
/// read would produce no change at all, and the flag would never come off.
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

/// Camel marks a row dirty for reasons that are not keywords at all —
/// `CAMEL_MESSAGE_DELETED` is a local mark JMAP has no keyword for. Such a row
/// must leave the server exactly as it was, and must still leave the work list:
/// a bit nothing can clear is a row retried on every synchronisation forever.
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

/// A uid in a summary is a claim about the last listing, and another client
/// destroying the message since is ordinary. The flag change is moot rather
/// than failed: reporting it would put an alert in front of the user about a
/// message that is not there, and leaving the row queued would retry a write
/// that can never succeed.
#[test]
fn a_message_another_client_deleted_does_not_fail_the_synchronisation() {
    let fixture = Fixture::start(&[]);
    fixture.set_flags(CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);
    fixture.destroyed_elsewhere();

    fixture.synchronize().expect_ok();

    assert!(!fixture.is_dirty(), "the row will be retried forever");
}

/// The race between the two directions, end to end. Evolution refreshes a
/// folder on a timer and synchronises it when it is closed, so a listing made
/// *before* the user's click routinely arrives *after* it. Written over the row
/// whole, that listing would undo the click on screen and leave the row claiming
/// exactly what it remembers the server holding — so the diff below would be
/// empty and the change would be lost in silence, with the row still on the work
/// list and nothing left on it to send.
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

/// And the listing is still applied: what another client did between the click
/// and the refresh is news the user's outstanding change says nothing about. The
/// row ends up carrying both, and the write that follows sends only the half
/// that is this folder's to send.
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

/// The other half of the same rule, and the reason the listing is renewed onto
/// the row rather than the row's own claim: a server that has *already* been
/// told — by this folder's own earlier write, or by the user's phone — leaves
/// nothing to send, and the row settles instead of writing the same keyword on
/// every synchronisation for as long as the folder is open.
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

/// A folder with nothing queued asks the server nothing — and so succeeds even
/// with no server to ask, which is what proves no request was made. Camel
/// synchronises a folder every time it is closed, and a provider that made a
/// round trip per close would pay for one on every folder the user clicks
/// through.
#[test]
fn a_folder_with_nothing_queued_needs_no_server() {
    let fixture = Fixture::start(&[]);
    assert!(fixture.account.jmap().drop_connection());

    fixture.synchronize_straight().expect_ok();
}

/// The folder is asked, and the connection belongs to the store. `NOT_CONNECTED`
/// is the code that makes Camel connect and ask again rather than showing the
/// account as broken — the same rule `refresh_info_sync` follows.
///
/// Called through the class rather than through the wrapper, the way
/// `tests/refresh.rs` calls its own: what is under test is the window after
/// Camel satisfied itself there was a connection, not a state Camel would have
/// fixed on the way in.
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
    // And the change stays queued: a write that never happened is one the next
    // synchronisation has to make.
    assert!(fixture.is_dirty(), "the unsent change was forgotten");
}
