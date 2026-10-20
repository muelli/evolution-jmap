// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `refresh_info_sync`: the vfunc where a folder and a server meet.
//!
//! `jmap-mail-sync` can list a mailbox but knows nothing about Camel.
//! `message_info` turns a listing row into a `CamelMessageInfo`, but no caller
//! invokes it yet. `summary` reconciles a listing against the rows a folder
//! holds, but needs the listing handed to it. This vfunc joins all three
//! pieces. It is the first vfunc in the crate that answers a question about a
//! folder's *contents*, so it is the first thing that lets Evolution show
//! mail.
//!
//! The tests cover two parts:
//!
//! - **The fetch.** Camel calls the vfunc on the folder. The folder holds only
//!   a JMAP mailbox id, so the connection comes from the store it belongs to.
//!   Tests check the result through Camel's own accessors,
//!   `camel_folder_get_message_count` and the folder's uid list, instead of
//!   through the summary. These two accessors are what Evolution actually
//!   calls, and `CamelFolder`'s base class answers them from the summary this
//!   provider fills.
//! - **The notice.** A folder that fills its summary but emits nothing leaves
//!   new mail invisible until the user clicks away and back. Camel's `changed`
//!   signal is the notice. Its argument is the diff that `apply_listing`
//!   produces. Tests check that the signal fires when there is something to
//!   report, and stays silent when there is not.
//!
//! ## Two Camel behaviors a test must account for
//!
//! **The wrapper connects first.** `camel_folder_refresh_info_sync` asks the
//! folder's parent store to connect before it calls the vfunc. A refresh never
//! reaches this vfunc on a store that is disconnected but reconnectable. For
//! this reason, the disconnected-case test below calls the vfunc through the
//! class pointer instead of the wrapper. It tests the race that the
//! `Disconnected` error exists for: the connection drops after Camel confirms
//! one exists. It does not test a state that Camel already prevents on entry.
//!
//! **`changed` delivers away from where it is reported.**
//! `camel_folder_changed` queues the diff and emits it from the folder's main
//! context. It coalesces this emission with whatever else is pending. A thread
//! that never iterates a main loop never receives it, and a Rust test thread
//! does not iterate one. For this reason, [`emissions`] pumps the context
//! before it reads. A test that skips the pump sees silence and passes
//! wrongly. The choice of context also matters: see [`Context`]. Both live in
//! `common::signals`, after the store's five folder signals showed the same
//! issue.

mod common;

use std::ffi::{CStr, CString};
use std::ptr;

