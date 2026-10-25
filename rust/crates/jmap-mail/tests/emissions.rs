// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the store says back: the five folder signals that the management and
//! subscription vfuncs end in.
//!
//! Every one of these signals is written but never observed elsewhere.
//! `tests/manage.rs` and `tests/subscriptions.rs` both say so in their
//! headers, for the same reason: `camel_store_folder_created` begins by
//! taking the service's session and queueing the emission on it. A store with
//! no `CamelSession` behind it cannot emit at all, and the stores those two
//! files use are [`JmapStore::detached`] instances, which are not GObjects.
//! Those two files thoroughly test what each vfunc *decides*. This file tests
//! the last two lines of each, and nothing else tests them.
//!
//! Closing this gap matters beyond tidiness. Camel emits none of the five
//! signals for us: its own `camel_store_create_folder_sync` and
//! `camel_store_delete_folder_sync` call only the vfunc, and the emitters are
//! called nowhere in libcamel outside `CamelVeeStore`. These lines are the
//! *only* thing that tells Evolution's folder tree the account changed. A
//! provider that gets them wrong looks correct in every other test in this
//! crate, and leaves the user staring at a tree that never moves.
//!
//! This file establishes three facts about the emissions, none visible from
//! the vfuncs' source:
//!
//! - **They are queued, not delivered.** See `common::signals`. Every test
//!   here holds a [`Context`] and reads through [`events`], which pumps first.
//! - **The queue belongs to the session, not the store.** `camel_session_init`
//!   captures the context once. For this reason, [`Context::push`] runs before
//!   [`Account::open`] in every test below, not merely first out of habit.
//! - **The emitters clone what they are handed.** They must: the vfunc frees
//!   its `CamelFolderInfo` chain when it returns, long before the idle source
//!   runs. The handler reads a copy that outlives the chain the vfunc built.
//!   These tests also confirm Camel follows the ownership rule that
//!   `crate::manage` states.
//!
//! [`JmapStore::detached`]: jmap_mail::store::JmapStore::detached

mod common;

use std::ffi::CString;
use std::ptr;

use common::Account;
use common::signals::{Context, FolderEvent, events, watch_store};
use common::sync_against;
use eds_sys::{
    CAMEL_FOLDER_SUBSCRIBED, CamelFolderInfoFlags, CamelSubscribable, camel_folder_info_free,
    camel_store_create_folder_sync, camel_store_delete_folder_sync, camel_store_rename_folder_sync,
    camel_subscribable_subscribe_folder_sync, camel_subscribable_unsubscribe_folder_sync,
};
use glib_sys::{GError, GFALSE};
use jmap_mock::MockServer;
use jmap_proto::mail::role;

/// An account with an inbox, a folder beside it, and a folder under that. The
/// third folder gives a rename descendants to carry, the one thing that makes
/// `folder-renamed` more than `folder-created` with an extra string.
struct Mail {
    /// The store connects to this server. Dropping it would take the port
    /// down under the vfunc. The push tests change the account on it.
    server: MockServer,
    account: Account,
}

fn connected() -> Mail {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_mailbox("Inbox", Some(role::INBOX));
        let projects = account.seed_mailbox("Projects", None);
        account.seed_child_mailbox("Nineteen", None, &projects);
    }

    let account = Account::open();
    account.connect(sync_against(&server));
    // The server pushes a state change after each write, and the push handler
    // can announce the same folder again. Without the subscription, each signal
    // comes from the call under test. The push tests call the handler directly.
    account.jmap().stop_push();
    // The listing every vfunc below resolves its paths against. Taken here so
    // a test does not need to account for its cost.
    account.jmap().folders(0).expect("a listing");
    Mail { server, account }
}

impl Mail {
    /// Changes the account on the server, as another client does.
    fn on_server<R>(&self, edit: impl FnOnce(&mut jmap_mock::AccountState) -> R) -> R {
        let account_id = self.server.account_id();
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        edit(state.account_mut(&account_id).unwrap())
    }

