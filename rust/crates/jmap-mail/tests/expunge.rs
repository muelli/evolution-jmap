// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `expunge_sync`: the vfunc that carries out the user's deletions.
//!
//! Until now, nothing has acted on `CAMEL_MESSAGE_DELETED`. This flag belongs
//! to Camel: `jmap-mail-sync`'s `MessageFlags` has always noted that JMAP has
//! no deleted keyword. Pressing Delete in Evolution marks a row and reaches no
//! server. [`crate::synchronize`] deliberately produces no keyword change for
//! it. This vfunc is where the mark is finally read.
//!
//! ## What "expunge" means here
//!
//! Camel's vfunc asks a *folder* to get rid of its messages marked deleted.
//! `jmap-mail-sync`'s `expunge_message` decides between the two writes this
//! can mean: destroy, for a message whose last mailbox is this one, or
//! `mailboxIds/<this>: null`, for a message the user also filed elsewhere.
//! This file decides everything around that choice: which rows go on the work
//! list, what happens to a row whose message is gone, and what the folder
//! announces.
//!
//! ## The rows go now, not at the next listing
//!
//! This follows the same rule [`crate::transfer`] applies to a move, for the
//! same reason. A refresh would reach the same result, but "the next refresh"
//! runs on a timer. Until it fires, the message list would keep offering a
//! message that is gone. So a row whose message left is removed and announced
//! at once, and the announcement redraws a window that is already open.
//!
//! ## Two ways in
//!
//! `camel_folder_expunge_sync` is one way in: Evolution's "Expunge" and
//! "Empty Trash" commands call it. The other is
//! `camel_folder_synchronize_sync` with its `expunge` argument set, which
//! Evolution calls when a folder closes and the account is configured to
//! expunge on exit. Both end in the same walk.
//! [`synchronising_with_expunge_gets_rid_of_the_deleted_rows_too`] tests the
//! second path. Before this increment, that argument was ignored: honest while
//! nothing could act on it, but a silent no-op now.

mod common;

use std::collections::BTreeMap;
use std::ffi::CString;
use std::ptr;

use common::Account;
use common::signals::{Context, emissions, watch};
use eds_sys::{
    CAMEL_MESSAGE_DELETED, CamelFolder, camel_folder_expunge_sync, camel_folder_get_folder_summary,
    camel_folder_get_message_count, camel_folder_refresh_info_sync, camel_folder_summary_get,
    camel_folder_synchronize_sync, camel_message_info_set_flags,
};
use glib_sys::{GError, GFALSE};
use gobject_sys::g_object_unref;
use jmap_client::{Client, Credentials};
use jmap_mail_sync::MailSync;
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::role;

/// The two messages the fixture seeds into the inbox, in order.
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