use common::Account;
use common::signals::{Context, emissions, uid_list, watch};
use eds_sys::compat::{folder_dup_uids, folder_free_uids};
use eds_sys::{
    CAMEL_SERVICE_ERROR_NOT_CONNECTED, CAMEL_STORE_FOLDER_NONE, CamelFolder, CamelFolderClass,
    camel_folder_get_folder_summary, camel_folder_get_message_count,
    camel_folder_refresh_info_sync, camel_service_error_quark, camel_store_get_folder_sync,
};
use glib_sys::{GError, GFALSE, gboolean};
use gobject_sys::{g_object_unref, g_type_class_ref, g_type_class_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::folder::folder_type;
use jmap_mail::summary::{set_summary_state, summary_state};
use jmap_mail_sync::MailSync;
use jmap_mock::{EmailSeed, MockServer, MockServerBuilder};
use jmap_proto::mail::role;
use jmap_proto::{Id, State};

fn sync_against(server: &MockServer) -> MailSync {
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    MailSync::new(client, server.account_id())
}

/// Mutate the account the way another client would.
fn edit<R>(server: &MockServer, edit: impl FnOnce(&mut jmap_mock::AccountState) -> R) -> R {
    let account_id = server.account_id();
    let state = server.state();
    let mut state = state.lock().unwrap();
    edit(state.account_mut(&account_id).unwrap())
}

/// A connected account whose inbox holds two messages. The inbox opens the way
/// Camel opens it: through the store. The folder is the one in the store's own
/// bag, and carries the mailbox id that `new_folder` set.
///
/// The archive beside it holds nothing and stays unopened. It exists for the
/// delta tests. `Email/changes` answers for the whole account, so a mailbox
/// this folder is *not* refreshing is the only way to test what a folder does
/// with a change outside its own business.
struct Mailbox {
    server: MockServer,
    account: Account,
    folder: *mut CamelFolder,
    inbox: Id,
    archive: Id,
}

fn with_mail() -> Mailbox {
    with_mail_on(MockServer::builder())
}

/// The same account on a server built to order. Tests about *how much* a
/// refresh fetches need this, because the bound they exercise is measured in
/// `Email/get` calls, and this mock answers for 256 messages in one call.
fn with_mail_on(builder: MockServerBuilder) -> Mailbox {
    let server = builder.start();
    let (inbox, archive) = edit(&server, |account| {
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        let archive = account.seed_mailbox("Archive", None);
        account.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Bob", "bob@example.com"),
            "First",
            "one",
            "2026-01-01T09:00:00Z",
        ));
        account.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Bob", "bob@example.com"),
            "Second",
            "two",
            "2026-01-02T09:00:00Z",
        ));
        (inbox, archive)
    });

    let account = Account::open();
    account.connect(sync_against(&server));
    let folder = open(&account, "Inbox");
    Mailbox {
        server,
        account,
        folder,
        inbox,
        archive,
    }
}

/// One message arriving in a mailbox, as a state transition. A delta asked
/// from an earlier state names it.
fn deliver(server: &MockServer, mailbox: &Id, subject: &str, received_at: &str) -> Id {
    edit(server, |account| {
        account.deliver_email(EmailSeed::new(
            mailbox.clone(),
            ("Bob", "bob@example.com"),
            subject,
            "and a body",
            received_at,
        ))
    })
}

/// The folder Camel would hand the user, opened by path.
fn open(account: &Account, path: &str) -> *mut CamelFolder {
    let path = CString::new(path).expect("a path with no NUL");
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
    folder
}

/// One refresh and both of its answers, owned the way Camel owns them.
struct Refreshed {
    ok: bool,
    error: *mut GError,
}

impl Refreshed {
    /// Calls the vfunc through Camel's own wrapper. The wrapper takes the
    /// folder lock, connects the parent store, and dispatches to the class.
    fn of(folder: *mut CamelFolder) -> Self {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live folder, and an out-parameter that is writable and
        // currently NULL.
        let ok = unsafe { camel_folder_refresh_info_sync(folder, ptr::null_mut(), &mut error) };
        Self::new(ok, error)
    }

    /// Calls the vfunc through the pointer in the class, skipping the
    /// wrapper's preconditions. See this file's note on the reconnect.
    fn straight(folder: *mut CamelFolder) -> Self {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: referencing the class runs the class_init that installs the
        // vfunc; `folder` is an instance of that class, and `error` is writable
        // and currently NULL.
        let ok = unsafe {
            let class = g_type_class_ref(folder_type()).cast::<CamelFolderClass>();
            let vfunc = (*class)
                .refresh_info_sync
                .expect("the folder cannot be refreshed");
            let ok = vfunc(folder, ptr::null_mut(), &mut error);
            g_type_class_unref(class.cast());
            ok
        };
        Self::new(ok, error)
    }

    fn new(ok: gboolean, error: *mut GError) -> Self {
        Self {
            ok: ok != GFALSE,
            error,
        }
    }