    /// The folder-list half of a pushed `Mailbox` change.
    fn pushed_mailbox_change(&self) {
        // SAFETY: a live store of ours, made through a `CamelSession`.
        unsafe { jmap_mail::push::refresh_folder_list_if_structural(self.account.store) };
    }

    fn create(&self, parent: Option<&str>, name: &str) {
        // Bound to a variable, not converted inline. `as_ptr` on a temporary
        // `CString` would hand Camel a pointer freed at the end of the
        // expression, and the vfunc would read it as an empty path.
        let parent = parent.map(|parent| CString::new(parent).expect("a path with no NUL"));
        let name = CString::new(name).expect("a name with no NUL");
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live store of ours, two NUL-terminated strings alive across
        // the call, and an out-parameter that is writable and currently NULL.
        unsafe {
            let created = camel_store_create_folder_sync(
                self.account.store,
                parent
                    .as_ref()
                    .map_or(ptr::null(), |parent| parent.as_ptr()),
                name.as_ptr(),
                ptr::null_mut(),
                &mut error,
            );
            assert!(
                !created.is_null() && error.is_null(),
                "the folder would not be created: {}",
                why(error)
            );
            // The chain a create answers with belongs to the caller.
            camel_folder_info_free(created);
        }
    }

    fn delete(&self, path: &str) {
        let path = CString::new(path).expect("a path with no NUL");
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: as above.
        unsafe {
            let ok = camel_store_delete_folder_sync(
                self.account.store,
                path.as_ptr(),
                ptr::null_mut(),
                &mut error,
            );
            assert!(
                ok != GFALSE && error.is_null(),
                "the delete failed: {}",
                why(error)
            );
        }
    }

    fn rename(&self, from: &str, to: &str) {
        let from = CString::new(from).expect("a path with no NUL");
        let to = CString::new(to).expect("a path with no NUL");
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: as above.
        unsafe {
            let ok = camel_store_rename_folder_sync(
                self.account.store,
                from.as_ptr(),
                to.as_ptr(),
                ptr::null_mut(),
                &mut error,
            );
            assert!(
                ok != GFALSE && error.is_null(),
                "the rename failed: {}",
                why(error)
            );
        }
    }

    fn set_subscribed(&self, path: &str, subscribed: bool) {
        let path = CString::new(path).expect("a path with no NUL");
        let mut error: *mut GError = ptr::null_mut();
        let subscribable = self.account.store.cast::<CamelSubscribable>();
        // SAFETY: the store implements `CamelSubscribable`, which is what makes
        // the cast sound; the string is alive across the call and the
        // out-parameter is writable and currently NULL.
        unsafe {
            let ok = if subscribed {
                camel_subscribable_subscribe_folder_sync(
                    subscribable,
                    path.as_ptr(),
                    ptr::null_mut(),
                    &mut error,
                )
            } else {
                camel_subscribable_unsubscribe_folder_sync(
                    subscribable,
                    path.as_ptr(),
                    ptr::null_mut(),
                    &mut error,
                )
            };
            assert!(
                ok != GFALSE && error.is_null(),
                "the tick would not change: {}",
                why(error)
            );
        }
    }
}

/// Why a call failed, for the assertion that is about to say it did.
///
/// # Safety
///
/// `error` must be NULL or a live `GError`.
unsafe fn why(error: *mut GError) -> String {
    if error.is_null() {
        return "no error".to_owned();
    }
    // SAFETY: the contract above; the message is NUL-terminated.
    unsafe {
        std::ffi::CStr::from_ptr((*error).message)
            .to_string_lossy()
            .into_owned()
    }
}

/// The one event a test expected, or a failure naming everything that
/// arrived. This assertion matters twice over: a store that announces a
/// subscribe *and* a create redraws the tree as wrongly as one that announces
/// nothing.
fn one(events: Vec<FolderEvent>, signal: &str) -> FolderEvent {
    assert_eq!(events.len(), 1, "the store announced {events:?}");
    let event = events.into_iter().next().expect("one event");
    assert_eq!(event.signal, signal);
    event
}

