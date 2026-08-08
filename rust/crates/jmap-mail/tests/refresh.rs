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
//! is not a detail either: see [`Context`].

mod common;

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::ptr;

use common::Account;
use eds_sys::{
    CAMEL_SERVICE_ERROR_NOT_CONNECTED, CAMEL_STORE_FOLDER_NONE, CamelFolder, CamelFolderChangeInfo,
    CamelFolderClass, camel_folder_change_info_get_added_uids,
    camel_folder_change_info_get_changed_uids, camel_folder_change_info_get_removed_uids,
    camel_folder_free_uids, camel_folder_get_message_count, camel_folder_get_uids,
    camel_folder_refresh_info_sync, camel_service_error_quark, camel_store_get_folder_sync,
};
use glib_sys::{
    GError, GFALSE, GMainContext, GPtrArray, g_main_context_iteration, g_main_context_new,
    g_main_context_pop_thread_default, g_main_context_push_thread_default, g_main_context_unref,
    gboolean, gpointer,
};
use gobject_sys::{g_object_unref, g_signal_connect_data, g_type_class_ref, g_type_class_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::folder::folder_type;
use jmap_mail_sync::MailSync;
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::mail::role;

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
fn with_mail() -> (MockServer, Account, *mut CamelFolder) {
    let server = MockServer::builder().start();
    edit(&server, |account| {
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Bob", "bob@example.com"),
            "First",
            "one",
            "2026-01-01T09:00:00Z",
        ));
        account.seed_email(EmailSeed::new(
            inbox,
            ("Bob", "bob@example.com"),
            "Second",
            "two",
            "2026-01-02T09:00:00Z",
        ));
    });

    let account = Account::open();
    account.connect(sync_against(&server));
    let folder = open(&account, "Inbox");
    (server, account, folder)
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

/// What one emission of the `changed` signal carried.
#[derive(Debug, PartialEq, Eq)]
struct Emission {
    added: Vec<String>,
    removed: Vec<String>,
    changed: Vec<String>,
}

thread_local! {
    /// Every emission the folder made, in order. A thread local rather than
    /// user data threaded through the handler, which is sound because
    /// [`Context`] makes the pumping thread the only one that can deliver.
    static EMISSIONS: RefCell<Vec<Emission>> = const { RefCell::new(Vec::new()) };
}

/// A main context of this test's own, pushed as the thread default for as long
/// as it is held.
///
/// Not a convenience. `g_main_context_iteration` on the *global* default
/// acquires it first and returns immediately, dispatching nothing, when another
/// thread already owns it — and a Rust test binary runs its tests on threads of
/// one process, so the tests here pump the same context concurrently and steal
/// each other's turn. A queued emission then arrives one pump too late, or not
/// within the test at all, which is where this file's intermittent failures
/// came from. Camel queues the `changed` signal onto the context that was
/// thread-default when `camel_folder_changed` was called, so a context per test
/// is a queue per test.
struct Context(*mut GMainContext);

impl Context {
    /// Pushed before anything else a test does, because what matters is which
    /// context is current when Camel queues, not when the test reads.
    fn push() -> Self {
        // SAFETY: a fresh context, pushed on this thread and popped in `drop`
        // — the stack discipline `g_main_context_pop_thread_default` requires.
        unsafe {
            let context = g_main_context_new();
            g_main_context_push_thread_default(context);
            Self(context)
        }
    }

    /// Delivers everything queued, without blocking on anything that is not.
    fn pump(&self) {
        // SAFETY: a live context this thread is the only user of, and FALSE is
        // what asks it not to wait for a source to become ready.
        unsafe { while g_main_context_iteration(self.0, GFALSE) != GFALSE {} }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: this is the context pushed in `push`, and the reference taken
        // there is the one released here.
        unsafe {
            g_main_context_pop_thread_default(self.0);
            g_main_context_unref(self.0);
        }
    }
}

/// Listens the way Evolution's message list listens.
fn watch(folder: *mut CamelFolder) {
    EMISSIONS.with(|seen| seen.borrow_mut().clear());
    // SAFETY: `folder` is a live GObject, the signal name is one `CamelFolder`
    // declares, and the handler has the signature that signal's marshaller
    // calls with. The transmute to `GCallback` is what every `g_signal_connect`
    // in C is, spelled out.
    unsafe {
        let id = g_signal_connect_data(
            folder.cast(),
            c"changed".as_ptr(),
            Some(std::mem::transmute::<
                unsafe extern "C" fn(*mut CamelFolder, *mut CamelFolderChangeInfo, gpointer),
                unsafe extern "C" fn(),
            >(on_changed)),
            ptr::null_mut(),
            None,
            0,
        );
        assert_ne!(id, 0, "nothing connected to the folder's changed signal");
    }
}

unsafe extern "C" fn on_changed(
    _folder: *mut CamelFolder,
    changes: *mut CamelFolderChangeInfo,
    _data: gpointer,
) {
    // SAFETY: the signal hands over a live change info for the duration of the
    // emission, and the three accessors borrow its arrays.
    let emission = unsafe {
        Emission {
            added: uid_list(camel_folder_change_info_get_added_uids(changes)),
            removed: uid_list(camel_folder_change_info_get_removed_uids(changes)),
            changed: uid_list(camel_folder_change_info_get_changed_uids(changes)),
        }
    };
    EMISSIONS.with(|seen| seen.borrow_mut().push(emission));
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

/// Everything the folder has announced since [`watch`], after giving the
/// test's main context the chance to deliver it. See this file's header:
/// `camel_folder_changed` queues, so reading without pumping reads nothing,
/// always.
fn emissions(context: &Context) -> Vec<Emission> {
    context.pump();
    EMISSIONS.with(|seen| seen.take())
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
    let (_server, _account, folder) = with_mail();

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
    let (_server, _account, folder) = with_mail();
    watch(folder);

    Refreshed::of(folder).expect_ok();

    let emissions = emissions(&context);
    assert_eq!(emissions.len(), 1, "the folder emitted {emissions:?}");
    assert_eq!(emissions[0].added.len(), 2);
    assert!(emissions[0].removed.is_empty());
    assert!(emissions[0].changed.is_empty());

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
    let (_server, _account, folder) = with_mail();

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
    let (_server, account, folder) = with_mail();
    assert!(account.jmap().drop_connection());

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