    fn expect_ok(self) {
        assert!(self.ok, "the refresh failed: {}", self.message());
        assert!(self.error.is_null(), "a refresh that worked set an error");
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

impl Drop for Refreshed {
    fn drop(&mut self) {
        if !self.error.is_null() {
            // SAFETY: the one reference, taken by the call above.
            unsafe { glib_sys::g_error_free(self.error) };
        }
    }
}

/// The uids Camel draws the message list from. Asked of the folder, the way
/// Evolution asks, not of its summary.
fn listed(folder: *mut CamelFolder) -> Vec<String> {
    // SAFETY: a live folder; the array comes back owned and is freed with the
    // function Camel documents for it.
    unsafe {
        let array = folder_dup_uids(folder);
        let uids = uid_list(array);
        folder_free_uids(folder, array);
        uids
    }
}

/// The point of this increment: a refreshed folder holds the mailbox's mail,
/// in the place Camel looks for it. Both accessors are `CamelFolder`'s own
/// defaults, answering from the summary this provider fills. A test against
/// the summary alone cannot show this half.
#[test]
fn a_refresh_fills_the_folder_from_the_mailbox() {
    let mail = with_mail();
    let folder = mail.folder;

    Refreshed::of(folder).expect_ok();

    // SAFETY: `folder` is live.
    unsafe {
        assert_eq!(camel_folder_get_message_count(folder), 2);
    }
    let uids = listed(folder);
    assert_eq!(uids.len(), 2, "the folder listed {uids:?}");

    // SAFETY: the one reference this test took from `get_folder_sync`.
    unsafe { g_object_unref(folder.cast()) };
}

/// The folder also announces the refresh. A message list already on screen
/// redraws only from this signal.
#[test]
fn a_refresh_tells_camel_what_arrived() {
    let context = Context::push();
    let mail = with_mail();
    let folder = mail.folder;
    watch(folder);

    Refreshed::of(folder).expect_ok();

    let emissions = emissions(&context);
    assert_eq!(emissions.len(), 1, "the folder emitted {emissions:?}");
    assert_eq!(emissions[0].added.len(), 2);
    assert!(emissions[0].removed.is_empty());
    assert!(emissions[0].changed.is_empty());
    // A listing cannot tell a new arrival from old mail, so none of it counts
    // as recent. Otherwise the first refresh would run incoming filters over
    // the whole mailbox.
    assert!(
        emissions[0].recent.is_empty(),
        "a listing called {:?} recent",
        emissions[0].recent
    );

    // SAFETY: the one reference this test took.
    unsafe { g_object_unref(folder.cast()) };
}

/// A refresh is a poll. Camel runs one on a timer, and one each time the
/// folder opens, so usually nothing happened. A folder that emits `changed`
/// anyway redraws the message list and loses the user's place in it every
/// minute.
#[test]
fn a_refresh_that_found_nothing_new_says_nothing() {
    let context = Context::push();
    let mail = with_mail();
    let folder = mail.folder;

    Refreshed::of(folder).expect_ok();
    emissions(&context);
    watch(folder);
    Refreshed::of(folder).expect_ok();

    let emissions = emissions(&context);
    assert!(
        emissions.is_empty(),
        "an unchanged mailbox emitted {emissions:?}"
    );
    // SAFETY: `folder` is live.
    unsafe {
        assert_eq!(
            camel_folder_get_message_count(folder),
            2,
            "the second listing lost or duplicated the first one's rows"
        );
        g_object_unref(folder.cast());
    }
}

/// The folder is asked, but the connection belongs to the store. A store with
/// no connection cannot answer. `NOT_CONNECTED` is the code that makes Camel
/// reconnect and retry, instead of marking the account as broken. The same
/// rule applies one level down, in `get_folder_info_sync`.
///
/// This test calls the vfunc through the class, not the wrapper, for the
/// reason given in this file's header: the wrapper reconnects the store
/// first. This test checks the window after Camel believes it is connected.
#[test]
fn a_folder_whose_store_has_no_connection_reports_it() {
    let mail = with_mail();
    let folder = mail.folder;
    assert!(mail.account.jmap().drop_connection());

    let refreshed = Refreshed::straight(folder);

    assert!(!refreshed.ok, "a disconnected folder refreshed anyway");
    assert!(!refreshed.error.is_null(), "it failed without saying why");
    // SAFETY: a live GError, and the quark accessor takes no arguments.
    unsafe {
        assert_eq!((*refreshed.error).domain, camel_service_error_quark());
        assert_eq!(
            (*refreshed.error).code,
            CAMEL_SERVICE_ERROR_NOT_CONNECTED as i32
        );
        g_object_unref(folder.cast());
    }
}

/// A listing comes with the state it was taken at. That state serves the
/// *next* refresh: `Email/changes` asked from it costs one round trip, while a
/// full listing costs one round trip per page of the mailbox. A refresh that
/// dropped the state would make every later refresh as expensive as the
/// first.
///
/// This test only asserts that the folder kept the state. Asking a delta from
/// it is covered by a later increment.
#[test]
fn a_refresh_keeps_the_state_the_listing_was_taken_at() {
    let mail = with_mail();
    let folder = mail.folder;

    Refreshed::of(folder).expect_ok();

    // SAFETY: `folder` is live and was built with a summary.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        assert!(
            summary_state(summary).is_some(),
            "the refresh dropped the state its listing came with"
        );
        g_object_unref(folder.cast());
    }
}