/// A create is announced, with the folder that was made.
///
/// Evolution's folder tree adds a row only from this signal. The
/// `CamelFolderInfo` the vfunc *returns* goes only to its caller: one window,
/// for a folder made from the New Folder dialog. Every other view of the
/// account learns about the new folder here.
#[test]
fn a_created_folder_is_announced() {
    let context = Context::push();
    let mail = connected();
    watch_store(mail.account.store);

    mail.create(Some("Projects"), "Twenty");

    let event = one(events(&context), "folder-created");
    assert_eq!(event.paths(), ["Projects/Twenty"]);
    assert_eq!(event.folders[0].display_name, "Twenty");
    assert_eq!(event.old_name, None);
}

/// A delete, with the folder that went. The info must describe the folder as
/// it *was*: it is the only handle Evolution has for the row it must remove,
/// and by the time the signal delivers, nothing remains on the server to look
/// it up.
#[test]
fn a_deleted_folder_is_announced() {
    let context = Context::push();
    let mail = connected();
    watch_store(mail.account.store);

    mail.delete("Projects/Nineteen");

    let event = one(events(&context), "folder-deleted");
    assert_eq!(event.paths(), ["Projects/Nineteen"]);
    assert_eq!(event.folders[0].display_name, "Nineteen");
}

/// A rename carries both paths: the old one, which is the row Evolution has,
/// and the new one, which is what it becomes. Without the first the handler
/// cannot tell which row it is being asked about.
#[test]
fn a_renamed_folder_is_announced_under_both_paths() {
    let context = Context::push();
    let mail = connected();
    watch_store(mail.account.store);

    mail.rename("Projects/Nineteen", "Projects/Twenty");

    let event = one(events(&context), "folder-renamed");
    assert_eq!(event.old_name.as_deref(), Some("Projects/Nineteen"));
    assert_eq!(event.paths(), ["Projects/Twenty"]);
    assert_eq!(event.folders[0].display_name, "Twenty");
}

/// A rename also carries what is *under* the folder. This is why the rename's
/// chain is built to full depth, where a create's and a delete's chains are
/// one folder deep. Every descendant's path changed, and each path is a key
/// Camel uses to open a folder. The handler for this signal walks the
/// children it is handed, so a chain stopping at the folder itself would
/// leave rows nothing can open.
#[test]
fn a_renamed_folder_is_announced_with_its_children() {
    let context = Context::push();
    let mail = connected();
    watch_store(mail.account.store);

    mail.rename("Projects", "Archive");

    let event = one(events(&context), "folder-renamed");
    assert_eq!(event.old_name.as_deref(), Some("Projects"));
    assert_eq!(event.paths(), ["Archive", "Archive/Nineteen"]);
}

/// The tick going on. Camel's wrapper does not emit this signal for the
/// implementation: `camel_subscribable_subscribe_folder_sync` calls the vfunc
/// and returns. The folder tree learns the user's tick took effect only from
/// the line at the end of this vfunc.
#[test]
fn a_subscribed_folder_is_announced() {
    let context = Context::push();
    let mail = connected();
    watch_store(mail.account.store);

    mail.set_subscribed("Projects", true);

    let event = one(events(&context), "folder-subscribed");
    assert_eq!(event.paths(), ["Projects"]);
    assert!(
        event.folders[0].flags & CAMEL_FOLDER_SUBSCRIBED != 0,
        "a folder was announced as subscribed without the flag that says so: \
         {:?}",
        event.folders[0]
    );
}

