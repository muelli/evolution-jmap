// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the store says back: the five folder signals the management and
//! subscription vfuncs end in.
//!
//! Every one of them has been written and none has ever been observed.
//! `tests/manage.rs` and `tests/subscriptions.rs` both say so in their headers,
//! and both give the same reason: `camel_store_folder_created` begins by taking
//! the service's session and queueing the emission on it, so a store without a
//! `CamelSession` behind it cannot emit at all — and the stores those two files
//! use are [`JmapStore::detached`] instances that are not GObjects. What each
//! vfunc *decides* is tested there, thoroughly. The last two lines of each are
//! tested here, and nowhere else.
//!
//! That gap is worth closing on its own account rather than for tidiness. Camel
//! emits none of the five for us — its own `camel_store_create_folder_sync` and
//! `camel_store_delete_folder_sync` call the vfunc and nothing else, and the
//! emitters are called nowhere in libcamel outside `CamelVeeStore` — so these
//! lines are the *only* thing that tells Evolution's folder tree that the
//! account changed. A provider that got them wrong would look correct in every
//! other test in this crate and leave the user staring at a tree that never
//! moves.
//!
//! Three things about the emissions this file had to establish, none of which
//! is visible from the source of the vfuncs:
//!
//! - **They are queued, not delivered.** See `common::signals`; every test here
//!   holds a [`Context`] and reads through [`events`], which pumps first.
//! - **The queue belongs to the session, not to the store.** The context is
//!   captured once, by `camel_session_init`, which is why [`Context::push`] runs
//!   before [`Account::open`] in every test below and not merely first out of
//!   habit.
//! - **The emitters clone what they are handed.** They have to: the vfunc frees
//!   its `CamelFolderInfo` chain when it returns, which is long before the idle
//!   source runs. What the handler reads here is a copy that outlived the chain
//!   the vfunc built — so these tests are also the check that the ownership rule
//!   `crate::manage` states is the one Camel actually follows.
//!
//! [`JmapStore::detached`]: jmap_mail::store::JmapStore::detached

mod common;

use std::ffi::CString;
use std::ptr;

use common::Account;
use common::signals::{Context, FolderEvent, events, watch_store};
use eds_sys::{
    CAMEL_FOLDER_SUBSCRIBED, CamelFolderInfoFlags, CamelSubscribable, camel_folder_info_free,
    camel_store_create_folder_sync, camel_store_delete_folder_sync, camel_store_rename_folder_sync,
    camel_subscribable_subscribe_folder_sync, camel_subscribable_unsubscribe_folder_sync,
};
use glib_sys::{GError, GFALSE};
use jmap_client::{Client, Credentials};
use jmap_mail_sync::MailSync;
use jmap_mock::MockServer;
use jmap_proto::mail::role;

fn sync_against(server: &MockServer) -> MailSync {
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    MailSync::new(client, server.account_id())
}

/// An account with an inbox, a folder beside it and a folder under that — the
/// third so that a rename has descendants to carry, which is the one thing that
/// makes `folder-renamed` more than `folder-created` with an extra string.
struct Mail {
    /// Held for the account's sake: the store's connection is to this server,
    /// and dropping it would take the port down under the vfunc.
    _server: MockServer,
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
    // The listing every vfunc below resolves its paths against. Taken here so
    // that the request it costs is not something a test has to think about.
    account.jmap().folders(0).expect("a listing");
    Mail {
        _server: server,
        account,
    }
}

impl Mail {
    fn create(&self, parent: Option<&str>, name: &str) {
        // Bound rather than converted inline: `as_ptr` on a temporary `CString`
        // hands Camel a pointer to a string freed at the end of the expression,
        // which the vfunc then reads as the empty path.
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

/// The one event a test expected, or a failure naming everything that did
/// arrive — which is the assertion that matters twice over, since a store that
/// announced a subscribe *and* a create would redraw the tree wrongly just as
/// surely as one that announced nothing.
fn one(events: Vec<FolderEvent>, signal: &str) -> FolderEvent {
    assert_eq!(events.len(), 1, "the store announced {events:?}");
    let event = events.into_iter().next().expect("one event");
    assert_eq!(event.signal, signal);
    event
}

/// A create is announced, and it is announced with the folder that was made.
///
/// Evolution's folder tree adds a row from this signal and from nothing else:
/// the `CamelFolderInfo` the vfunc *returns* goes to whoever called it, which
/// for a folder made from the New Folder dialog is one window, and every other
/// view of the account learns about it here.
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

/// And a delete, with the folder that went. The info has to describe the folder
/// as it *was*: it is the only handle Evolution has for the row it is being
/// told to take out, and by the time the signal is delivered there is nothing
/// left on the server to look it up in.
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

/// And it carries what is *under* the folder, which is the whole reason the
/// rename's chain is built to full depth where a create's and a delete's are
/// one folder deep. Every descendant's path changed, and every one of them is a
/// key Camel opens a folder by; the handler for this signal walks the children
/// of what it is handed, so a chain that stopped at the folder itself would
/// leave the account with rows nothing can open.
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

/// The tick going on. Camel's wrapper does not emit this for the
/// implementation — `camel_subscribable_subscribe_folder_sync` calls the vfunc
/// and returns — so the folder tree learns that the user's tick took effect
/// from the line at the end of our own vfunc or not at all.
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

/// And off again, with the mirror image of the flag. The info is what the tree
/// redraws the row from, so an unsubscribe that announced the folder still
/// carrying `CAMEL_FOLDER_SUBSCRIBED` would put the tick straight back.
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

/// Nothing is announced for a call that did nothing. A create under a parent
/// that is not there writes nothing to the server, and a store that told the
/// folder tree about it anyway would add a row for a folder that does not
/// exist — which Evolution then offers to open, and cannot.
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

/// The one folder Camel says nothing about: a subtree with nothing subscribed
/// anywhere in it.
///
/// This is the price of leaving the rename's announcement to Camel, and it is
/// pinned rather than papered over. The info Camel builds is asked of the store
/// with `CAMEL_STORE_FOLDER_INFO_SUBSCRIBED` — because this store is
/// subscribable — so a subtree the subscription filter drops entirely comes back
/// as nothing, and there is nothing to announce. The line above shows the same
/// filter's other half: an unsubscribed folder kept for a subscribed child *is*
/// announced, so what is silent here is exactly what the folder tree the rename
/// was invoked from is not showing.
///
/// A folder the user cannot see is one they cannot rename from the tree, which
/// is why this is Camel's rule rather than a gap of ours — every provider has
/// it. It is still the reason to be careful about calling the rename path done.
#[test]
fn a_rename_of_a_subtree_nothing_is_subscribed_to_is_announced_by_no_one() {
    let context = Context::push();
    let mail = connected();
    mail.set_subscribed("Projects/Nineteen", false);
    mail.set_subscribed("Projects", false);
    // Those two queued announcements of their own, and a handler connected
    // between the queueing and the delivery would still catch them: the
    // emission itself happens when the idle source runs. Pumped away first, so
    // that what is watched below is the rename alone.
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

/// And the other half of the same filter: an unsubscribed folder that is kept
/// because something under it is subscribed is announced, once.
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