/// The second refresh's state replaces the first one's. A folder that kept the
/// older state would ask every later delta from a point further in the past
/// with each refresh.
#[test]
fn a_second_refresh_replaces_the_state_the_first_one_kept() {
    let mail = with_mail();
    let folder = mail.folder;

    Refreshed::of(folder).expect_ok();
    // SAFETY: `folder` is live and was built with a summary.
    let first = unsafe { summary_state(camel_folder_get_folder_summary(folder)) };
    assert!(first.is_some());

    // Another client deletes one of the two messages: a state transition on
    // the account's mail. The mailbox does not matter here, only that the
    // server moved on since the first listing.
    let gone = listed(folder).pop().expect("the folder listed nothing");
    assert!(edit(&mail.server, |account| account
        .destroy_email(&Id::new(gone))));
    Refreshed::of(folder).expect_ok();

    // SAFETY: `folder` is live.
    unsafe {
        let second = summary_state(camel_folder_get_folder_summary(folder));
        assert!(second.is_some());
        assert_ne!(
            second, first,
            "mail arrived and the folder kept the state it had before"
        );
        g_object_unref(folder.cast());
    }
}

/// This increment: the second refresh of a mailbox asks what *changed*,
/// instead of fetching the whole mailbox again.
///
/// The state kept above exists for this. No assertion over the account's
/// objects can reach it: a listing and a delta leave the folder holding the
/// same rows either way. The difference is only in what went over the wire, so
/// the test asserts on the wire. `Email/changes` must be asked. `Email/query`,
/// which enumerates the whole mailbox, must not.
#[test]
fn a_second_refresh_asks_what_changed_instead_of_listing_again() {
    let mail = with_mail();
    let folder = mail.folder;

    Refreshed::of(folder).expect_ok();
    let listed = mail.server.method_calls().len();
    Refreshed::of(folder).expect_ok();

    let second = mail.server.method_calls().split_off(listed);
    assert!(
        second.iter().any(|call| call == "Email/changes"),
        "the second refresh never asked what changed: {second:?}"
    );
    assert!(
        !second.iter().any(|call| call == "Email/query"),
        "the second refresh listed the whole mailbox again: {second:?}"
    );

    // SAFETY: the one reference this test took.
    unsafe { g_object_unref(folder.cast()) };
}

