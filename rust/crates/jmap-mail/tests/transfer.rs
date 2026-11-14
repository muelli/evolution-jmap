// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `transfer_messages_to_sync`: the vfunc behind dragging a message into
//! another folder.
//!
//! The previous increment built and tested the `Email/set` call for a copy
//! or a move, in `jmap-mail-sync`'s `Filing`. This file covers the Camel
//! half: two folders of one store, the uids Evolution passes in, and the
//! summary the source folder keeps afterward.
//!
//! ## What a JMAP move is, from inside one folder
//!
//! JMAP has no `Email/move` and no `Email/copy`. RFC 8621 §4.6 defines
//! `mailboxIds` as the set of mailboxes a message belongs to. A copy adds a
//! member to the set. A move adds one member and removes another. Either
//! way the message stays one object with one id. Two facts follow:
//!
//! - The uid does not change.
//!   [`the_uid_of_a_transferred_message_is_the_one_it_had`] checks this
//!   through the out-parameter Camel provides for it. IMAPX has no
//!   equivalent assertion, because its server mints a new uid in the
//!   destination.
//! - A move out of a folder is the same event a later listing of that
//!   folder would report as a removed message. The row leaves now, not
//!   later. A message list that still shows a message the user just moved
//!   away is wrong until the next refresh.
//!
//! ## An unsaved flag change
//!
//! Camel keeps the user's unsaved flag changes on the summary row.
//! `synchronize_sync` writes them to the server. A move that removed the
//! row would discard the change too. Marking a message read, then dragging
//! it to another folder before anything synchronizes, is an ordinary
//! sequence. Losing the flag in it would silently drop a change the user
//! made. [`a_move_settles_a_flag_the_user_had_not_saved_yet`] checks this.
//!
//! ## The message list already on screen
//!
//! Removing the row controls what the *next* listing of the source folder
//! shows. The folder's `changed` signal controls what the list already on
//! screen shows. The two are separate. Until this increment, the signal was
//! written the way IMAPX writes it, and never tested. This file tests it
//! through `common::signals`: push a [`Context`] before opening the
//! account, and pump before every read (see that module's header for why).
//!
//! Four things are asserted about the signal. Three of them are silences:
//!
//! - A move announces the rows it removed, and nothing else.
//! - A copy announces nothing, because it removed none.
//! - The *destination* also announces nothing. The message appears there
//!   only when that folder is next listed: a row built from a uid alone
//!   would show no subject, sender, or date.
//! - A transfer that failed without moving a row announces nothing. The
//!   exception is a message destroyed by another client: it leaves the
//!   folder because the server no longer has it.
//!
//! ## A selection, not only a message
//!
//! The sections above transfer one message. The vfunc actually receives a
//! *list*: whatever the user had selected when dragging. Three claims in
//! this file need more than one message:
//!
//! - **The walk sends one request per uid, and it finishes.** This is the
//!   module's own design: one request per message, not one per transfer. A
//!   message destroyed by another client must not block the rest of the
//!   selection.
//!   [`a_message_that_is_gone_does_not_hold_up_the_rest_of_the_selection`]
//!   checks that the second message still moves after the first one fails.
//! - **The out-parameter's slots match the input list by position.** Camel
//!   reads the result for a message by its position in the array. A
//!   partially failed transfer leaves NULL in the slot of the message that
//!   did not move, instead of shortening the array.
//!   [`a_partial_transfer_reports_only_the_messages_that_landed`] checks
//!   this.
//! - **One signal emission for the whole selection.** Camel's change info
//!   holds four *lists* so that moving twenty messages redraws the message
//!   list once.
//!   [`the_rows_a_selection_left_behind_are_announced_together`] checks
//!   this.

mod common;

use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;