/// The tick going off, with the mirror image of the flag. The tree redraws
/// the row from this info, so an unsubscribe that still announces
/// `CAMEL_FOLDER_SUBSCRIBED` would put the tick straight back.
#[test]
fn an_unsubscribed_folder_is_announced_without_the_flag() {
    let context = Context::push();
    let mail = connected();
    watch_store(mail.account.store);
    mail.set_subscribed("Projects", true);
    let _ = events(&context);

    mail.set_subscribed("Projects", false);

    let event = one(events(&context), "folder-unsubscribed");
    assert_eq!(event.paths(), ["Projects"]);
    assert_eq!(
        event.folders[0].flags & CAMEL_FOLDER_SUBSCRIBED,
        0 as CamelFolderInfoFlags,
        "a folder was announced as unsubscribed and kept the tick"
    );
}

/// Nothing is announced for a call that did nothing. A create under a missing
/// parent writes nothing to the server. A store that announces it anyway adds
/// a row for a folder that does not exist, and Evolution then offers to open
/// it, and fails.
#[test]
fn a_vfunc_that_failed_announces_nothing() {
    let context = Context::push();
    let mail = connected();
    watch_store(mail.account.store);

    let parent = CString::new("Nowhere").expect("a path with no NUL");
    let name = CString::new("Twenty").expect("a name with no NUL");
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live store of ours, two NUL-terminated strings alive across the
    // call, and an out-parameter that is writable and currently NULL.
    unsafe {
        let created = camel_store_create_folder_sync(
            mail.account.store,
            parent.as_ptr(),
            name.as_ptr(),
            ptr::null_mut(),
            &mut error,
        );
        assert!(
            created.is_null(),
            "a folder was made under a missing parent"
        );
        assert!(!error.is_null(), "it failed without saying why");
        glib_sys::g_error_free(error);
    }

    let events = events(&context);
    assert!(events.is_empty(), "a failed create announced {events:?}");
}

/// The one case Camel announces nothing for: a subtree with nothing
/// subscribed anywhere in it.
///
/// This is the price of leaving the rename's announcement to Camel. This test
/// pins the behavior instead of hiding it. Camel asks the store for the info
/// with `CAMEL_STORE_FOLDER_INFO_SUBSCRIBED`, because this store is
/// subscribable. A subtree the subscription filter drops entirely comes back
/// as nothing, so there is nothing to announce. The test above shows the
/// other half of the same filter: an unsubscribed folder kept for a
/// subscribed child *is* announced. What stays silent here is exactly what
/// the folder tree is not showing, from the rename's own view.
///
/// A folder the user cannot see is one they cannot rename from the tree. This
/// is Camel's rule, not a gap in this provider: every provider has it. It
/// still means the rename path needs care before calling it complete.
#[test]
fn a_rename_of_a_subtree_nothing_is_subscribed_to_is_announced_by_no_one() {
    let context = Context::push();
    let mail = connected();
    mail.set_subscribed("Projects/Nineteen", false);
    mail.set_subscribed("Projects", false);
    // Those two calls queue announcements of their own. A handler connected
    // between the queueing and the delivery would still catch them, because
    // the emission happens when the idle source runs. Pumped away first, so
    // only the rename is watched below.
    context.pump();
    watch_store(mail.account.store);

    mail.rename("Projects", "Archive");

    let events = events(&context);
    assert!(
        events.is_empty(),
        "Camel has started announcing a rename it used to leave silent, so the \
         line this provider deliberately does not have may be wanted again: \
         {events:?}"
    );
}

/// The other half of the same filter: an unsubscribed folder kept because a
/// descendant is subscribed is announced, once.
#[test]
fn a_rename_of_an_unsubscribed_folder_with_a_subscribed_child_is_announced() {
    let context = Context::push();
    let mail = connected();
    mail.set_subscribed("Projects", false);
    // Pumped away for the reason the test above gives.
    context.pump();
    watch_store(mail.account.store);

    mail.rename("Projects", "Archive");

    let event = one(events(&context), "folder-renamed");
    assert_eq!(event.paths(), ["Archive", "Archive/Nineteen"]);
}

/// The signals of a test, as (signal, paths) pairs.
fn signals_and_paths(events: Vec<FolderEvent>) -> Vec<(String, Vec<String>)> {
    events
        .into_iter()
        .map(|event| {
            let paths = event.paths().into_iter().map(str::to_owned).collect();
            (event.signal, paths)
        })
        .collect()
}

