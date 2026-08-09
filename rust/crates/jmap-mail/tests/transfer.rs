// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `transfer_messages_to_sync`: the vfunc behind dragging a message into
//! another folder.
//!
//! The `Email/set` a copy or a move becomes was built and tested one increment
//! ago, in `jmap-mail-sync`'s `Filing`. This is the Camel half: two folders of
//! one store, the uids Evolution hands over, and the summary the source folder
//! is left holding afterwards.
//!
//! ## What a JMAP move is, from inside one folder
//!
//! There is no `Email/move` and no `Email/copy` — RFC 8621 §4.6 makes
//! `mailboxIds` the set of mailboxes a message is in, so a copy adds a member
//! and a move adds one and takes another away, and either way the message stays
//! *one* object with one id. Two consequences run through this file:
//!
//! - The uid does not change. [`the_uid_of_a_transferred_message_is_the_one_it_had`]
//!   is that, asserted through the out-parameter Camel offers for it — where
//!   IMAPX, whose server does mint a new uid in the destination, has nothing to
//!   say and says nothing.
//! - A move out of a folder is the same event the next listing of that folder
//!   would report as a message that has left it. The rows go now rather than
//!   then, because a message list that keeps showing what the user just moved
//!   away is a message list that is wrong for as long as the refresh timer
//!   takes.
//!
//! ## The click that has not been saved yet
//!
//! Camel keeps the user's unsaved flag changes on the summary row, and
//! `synchronize_sync` is what writes them. A move that removed the row would
//! therefore take the change with it, which is
//! [`a_move_settles_a_flag_the_user_had_not_saved_yet`]: marking a message read
//! and dragging it into another folder before anything synchronised is an
//! ordinary sequence, and losing the flag in it would be this provider dropping
//! a change of the user's in silence.
//!
//! ## The message list that is already on screen
//!
//! Removing the row is what the *next* listing of the source folder is drawn
//! from; `changed` is what the one the user is looking at is redrawn from. The
//! two are separate and the second was, until this increment, written the way
//! IMAPX writes it and never watched. It is watched here, through
//! `common::signals` — which means a [`Context`] pushed before the account is
//! opened and a pump before every read, for the reason that module's header
//! gives.
//!
//! Four things are asserted about it, and three of them are silences:
//!
//! - A move announces the rows it took away, and nothing else.
//! - A copy announces nothing, because it took none.
//! - The *destination* announces nothing either. The message appears there when
//!   that folder is next listed, which is the decision this file's "what is not
//!   decided here" section makes: a row built from a uid alone would be a line
//!   with no subject, sender or date.
//! - A transfer that failed without moving a row announces nothing. The one
//!   failure that does announce is a message another client destroyed, which
//!   leaves the folder precisely because it is not in it any more.
//!
//! ## A selection, and what only a selection can say
//!
//! Everything above transfers one message, and the vfunc is handed a *list* —
//! whatever the user had highlighted when they dragged. Three of this file's
//! claims are about the list rather than about a message, and none of them can
//! be made with one uid in it:
//!
//! - **The walk is a request per uid, and it finishes.** This is the module's
//!   own "one request per message, not one per transfer" decision, seen from
//!   outside: a message another client destroyed must not take the rest of the
//!   user's selection with it, so
//!   [`a_message_that_is_gone_does_not_hold_up_the_rest_of_the_selection`] moves
//!   the second message behind a first that cannot move.
//! - **The out-parameter's slots line up with the list that came in.** Camel
//!   reads the answer for a message by its position, so a half-worked transfer
//!   leaves NULL in the slot of the message that did not land rather than a
//!   shorter array — [`a_partial_transfer_reports_only_the_messages_that_landed`].
//! - **One emission for the whole selection.** Camel's change info is four
//!   *lists* precisely so that a drag of twenty messages redraws the message
//!   list once, and
//!   [`the_rows_a_selection_left_behind_are_announced_together`] is that.

mod common;

use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::ptr;

