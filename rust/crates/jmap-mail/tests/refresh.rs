// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `refresh_info_sync`: the vfunc where a folder and a server finally meet.
//!
//! Everything the last four increments built has been half a path.
//! `jmap-mail-sync` can list a mailbox and knows nothing of Camel;
//! `message_info` turns one of those rows into a `CamelMessageInfo` and is
//! never called; `summary` reconciles a listing against the rows a folder holds
//! and has to be handed the listing. This is the vfunc that joins them, and it
//! is the first thing in the crate that answers a question about a folder's
//! *contents* — which is to say the first thing Evolution can show mail from.
//!
//! Two halves, and the tests are about both:
//!
//! - **The fetch.** Camel calls the vfunc on the folder; the folder has a JMAP
//!   mailbox id and nothing else, so the connection has to come from the store
//!   it hangs off. What comes back is asserted through Camel's own accessors —
//!   `camel_folder_get_message_count` and `camel_folder_get_uids` — rather than
//!   through the summary, because those are the two questions Evolution
//!   actually asks and they are answered by `CamelFolder`'s base class out of
//!   the summary this provider fills.
//! - **The telling.** A folder that filled its summary and said nothing is a
//!   folder whose new mail appears the next time the user clicks away and back.
//!   Camel's `changed` signal is the notice, and its argument is the diff
//!   `apply_listing` produces — so what is tested here is that the signal is
//!   emitted when there is something to say and, just as much, that it is not
//!   emitted when there is not.
//!
//! ## Two things Camel does that a test has to know about
//!
//! **The wrapper connects first.** `camel_folder_refresh_info_sync` asks the
//! folder's parent store to connect before it dispatches to the class, so a
//! refresh never reaches this vfunc on a store that is disconnected and could
//! be reconnected. That is why the disconnected case below calls the vfunc
//! through the class pointer instead: what it tests is the race the
//! `Disconnected` error exists for — the connection going away after Camel
//! satisfied itself there was one — rather than a state Camel would have fixed
//! on the way in.
//!
//! **`changed` is not delivered where it is reported.**
//! `camel_folder_changed` queues the diff and emits it from the folder's main
//! context, coalescing whatever else is pending into the same emission.
//! Nothing arrives on a thread that never iterates a main loop — which a Rust
//! test thread does not — so [`emissions`] pumps a context before it reads. A
//! test that did not would observe silence and call it a pass. Which context
//! is not a detail either: see [`Context`]. Both live in `common::signals`,
//! which is where they went once the store's five folder signals turned out to
//! have the same problem.

mod common;

use std::ffi::{CStr, CString};
use std::ptr;

use common::Account;
use common::signals::{Context, emissions, uid_list, watch};
use eds_sys::{
    CAMEL_SERVICE_ERROR_NOT_CONNECTED, CAMEL_STORE_FOLDER_NONE, CamelFolder, CamelFolderClass,
    camel_folder_free_uids, camel_folder_get_folder_summary, camel_folder_get_message_count,
    camel_folder_get_uids, camel_folder_refresh_info_sync, camel_service_error_quark,
    camel_store_get_folder_sync,
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

/// A connected account whose inbox holds two messages, and that inbox opened
/// the way Camel opens it — through the store, so the folder is the one in the
/// store's own bag and carries the mailbox id `new_folder` put there.
///
/// The archive beside it holds nothing and is never opened. It is there for the
/// delta tests: `Email/changes` answers for the whole account, so a mailbox
/// this folder is *not* refreshing is the only way to ask what a folder does
/// with a change that is none of its business.
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

/// The same account on a server built to order — which the tests about *how
/// much* a refresh fetches need, because the bound they exercise is measured in
/// `Email/get` calls and this mock will answer for two hundred and fifty-six
/// messages in one.
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

/// One message arriving in a mailbox the way one arrives at a server: as a
/// state transition, so that a delta asked from an earlier state names it.
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
    /// Through Camel's own wrapper, which is what takes the folder lock,
    /// connects the parent store and dispatches to the class.
    fn of(folder: *mut CamelFolder) -> Self {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live folder, and an out-parameter that is writable and
        // currently NULL.
        let ok = unsafe { camel_folder_refresh_info_sync(folder, ptr::null_mut(), &mut error) };
        Self::new(ok, error)
    }

    /// Through the pointer in the class, skipping the wrapper's own
    /// preconditions — see this file's note on the reconnect.
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

/// The uids Camel would draw the message list from — asked for the way
/// Evolution asks, which is of the folder rather than of its summary.
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

/// The whole point of the increment: a folder that has been refreshed holds the
/// mailbox's mail, and holds it where Camel looks for it. Both accessors are
/// `CamelFolder`'s own defaults answering out of the summary this provider
/// filled, which is the half a test against the summary alone could not show.
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

/// And it says so. A message list already on screen is redrawn from this signal
/// and from nothing else.
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
    // A listing cannot tell an arrival from mail the user has had for years,
    // so none of it is recent. Otherwise the first refresh of an account would
    // run the user's incoming filters over their whole mailbox.
    assert!(
        emissions[0].recent.is_empty(),
        "a listing called {:?} recent",
        emissions[0].recent
    );

    // SAFETY: the one reference this test took.
    unsafe { g_object_unref(folder.cast()) };
}