/// One connected account with a listed inbox and an unopened archive. Every
/// expunge test in this file starts from this state.
struct Fixture {
    server: MockServer,
    /// Held but not read. The folder below borrows its store from this
    /// account. A dropped account leaves the folder pointing at nothing.
    _account: Account,
    inbox: *mut CamelFolder,
    archive_id: Id,
    uids: Vec<Id>,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let (archive_id, uids) = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            let inbox_id = account.seed_mailbox("Inbox", Some(role::INBOX));
            let archive_id = account.seed_mailbox("Archive", None);
            let uids = MESSAGES
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
            (archive_id, uids)
        };

        let account = Account::open();
        let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
        account.connect(MailSync::new(client, account_id));

        let inbox = open(&account, "Inbox");
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live folder of ours, and an out-parameter that is writable
        // and currently NULL.
        unsafe {
            assert_ne!(
                camel_folder_refresh_info_sync(inbox, ptr::null_mut(), &mut error),
                GFALSE,
                "the inbox would not refresh"
            );
        }

        Self {
            server,
            _account: account,
            inbox,
            archive_id,
            uids,
        }
    }

    /// The same fixture, with the inbox watched from here on. This first
    /// pumps away the setup's own emission, for the reason `common::signals`
    /// gives.
    fn watched(self, context: &Context) -> Self {
        emissions(context);
        watch(self.inbox);
        self
    }

    fn uid(&self) -> &Id {
        &self.uids[0]
    }

    fn second(&self) -> &Id {
        &self.uids[1]
    }

    /// Marks a row the way Evolution's Delete key does.
    fn mark_deleted(&self, uid: &Id) {
        let uid = CString::new(uid.as_str()).unwrap();
        // SAFETY: a live summary of a listed folder, a NUL-terminated uid it
        // holds a row for, and a reference this releases.
        unsafe {
            let summary = camel_folder_get_folder_summary(self.inbox);
            let info = camel_folder_summary_get(summary, uid.as_ptr());
            assert!(!info.is_null(), "the summary has no row for {uid:?}");
            camel_message_info_set_flags(info, CAMEL_MESSAGE_DELETED, CAMEL_MESSAGE_DELETED);
            g_object_unref(info.cast());
        }
    }

    /// Calls the vfunc through Camel's own wrapper. Evolution's "Expunge"
    /// command calls this wrapper.
    fn expunge(&self) -> Expunged {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live folder of ours and an out-parameter that is writable
        // and currently NULL.
        let ok = unsafe { camel_folder_expunge_sync(self.inbox, ptr::null_mut(), &mut error) };
        Expunged::new(ok, error)
    }

    /// The other way in: a synchronisation asked to expunge.
    fn synchronize(&self, expunge: bool) -> Expunged {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: as above.
        let ok = unsafe {
            camel_folder_synchronize_sync(
                self.inbox,
                glib_sys::gboolean::from(expunge),
                ptr::null_mut(),
                &mut error,
            )
        };
        Expunged::new(ok, error)
    }

    /// How many rows the folder is left offering.
    fn rows(&self) -> u32 {
        // SAFETY: a live folder with a summary on it.
        unsafe { camel_folder_get_message_count(self.inbox) as u32 }
    }

    /// Whether the account still holds the message at all.
    fn holds(&self, uid: &Id) -> bool {
        let account_id = self.server.account_id();
        let state = self.server.state();
        let state = state.lock().unwrap();
        state
            .account(&account_id)
            .unwrap()
            .emails
            .get(uid)
            .is_some()
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

    /// Files a message into a second mailbox behind the folder's back, the way
    /// another client would.
    fn also_file_into_archive(&self, uid: &Id) {
        let account_id = self.server.account_id();
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let mut email = account.emails.get(uid).expect("the seeded message").clone();
        let mut mailboxes = email.mailbox_ids.take().unwrap_or_default();
        mailboxes.insert(self.archive_id.clone(), true);
        email.mailbox_ids = Some(mailboxes);
        account
            .emails
            .transaction(|transaction| transaction.update(uid, email));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // SAFETY: the reference `camel_store_get_folder_sync` handed over.
        unsafe { g_object_unref(self.inbox.cast()) };
    }
}

/// What one call to the vfunc answered.
struct Expunged {
    ok: bool,
    error: *mut GError,
}

impl Expunged {
    fn new(ok: glib_sys::gboolean, error: *mut GError) -> Self {
        Self {
            ok: ok != GFALSE,
            error,
        }
    }

    fn expect_ok(&self) {
        assert!(self.ok, "the expunge failed: {}", self.message());
        assert!(self.error.is_null(), "a success left an error behind");
    }