use common::Account;
use common::array::StringArray;
use common::signals::{Context, Emission, emissions, watch};
use eds_sys::compat::{folder_dup_uids, folder_free_uids};
use eds_sys::{
    CAMEL_FOLDER_ERROR_INVALID_UID, CAMEL_MESSAGE_FOLDER_FLAGGED, CAMEL_MESSAGE_SEEN,
    CAMEL_STORE_FOLDER_NONE, CamelFolder, CamelFolderClass, CamelMessageInfo,
    camel_folder_error_quark, camel_folder_get_folder_summary, camel_folder_lock,
    camel_folder_refresh_info_sync, camel_folder_summary_get, camel_folder_synchronize_sync,
    camel_folder_transfer_messages_to_sync, camel_folder_unlock, camel_message_info_get_flags,
    camel_message_info_get_subject, camel_message_info_set_flags, camel_mime_message_new,
    camel_store_get_folder_sync,
};
use eds_sys::{CamelDataWrapper, camel_data_wrapper_construct_from_data_sync};
use glib_sys::{
    GError, GFALSE, GPtrArray, GTRUE, g_free, g_ptr_array_free, gboolean, gchar, gssize,
};
use gobject_sys::{g_object_unref, g_type_class_ref, g_type_class_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::folder::folder_type;
use jmap_mail_sync::MailSync;
use jmap_mock::{EmailSeed, MockServer, MockServerBuilder};
use jmap_proto::Id;
use jmap_proto::mail::role;
use jmap_tracing_capture::{capture, has};
use tracing::Level;

/// The messages the fixture can seed, in seeding order. A transfer of a
/// *message* uses the first. A transfer of a *selection* uses both.
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

/// One connected account with two folders open: an inbox holding the seeded
/// messages, and an archive holding none. Every transfer test starts from
/// this state.
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
    /// One message in the inbox. Most of this file tests a transfer of a
    /// single message.
    fn start() -> Self {
        Self::seeded(1)
    }

    /// Two messages, for the tests about a selection rather than a single
    /// message: the walk sends one request per uid, the out-parameter's
    /// slots match the input list by position, and the removed rows are
    /// announced together.
    fn selection() -> Self {
        Self::seeded(2)
    }

    fn seeded(messages: usize) -> Self {
        Self::seeded_with(messages, MockServer::builder())
    }

    /// [`Self::seeded`], with a caller-built server instead of the default
    /// one, for a test against a hostile or edge-case server.
    fn seeded_with(messages: usize, builder: MockServerBuilder) -> Self {
        let server = builder.start();
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
        // SAFETY: a live folder of ours, and an out-parameter that is
        // writable and currently NULL. Only the inbox is refreshed. The
        // archive stays unlisted, because that is the ordinary state of a
        // folder a message gets dragged into.
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

    /// The first seeded message. Tests transfer this one unless stated
    /// otherwise.
    fn uid(&self) -> &Id {
        &self.uids[0]
    }

    /// The second message, for a fixture built by [`Fixture::selection`].
    fn second(&self) -> &Id {
        &self.uids[1]
    }

    /// The same fixture, with one of its folders listened to from here on.
    ///
    /// This drains pending emissions first, then connects the handler.
    /// Otherwise the emission queued by the inbox's own refresh would reach a
    /// test that asks what the *transfer* announced. `context` must have been
    /// pushed before the fixture opened the account: see
    /// `common::signals::Context`.
    fn watched(self, context: &Context, folder: fn(&Self) -> *mut CamelFolder) -> Self {
        emissions(context);
        watch(folder(&self));
        self
    }

    /// Through Camel's own wrapper, the function Evolution calls when the
    /// user drags a message or picks "Move to Folder".
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
        let list = StringArray::of(uids);
        let mut transferred: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: two live folders of one store, an array of NUL-terminated
        // uids alive across the call, and two out-parameters that are
        // writable and currently NULL.
        let ok = unsafe {
            camel_folder_transfer_messages_to_sync(
                source,
                list.as_ptr(),
                destination,
                gboolean::from(delete_originals),
                &mut transferred,
                ptr::null_mut(),
                &mut error,
            )
        };
        Transferred::new(ok, transferred, error)
    }

    /// Appends a message to `folder` while the store has no connection, and
    /// gives back the synthetic uid of the row that waits for the upload.
    ///
    /// This goes through the pointer in the class for the reason
    /// [`Self::transfer_straight`] gives: Camel's own wrapper reconnects the
    /// store first, and the reconnection fails before the vfunc runs.
    fn append_offline(&self, folder: *mut CamelFolder) -> Id {
        let source = b"Subject: An offline draft\r\n\r\nBody.\r\n";
        let mut uid: *mut gchar = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a fresh message is a valid `CamelDataWrapper`, the buffer
        // is live for the call, referencing the class runs the class_init
        // that installs the vfunc, and both out-parameters are writable and
        // currently NULL.
        let appended = unsafe {
            let message = camel_mime_message_new();
            assert_ne!(
                camel_data_wrapper_construct_from_data_sync(
                    message.cast::<CamelDataWrapper>(),
                    source.as_ptr().cast(),
                    source.len() as gssize,
                    ptr::null_mut(),
                    &mut error,
                ),
                GFALSE,
                "the fixture message would not parse"
            );
            let class = g_type_class_ref(folder_type()).cast::<CamelFolderClass>();
            let vfunc = (*class)
                .append_message_sync
                .expect("the folder cannot append messages");
            let ok = vfunc(
                folder,
                message,
                ptr::null_mut(),
                &mut uid,
                ptr::null_mut(),
                &mut error,
            );
            g_type_class_unref(class.cast());
            g_object_unref(message.cast());
            ok
        };
        assert_ne!(appended, GFALSE, "the offline append failed");
        // SAFETY: a NUL-terminated string the vfunc gives to the caller.
        let text = unsafe { CStr::from_ptr(uid).to_string_lossy().into_owned() };
        // SAFETY: the same string, released the way Camel's own callers do.
        unsafe { g_free(uid.cast()) };
        Id::new(&text)
    }

    /// Through the pointer in the class, skipping the checks Camel's own
    /// wrapper makes before calling a provider. Camel itself answers a
    /// transfer into the message's own folder, and a transfer of no
    /// messages, without asking the provider.
    fn transfer_straight(
        &self,
        source: *mut CamelFolder,
        destination: *mut CamelFolder,
        uids: &[&Id],
        delete_originals: bool,
    ) -> Transferred {
        let list = StringArray::of(uids);
        let mut transferred: *mut GPtrArray = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: referencing the class runs the class_init that installs
        // the vfunc. Both folders are instances of that class, and the two
        // out-parameters are writable and currently NULL.
        let ok = unsafe {
            let class = g_type_class_ref(folder_type()).cast::<CamelFolderClass>();
            let vfunc = (*class)
                .transfer_messages_to_sync
                .expect("the folder cannot transfer messages");
            let ok = vfunc(
                source,
                list.as_ptr(),
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

    /// The mailboxes the server has the message in now.
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

    /// The keywords the server has for the message now.
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

    /// Destroys the message, as another client would.
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

    /// Changes the message's flags the way Evolution's message list does,
    /// without saving them.
    fn mark_read(&self) {
        let uid = CString::new(self.uid().as_str()).expect("a uid with no NUL");
        // SAFETY: a live folder with a summary, a NUL-terminated uid alive
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

    /// Installs a new connection, the way Camel reconnects a store after an
    /// outage. The store forgets its folder listing; the next request lists
    /// it again.
    fn reconnect(&self) {
        let client = Client::connect(self.server.origin(), Credentials::none()).expect("connected");
        self.account
            .connect(MailSync::new(client, self.server.account_id()));
    }

    /// Through Camel's own wrapper, the way Evolution's generic reconnect
    /// flush calls it on every open folder.
    fn synchronize(&self, folder: *mut CamelFolder) -> Synchronised {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live folder, and an out-parameter that is writable and
        // currently NULL.
        let ok =
            unsafe { camel_folder_synchronize_sync(folder, GFALSE, ptr::null_mut(), &mut error) };
        Synchronised::new(ok, error)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // SAFETY: the two references this fixture took from
        // `get_folder_sync`, released before the store they belong to.
        unsafe {
            g_object_unref(self.inbox.cast());
            g_object_unref(self.archive.cast());
        }
    }
}

/// A raw folder pointer, sent to a background thread.
///
/// Sound for the same reason as `transfer_race.rs`'s own copy: the folder is
/// a real `CamelFolder` GObject, kept alive by the fixture for as long as
/// the background thread using it runs.
#[derive(Clone, Copy)]
struct FolderPtr(*mut CamelFolder);
// SAFETY: the folder outlives every use of this wrapper, and every use goes
// through Camel's own thread-safe locking functions.
unsafe impl Send for FolderPtr {}

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

/// The uids Camel would draw a message list from, asked of the folder
/// rather than of its summary, the way Evolution asks.
fn listed(folder: *mut CamelFolder) -> Vec<String> {
    // SAFETY: a live folder. The array comes back owned and is freed with
    // the function Camel documents for it.
    unsafe {
        let array = folder_dup_uids(folder);
        let uids = uid_list(array);
        folder_free_uids(folder, array);
        uids
    }
}

/// The subject the folder's own summary has for `uid`, if it has a row.
fn subject_in(folder: *mut CamelFolder, uid: &Id) -> Option<String> {
    let key = CString::new(uid.as_str()).expect("a uid with no NUL");
    // SAFETY: a live folder of ours, and a key that outlives both calls.
    // `camel_folder_summary_get` gives a reference that this function
    // releases before it returns.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        let info = camel_folder_summary_get(summary, key.as_ptr());
        if info.is_null() {
            return None;
        }
        let subject = camel_message_info_get_subject(info);
        let text =
            (!subject.is_null()).then(|| CStr::from_ptr(subject).to_string_lossy().into_owned());
        g_object_unref(info.cast());
        text
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
    // SAFETY: the contract above. The strings live as long as the array.
    unsafe {
        (0..(*array).len)
            .map(|index| {
                let uid = *(*array).pdata.add(index as usize);
                CStr::from_ptr(uid.cast()).to_string_lossy().into_owned()
            })
            .collect()
    }
}

/// One transfer and all three of its results, owned the way Camel owns
/// them.
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
        // SAFETY: a live array this value owns, holding NUL-terminated
        // strings and NULLs.
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
        // SAFETY: both are references this value took from the call that
        // made it. The array is owned whole, but how a filled slot's string
        // is released depends on the installed EDS, the split
        // `jmap_mail::transfer::Reported`'s own doc names: before EDS 3.58,
        // Camel's own callers free the members with `g_free`, then the
        // array; from 3.58, the array carries its own `g_free` and one call
        // frees everything. The error is one allocation.
        unsafe {
            if !self.uids.is_null() {
                #[cfg(camel_folder_get_uids)]
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

/// One synchronisation, and both of its answers, owned the same way
/// [`Transferred`] owns its own.
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

    fn expect_ok(&self) {
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
        // SAFETY: an allocation this value owns from the call that made it.
        unsafe {
            if !self.error.is_null() {
                glib_sys::g_error_free(self.error);
            }
        }
    }
}

/// The copy half: the message gains a mailbox and keeps the one it was in.
/// In JMAP a mailbox is a member of a set, not a place.
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

/// The message stays in the folder it was copied out of. This is the
/// effect of the `delete_originals` flag, seen from this side.
#[test]
fn a_copied_message_stays_in_the_folder_it_came_from() {
    let fixture = Fixture::start();

    fixture.transfer(&[fixture.uid()], false).expect_ok();

    assert_eq!(listed(fixture.inbox), vec![fixture.uid().to_string()]);
}

/// The move half: one patch, and the mailbox it came out of is gone from
/// the set.
#[test]
fn a_message_the_user_moved_is_only_in_the_destination() {
    let fixture = Fixture::start();

    fixture.transfer(&[fixture.uid()], true).expect_ok();

    assert_eq!(
        fixture.mailboxes_on_server(fixture.uid()),
        BTreeMap::from([(fixture.archive_id.clone(), true)])
    );
}

/// The row goes with it. Waiting for the next listing to notice would leave
/// the message the user just moved sitting in the message list they moved it
/// out of, until the next refresh.
#[test]
fn a_moved_message_leaves_the_folder_it_came_from() {
    let fixture = Fixture::start();

    fixture.transfer(&[fixture.uid()], true).expect_ok();

    assert!(
        listed(fixture.inbox).is_empty(),
        "the moved message is still in the folder it left"
    );
}

/// `transfer_messages_to_sync` takes `camel_folder_lock` on the source
/// folder itself, because Camel's own wrapper does not (see this module's
/// header for why). A lock that a transfer takes and never releases would
/// block every later operation on that folder. This test checks that a lock
/// taken from another thread, right after a transfer, still returns.
#[test]
fn a_transfer_releases_the_source_folders_lock() {
    let fixture = Fixture::start();

    fixture.transfer(&[fixture.uid()], true).expect_ok();

    let inbox = FolderPtr(fixture.inbox);
    let (done_tx, done_rx) = channel();
    thread::spawn(move || {
        let inbox = inbox;
        // SAFETY: a live folder, locked and unlocked in a balanced pair.
        unsafe {
            camel_folder_lock(inbox.0);
            camel_folder_unlock(inbox.0);
        }
        let _ = done_tx.send(());
    });

    assert!(
        done_rx.recv_timeout(Duration::from_secs(2)).is_ok(),
        "camel_folder_lock on the source folder never returned after the transfer; \
         transfer_messages_to_sync left it locked"
    );
}

/// RFC 8621 §4.1 gives an `Email` one immutable id per account. Filing it
/// somewhere else does not make a second object, so the message in the
/// destination is the same message that was in the source, under the uid
/// the caller passed in. Camel's out-parameter reports exactly this.
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

/// The user marks a message read, then drags it into another folder before
/// anything synchronizes. The flag lives on the summary row until
/// `synchronize_sync` writes it. A move that dropped the row would drop the
/// change with it. The destination would never learn of the change either,
/// because what it lists is what the server holds.
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

/// A move into the folder the message is already in cannot be written down:
/// the same `mailboxIds` member would have to be both set and cleared. So
/// nothing is sent, and the row is not removed as though the message had
/// left. This test calls the vfunc through the class, because Camel's own
/// wrapper answers `source == destination` before asking any provider.
/// Succeeding with no connection proves no request was made.
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

/// A transfer of an empty selection asks the server nothing, the same way a
/// folder with nothing queued synchronizes without a server call. This test
/// calls the vfunc directly, bypassing the wrapper's own shortcut for an
/// empty selection, to check that the provider itself handles it too.
#[test]
fn a_transfer_of_no_messages_needs_no_server() {
    let fixture = Fixture::start();
    assert!(fixture.account.jmap().drop_connection());

    fixture
        .transfer_straight(fixture.inbox, fixture.archive, &[], true)
        .expect_ok();
}

/// ROADMAP item 176(b): a move or copy made with no connection must not fail
/// outright. `file_message` queues the filing on the row and reports success,
/// the same way an offline flag change or an offline Expunge already
/// succeeds locally, and the row stays in this folder until
/// `synchronize_sync`'s reconnect walk actually lands it (see
/// [`a_queued_move_lands_once_the_store_reconnects`]).
#[test]
fn a_move_with_no_connection_is_queued_and_the_row_stays() {
    let fixture = Fixture::start();
    assert!(fixture.account.jmap().drop_connection());

    let transferred =
        fixture.transfer_straight(fixture.inbox, fixture.archive, &[fixture.uid()], true);

    transferred.expect_ok();
    assert_eq!(
        transferred.reported(),
        vec![Some(fixture.uid().to_string())],
        "a queued filing must be reported as landed, the same as an \
         immediate one, so the caller sees the move as done"
    );
    assert_eq!(
        listed(fixture.inbox),
        vec![fixture.uid().to_string()],
        "the row must stay in the folder while offline: removal happens \
         only after a server write actually confirms the move"
    );
}

/// The queued filing reaches the server once the store reconnects, with no
/// explicit `Email/set` call from the client beyond the generic
/// `CamelOfflineStore` reconnect flush: the same flush
/// `expunge.rs`'s own offline test already proves exists for a deleted mark.
#[test]
fn a_queued_move_lands_once_the_store_reconnects() {
    let fixture = Fixture::start();
    assert!(fixture.account.jmap().drop_connection());
    fixture
        .transfer_straight(fixture.inbox, fixture.archive, &[fixture.uid()], true)
        .expect_ok();

    fixture.reconnect();
    fixture.synchronize(fixture.inbox).expect_ok();

    assert_eq!(
        fixture.mailboxes_on_server(fixture.uid()),
        BTreeMap::from([(fixture.archive_id.clone(), true)]),
        "the queued move never reached the server after reconnecting"
    );
    assert!(
        listed(fixture.inbox).is_empty(),
        "the row stayed in the source folder after the queued move finally landed"
    );
}

/// ROADMAP item 181(b): a transport failure (a DNS failure, a refused
/// connection, a TLS failure) is `StoreError::Client`, not
/// `StoreError::Disconnected`. `push_row`, run just before `replay_filing` in
/// the same walk, already cleared the row's `CAMEL_MESSAGE_FOLDER_FLAGGED`
/// mark (this filing changes no keyword, so `push_row` sends no request of
/// its own and settles the row on `Ok(())`). Unlike a `Disconnected` answer,
/// nothing restored the mark for this other kind of failure, so the filing
/// fell off `synchronize_sync`'s work list and the real server coming back
/// later never saw it again (the roadmap's own report: "after the failed DNS
/// replay and a restart, the queued move and copy never did [land]").
///
/// This checks the mark directly, not through a second `synchronize_sync`
/// call: Camel's own `dirty` bit on the row (separate from
/// `CAMEL_MESSAGE_FOLDER_FLAGGED`, and not `replay_filing`'s to restore)
/// also keeps a uid on the work list, purely because `push_row`'s own write
/// above changed the row at all, and stays set for as long as this row's
/// `CamelMessageInfo` is loaded in memory. The roadmap's own reproduction
/// needed a restart to lose that transient bit; this test does not restart,
/// so it reads the one mark `replay_filing` actually owns instead.
///
/// `doomed` stands in for the real server's address while it cannot be
/// reached: `account` connects to it successfully (so this looks like an
/// ordinary reconnect, not `Disconnected`), and then `doomed` stops
/// listening, so the next request against it fails at the transport level.
#[test]
fn a_transport_failure_restores_the_filing_mark() {
    let fixture = Fixture::start();
    assert!(fixture.account.jmap().drop_connection());
    fixture
        .transfer_straight(fixture.inbox, fixture.archive, &[fixture.uid()], true)
        .expect_ok();

    let doomed = MockServer::builder().start();
    let client = Client::connect(doomed.origin(), Credentials::none()).expect("connected");
    fixture
        .account
        .connect(MailSync::new(client, fixture.server.account_id()));
    drop(doomed);

    let synced = fixture.synchronize(fixture.inbox);
    assert!(
        !synced.ok,
        "a transport failure must be reported, not swallowed"
    );

    let uid = CString::new(fixture.uid().as_str()).expect("a uid with no NUL");
    // SAFETY: a live folder with a summary, a NUL-terminated uid alive
    // across the call, and one reference to the row, released here.
    unsafe {
        let summary = camel_folder_get_folder_summary(fixture.inbox);
        let info: *mut CamelMessageInfo = camel_folder_summary_get(summary, uid.as_ptr());
        assert!(!info.is_null(), "the refresh left no row for the message");
        assert_ne!(
            camel_message_info_get_flags(info) & CAMEL_MESSAGE_FOLDER_FLAGGED,
            0,
            "a transport failure must leave the row marked for retry, the \
             same as a Disconnected answer does, or synchronize_sync's own \
             work list drops this filing for good"
        );
        g_object_unref(info.cast());
    }
}

/// A queued copy behaves the same way as a queued move, except that it never
/// removes the row: a copy leaves every mailbox the message was already in.
#[test]
fn a_queued_copy_lands_once_the_store_reconnects() {
    let fixture = Fixture::start();
    assert!(fixture.account.jmap().drop_connection());
    fixture
        .transfer_straight(fixture.inbox, fixture.archive, &[fixture.uid()], false)
        .expect_ok();

    fixture.reconnect();
    fixture.synchronize(fixture.inbox).expect_ok();

    assert_eq!(
        fixture.mailboxes_on_server(fixture.uid()),
        BTreeMap::from([
            (fixture.inbox_id.clone(), true),
            (fixture.archive_id.clone(), true)
        ]),
        "the queued copy never reached the server after reconnecting"
    );
    assert_eq!(
        listed(fixture.inbox),
        vec![fixture.uid().to_string()],
        "a copy must never remove the row it came from"
    );
}

/// ROADMAP item 181(a): a queued move or copy must show in the destination
/// at once, the way the offline journal of IMAPX shows one. Until the
/// filing lands, the server knows nothing about it, so only a local row
/// can show it. The row is a clone of the source row, so it has the
/// subject, sender and date, not only a uid.
#[test]
fn a_queued_move_shows_the_row_in_the_destination() {
    let fixture = Fixture::start();
    assert!(fixture.account.jmap().drop_connection());

    fixture
        .transfer_straight(fixture.inbox, fixture.archive, &[fixture.uid()], true)
        .expect_ok();

    assert_eq!(
        listed(fixture.archive),
        vec![fixture.uid().to_string()],
        "a queued move must show the message in the destination at once"
    );
    assert_eq!(
        subject_in(fixture.archive, fixture.uid()).as_deref(),
        Some(MESSAGES[0].2),
        "the destination row must be a full clone, not a bare uid"
    );
}

/// A queued copy shows in the destination for the same reason, and also
/// stays in the folder it came from.
#[test]
fn a_queued_copy_shows_the_row_in_the_destination() {
    let fixture = Fixture::start();
    assert!(fixture.account.jmap().drop_connection());

    fixture
        .transfer_straight(fixture.inbox, fixture.archive, &[fixture.uid()], false)
        .expect_ok();

    assert_eq!(
        listed(fixture.archive),
        vec![fixture.uid().to_string()],
        "a queued copy must show the message in the destination at once"
    );
    assert_eq!(
        listed(fixture.inbox),
        vec![fixture.uid().to_string()],
        "a copy must never remove the row it came from"
    );
}

/// The destination row is also the record that makes the filing land when
/// the source folder is not walked.
///
/// `camel_store_synchronize_sync` walks the open folders of the store
/// (`camel-store.c`, `store_synchronize_sync`), and a restart leaves that
/// list empty. Thus a reconnect flush can reach the destination folder and
/// not the source folder. The destination row carries a filing of its own,
/// shaped as a copy into the destination, so that walk alone puts the
/// message where the user put it. The source row keeps the move filing and
/// removes the message from the source mailbox when its own folder is
/// walked. Both requests are idempotent, so the order does not matter.
#[test]
fn a_queued_filing_lands_from_the_destination_folder_alone() {
    let fixture = Fixture::start();
    assert!(fixture.account.jmap().drop_connection());
    fixture
        .transfer_straight(fixture.inbox, fixture.archive, &[fixture.uid()], true)
        .expect_ok();

    fixture.reconnect();
    fixture.synchronize(fixture.archive).expect_ok();

    assert_eq!(
        fixture
            .mailboxes_on_server(fixture.uid())
            .get(&fixture.archive_id),
        Some(&true),
        "a walk of the destination folder alone must put the message there"
    );
}

/// A message that waits for its own upload has no id on the server, so a
/// clone of its row in a second folder would name nothing there and would
/// ask for a second upload. Such a row stays in the source folder only.
#[test]
fn a_queued_filing_of_an_unsent_append_adds_no_destination_row() {
    let fixture = Fixture::start();
    assert!(fixture.account.jmap().drop_connection());
    let appended = fixture.append_offline(fixture.inbox);

    fixture
        .transfer_straight(fixture.inbox, fixture.archive, &[&appended], true)
        .expect_ok();

    assert!(
        listed(fixture.archive).is_empty(),
        "a message with no id on the server must not get a destination row"
    );
}

/// ROADMAP item 182(c): the offline mail queue traces its own uid so a GUI
/// scenario can assert "queued", "replayed" and "dropped" without reading
/// server state.
#[test]
fn queuing_a_filing_offline_traces_the_uid() {
    let fixture = Fixture::start();
    assert!(fixture.account.jmap().drop_connection());

    let (transferred, logs) = capture(|| {
        fixture.transfer_straight(fixture.inbox, fixture.archive, &[fixture.uid()], true)
    });

    transferred.expect_ok();
    assert!(
        has(&logs, Level::TRACE, "message", "queued filing"),
        "expected a queued-filing trace event, got {logs:?}"
    );
    assert!(
        has(&logs, Level::TRACE, "uid", fixture.uid().as_str()),
        "expected the queued uid as a field, got {logs:?}"
    );
}

/// The reconnect walk's `replay_filing` traces the uid it actually sent, not
/// only the one it queued: a test or a support report can tell the two
/// events apart.
#[test]
fn a_queued_filing_landing_on_reconnect_traces_the_uid() {
    let fixture = Fixture::start();
    assert!(fixture.account.jmap().drop_connection());
    fixture
        .transfer_straight(fixture.inbox, fixture.archive, &[fixture.uid()], true)
        .expect_ok();
    fixture.reconnect();

    let (synced, logs) = capture(|| fixture.synchronize(fixture.inbox));

    synced.expect_ok();
    assert!(
        has(&logs, Level::TRACE, "message", "replayed filing"),
        "expected a replayed-filing trace event, got {logs:?}"
    );
    assert!(
        has(&logs, Level::TRACE, "uid", fixture.uid().as_str()),
        "expected the replayed uid as a field, got {logs:?}"
    );
}

/// Another client can destroy the message between the offline queue and the
/// reconnect walk. `replay_filing` then has nothing left to send: it clears
/// the pending filing instead of retrying it forever, and traces that this
/// uid's filing was dropped, not replayed.
#[test]
fn a_queued_filing_whose_message_is_gone_traces_dropped() {
    let fixture = Fixture::start();
    assert!(fixture.account.jmap().drop_connection());
    fixture
        .transfer_straight(fixture.inbox, fixture.archive, &[fixture.uid()], true)
        .expect_ok();
    fixture.destroyed_elsewhere(fixture.uid());
    fixture.reconnect();

    let (_synced, logs) = capture(|| fixture.synchronize(fixture.inbox));

    assert!(
        has(&logs, Level::TRACE, "message", "dropped filing"),
        "expected a dropped-filing trace event, got {logs:?}"
    );
    assert!(
        has(&logs, Level::TRACE, "uid", fixture.uid().as_str()),
        "expected the dropped uid as a field, got {logs:?}"
    );
    assert!(
        !has(&logs, Level::TRACE, "message", "replayed filing"),
        "a dropped filing must not also report as replayed, got {logs:?}"
    );
}

/// A uid in a summary is a claim about the last listing. Another client
/// destroying the message since then is ordinary. A flag write is a
/// consequence the user never asked for, so it is settled in silence. A
/// transfer is something the user did, so it is reported, as `INVALID_UID`.
/// Evolution reads this code as "that message is gone", not as a reason to
/// take the account offline. The row still goes, because the message is not
/// in this folder either.
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

/// A hostile or merely buggy server can answer `notFound` for an id that it
/// still lists everywhere else (SECURITY-AUDIT-2026-11-01 row S3-3). This
/// code cannot tell that apart from the genuine deletion race the test
/// above covers, so it removes the row the same way, for a copy as much as
/// for a move: a failed `Email/set` never touches the server's own
/// `mailboxIds` for the message, so the server's data stays correct.
///
/// This does not "self heal" on a later refresh, unlike the audit row's own
/// first guess. `refresh_info_sync` asks `Email/changes` since the state the
/// folder already recorded; the failed `Email/set` never moved that state,
/// so the answer has nothing new to say, and the row stays gone until
/// something else invalidates the folder's recorded state (`refresh.rs`'s
/// own module doc: "a folder that has none has never listed this mailbox,
/// and lists"). A plain reconnect does not do that either: the state lives
/// in the folder's own summary, not the connection.
#[test]
fn a_notfound_copy_from_a_server_that_still_lists_the_message_stays_gone() {
    let fixture = Fixture::seeded_with(1, MockServer::builder().email_update_always_not_found());
    let before = fixture.mailboxes_on_server(fixture.uid());

    assert!(
        !fixture.transfer(&[fixture.uid()], false).ok,
        "a copy the server answered notFound for was reported as transferred"
    );
    assert!(
        listed(fixture.inbox).is_empty(),
        "the folder still lists a message the server answered notFound for"
    );
    assert_eq!(
        fixture.mailboxes_on_server(fixture.uid()),
        before,
        "a failed Email/set must not have changed the server's own mailboxIds"
    );

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder of ours, and an out-parameter that is writable
    // and currently NULL.
    unsafe {
        assert_ne!(
            camel_folder_refresh_info_sync(fixture.inbox, ptr::null_mut(), &mut error),
            GFALSE,
            "the inbox would not refresh"
        );
    }
    assert!(
        listed(fixture.inbox).is_empty(),
        "the ordinary incremental refresh, Email/changes since a state the \
         failed Email/set never moved, has nothing to report, so it does \
         NOT bring the row back; see this test's module-level note"
    );
}

/// Taking the row away settles what the next listing of the folder would
/// show. This emission tells the list already on screen the same thing. A
/// move is the only kind of transfer with anything to announce about the
/// folder it left.
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
/// Announcing anyway would redraw the message list the user is reading, and
/// lose their place in it, for a folder where nothing happened.
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

/// The folder the message arrived in says nothing either. This is the
/// visible half of this file's decision not to build a row there: the
/// message appears in the destination when that folder is next listed,
/// because the listing is the only thing that knows what its row should
/// say.
///
/// This test watches the archive, not the inbox. The recorder tracks one
/// list for whatever folder it is connected to, so watching both folders at
/// once would not show which folder emitted.
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

/// A message another client destroyed is the one failure that still changes
/// the folder: it is not in this mailbox because it is not anywhere, so the
/// row goes and the list on screen is told. The transfer is still reported
/// as failed. See
/// [`a_message_another_client_deleted_is_reported_and_leaves_the_folder`] for
/// that half.
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

/// A queued offline filing does not announce the row as removed, because it
/// leaves the row where it is (see
/// [`a_move_with_no_connection_is_queued_and_the_row_stays`]): only
/// `replay_filing`'s later, successful write removes it and announces that. A
/// folder that announced the message as removed before the server actually
/// agreed would take it off the user's screen too soon. Camel still announces
/// the row itself as *changed*: marking it for the reconnect walk to find is a
/// real mutation of the row (`CAMEL_MESSAGE_FOLDER_FLAGGED` goes from unset to
/// set), the same as any other summary property change.
#[test]
fn a_queued_transfer_does_not_announce_the_row_as_removed() {
    let context = Context::push();
    let fixture = Fixture::start().watched(&context, |fixture| fixture.inbox);
    assert!(fixture.account.jmap().drop_connection());

    let transferred =
        fixture.transfer_straight(fixture.inbox, fixture.archive, &[fixture.uid()], true);

    transferred.expect_ok();
    assert_eq!(
        emissions(&context),
        vec![Emission {
            added: Vec::new(),
            removed: Vec::new(),
            changed: vec![fixture.uid().to_string()],
            recent: Vec::new(),
        }],
        "a queued transfer must not announce the row as removed before the \
         server agreed"
    );
}

// ---------------------------------------------------------------------------
// a selection rather than a message

/// The ordinary drag: the user highlighted two messages and dropped both.
/// Each is its own `Email/set` call, and both are written.
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

/// The walk sends a request per uid, not one `Email/set` for the whole
/// selection, because a message another client destroyed must not block the
/// rest of the user's selection. One `Email/set` call carrying every update
/// would apply as a single state change: a half-failed transfer would come
/// back as one failure with no way to say which messages moved. A walk that
/// stopped at the first error would also leave later messages unmoved for a
/// reason unrelated to them.
///
/// The transfer is still reported as failed, because one of the messages
/// the user asked about did not move.
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

/// What the caller is told about a transfer that half worked. Camel's
/// out-parameter is an array with one slot per uid the caller passed. The
/// answer for a message is read by its position: the uid for the one that
/// landed (see [`the_uid_of_a_transferred_message_is_the_one_it_had`] for
/// why it is the same uid it came in as), and NULL for the one that did
/// not. A shorter array, or one with the gap closed up, would tell the
/// caller the wrong message had moved.
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

/// One emission for the whole selection, not one per message. Camel's
/// change info is four *lists* for exactly this reason: redrawing the
/// message list once for a drag of twenty messages, rather than twenty
/// times, is the point of it.
///
/// Both rows are on the emission although only one message moved. The other
/// is not in this mailbox because it is not anywhere, the same case
/// [`a_message_another_client_deleted_is_announced_as_its_row_goes`] checks
/// for a selection of one. So this test also shows the emission and the
/// reported outcome deliberately disagree: the transfer failed, but both
/// rows still left the folder.
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

/// ROADMAP item 179(d): a mailbox
/// [`jmap_mail_sync::FolderTree::merge_shared`] merges in from a shared
/// account is a mailbox of a *different* JMAP account, and `Email/set` is a
/// method call under one `accountId`. A transfer between two such mailboxes
/// of the same shared account must reach that account, not the account this
/// store otherwise addresses.
#[test]
fn transferring_between_two_shared_mailboxes_lands_in_their_owning_account() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();

    let shared_account_id = Id::new("agent-shared-owner");
    let (shared_archive_id, shared_uid) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let sharer = state.add_account(shared_account_id.clone(), "Bob");
        sharer.is_personal = false;
        let inbox = sharer.seed_mailbox("Inbox", None);
        let archive = sharer.seed_mailbox("Archive", None);
        let uid = sharer.seed_email(EmailSeed::new(
            inbox,
            ("Bob", "bob@example.com"),
            "Lunch?",
            "One o'clock.",
            "2026-01-15T09:30:00Z",
        ));
        (archive, uid)
    };

    let account = Account::open();
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    account.connect(MailSync::new(client, account_id.clone()));

    let source = open(&account, "Shared/Bob/Inbox");
    let destination = open(&account, "Shared/Bob/Archive");

    let list = StringArray::of(&[&shared_uid]);
    let mut transferred: *mut GPtrArray = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: two live folders of one store, an array of NUL-terminated uids
    // alive across the call, and two out-parameters that are writable and
    // currently NULL.
    let ok = unsafe {
        camel_folder_transfer_messages_to_sync(
            source,
            list.as_ptr(),
            destination,
            GTRUE,
            &mut transferred,
            ptr::null_mut(),
            &mut error,
        )
    };
    Transferred::new(ok, transferred, error).expect_ok();

    let state = server.state();
    let state = state.lock().unwrap();
    assert!(
        state.account(&account_id).unwrap().emails.is_empty(),
        "the message must not change in this store's own account"
    );
    assert_eq!(
        state
            .account(&shared_account_id)
            .unwrap()
            .emails
            .get(&shared_uid)
            .unwrap()
            .mailbox_ids
            .clone()
            .unwrap_or_default()
            .into_keys()
            .collect::<Vec<_>>(),
        vec![shared_archive_id],
        "the message must move in the shared mailbox's own account"
    );

    // SAFETY: the two references this test took from `get_folder_sync`.
    unsafe {
        g_object_unref(source.cast());
        g_object_unref(destination.cast());
    }
}