/// A message that arrived since that state is *recent*: the fourth list, and
/// the one with consequences. Camel hands it to the session's filter driver,
/// so the folder says "run the user's rules over this one".
///
/// Only a delta may call a message recent. The listing path deliberately does
/// not: see the first-refresh test above, where two messages are added and
/// none is recent.
#[test]
fn a_message_that_arrived_since_the_last_refresh_is_recent() {
    let context = Context::push();
    let mail = with_mail();
    let folder = mail.folder;

    Refreshed::of(folder).expect_ok();
    emissions(&context);
    watch(folder);

    let arrived = deliver(&mail.server, &mail.inbox, "Third", "2026-01-03T09:00:00Z");
    Refreshed::of(folder).expect_ok();

    let emissions = emissions(&context);
    assert_eq!(emissions.len(), 1, "the folder emitted {emissions:?}");
    assert_eq!(emissions[0].added, vec![arrived.as_str().to_owned()]);
    assert_eq!(
        emissions[0].recent,
        vec![arrived.as_str().to_owned()],
        "new mail arrived and the folder did not call it recent"
    );
    // SAFETY: `folder` is live.
    unsafe {
        assert_eq!(camel_folder_get_message_count(folder), 3);
        g_object_unref(folder.cast());
    }
}

/// The rule that makes a delta a separate path, not just an argument to the
/// listing: a row the delta says nothing about stays unchanged.
///
/// `Email/changes` answers for the whole *account*. A message delivered to the
/// archive appears in this folder's delta too, even though it holds none of
/// this folder's messages. Reconciled as if it were a listing, that delta
/// would empty the inbox. Applied as a delta, it changes nothing here, so the
/// folder must stay silent.
#[test]
fn a_refresh_leaves_the_rows_a_delta_did_not_mention() {
    let context = Context::push();
    let mail = with_mail();
    let folder = mail.folder;

    Refreshed::of(folder).expect_ok();
    emissions(&context);
    watch(folder);

    deliver(&mail.server, &mail.archive, "Filed", "2026-01-03T09:00:00Z");
    Refreshed::of(folder).expect_ok();

    // SAFETY: `folder` is live.
    unsafe {
        assert_eq!(
            camel_folder_get_message_count(folder),
            2,
            "a change in another mailbox emptied this one"
        );
    }
    let emissions = emissions(&context);
    assert!(
        emissions.is_empty(),
        "mail arriving elsewhere made this folder announce {emissions:?}"
    );

    // SAFETY: the one reference this test took.
    unsafe { g_object_unref(folder.cast()) };
}

/// A message the delta says this mailbox no longer holds loses its row. The
/// folder announces it as removed, so the message list stops drawing it.
#[test]
fn a_message_a_delta_says_is_gone_leaves_the_folder() {
    let context = Context::push();
    let mail = with_mail();
    let folder = mail.folder;

    Refreshed::of(folder).expect_ok();
    emissions(&context);
    watch(folder);

    let gone = listed(folder).pop().expect("the folder listed nothing");
    assert!(edit(&mail.server, |account| account
        .destroy_email(&Id::new(gone.clone()))));
    Refreshed::of(folder).expect_ok();

    let emissions = emissions(&context);
    assert_eq!(emissions.len(), 1, "the folder emitted {emissions:?}");
    assert_eq!(emissions[0].removed, vec![gone]);
    // SAFETY: `folder` is live.
    unsafe {
        assert_eq!(camel_folder_get_message_count(folder), 1);
        g_object_unref(folder.cast());
    }
}

/// The recovery path. A server may refuse to calculate a delta from a state,
/// for example a state too old for its log, or, as here, one it never issued.
/// Camel has no way to report this refusal, so the folder must fall back to
/// listing the mailbox itself. A folder that gives up here never recovers.
///
/// This test plants an invalid state instead of aging a real one out, because
/// aging one out needs a server with a bounded changes log. The folder's
/// response to the refusal is the same either way.
///
/// A message is destroyed first so the result is a *reconciled* listing, not
/// just the same rows written again. A relist returns the whole mailbox, so
/// the folder must drop the row for any message the relist does not name.
/// Applied the way a delta is applied, that row would stay forever.
#[test]
fn a_refresh_from_a_state_the_server_will_not_calculate_from_lists_again() {
    let mail = with_mail();
    let folder = mail.folder;

    Refreshed::of(folder).expect_ok();
    let gone = listed(folder).pop().expect("the folder listed nothing");
    assert!(edit(&mail.server, |account| account
        .destroy_email(&Id::new(gone))));
    // SAFETY: `folder` is live and was built with a summary of ours.
    unsafe {
        set_summary_state(
            camel_folder_get_folder_summary(folder),
            State::new("a state from some other server"),
        );
    }

    let before = mail.server.method_calls().len();
    Refreshed::of(folder).expect_ok();

    let calls = mail.server.method_calls().split_off(before);
    assert!(
        calls.iter().any(|call| call == "Email/query"),
        "a refused delta did not fall back to listing the mailbox: {calls:?}"
    );
    // SAFETY: `folder` is live.
    unsafe {
        assert_eq!(
            camel_folder_get_message_count(folder),
            1,
            "the listing that recovered from a refused delta was not reconciled"
        );
        g_object_unref(folder.cast());
    }
}