fn pair(signal: &str, path: &str) -> (String, Vec<String>) {
    (signal.to_owned(), vec![path.to_owned()])
}

/// A mailbox that another client made comes to the tree as one new row.
///
/// `folder-info-stale` makes Evolution build the full tree of the account
/// again, and that loses the folder that the user has open. The snooze of a
/// message makes a Snoozed mailbox in this way.
#[test]
fn a_mailbox_that_another_client_made_is_announced_alone() {
    let context = Context::push();
    let mail = connected();
    watch_store(mail.account.store);

    mail.on_server(|account| account.create_mailbox("Receipts", None, None));
    mail.pushed_mailbox_change();

    assert_eq!(
        signals_and_paths(events(&context)),
        [
            pair("folder-created", "Receipts"),
            pair("folder-subscribed", "Receipts"),
        ]
    );
}

/// A mailbox that another client destroyed leaves the tree as one row. The
/// child goes before its parent, so that each signal finds its row.
#[test]
fn a_mailbox_that_another_client_destroyed_is_announced_alone() {
    let context = Context::push();
    let mail = connected();
    watch_store(mail.account.store);

    mail.on_server(|account| {
        let ids: Vec<_> = account
            .mailboxes
            .iter()
            .filter(|(_, mailbox)| mailbox.name != "Inbox")
            .map(|(id, _)| id.clone())
            .collect();
        account.mailboxes.transaction(|transaction| {
            for id in &ids {
                transaction.destroy(id);
            }
        });
    });
    mail.pushed_mailbox_change();

    assert_eq!(
        signals_and_paths(events(&context)),
        [
            pair("folder-unsubscribed", "Projects/Nineteen"),
            pair("folder-deleted", "Projects/Nineteen"),
            pair("folder-unsubscribed", "Projects"),
            pair("folder-deleted", "Projects"),
        ]
    );
}

/// A subscription that another client changed moves the tick of one row.
#[test]
fn a_subscription_that_another_client_changed_is_announced_alone() {
    let context = Context::push();
    let mail = connected();
    watch_store(mail.account.store);

    mail.on_server(|account| {
        let (id, mailbox) = account
            .mailboxes
            .iter()
            .find(|(_, mailbox)| mailbox.name == "Projects")
            .map(|(id, mailbox)| (id.clone(), mailbox.clone()))
            .expect("Projects seeded");
        let unsubscribed = mailbox.is_subscribed(false);
        account
            .mailboxes
            .transaction(|transaction| transaction.update(&id, unsubscribed));
    });
    mail.pushed_mailbox_change();

    assert_eq!(
        signals_and_paths(events(&context)),
        [pair("folder-unsubscribed", "Projects")]
    );
}

/// A rename changes the path of the folder and of each descendant. The store
/// does not know the old path that `folder-renamed` needs for each of them in
/// all cases, so it still tells Camel that the full list is stale.
#[test]
fn a_mailbox_that_another_client_renamed_makes_the_list_stale() {
    let context = Context::push();
    let mail = connected();
    watch_store(mail.account.store);

    mail.on_server(|account| {
        let id = account
            .mailboxes
            .iter()
            .find(|(_, mailbox)| mailbox.name == "Projects")
            .map(|(id, _)| id.clone())
            .expect("Projects seeded");
        account.rename_mailbox(&id, "Plans")
    });
    mail.pushed_mailbox_change();

    let events = events(&context);
    assert_eq!(
        signals_and_paths(events),
        [("folder-info-stale".to_owned(), Vec::new())]
    );
}

/// A push that changed only counts announces nothing.
#[test]
fn a_mailbox_change_of_counts_only_is_announced_by_nothing() {
    let context = Context::push();
    let mail = connected();
    watch_store(mail.account.store);

    mail.pushed_mailbox_change();

    assert_eq!(signals_and_paths(events(&context)), []);
}