/// A refresh is a poll — Camel runs one on a timer and one every time the
/// folder is opened — so the usual answer is that nothing happened. A folder
/// that emitted `changed` anyway would redraw the user's message list, and lose
/// where they were in it, every minute.
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

/// The folder is asked, and the connection belongs to the store. A store that
/// has none cannot answer, and `NOT_CONNECTED` is the code that makes Camel
/// connect and ask again rather than showing the account as broken — the same
/// rule `get_folder_info_sync` follows, asked one level down.
///
/// Called through the class rather than through the wrapper, for the reason
/// this file's header gives: the wrapper would reconnect the store first, and
/// what is under test is the window after Camel decided it had.
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

/// A listing comes with the state it was taken at, and the point of that state
/// is the *next* refresh: `Email/changes` asked from it is one round trip where
/// a listing is one per page of the mailbox. So a refresh that dropped it would
/// leave every later refresh as expensive as the first.
///
/// What is asserted here is only that the folder kept it — asking a delta from
/// it is the increment after this one.
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

/// And the second refresh's state replaces the first one's. A folder that kept
/// the older of the two would ask every later delta from a point that recedes
/// further into the past with every refresh.
#[test]
fn a_second_refresh_replaces_the_state_the_first_one_kept() {
    let mail = with_mail();
    let folder = mail.folder;

    Refreshed::of(folder).expect_ok();
    // SAFETY: `folder` is live and was built with a summary.
    let first = unsafe { summary_state(camel_folder_get_folder_summary(folder)) };
    assert!(first.is_some());

    // Another client deletes one of the two, which is a state transition on
    // the account's mail — the mailbox it happened in does not matter here,
    // only that the server has moved on since the first listing.
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

/// The increment: the second refresh of a mailbox asks what *changed* rather
/// than fetching the whole thing again.
///
/// This is what the state kept above is for, and it is the one thing about a
/// refresh that no assertion over the account's objects can reach: a listing
/// and a delta leave the folder holding exactly the same rows. The difference
/// is entirely in what went over the wire, so the wire is what is asserted —
/// `Email/changes` was asked, and `Email/query`, which is how the whole mailbox
/// is enumerated, was not.
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

/// And a message that arrived since that state is *recent*, which is the fourth
/// list and the one with consequences: Camel hands it to the session's filter
/// driver, so this is the folder saying "run the user's rules over this one".
///
/// Only a delta may say it. The listing path deliberately does not — see the
/// first-refresh test below, where two messages are added and none is recent.
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

/// The rule that makes a delta a second path rather than an argument to the
/// first: a row nothing was said about stays.
///
/// `Email/changes` answers for the *account*, so a message delivered to the
/// archive is on this folder's delta too — and it holds none of this folder's
/// messages. Reconciled as if it were a listing, that delta would empty the
/// inbox; applied as a delta it changes nothing here, which is also why the
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

/// And a message the delta says this mailbox no longer holds loses its row, and
/// is announced as removed so the message list stops drawing it.
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

/// The recovery. A server may refuse to calculate a delta from a state — too
/// old to still be in its log, or, as here, one it never issued at all — and
/// Camel has nowhere to report that to, so the answer has to be the mailbox
/// itself. A folder that gave up here would be one that never comes back.
///
/// The state is planted rather than aged, because ageing one out means a
/// server with a bounded changes log; what the folder does with the refusal is
/// the same either way.
///
/// A message is destroyed first so that the answer is a *reconciled* listing
/// and not just rows written again: what comes back from a relist is the whole
/// mailbox, so the message it does not name has to lose its row. Applied the
/// way a delta is applied, it would keep it forever.
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

/// The bound on catching up, from the folder's side: the row count it holds is
/// what says whether following a delta is still the cheap answer, and it is the
/// one number the layer below cannot know without paying a round trip for it.
///
/// Two mailboxes and a small `Email/get` limit make the difference observable.
/// The account moves by three messages that are none of this folder's business;
/// the folder holds two rows, so catching up would fetch more messages than
/// listing the mailbox would, and it lists instead.
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

/// And the same folder once it holds enough rows for the delta to be worth
/// following again — which is what pins the number being passed down to the
/// folder's own count rather than to nothing at all: the account moves by the
/// same three messages, and this time the mailbox is bigger than they are.
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