use common::Account;
use common::signals::{Context, Emission, emissions, watch};
use eds_sys::{
    CAMEL_FOLDER_ERROR_INVALID_UID, CAMEL_MESSAGE_SEEN, CAMEL_SERVICE_ERROR_NOT_CONNECTED,
    CAMEL_STORE_FOLDER_NONE, CamelFolder, CamelFolderClass, CamelMessageInfo,
    camel_folder_error_quark, camel_folder_free_uids, camel_folder_get_folder_summary,
    camel_folder_get_uids, camel_folder_refresh_info_sync, camel_folder_summary_get,
    camel_folder_transfer_messages_to_sync, camel_message_info_set_flags,
    camel_service_error_quark, camel_store_get_folder_sync,
};
use glib_sys::{
    GError, GFALSE, GPtrArray, GTRUE, g_free, g_ptr_array_add, g_ptr_array_free, g_ptr_array_new,
    gboolean, gpointer,
};
use gobject_sys::{g_object_unref, g_type_class_ref, g_type_class_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::folder::folder_type;
use jmap_mail_sync::MailSync;
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::role;

/// The messages the fixture can seed, in the order it seeds them. A transfer of
/// a *message* takes the first; a transfer of a *selection* takes both.
const MESSAGES: [(&str, &str, &str, &str, &str); 2] = [
    (
        "Bob",
        "bob@example.com",
        "Lunch?",
        "One o'clock.",
        "2026-01-15T09:30:00Z",
    ),
    (
        "Carla",
        "carla@example.com",
        "The invoice",
        "Attached, as promised.",
        "2026-01-15T11:05:00Z",
    ),
];

/// One connected account with two folders open — an inbox holding the seeded
/// messages, and an archive holding none — which is the state every transfer
/// starts from.
struct Fixture {
    server: MockServer,
    account: Account,
    inbox: *mut CamelFolder,
    archive: *mut CamelFolder,
    inbox_id: Id,
    archive_id: Id,
    uids: Vec<Id>,
}

impl Fixture {
    /// One message in the inbox: a transfer of a single message, which is what
    /// most of this file is about.
    fn start() -> Self {
        Self::seeded(1)
    }

    /// Two, for the transfers that are about a selection rather than a message.
    /// Evolution hands the vfunc whatever the user had highlighted, and what a
    /// selection asserts is what one message cannot: that the walk makes a
    /// request per uid, that the out-parameter's slots line up with the list
    /// that came in, and that the rows that left are announced together.
    fn selection() -> Self {
        Self::seeded(2)
    }

    fn seeded(messages: usize) -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let (inbox_id, archive_id, uids) = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            let inbox_id = account.seed_mailbox("Inbox", Some(role::INBOX));
            let archive_id = account.seed_mailbox("Archive", None);
            let uids = MESSAGES[..messages]
                .iter()
                .map(|(name, address, subject, body, received_at)| {
                    account.seed_email(EmailSeed::new(
                        inbox_id.clone(),
                        (*name, *address),
                        subject,
                        body,
                        received_at,
                    ))
                })
                .collect();
            (inbox_id, archive_id, uids)
        };

        let account = Account::open();
        let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
        account.connect(MailSync::new(client, account_id));

        let inbox = open(&account, "Inbox");
        let archive = open(&account, "Archive");

        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live folder of ours, and an out-parameter that is writable
        // and currently NULL. Only the inbox is refreshed: what the archive
        // holds is not this vfunc's business, and a folder that was never
        // listed is the ordinary state of the one a message is dragged into.
        unsafe {
            assert_ne!(
                camel_folder_refresh_info_sync(inbox, ptr::null_mut(), &mut error),
                GFALSE,
                "the inbox would not refresh"
            );
        }

        Self {
            server,
            account,
            inbox,
            archive,
            inbox_id,
            archive_id,
            uids,
        }
    }

    /// The first seeded message, which is the one a test transfers unless it
    /// says otherwise.
    fn uid(&self) -> &Id {
        &self.uids[0]
    }

    /// The second, for a fixture built by [`Fixture::selection`].
    fn second(&self) -> &Id {
        &self.uids[1]
    }

    /// The same fixture, with one of its folders listened to from here on.
    ///
    /// The setup is pumped away first and only then is the handler connected,
    /// because the emission the inbox's refresh queued would otherwise be
    /// delivered into a test that is asking what the *transfer* announced. The
    /// context is the caller's, and has to have been pushed before the fixture
    /// opened the account: see `common::signals::Context`.
    fn watched(self, context: &Context, folder: fn(&Self) -> *mut CamelFolder) -> Self {
        emissions(context);
        watch(folder(&self));
        self
    }

    /// Through Camel's own wrapper, which is what Evolution calls when the user
    /// drags a message or picks "Move to Folder".
    fn transfer(&self, uids: &[&Id], delete_originals: bool) -> Transferred {
        self.transfer_between(self.inbox, self.archive, uids, delete_originals)
    }

    fn transfer_between(
        &self,
        source: *mut CamelFolder,
        destination: *mut CamelFolder,
        uids: &[&Id],
        delete_originals: bool,
    ) -> Transferred {
        let list = UidList::of(uids);
        let mut transferred: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: two live folders of one store, an array of NUL-terminated
        // uids alive across the call, and two out-parameters that are writable
        // and currently NULL.
        let ok = unsafe {
            camel_folder_transfer_messages_to_sync(
                source,
                list.array,
                destination,
                gboolean::from(delete_originals),
                &mut transferred,
                ptr::null_mut(),
                &mut error,
            )
        };
        Transferred::new(ok, transferred, error)
    }

    /// Through the pointer in the class, skipping what the wrapper settles on
    /// the way in — a transfer into the folder the message is already in, and
    /// one of no messages at all, are both answered by Camel before any
    /// provider is asked.
    fn transfer_straight(
        &self,
        source: *mut CamelFolder,
        destination: *mut CamelFolder,
        uids: &[&Id],
        delete_originals: bool,
    ) -> Transferred {
        let list = UidList::of(uids);
        let mut transferred: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: referencing the class runs the class_init that installs the
        // vfunc; both folders are instances of that class, and the two
        // out-parameters are writable and currently NULL.
        let ok = unsafe {
            let class = g_type_class_ref(folder_type()).cast::<CamelFolderClass>();
            let vfunc = (*class)
                .transfer_messages_to_sync
                .expect("the folder cannot transfer messages");
            let ok = vfunc(
                source,
                list.array,
                destination,
                gboolean::from(delete_originals),
                &mut transferred,
                ptr::null_mut(),
                &mut error,
            );
            g_type_class_unref(class.cast());
            ok
        };
        Transferred::new(ok, transferred, error)
    }

    /// Which mailboxes the server has the message in now.
    fn mailboxes_on_server(&self, uid: &Id) -> BTreeMap<Id, bool> {
        let account_id = self.server.account_id();
        let state = self.server.state();
        let state = state.lock().unwrap();
        let account = state.account(&account_id).unwrap();
        account
            .emails
            .get(uid)
            .expect("the seeded message")
            .mailbox_ids
            .clone()
            .unwrap_or_default()
    }

    /// And what it holds for its keywords.
    fn keywords_on_server(&self, uid: &Id) -> BTreeMap<String, bool> {
        let account_id = self.server.account_id();
        let state = self.server.state();
        let state = state.lock().unwrap();
        let account = state.account(&account_id).unwrap();
        account
            .emails
            .get(uid)
            .expect("the seeded message")
            .keywords
            .clone()
            .unwrap_or_default()
    }

    /// Destroys the message the way another client would.
    fn destroyed_elsewhere(&self, uid: &Id) {
        let account_id = self.server.account_id();
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let uid = uid.clone();
        account.emails.transaction(|emails| {
            assert!(emails.destroy(&uid), "the seeded message was not there");
        });
    }

    /// Changes the message's flags the way Evolution's message list does, and
    /// without saving them.
    fn mark_read(&self) {
        let uid = CString::new(self.uid().as_str()).expect("a uid with no NUL");
        // SAFETY: a live folder that has a summary, a NUL-terminated uid alive
        // across the call, and one reference to the row, released here.
        unsafe {
            let summary = camel_folder_get_folder_summary(self.inbox);
            assert!(!summary.is_null(), "the folder has no summary");
            let info: *mut CamelMessageInfo = camel_folder_summary_get(summary, uid.as_ptr());
            assert!(!info.is_null(), "the refresh left no row for the message");
            camel_message_info_set_flags(info, CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);
            g_object_unref(info.cast());
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // SAFETY: the two references this fixture took from `get_folder_sync`,
        // released before the store they hang off.
        unsafe {
            g_object_unref(self.inbox.cast());
            g_object_unref(self.archive.cast());
        }
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

/// The uids Camel would draw a message list from, asked for the way Evolution
/// asks — of the folder rather than of its summary.
fn listed(folder: *mut CamelFolder) -> Vec<String> {
    // SAFETY: a live folder; the array comes back owned and is freed with the
    // function Camel documents for it.
    unsafe {
        let array = camel_folder_get_uids(folder);
        let uids = uid_list(array);
        camel_folder_free_uids(folder, array);
        uids
    }
}

/// A borrowed `GPtrArray` of uids, as strings.
///
/// # Safety
///
/// `array` must be NULL or a live array of NUL-terminated strings.
unsafe fn uid_list(array: *mut GPtrArray) -> Vec<String> {
    if array.is_null() {
        return Vec::new();
    }
    // SAFETY: the contract above; the strings live as long as the array.
    unsafe {
        (0..(*array).len)
            .map(|index| {
                let uid = *(*array).pdata.add(index as usize);
                CStr::from_ptr(uid.cast()).to_string_lossy().into_owned()
            })
            .collect()
    }
}

/// The array of uids Camel hands the vfunc, owned for the length of one call.
struct UidList {
    array: *mut GPtrArray,
    // The strings the array points at; freed with it.
    uids: Vec<CString>,
}

impl UidList {
    fn of(uids: &[&Id]) -> Self {
        let uids: Vec<CString> = uids
            .iter()
            .map(|uid| CString::new(uid.as_str()).expect("a uid with no NUL"))
            .collect();
        // SAFETY: a fresh array, filled with pointers into strings this value
        // owns and outlives it by.
        let array = unsafe {
            let array = g_ptr_array_new();
            for uid in &uids {
                g_ptr_array_add(array, uid.as_ptr() as gpointer);
            }
            array
        };
        Self { array, uids }
    }
}

impl Drop for UidList {
    fn drop(&mut self) {
        // SAFETY: the one array, allocated above; FALSE because the pointers in
        // it belong to `self.uids`.
        unsafe { g_ptr_array_free(self.array, GFALSE) };
        self.uids.clear();
    }
}

/// One transfer and all three of its answers, owned the way Camel owns them.
struct Transferred {
    ok: bool,
    uids: *mut GPtrArray,
    error: *mut GError,
}

impl Transferred {
    fn new(ok: gboolean, uids: *mut GPtrArray, error: *mut GError) -> Self {
        Self {
            ok: ok != GFALSE,
            uids,
            error,
        }
    }

    fn expect_ok(&self) {
        assert!(self.ok, "the transfer failed: {}", self.message());
        assert!(self.error.is_null(), "a transfer that worked set an error");
    }

    /// The uids the messages have in the destination, as reported.
    fn reported(&self) -> Vec<Option<String>> {
        if self.uids.is_null() {
            return Vec::new();
        }
        // SAFETY: a live array this value owns, of NUL-terminated strings and
        // NULLs.
        unsafe {
            (0..(*self.uids).len)
                .map(|index| {
                    let uid = *(*self.uids).pdata.add(index as usize);
                    (!uid.is_null())
                        .then(|| CStr::from_ptr(uid.cast()).to_string_lossy().into_owned())
                })
                .collect()
        }
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

impl Drop for Transferred {
    fn drop(&mut self) {
        // SAFETY: both are references this value took from the call that made
        // it. The array is owned whole — Camel's own callers free its members
        // with `g_free` and then the array — and the error is one allocation.
        unsafe {
            if !self.uids.is_null() {
                for index in 0..(*self.uids).len {
                    g_free(*(*self.uids).pdata.add(index as usize));
                }
                g_ptr_array_free(self.uids, GTRUE);
            }
            if !self.error.is_null() {
                glib_sys::g_error_free(self.error);
            }
        }
    }
}

/// The copy half: the message gains a mailbox and keeps the one it was in,
/// because in JMAP a mailbox is a member of a set rather than a place.
#[test]
fn a_message_the_user_copied_is_in_both_mailboxes() {
    let fixture = Fixture::start();

    fixture.transfer(&[fixture.uid()], false).expect_ok();

    assert_eq!(
        fixture.mailboxes_on_server(fixture.uid()),
        BTreeMap::from([
            (fixture.inbox_id.clone(), true),
            (fixture.archive_id.clone(), true)
        ])
    );
}

/// And it stays in the folder it was copied out of — which is the difference
/// the `delete_originals` flag makes, seen from this side.
#[test]
fn a_copied_message_stays_in_the_folder_it_came_from() {
    let fixture = Fixture::start();

    fixture.transfer(&[fixture.uid()], false).expect_ok();

    assert_eq!(listed(fixture.inbox), vec![fixture.uid().to_string()]);
}

/// The move half: one patch, and the mailbox it came out of is gone from the
/// set.
#[test]
fn a_message_the_user_moved_is_only_in_the_destination() {
    let fixture = Fixture::start();

    fixture.transfer(&[fixture.uid()], true).expect_ok();

    assert_eq!(
        fixture.mailboxes_on_server(fixture.uid()),
        BTreeMap::from([(fixture.archive_id.clone(), true)])
    );
}

/// And the row goes with it. Waiting for the next listing to notice would leave
/// the message the user just moved sitting in the message list they moved it
/// out of, for as long as the refresh timer takes.
#[test]
fn a_moved_message_leaves_the_folder_it_came_from() {
    let fixture = Fixture::start();

    fixture.transfer(&[fixture.uid()], true).expect_ok();

    assert!(
        listed(fixture.inbox).is_empty(),
        "the moved message is still in the folder it left"
    );
}

/// RFC 8621 §4.1 gives an `Email` one immutable id per account, and filing it
/// somewhere else does not make a second object — so the message in the
/// destination is the message that was in the source, under the uid the caller
/// passed in. Camel offers an out-parameter for exactly this answer.
#[test]
fn the_uid_of_a_transferred_message_is_the_one_it_had() {
    let fixture = Fixture::start();

    let transferred = fixture.transfer(&[fixture.uid()], true);

    transferred.expect_ok();
    assert_eq!(
        transferred.reported(),
        vec![Some(fixture.uid().to_string())],
        "the transfer did not report where the message ended up"
    );
}

/// The user marks a message read and drags it into another folder before
/// anything has synchronised. The flag lives on the summary row until
/// `synchronize_sync` writes it, so a move that dropped the row would drop the
/// change with it — and the destination would never learn of it either, because
/// what it lists is what the server holds.
#[test]
fn a_move_settles_a_flag_the_user_had_not_saved_yet() {
    let fixture = Fixture::start();
    fixture.mark_read();

    fixture.transfer(&[fixture.uid()], true).expect_ok();

    assert_eq!(
        fixture.keywords_on_server(fixture.uid()),
        BTreeMap::from([("$seen".to_owned(), true)]),
        "the move lost the user's unsaved flag change"
    );
    assert_eq!(
        fixture.mailboxes_on_server(fixture.uid()),
        BTreeMap::from([(fixture.archive_id.clone(), true)])
    );
}

/// A move into the folder the message is already in cannot be written down —
/// the same `mailboxIds` member would have to be both set and cleared — so
/// nothing is sent, and in particular the row is not removed as though the
/// message had left. Called through the class because Camel's own wrapper
/// answers `source == destination` before any provider is asked; succeeding
/// with no connection is what proves no request was made.
#[test]
fn a_move_into_the_folder_the_message_is_already_in_is_not_a_request() {
    let fixture = Fixture::start();
    assert!(fixture.account.jmap().drop_connection());

    fixture
        .transfer_straight(fixture.inbox, fixture.inbox, &[fixture.uid()], true)
        .expect_ok();

    assert_eq!(
        listed(fixture.inbox),
        vec![fixture.uid().to_string()],
        "a message that went nowhere was taken out of its folder"
    );
}

/// A transfer of nothing asks the server nothing, for the same reason a folder
/// with nothing queued synchronises without one: Camel calls a provider for
/// selections a user can make empty.
#[test]
fn a_transfer_of_no_messages_needs_no_server() {
    let fixture = Fixture::start();
    assert!(fixture.account.jmap().drop_connection());

    fixture
        .transfer_straight(fixture.inbox, fixture.archive, &[], true)
        .expect_ok();
}

/// The folder is asked and the connection belongs to the store. `NOT_CONNECTED`
/// is the code that makes Camel connect and ask again rather than showing the
/// account as broken — the same rule the other three folder vfuncs follow — and
/// the message stays where it is, row and all.
#[test]
fn a_folder_whose_store_has_no_connection_reports_it() {
    let fixture = Fixture::start();
    assert!(fixture.account.jmap().drop_connection());

    let transferred =
        fixture.transfer_straight(fixture.inbox, fixture.archive, &[fixture.uid()], true);

    assert!(!transferred.ok, "a disconnected folder transferred anyway");
    assert!(!transferred.error.is_null(), "it failed without saying why");
    // SAFETY: a live GError, and the quark accessor takes no arguments.
    unsafe {
        assert_eq!((*transferred.error).domain, camel_service_error_quark());
        assert_eq!(
            (*transferred.error).code,
            CAMEL_SERVICE_ERROR_NOT_CONNECTED as i32
        );
    }
    assert_eq!(
        listed(fixture.inbox),
        vec![fixture.uid().to_string()],
        "a message that never moved was taken out of its folder"
    );
}

/// A uid in a summary is a claim about the last listing, and another client
/// destroying the message since is ordinary. Unlike a flag write — which is a
/// consequence the user never asked for and which is therefore settled in
/// silence — a transfer is something they did, so it is reported: as
/// `INVALID_UID`, which Evolution reads as "that message is gone" rather than
/// as a reason to take the account offline. The row still goes, because the
/// message is not in this folder either.
#[test]
fn a_message_another_client_deleted_is_reported_and_leaves_the_folder() {
    let fixture = Fixture::start();
    fixture.destroyed_elsewhere(fixture.uid());

    let transferred = fixture.transfer(&[fixture.uid()], true);

    assert!(!transferred.ok, "a message that is gone was transferred");
    // SAFETY: a live GError, and the quark accessor takes no arguments.
    unsafe {
        assert_eq!((*transferred.error).domain, camel_folder_error_quark());
        assert_eq!(
            (*transferred.error).code,
            CAMEL_FOLDER_ERROR_INVALID_UID as i32
        );
    }
    assert!(
        listed(fixture.inbox).is_empty(),
        "the folder still lists a message the server does not have"
    );
}

/// Taking the row away settles what the next listing of the folder would show.
/// This is the folder saying so to the list that is on screen now — and a move
/// is the one transfer that has anything to say about the folder it left.
#[test]
fn a_move_tells_camel_the_row_that_left() {
    let context = Context::push();
    let fixture = Fixture::start().watched(&context, |fixture| fixture.inbox);

    fixture.transfer(&[fixture.uid()], true).expect_ok();

    assert_eq!(
        emissions(&context),
        vec![Emission {
            added: Vec::new(),
            removed: vec![fixture.uid().to_string()],
            changed: Vec::new(),
            recent: Vec::new(),
        }]
    );
}

/// A copy leaves every row where it was, so there is nothing to announce.
/// Announcing anyway would redraw the message list the user is reading — and
/// lose where they were in it — for a folder in which nothing has happened.
#[test]
fn a_copy_says_nothing_about_the_folder_it_came_from() {
    let context = Context::push();
    let fixture = Fixture::start().watched(&context, |fixture| fixture.inbox);

    fixture.transfer(&[fixture.uid()], false).expect_ok();

    let emissions = emissions(&context);
    assert!(
        emissions.is_empty(),
        "a copy announced {emissions:?} about the folder it left"
    );
}

/// And the folder the message arrived in says nothing either, which is the
/// visible half of this file's decision not to build a row there: the message
/// appears in the destination when that folder is next listed, because the
/// listing is the only thing that knows what its row should say.
///
/// Watched on the archive rather than the inbox — the recorder is one list for
/// whatever it is connected to, so a test that watched both would not be able
/// to tell which folder spoke.
#[test]
fn a_move_says_nothing_about_the_folder_it_arrives_in() {
    let context = Context::push();
    let fixture = Fixture::start().watched(&context, |fixture| fixture.archive);

    fixture.transfer(&[fixture.uid()], true).expect_ok();

    let emissions = emissions(&context);
    assert!(
        emissions.is_empty(),
        "the destination announced {emissions:?} for a message it has no row for"
    );
    assert!(
        listed(fixture.archive).is_empty(),
        "the destination gained a row from a transfer rather than from a listing"
    );
}

/// A message another client destroyed is the one failure that still changes the
/// folder: it is not in this mailbox because it is not anywhere, so the row goes
/// and the list on screen is told. The transfer is reported as failed all the
/// same — see [`a_message_another_client_deleted_is_reported_and_leaves_the_folder`],
/// which asserts the other half.
#[test]
fn a_message_another_client_deleted_is_announced_as_its_row_goes() {
    let context = Context::push();
    let fixture = Fixture::start().watched(&context, |fixture| fixture.inbox);
    fixture.destroyed_elsewhere(fixture.uid());

    assert!(
        !fixture.transfer(&[fixture.uid()], true).ok,
        "a message that is gone was transferred"
    );

    assert_eq!(
        emissions(&context),
        vec![Emission {
            added: Vec::new(),
            removed: vec![fixture.uid().to_string()],
            changed: Vec::new(),
            recent: Vec::new(),
        }]
    );
}

/// Every other failure leaves the message list alone, because it leaves the rows
/// alone. A disconnected store is the case Camel retries after reconnecting, and
/// a folder that had announced the message as removed in the meantime would have
/// taken it off the user's screen for the duration.
#[test]
fn a_transfer_that_failed_leaves_the_message_list_alone() {
    let context = Context::push();
    let fixture = Fixture::start().watched(&context, |fixture| fixture.inbox);
    assert!(fixture.account.jmap().drop_connection());

    let transferred =
        fixture.transfer_straight(fixture.inbox, fixture.archive, &[fixture.uid()], true);

    assert!(!transferred.ok, "a disconnected folder transferred anyway");
    let emissions = emissions(&context);
    assert!(
        emissions.is_empty(),
        "a transfer that failed announced {emissions:?}"
    );
}

// ---------------------------------------------------------------------------
// a selection rather than a message

/// The ordinary drag: the user highlighted two messages and dropped both. Each
/// is its own `Email/set`, and both are written.
#[test]
fn every_message_of_a_selection_is_filed() {
    let fixture = Fixture::selection();

    fixture
        .transfer(&[fixture.uid(), fixture.second()], true)
        .expect_ok();

    let archived = BTreeMap::from([(fixture.archive_id.clone(), true)]);
    assert_eq!(fixture.mailboxes_on_server(fixture.uid()), archived);
    assert_eq!(fixture.mailboxes_on_server(fixture.second()), archived);
    assert!(
        listed(fixture.inbox).is_empty(),
        "a selection the user moved is still in the folder it left"
    );
}

/// The reason the walk is a request per uid rather than one `Email/set` for the
/// whole selection: a message another client destroyed must not take the rest of
/// the user's selection with it. One `Email/set` carrying every update would be
/// applied as a single state change, so a half-failed transfer would come back
/// as one failure with no way to say which messages moved — and a walk that
/// stopped at the first error would leave messages behind for a reason that says
/// nothing about them.
///
/// The transfer is still reported as failed, because one of the messages the
/// user asked about did not move.
#[test]
fn a_message_that_is_gone_does_not_hold_up_the_rest_of_the_selection() {
    let fixture = Fixture::selection();
    fixture.destroyed_elsewhere(fixture.uid());

    let transferred = fixture.transfer(&[fixture.uid(), fixture.second()], true);

    assert!(!transferred.ok, "a message that is gone was transferred");
    // SAFETY: a live GError, and the quark accessor takes no arguments.
    unsafe {
        assert_eq!((*transferred.error).domain, camel_folder_error_quark());
        assert_eq!(
            (*transferred.error).code,
            CAMEL_FOLDER_ERROR_INVALID_UID as i32
        );
    }
    assert_eq!(
        fixture.mailboxes_on_server(fixture.second()),
        BTreeMap::from([(fixture.archive_id.clone(), true)]),
        "the message behind the one that is gone was left where it was"
    );
}

/// And what the caller is told about a transfer that half worked. Camel's
/// out-parameter is an array with one slot per uid the caller passed, so the
/// answer for a message is read by its position: the uid it kept for the one
/// that landed — see [`the_uid_of_a_transferred_message_is_the_one_it_had`] for
/// why that is the uid it came in as — and NULL in the slot of the one that did
/// not. A shorter array, or one that closed the gap up, would tell the caller
/// the wrong message had moved.
#[test]
fn a_partial_transfer_reports_only_the_messages_that_landed() {
    let fixture = Fixture::selection();
    fixture.destroyed_elsewhere(fixture.uid());

    let transferred = fixture.transfer(&[fixture.uid(), fixture.second()], true);

    assert!(!transferred.ok, "a message that is gone was transferred");
    assert_eq!(
        transferred.reported(),
        vec![None, Some(fixture.second().to_string())]
    );
}

/// One emission for the whole selection, not one per message. Camel's change
/// info is four *lists* for exactly this reason: a message list redrawn once
/// for a drag of twenty messages is the point of it, and twenty emissions would
/// be twenty redraws of a list the user is looking at.
///
/// Both rows are on it although only one message moved — the other is not in
/// this mailbox because it is not anywhere, which is what
/// [`a_message_another_client_deleted_is_announced_as_its_row_goes`] asserts for
/// a selection of one. So this is also the case where the announcement and the
/// reported outcome deliberately disagree: the transfer failed, and both rows
/// still left the folder.
#[test]
fn the_rows_a_selection_left_behind_are_announced_together() {
    let context = Context::push();
    let fixture = Fixture::selection().watched(&context, |fixture| fixture.inbox);
    fixture.destroyed_elsewhere(fixture.uid());

    assert!(
        !fixture
            .transfer(&[fixture.uid(), fixture.second()], true)
            .ok,
        "a message that is gone was transferred"
    );

    assert_eq!(
        emissions(&context),
        vec![Emission {
            added: Vec::new(),
            removed: vec![fixture.uid().to_string(), fixture.second().to_string()],
            changed: Vec::new(),
            recent: Vec::new(),
        }]
    );
}