    fn message(&self) -> String {
        if self.error.is_null() {
            return "(no error was set)".to_owned();
        }
        // SAFETY: a GError this owns; the message is a string it owns.
        unsafe { std::ffi::CStr::from_ptr((*self.error).message) }
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for Expunged {
    fn drop(&mut self) {
        if !self.error.is_null() {
            // SAFETY: the out-parameter's GError, owned here.
            unsafe { glib_sys::g_error_free(self.error) };
        }
    }
}

fn open(account: &Account, path: &str) -> *mut CamelFolder {
    let path = CString::new(path).unwrap();
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live store of ours, a NUL-terminated path it lists, and an
    // out-parameter that is writable and currently NULL.
    let folder = unsafe {
        eds_sys::camel_store_get_folder_sync(
            account.store,
            path.as_ptr(),
            0,
            ptr::null_mut(),
            &mut error,
        )
    };
    assert!(!folder.is_null(), "the folder would not open");
    folder
}

#[test]
fn a_message_this_mailbox_is_the_last_home_of_is_destroyed() {
    // The ordinary delete-then-expunge path. The mark Camel puts on the row
    // becomes an `Email/set` destroy, because the inbox is the only mailbox
    // naming the message.
    let fixture = Fixture::start();
    fixture.mark_deleted(fixture.uid());

    fixture.expunge().expect_ok();

    assert!(
        !fixture.holds(fixture.uid()),
        "the expunged message should be gone from the account"
    );
    assert!(
        fixture.holds(fixture.second()),
        "a message nobody deleted must survive the expunge beside it"
    );
}

#[test]
fn an_expunged_row_leaves_the_folder_at_once() {
    // Not at the next listing. The message list the user is viewing was drawn
    // from these rows. A list that keeps offering a message the user just
    // destroyed is wrong until the refresh timer fires.
    let fixture = Fixture::start();
    fixture.mark_deleted(fixture.uid());

    fixture.expunge().expect_ok();

    assert_eq!(
        fixture.rows(),
        1,
        "the folder should be left holding only the message that was not deleted"
    );
}

#[test]
fn an_expunge_announces_the_rows_it_took_away() {
    // The expunge announces the removed rows once, in one emission. This is
    // what redraws a window already open. The row count alone only affects
    // what the next listing would draw from.
    let context = Context::push();
    let fixture = Fixture::start().watched(&context);
    fixture.mark_deleted(fixture.uid());
    fixture.mark_deleted(fixture.second());

    fixture.expunge().expect_ok();

    let announced = emissions(&context);
    assert_eq!(announced.len(), 1, "one emission for the whole expunge");
    let mut removed = announced[0].removed.clone();
    removed.sort();
    let mut expected = vec![
        fixture.uid().as_str().to_owned(),
        fixture.second().as_str().to_owned(),
    ];
    expected.sort();
    assert_eq!(removed, expected);
    assert!(announced[0].added.is_empty());
    assert!(announced[0].changed.is_empty());
}

#[test]
fn an_expunge_with_nothing_marked_deleted_announces_nothing() {
    // Makes no request either. Evolution synchronises a folder every time it
    // closes one, so the common case must be free.
    let context = Context::push();
    let fixture = Fixture::start().watched(&context);

    fixture.expunge().expect_ok();

    assert!(emissions(&context).is_empty());
    assert_eq!(fixture.rows(), 2);
    assert!(fixture.holds(fixture.uid()));
}

#[test]
fn a_message_filed_elsewhere_too_only_leaves_this_mailbox() {
    // The decision `jmap-mail-sync` makes, seen from the folder. The user's
    // copy in another folder survives an expunge of the inbox. The row still
    // goes, because the message has left *this* mailbox.
    let fixture = Fixture::start();
    fixture.also_file_into_archive(fixture.uid());
    fixture.mark_deleted(fixture.uid());

    fixture.expunge().expect_ok();

    assert!(
        fixture.holds(fixture.uid()),
        "a message the user also filed elsewhere must survive"
    );
    assert_eq!(
        fixture
            .mailboxes_on_server(fixture.uid())
            .keys()
            .collect::<Vec<_>>(),
        vec![&fixture.archive_id],
        "only the expunged mailbox should have been taken off the message"
    );
    assert_eq!(fixture.rows(), 1, "the row leaves this folder either way");
}

#[test]
fn a_message_another_client_destroyed_takes_its_row_with_it() {
    // Not a failure. A uid in a summary is a claim about the last listing. A
    // message that is already gone is a message the expunge wanted gone. An
    // error here would alert the user about a message that does not exist.
    let fixture = Fixture::start();
    fixture.mark_deleted(fixture.uid());
    {
        let account_id = fixture.server.account_id();
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account
            .emails
            .transaction(|transaction| assert!(transaction.destroy(fixture.uid())));
    }

    fixture.expunge().expect_ok();

    assert_eq!(
        fixture.rows(),
        1,
        "the row of a message that is already gone should go too"
    );
}

#[test]
fn a_row_nobody_deleted_is_left_where_it_is() {
    // The deleted flag is the whole work list. An expunge must leave alone a
    // folder full of mail the user read but did not delete.
    let fixture = Fixture::start();
    fixture.mark_deleted(fixture.second());

    fixture.expunge().expect_ok();

    assert!(fixture.holds(fixture.uid()));
    assert!(!fixture.holds(fixture.second()));
    assert_eq!(fixture.rows(), 1);
}

#[test]
fn synchronising_with_expunge_gets_rid_of_the_deleted_rows_too() {
    // The second way in. Evolution calls `synchronize_sync` with this argument
    // set when a folder closes and the account expunges on exit. Before this
    // increment the argument was ignored, honest while nothing acted on it.
    let fixture = Fixture::start();
    fixture.mark_deleted(fixture.uid());

    fixture.synchronize(true).expect_ok();

    assert!(!fixture.holds(fixture.uid()));
    assert_eq!(fixture.rows(), 1);
}

#[test]
fn synchronising_without_expunge_leaves_the_deleted_rows_alone() {
    // The argument is read, not assumed. A synchronisation not asked to
    // expunge must leave the mark exactly where it was, because the user can
    // still undelete the message.
    let fixture = Fixture::start();
    fixture.mark_deleted(fixture.uid());

    fixture.synchronize(false).expect_ok();

    assert!(
        fixture.holds(fixture.uid()),
        "a plain synchronisation must not destroy anything"
    );
    assert_eq!(fixture.rows(), 2);
}

#[test]
fn expunging_a_folder_of_a_store_that_is_not_connected_fails() {
    // Camel cannot be left to guess the result. An expunge that answers TRUE
    // without a connection would report the user's deletions as carried out
    // when they were not.
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let inbox_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_mailbox("Inbox", Some(role::INBOX))
    };
    let uid = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_email(EmailSeed::new(
            inbox_id,
            ("Bob", "bob@example.com"),
            "Lunch?",
            "One o'clock.",
            "2026-01-15T09:30:00Z",
        ))
    };

    let account = Account::open();
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    account.connect(MailSync::new(client, account_id));
    let inbox = open(&account, "Inbox");
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder of ours and a writable out-parameter.
    unsafe {
        assert_ne!(
            camel_folder_refresh_info_sync(inbox, ptr::null_mut(), &mut error),
            GFALSE
        );
    }
    let uid_c = CString::new(uid.as_str()).unwrap();
    // SAFETY: a live summary with a row for the listed message.
    unsafe {
        let summary = camel_folder_get_folder_summary(inbox);
        let info = camel_folder_summary_get(summary, uid_c.as_ptr());
        camel_message_info_set_flags(info, CAMEL_MESSAGE_DELETED, CAMEL_MESSAGE_DELETED);
        g_object_unref(info.cast());
    }
    assert!(account.jmap().drop_connection());

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: as above.
    let ok = unsafe { camel_folder_expunge_sync(inbox, ptr::null_mut(), &mut error) };
    let answered = Expunged::new(ok, error);

    assert!(!answered.ok, "a disconnected store cannot expunge anything");
    assert!(!answered.error.is_null(), "a failure with no error set");
    // SAFETY: the reference `camel_store_get_folder_sync` handed over.
    unsafe { g_object_unref(inbox.cast()) };
}