/// The bound on catching up, from the folder's side. The row count it holds
/// decides whether following a delta is still the cheap option. The layer
/// below cannot know this number without paying for a round trip.
///
/// Two mailboxes and a small `Email/get` limit make the difference
/// observable. The account moves by three messages outside this folder's
/// business. The folder holds two rows, so catching up would fetch more
/// messages than listing the mailbox, and the folder lists instead.
#[test]
fn a_delta_bigger_than_the_folder_lists_the_mailbox_again() {
    let mail = with_mail_on(MockServer::builder().objects_in_get(2));
    let folder = mail.folder;
    Refreshed::of(folder).expect_ok();
    // SAFETY: `folder` is live.
    assert_eq!(unsafe { camel_folder_get_message_count(folder) }, 2);

    for index in 0..3 {
        deliver(
            &mail.server,
            &mail.archive,
            &format!("Filed {index}"),
            &format!("2026-01-03T{:02}:00:00Z", 9 + index),
        );
    }

    let before = mail.server.method_calls().len();
    Refreshed::of(folder).expect_ok();

    let calls = mail.server.method_calls().split_off(before);
    assert!(
        calls.iter().any(|call| call == "Email/query"),
        "a delta costlier than the mailbox was followed anyway: {calls:?}"
    );
    // SAFETY: `folder` is live.
    unsafe {
        assert_eq!(
            camel_folder_get_message_count(folder),
            2,
            "the listing that replaced the delta lost the folder's rows"
        );
        g_object_unref(folder.cast());
    }
}

/// The same folder, once it holds enough rows for the delta to be worth
/// following again. This confirms the bound passed down is the folder's own
/// row count, not a constant. The account moves by the same three messages,
/// but this time the mailbox is bigger than they are.
#[test]
fn a_delta_smaller_than_the_folder_is_still_followed() {
    let mail = with_mail_on(MockServer::builder().objects_in_get(2));
    let folder = mail.folder;
    Refreshed::of(folder).expect_ok();

    for index in 0..2 {
        deliver(
            &mail.server,
            &mail.inbox,
            &format!("Arrived {index}"),
            &format!("2026-01-03T{:02}:00:00Z", 9 + index),
        );
    }
    Refreshed::of(folder).expect_ok();
    // SAFETY: `folder` is live.
    assert_eq!(unsafe { camel_folder_get_message_count(folder) }, 4);

    for index in 0..3 {
        deliver(
            &mail.server,
            &mail.archive,
            &format!("Filed {index}"),
            &format!("2026-01-04T{:02}:00:00Z", 9 + index),
        );
    }

    let before = mail.server.method_calls().len();
    Refreshed::of(folder).expect_ok();

    let calls = mail.server.method_calls().split_off(before);
    assert!(
        !calls.iter().any(|call| call == "Email/query"),
        "a folder bigger than the delta listed itself again: {calls:?}"
    );
    // SAFETY: `folder` is live.
    unsafe {
        assert_eq!(camel_folder_get_message_count(folder), 4);
        g_object_unref(folder.cast());
    }
}
