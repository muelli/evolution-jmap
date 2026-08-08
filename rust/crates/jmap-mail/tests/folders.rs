// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The folder listing a store keeps, and what Camel's flags word asks of it.
//!
//! `jmap-mail-sync` already answers both halves — the listing, and whether it
//! is still current. What is tested here is the third thing
//! `get_folder_info_sync` needs and neither half provides: somewhere to keep
//! the answer between calls. Camel asks a store for its folder tree constantly
//! — every folder the user clicks, every counter update — and passes
//! `CAMEL_STORE_FOLDER_INFO_REFRESH` on the few of those that mean "go and
//! look".
//!
//! The second part of the file is the other question the vfunc's arguments ask
//! of that tree: not *whether it is current* but *which part of it the caller
//! wants* — the `top` the answer is rooted at, and the depth
//! `CAMEL_STORE_FOLDER_INFO_RECURSIVE` cuts it to.
//!
//! The third part is the vfunc itself, called the way Camel calls it: through
//! the pointer in the class rather than by name. That is the only test that
//! proves the two halves above are actually joined to each other and to the
//! slot — a `Request` that is never built from a real `top`, or an
//! implementation that never reaches the class, is a store with no folders and
//! a suite that still passes.
//!
//! The fourth is the other direction: `get_folder_sync`, which takes a path out
//! of that listing and gives back the folder it names. It needs a real store —
//! a GObject one — because it is the first vfunc that builds something Camel
//! keeps, and Camel refuses to build a folder on a store it cannot type-check.

mod common;

use std::ffi::CString;
use std::ptr;
use std::sync::Arc;
use std::time::Duration;

use common::Account;
use eds_sys::{
    CAMEL_SERVICE_ERROR_NOT_CONNECTED, CAMEL_STORE_ERROR_NO_FOLDER,
    CAMEL_STORE_FOLDER_INFO_RECURSIVE, CAMEL_STORE_FOLDER_INFO_REFRESH, CAMEL_STORE_FOLDER_NONE,
    CamelFolder, CamelFolderInfo, CamelStore, CamelStoreClass, camel_folder_get_full_name,
    camel_folder_info_free, camel_offline_store_get_type, camel_service_error_quark,
    camel_store_error_quark, camel_store_get_folder_sync, camel_store_get_inbox_folder_sync,
};
use glib_sys::GError;
use gobject_sys::{g_object_unref, g_type_class_peek, g_type_class_ref, g_type_class_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::connect::StoreError;
use jmap_mail::folder::JmapFolder;
use jmap_mail::folders::Request;
use jmap_mail::store::{JmapStore, store_type};
use jmap_mail_sync::{FolderTree, MailSync};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::mail::{Mailbox, role};

/// No flags at all: what Camel passes when it wants the tree it was given last
/// time.
const CACHED: eds_sys::CamelStoreGetFolderInfoFlags = 0;
const REFRESH: eds_sys::CamelStoreGetFolderInfoFlags = CAMEL_STORE_FOLDER_INFO_REFRESH;
/// Every real caller in Camel and Evolution sets this one; the two that do not
/// are `camel_store_get_folder_info_sync`'s own virtual-folder paths.
const RECURSIVE: eds_sys::CamelStoreGetFolderInfoFlags = CAMEL_STORE_FOLDER_INFO_RECURSIVE;

fn sync_against(server: &MockServer) -> MailSync {
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    MailSync::new(client, server.account_id())
}

/// A server with an inbox, and a store already connected to it.
fn connected() -> (MockServer, Box<JmapStore>) {
    let server = MockServer::builder().start();
    edit(&server, |account| {
        account.seed_mailbox("Inbox", Some(role::INBOX))
    });
    let store = JmapStore::detached();
    store.store_connection(sync_against(&server));
    (server, store)
}

/// Mutate the account the way another client would — as a state transition,
/// which is what a refresh has a chance of noticing.
fn edit<R>(server: &MockServer, edit: impl FnOnce(&mut jmap_mock::AccountState) -> R) -> R {
    let account_id = server.account_id();
    let state = server.state();
    let mut state = state.lock().unwrap();
    edit(state.account_mut(&account_id).unwrap())
}

fn paths(tree: &FolderTree) -> Vec<&str> {
    tree.iter().map(|folder| folder.path.as_str()).collect()
}

/// Camel calls `get_folder_info_sync` on a store it believes is connected, but
/// "believes" is the operative word: a store whose connection went away has no
/// tree of its own to fall back on, and `NOT_CONNECTED` is the code that makes
/// Camel connect and ask again rather than showing the account as broken.
#[test]
fn a_store_with_no_connection_has_no_folders_to_list() {
    let store = JmapStore::detached();

    let error = store
        .folders(CACHED)
        .expect_err("no connection, no folders");
    assert!(
        matches!(error, StoreError::Disconnected),
        "expected a disconnected store, got {error:?}"
    );

    let gerror = error.to_gerror();
    // SAFETY: `to_gerror` handed over an owned GError, freed below.
    unsafe {
        assert_eq!((*gerror).domain, camel_service_error_quark());
        assert_eq!((*gerror).code, CAMEL_SERVICE_ERROR_NOT_CONNECTED as i32);
        glib_sys::g_error_free(gerror);
    }
}

/// The first call has nothing to serve from, so it lists — flags or no flags.
/// A store that answered "no folders" until someone asked it to refresh would
/// be an account that opens empty.
#[test]
fn the_first_listing_reaches_the_server_with_no_refresh_asked_for() {
    let (_server, store) = connected();

    let tree = store.folders(CACHED).expect("listed");
    assert_eq!(paths(&tree), ["Inbox"]);
}

/// And the second call does not. The server is stopped between the two, so the
/// tree that comes back cannot have come from it.
#[test]
fn a_second_listing_is_answered_without_the_server() {
    let (server, store) = connected();
    store.folders(CACHED).expect("listed");
    drop(server);

    let tree = store.folders(CACHED).expect("served from the store");
    assert_eq!(paths(&tree), ["Inbox"]);
}

/// The cached tree is the same tree, not an equal one: `get_folder_info_sync`
/// is called often enough that re-walking the account's mailboxes on each of
/// them would be work with no answer attached to it.
#[test]
fn the_cached_listing_is_the_one_that_was_listed() {
    let (_server, store) = connected();

    let first = store.folders(CACHED).expect("listed");
    let second = store.folders(CACHED).expect("served from the store");
    assert!(Arc::ptr_eq(&first, &second));
}

/// Without the flag, a store does not go and look — which is the whole point of
/// the flag. A mailbox another client made after the listing is not in the
/// answer.
#[test]
fn a_mailbox_made_after_the_listing_is_not_noticed_until_a_refresh() {
    let (server, store) = connected();
    store.folders(CACHED).expect("listed");
    edit(&server, |account| {
        account.create_mailbox("Receipts", None, None)
    });

    assert_eq!(paths(&store.folders(CACHED).expect("cached")), ["Inbox"]);
    assert_eq!(
        paths(&store.folders(REFRESH).expect("refreshed")),
        ["Inbox", "Receipts"]
    );
}

/// A refresh that finds nothing keeps the tree it has, rather than replacing it
/// with an equal one: Camel compares the `CamelFolderInfo` forests it is given
/// to decide which folders to signal as created or deleted, so a tree that is
/// new every refresh is churn no folder actually did.
#[test]
fn a_refresh_that_finds_nothing_keeps_the_tree_it_has() {
    let (_server, store) = connected();

    let first = store.folders(CACHED).expect("listed");
    let refreshed = store.folders(REFRESH).expect("refreshed");
    assert!(Arc::ptr_eq(&first, &refreshed));
}

/// A refresh after the listing was rebuilt is asked from the *new* state. The
/// mailbox created here moved the account on twice over; a store that kept
/// asking from the state of its first listing would rebuild the tree on every
/// refresh forever after.
#[test]
fn a_rebuilt_listing_is_what_the_next_refresh_is_measured_against() {
    let (server, store) = connected();
    store.folders(CACHED).expect("listed");
    edit(&server, |account| {
        account.create_mailbox("Receipts", None, None)
    });

    let rebuilt = store.folders(REFRESH).expect("refreshed");
    let again = store.folders(REFRESH).expect("refreshed again");
    assert!(
        Arc::ptr_eq(&rebuilt, &again),
        "nothing changed between the two refreshes, so the tree should not have been rebuilt"
    );
}

/// A listing belongs to the connection it was read over. Camel reconnects a
/// store whose connection it believes has gone away, and the account behind the
/// new one may be a different account entirely — the user edited the server, or
/// the password, or both.
#[test]
fn a_new_connection_discards_the_listing_the_old_one_answered() {
    let (server, store) = connected();
    store.folders(CACHED).expect("listed");
    edit(&server, |account| {
        account.create_mailbox("Receipts", None, None)
    });

    store.store_connection(sync_against(&server));
    assert_eq!(
        paths(&store.folders(CACHED).expect("listed again")),
        ["Inbox", "Receipts"],
        "the tree the new connection lists, not the one the old one had"
    );
}

/// And a disconnected store does not answer from the tree it had, which is the
/// connection's doing rather than the listing's: there is nothing to read the
/// account over, so there is no answer to give. Coming back is a fresh listing
/// for the same reason as above.
///
/// `drop_connection` frees the tree as well, which nothing here can see —
/// deliberately, since a listing no reader can reach is memory and not
/// behaviour.
#[test]
fn a_disconnected_store_does_not_answer_from_the_tree_it_had() {
    let (server, store) = connected();
    store.folders(CACHED).expect("listed");

    assert!(store.drop_connection());
    assert!(matches!(
        store.folders(CACHED),
        Err(StoreError::Disconnected)
    ));

    edit(&server, |account| {
        account.create_mailbox("Receipts", None, None)
    });
    store.store_connection(sync_against(&server));
    assert_eq!(
        paths(&store.folders(CACHED).expect("listed again")),
        ["Inbox", "Receipts"]
    );
}

/// A server that cannot be reached is reported, not papered over with an empty
/// tree — and a store that never listed successfully has nothing else to say.
///
/// A short timeout, because the socket this reaches for is one the mock server
/// was listening on a moment ago rather than a port nothing ever answered: the
/// request goes out on a pooled connection whose peer has gone, and waiting the
/// client's default half-minute for that to become obvious is half a minute of
/// test suite.
#[test]
fn a_listing_that_fails_is_reported_rather_than_answered_empty() {
    let server = MockServer::builder().start();
    let client = Client::builder()
        .timeout(Duration::from_millis(500))
        .connect(server.origin(), Credentials::none())
        .expect("connected");
    let store = JmapStore::detached();
    store.store_connection(MailSync::new(client, server.account_id()));
    drop(server);

    let error = store.folders(CACHED).expect_err("the server is gone");
    assert!(
        matches!(error, StoreError::Client(_)),
        "expected a client error, got {error:?}"
    );
}

/// A store whose account has no mailboxes at all lists an empty tree, and keeps
/// it: "nothing yet" is an answer, and re-asking the server for it on every
/// call would be the cache never engaging on exactly the accounts that are
/// cheapest to be wrong about.
#[test]
fn an_account_with_no_mailboxes_caches_its_empty_tree() {
    let server = MockServer::builder().start();
    let store = JmapStore::detached();
    store.store_connection(sync_against(&server));

    let first = store.folders(CACHED).expect("listed");
    assert!(first.is_empty());
    assert!(Arc::ptr_eq(&first, &store.folders(CACHED).expect("cached")));
}

/// The tree a folder id is looked up in is the account's own. Nothing here
/// depends on the ids themselves, but a listing that came back for some other
/// account would still pass every assertion above, so the account it was read
/// for is checked once.
#[test]
fn the_listing_is_the_connected_accounts() {
    let (server, store) = connected();
    let tree = store.folders(CACHED).expect("listed");
    let inbox = tree.find("Inbox").expect("an inbox");

    let expected: Id = edit(&server, |account| {
        account
            .mailboxes
            .iter()
            .next()
            .map(|(id, _)| id.clone())
            .expect("the seeded mailbox")
    });
    assert_eq!(inbox.id, expected);
}

// ---------------------------------------------------------------------------
// which part of the tree the call asks for

/// A tree built by hand, for the questions that are about the request rather
/// than about the server. Sibling order is RFC 8621's — `sortOrder`, then the
/// name — so `Personal` comes before `Work`.
fn hand_built() -> FolderTree {
    let mailbox = |id: &str, name: &str, parent: Option<&str>| Mailbox {
        id: Some(Id::new(id)),
        name: name.to_owned(),
        parent_id: parent.map(Id::new),
        ..Mailbox::default()
    };
    FolderTree::from_mailboxes(&[
        mailbox("M1", "Work", None),
        mailbox("M2", "Personal", None),
        mailbox("M3", "Invoices", Some("M1")),
        mailbox("M4", "Paid", Some("M3")),
    ])
    .expect("a well-formed mailbox list")
}

/// The request one call makes, as something a test can compare: the paths it is
/// rooted at, and how far down it goes.
fn requested(
    tree: &FolderTree,
    top: Option<&str>,
    flags: eds_sys::CamelStoreGetFolderInfoFlags,
) -> (Vec<String>, Option<usize>) {
    let request = Request::new(tree, top, flags);
    let paths = request
        .roots
        .iter()
        .map(|folder| folder.path.clone())
        .collect();
    (paths, request.depth)
}

/// `top` is nullable, and NULL means the account: Camel's own documentation
/// calls it "the name of the folder to start from", and starting from nowhere
/// is starting from the root.
#[test]
fn a_call_with_no_top_is_rooted_at_every_top_level_folder() {
    let tree = hand_built();

    let (paths, _) = requested(&tree, None, RECURSIVE);
    assert_eq!(paths, ["Personal", "Work"]);
}

/// The empty string is the same thing, and not a folder whose path is empty:
/// `camel_store_get_folder_info_sync` tests `top == NULL || *top == '\0'` for
/// its own "start at root" decision, so a store that read the two differently
/// would disagree with the wrapper calling it.
#[test]
fn an_empty_top_is_the_account_too() {
    let tree = hand_built();

    assert_eq!(
        requested(&tree, Some(""), RECURSIVE).0,
        ["Personal", "Work"]
    );
}

/// A `top` roots the answer at that folder — which is *included*, not skipped:
/// it is the head of the chain the caller gets back, the way IMAPX's is. Its
/// siblings are not.
#[test]
fn a_top_is_answered_with_that_folder_at_the_root() {
    let tree = hand_built();

    let (paths, _) = requested(&tree, Some("Work"), RECURSIVE);
    assert_eq!(paths, ["Work"]);
}

/// And it is matched on the Camel path, at any depth — not on the display name,
/// and not only among the top-level folders. Camel keys every folder by the
/// `full_name` this side produced, so that is the only string it can ask with.
#[test]
fn a_top_deeper_in_the_tree_is_found_by_its_path() {
    let tree = hand_built();

    let (paths, _) = requested(&tree, Some("Work/Invoices"), RECURSIVE);
    assert_eq!(paths, ["Work/Invoices"]);
}

/// A `top` no folder answers to asks for nothing, which Camel reads as a NULL
/// chain with no error set — its own documentation for the wrapper says the
/// call "can return NULL without setting a GError if no folders match the
/// search criteria". An error instead would turn a folder deleted by another
/// client into a broken account.
#[test]
fn a_top_that_names_no_folder_asks_for_nothing() {
    let tree = hand_built();

    let (paths, _) = requested(&tree, Some("Nowhere"), RECURSIVE);
    assert!(paths.is_empty());
}

/// The flag Camel documents as "the returned tree will include all levels of
/// hierarchy below @top. If not, it will only include the immediate subfolders
/// of @top".
#[test]
fn recursive_asks_for_every_level_below_the_root() {
    let tree = hand_built();

    assert_eq!(requested(&tree, None, RECURSIVE).1, None);
    assert_eq!(requested(&tree, Some("Work"), RECURSIVE).1, None);
}

/// Without it, one level below `top` — and the two cases differ by one, because
/// the folder `top` names is itself in the answer and the account's root is
/// not. `top` = `Work` returns `Work` and its children; no `top` at all returns
/// the top-level folders, which *are* the root's children, and nothing under
/// them.
#[test]
fn without_recursive_only_the_level_below_top_is_asked_for() {
    let tree = hand_built();

    assert_eq!(requested(&tree, None, CACHED).1, Some(0));
    assert_eq!(requested(&tree, Some("Work"), CACHED).1, Some(1));
}

/// The refresh flag is the listing's business and not the request's: a call
/// that asks for one subtree still refreshes the whole tree, because JMAP has
/// no way to ask for part of a `Mailbox/changes` and a partial answer would
/// leave the store's state describing folders it did not fetch.
#[test]
fn the_refresh_flag_does_not_change_which_folders_are_asked_for() {
    let tree = hand_built();

    assert_eq!(
        requested(&tree, Some("Work"), RECURSIVE),
        requested(&tree, Some("Work"), RECURSIVE | REFRESH)
    );
}

/// And the slot itself. `CamelStore` leaves `get_folder_info_sync` NULL and
/// `camel_store_get_folder_info_sync` refuses to call a store that has not
/// filled it in, so an override that never reached the class is an account with
/// no folders and a runtime warning rather than a compile error.
#[test]
fn the_store_class_overrides_the_folder_listing_vfunc() {
    // SAFETY: the store type is registered by `store_type`, and referencing its
    // class is what runs the class_init that installs the vfunc; the reference
    // is released below. Peeking the parent's class is safe because referencing
    // the child's has initialised it.
    unsafe {
        let class = g_type_class_ref(store_type()).cast::<CamelStoreClass>();
        assert!(
            (*class).get_folder_info_sync.is_some(),
            "the store cannot list its folders"
        );

        let parent = g_type_class_peek(camel_offline_store_get_type()).cast::<CamelStoreClass>();
        assert!(!parent.is_null(), "the parent class is not initialised");
        assert!(
            (*parent).get_folder_info_sync.is_none(),
            "CamelOfflineStore grew an implementation of its own; the override \
             above is no longer the only thing filling the slot"
        );

        g_type_class_unref(class.cast());
    }
}

// ---------------------------------------------------------------------------
// the vfunc, called the way Camel calls it

/// One `get_folder_info_sync` call and both of its answers, owned the way
/// Camel owns them: the chain freed with `camel_folder_info_free`, the error
/// with `g_error_free`.
struct Answered {
    chain: *mut CamelFolderInfo,
    error: *mut GError,
}

impl Answered {
    /// Calls the vfunc through the pointer in the class, which is the only way
    /// Camel ever reaches it — by name would test a function that might not be
    /// installed anywhere.
    fn of(
        store: &JmapStore,
        top: Option<&str>,
        flags: eds_sys::CamelStoreGetFolderInfoFlags,
    ) -> Self {
        let top = top.map(|top| CString::new(top).expect("a top with no NUL"));
        let mut error: *mut GError = ptr::null_mut();

        // SAFETY: referencing the class runs the class_init that installs the
        // vfunc. The store is an instance of ours, which is what the vfunc's
        // contract asks for; `top` is NULL or a NUL-terminated string alive for
        // the call, and `error` is writable and currently NULL. The class
        // reference is released after the call, and the store outlives it.
        unsafe {
            let class = g_type_class_ref(store_type()).cast::<CamelStoreClass>();
            let vfunc = (*class)
                .get_folder_info_sync
                .expect("the store cannot list its folders");
            let chain = vfunc(
                (store as *const JmapStore).cast_mut().cast::<CamelStore>(),
                top.as_ref().map_or(ptr::null(), |top| top.as_ptr()),
                flags,
                ptr::null_mut(),
                &mut error,
            );
            g_type_class_unref(class.cast());
            Self { chain, error }
        }
    }

    /// The paths of the answer, parents before children, as Camel walks it.
    fn paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        // SAFETY: the chain is the forest the vfunc just handed over, and every
        // `full_name` in it was allocated from a Rust string.
        unsafe { collect(self.chain, &mut paths) };
        paths
    }
}

/// Depth-first over a sibling chain and its children, appending `full_name`s.
///
/// # Safety
///
/// `head` is NULL or the head of a `CamelFolderInfo` sibling chain whose
/// `full_name`s are non-NULL and NUL-terminated.
unsafe fn collect(head: *mut CamelFolderInfo, paths: &mut Vec<String>) {
    let mut info = head;
    while !info.is_null() {
        unsafe {
            paths.push(
                std::ffi::CStr::from_ptr((*info).full_name)
                    .to_string_lossy()
                    .into_owned(),
            );
            collect((*info).child, paths);
            info = (*info).next;
        }
    }
}

impl Drop for Answered {
    fn drop(&mut self) {
        // SAFETY: the vfunc handed over ownership of both, and Camel's contract
        // is that the caller frees them with exactly these two functions.
        unsafe {
            if !self.chain.is_null() {
                camel_folder_info_free(self.chain);
            }
            if !self.error.is_null() {
                glib_sys::g_error_free(self.error);
            }
        }
    }
}

/// A store with something worth rooting an answer at: `Work`, a child, and a
/// grandchild, plus a sibling that must stay out of a `top`ped answer.
fn nested() -> (MockServer, Box<JmapStore>) {
    let server = MockServer::builder().start();
    edit(&server, |account| {
        account.seed_mailbox("Inbox", Some(role::INBOX));
        let work = account.seed_mailbox("Work", None);
        let invoices = account.seed_child_mailbox("Invoices", None, &work);
        account.seed_child_mailbox("Paid", None, &invoices);
    });
    let store = JmapStore::detached();
    store.store_connection(sync_against(&server));
    (server, store)
}

/// The whole point of the vfunc, end to end: a connected store, a NULL `top`,
/// and the account's folders coming back as a C forest. Everything under it has
/// its own test; this is the one that proves they are wired to each other and
/// to the class.
#[test]
fn the_vfunc_answers_a_connected_store_with_its_folders() {
    let (_server, store) = nested();

    let answered = Answered::of(&store, None, RECURSIVE);

    assert!(
        answered.error.is_null(),
        "a successful listing set an error"
    );
    assert_eq!(
        answered.paths(),
        ["Inbox", "Work", "Work/Invoices", "Work/Invoices/Paid"]
    );
}

/// A NULL `top` and an empty one are the same question, and the vfunc has to
/// read a C NULL as such rather than as a folder whose path is the empty
/// string.
#[test]
fn the_vfunc_reads_an_empty_top_as_the_whole_account() {
    let (_server, store) = nested();

    assert_eq!(
        Answered::of(&store, Some(""), RECURSIVE).paths(),
        Answered::of(&store, None, RECURSIVE).paths()
    );
}

/// The `top` reaching the answer, not just the [`Request`]: the folder named is
/// the head of the chain, its sibling `Inbox` is absent, and the chain's head
/// has no `next` — a root chain that still linked the siblings would hand Camel
/// the whole account under the name of a subtree.
#[test]
fn the_vfunc_roots_the_answer_at_the_top_it_was_given() {
    let (_server, store) = nested();

    let answered = Answered::of(&store, Some("Work"), RECURSIVE);

    assert_eq!(
        answered.paths(),
        ["Work", "Work/Invoices", "Work/Invoices/Paid"]
    );
    // SAFETY: the chain is the forest the vfunc handed over, and it is not
    // empty — the assertion above walked it.
    unsafe { assert!((*answered.chain).next.is_null(), "a sibling came along") };
}

/// And the depth reaching it. Without `RECURSIVE` the answer is `top` and its
/// immediate children, which is the level below a folder that is itself in the
/// answer — so `Paid`, a level further down, is left out.
#[test]
fn the_vfunc_cuts_the_answer_when_recursive_is_not_asked_for() {
    let (_server, store) = nested();

    let answered = Answered::of(&store, Some("Work"), CACHED);

    assert_eq!(answered.paths(), ["Work", "Work/Invoices"]);
}

/// The case that must not be an error. Camel documents the wrapper as able to
/// "return NULL without setting a GError if no folders match the search
/// criteria", and a folder another client deleted between one call and the next
/// is asked for once more before Camel notices; reporting that as a failure
/// would turn someone else's tidying into a broken account.
#[test]
fn a_top_naming_no_folder_is_an_empty_answer_and_not_a_failure() {
    let (_server, store) = nested();

    let answered = Answered::of(&store, Some("Nowhere"), RECURSIVE);

    assert!(answered.chain.is_null(), "a folder that does not exist");
    assert!(answered.error.is_null(), "an empty answer set an error");
}

/// The other NULL, which *is* a failure and has to be told apart from the one
/// above by the error alone. `NOT_CONNECTED` is the code that makes Camel
/// connect and ask again rather than showing the account as broken.
#[test]
fn the_vfunc_answers_a_disconnected_store_with_null_and_an_error() {
    let store = JmapStore::detached();

    let answered = Answered::of(&store, None, RECURSIVE);

    assert!(answered.chain.is_null(), "a disconnected store listed");
    assert!(!answered.error.is_null(), "no reason given");
    // SAFETY: the error is the one the vfunc set, checked non-NULL above.
    unsafe {
        assert_eq!((*answered.error).domain, camel_service_error_quark());
        assert_eq!(
            (*answered.error).code,
            CAMEL_SERVICE_ERROR_NOT_CONNECTED as i32
        );
    }
}

/// A NULL instance pointer is not something Camel does, but it is what the
/// guard's failure path looks like from here, and the vfunc must answer it with
/// the same NULL-and-an-error rather than dereferencing it.
#[test]
fn a_null_store_is_reported_rather_than_dereferenced() {
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: referencing the class installs the vfunc; NULL is exactly the
    // instance pointer under test, and `error` is writable and NULL.
    let chain = unsafe {
        let class = g_type_class_ref(store_type()).cast::<CamelStoreClass>();
        let vfunc = (*class).get_folder_info_sync.expect("the vfunc");
        let chain = vfunc(
            ptr::null_mut(),
            ptr::null(),
            RECURSIVE,
            ptr::null_mut(),
            &mut error,
        );
        g_type_class_unref(class.cast());
        chain
    };

    assert!(chain.is_null());
    assert!(!error.is_null(), "no reason given");
    // SAFETY: owned by us, set above.
    unsafe { glib_sys::g_error_free(error) };
}

// ---------------------------------------------------------------------------
// opening one of them

/// A real store — connected, with a nested mailbox to open — and the id of the
/// mailbox the tests below ask for.
///
/// Real rather than [`JmapStore::detached`], which every test above uses,
/// because this vfunc is the first one that *builds* something: Camel refuses
/// to construct a folder whose parent is not a `CamelStore`, and a detached
/// store is not a GObject at all.
fn opened() -> (MockServer, Account, Id) {
    let server = MockServer::builder().start();
    let invoices = edit(&server, |account| {
        account.seed_mailbox("Inbox", Some(role::INBOX));
        let work = account.seed_mailbox("Work", None);
        account.seed_child_mailbox("Invoices", None, &work)
    });
    let account = Account::open();
    account.connect(sync_against(&server));
    (server, account, invoices)
}

/// One `get_folder_sync` call and both of its answers, owned the way Camel owns
/// them: the folder unreffed, the error freed.
struct Opened {
    folder: *mut CamelFolder,
    error: *mut GError,
}

impl Opened {
    /// Calls the vfunc through the pointer in the class, as Camel does.
    fn of(store: *mut CamelStore, path: &str) -> Self {
        let path = CString::new(path).expect("a path with no NUL");
        let mut error: *mut GError = ptr::null_mut();

        // SAFETY: referencing the class runs the class_init that installs the
        // vfunc. `store` is an instance of ours or NULL, which is what the
        // tests below hand over deliberately; `path` is NUL-terminated and
        // alive for the call, and `error` is writable and currently NULL.
        unsafe {
            let class = g_type_class_ref(store_type()).cast::<CamelStoreClass>();
            let vfunc = (*class)
                .get_folder_sync
                .expect("the store cannot open a folder");
            let folder = vfunc(
                store,
                path.as_ptr(),
                CAMEL_STORE_FOLDER_NONE,
                ptr::null_mut(),
                &mut error,
            );
            g_type_class_unref(class.cast());
            Self { folder, error }
        }
    }

    /// The path Camel keys the folder it got back by.
    fn path(&self) -> String {
        assert!(!self.folder.is_null(), "no folder to name");
        // SAFETY: a live folder of ours, whose name it owns and outlives.
        unsafe {
            std::ffi::CStr::from_ptr(camel_folder_get_full_name(self.folder))
                .to_string_lossy()
                .into_owned()
        }
    }

    /// The mailbox every request about that folder will filter on.
    fn mailbox(&self) -> Id {
        assert!(!self.folder.is_null(), "no folder to ask");
        // SAFETY: as above; the borrow ends inside this function.
        unsafe { JmapFolder::borrow(self.folder) }
            .expect("a folder of ours")
            .mailbox()
            .expect("a folder with no mailbox behind it")
            .clone()
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        // SAFETY: the vfunc handed over one reference to the folder and
        // ownership of the error.
        unsafe {
            if !self.folder.is_null() {
                g_object_unref(self.folder.cast());
            }
            if !self.error.is_null() {
                glib_sys::g_error_free(self.error);
            }
        }
    }
}

/// The whole point of the vfunc: a path Camel took out of a folder listing goes
/// back in, and the folder that comes out carries the JMAP mailbox id that path
/// cannot be turned back into.
#[test]
fn the_vfunc_opens_the_folder_a_path_names() {
    let (_server, account, invoices) = opened();

    let opened = Opened::of(account.store, "Work/Invoices");

    assert!(opened.error.is_null(), "opening a folder set an error");
    assert_eq!(opened.path(), "Work/Invoices");
    assert_eq!(opened.mailbox(), invoices);
}

/// A path no mailbox answers to. Unlike `get_folder_info_sync`, where an empty
/// answer is a legitimate one, there is no such thing as half a folder: NULL is
/// the only thing to return and an error is what says why.
#[test]
fn a_path_naming_no_mailbox_is_a_failure_with_a_reason() {
    let (_server, account, _) = opened();

    let opened = Opened::of(account.store, "Work/Nowhere");

    assert!(opened.folder.is_null(), "a folder that does not exist");
    assert!(!opened.error.is_null(), "no reason given");
    // SAFETY: the error is the one the vfunc set, checked non-NULL above.
    unsafe {
        assert_eq!((*opened.error).domain, camel_store_error_quark());
        assert_eq!((*opened.error).code, CAMEL_STORE_ERROR_NO_FOLDER as i32);
    }
}

/// A folder that appeared after the listing the store is holding. Evolution
/// reopens the folder the user last had selected at startup, from a URI in its
/// settings, before anything asks the store to refresh — so a path that is not
/// in the held tree is a reason to look again rather than to report a folder
/// that plainly exists as missing.
#[test]
fn a_mailbox_created_since_the_listing_is_found_by_looking_again() {
    let (server, account, _) = opened();
    Opened::of(account.store, "Inbox");

    edit(&server, |state| state.create_mailbox("Archive", None, None));

    let opened = Opened::of(account.store, "Archive");
    assert!(opened.error.is_null(), "opening a folder set an error");
    assert_eq!(opened.path(), "Archive");
}

/// The other NULL, told apart from the one above by the error alone.
/// `NOT_CONNECTED` is what makes Camel connect and ask again rather than show
/// the account as broken.
#[test]
fn a_disconnected_store_has_no_folder_to_open() {
    let account = Account::open();

    let opened = Opened::of(account.store, "Inbox");

    assert!(opened.folder.is_null(), "a disconnected store opened one");
    assert!(!opened.error.is_null(), "no reason given");
    // SAFETY: the error is the one the vfunc set, checked non-NULL above.
    unsafe {
        assert_eq!((*opened.error).domain, camel_service_error_quark());
        assert_eq!(
            (*opened.error).code,
            CAMEL_SERVICE_ERROR_NOT_CONNECTED as i32
        );
    }
}

/// The same guard as the listing vfunc's, for the same reason.
#[test]
fn a_null_store_has_no_folder_to_open_either() {
    let opened = Opened::of(ptr::null_mut(), "Inbox");

    assert!(opened.folder.is_null());
    assert!(!opened.error.is_null(), "no reason given");
}

/// Camel keeps the folder, and this provider must not keep a second one.
/// `CamelStore` owns a `CamelObjectBag` of open folders — reachable as
/// `camel_store_get_folders_bag`, keyed with the class's own
/// `hash_folder_name` — which `camel_store_get_folder_sync` reserves in before
/// it calls the vfunc at all. So the vfunc's contract is to build a folder
/// every time it is reached, and reaching it twice for one path is Camel's
/// business rather than ours. Called through the wrapper here, because the
/// caching is the wrapper's and calling the vfunc directly would test nothing.
#[test]
fn camel_hands_back_the_folder_it_already_opened() {
    let (_server, account, _) = opened();

    // SAFETY: a live store of ours, a NUL-terminated path, and an error
    // out-parameter that is NULL — the wrapper tolerates a NULL GError **.
    let (first, second) = unsafe {
        let first = camel_store_get_folder_sync(
            account.store,
            c"Work".as_ptr(),
            CAMEL_STORE_FOLDER_NONE,
            ptr::null_mut(),
            ptr::null_mut(),
        );
        let second = camel_store_get_folder_sync(
            account.store,
            c"Work".as_ptr(),
            CAMEL_STORE_FOLDER_NONE,
            ptr::null_mut(),
            ptr::null_mut(),
        );
        (first, second)
    };

    assert!(!first.is_null(), "the wrapper opened no folder");
    assert_eq!(first, second, "a second folder over the same mailbox");

    // SAFETY: one reference each, both handed over by the wrapper.
    unsafe {
        g_object_unref(first.cast());
        g_object_unref(second.cast());
    }
}

// ---------------------------------------------------------------------------
// and the one folder Camel asks for by purpose rather than by name

/// A store whose inbox is not where a name-matching provider would look for
/// it: nested under another mailbox, called something else entirely, and with a
/// decoy mailbox named `inbox` sitting at the top level.
///
/// The decoy is what `CamelStoreClass`'s *inherited* implementation opens — it
/// asks `get_folder_sync` for the folder called `inbox`, in exactly that case —
/// so an account laid out like this is the one that tells the override apart
/// from the default it replaces. Camel's own IMAPX does the same thing one
/// spelling up, matching a folder's name against `"INBOX"`. Both are IMAP
/// conventions rather than facts about mail stores: RFC 8621 §2 gives a mailbox
/// a `role`, and says nothing about its name or where in the hierarchy it sits.
fn with_inbox() -> (MockServer, Account, Id) {
    let server = MockServer::builder().start();
    let inbox = edit(&server, |account| {
        let accounts = account.seed_mailbox("Accounts", None);
        account.seed_mailbox("inbox", None);
        account.seed_child_mailbox("Posteingang", Some(role::INBOX), &accounts)
    });
    let account = Account::open();
    account.connect(sync_against(&server));
    (server, account, inbox)
}

/// One `camel_store_get_inbox_folder_sync` call and both of its answers, owned
/// the way Camel owns them.
struct Inbox {
    folder: *mut CamelFolder,
    error: *mut GError,
}

impl Inbox {
    /// Through the public wrapper, which is how Evolution asks — and which
    /// returns NULL without even reaching the store if the class left the vfunc
    /// unset, so calling it is also what pins the override.
    fn of(store: *mut CamelStore) -> Self {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: `store` is a live store of ours, and `error` is writable and
        // currently NULL.
        let folder =
            unsafe { camel_store_get_inbox_folder_sync(store, ptr::null_mut(), &mut error) };
        Self { folder, error }
    }

    fn path(&self) -> String {
        assert!(!self.folder.is_null(), "no folder to name");
        // SAFETY: a live folder of ours, whose name it owns and outlives.
        unsafe {
            std::ffi::CStr::from_ptr(camel_folder_get_full_name(self.folder))
                .to_string_lossy()
                .into_owned()
        }
    }

    fn mailbox(&self) -> Id {
        assert!(!self.folder.is_null(), "no folder to ask");
        // SAFETY: as above; the borrow ends inside this function.
        unsafe { JmapFolder::borrow(self.folder) }
            .expect("a folder of ours")
            .mailbox()
            .expect("a folder with no mailbox behind it")
            .clone()
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        // SAFETY: the call handed over one reference to the folder and
        // ownership of the error.
        unsafe {
            if !self.folder.is_null() {
                g_object_unref(self.folder.cast());
            }
            if !self.error.is_null() {
                glib_sys::g_error_free(self.error);
            }
        }
    }
}

/// The account's inbox is the mailbox whose role says so — not the one whose
/// name does, and not a top-level one. Left to the inherited implementation
/// this account opens the decoy instead, which is a folder the user's incoming
/// filters would then run over.
#[test]
fn the_inbox_is_the_mailbox_holding_the_inbox_role() {
    let (_server, account, inbox) = with_inbox();

    let opened = Inbox::of(account.store);

    assert!(opened.error.is_null(), "opening the inbox set an error");
    assert_eq!(opened.path(), "Accounts/Posteingang");
    assert_eq!(opened.mailbox(), inbox);
}

/// Evolution opens the inbox both ways — by purpose at startup and by path
/// when the user clicks it in the folder tree — and two `CamelFolder`s over one
/// mailbox would be two summaries and two sets of flags. Going through
/// `camel_store_get_folder_sync` rather than building a folder here is what
/// puts the answer through the store's folder bag, where the first of the two
/// calls left it.
#[test]
fn the_inbox_is_the_folder_camel_already_has_open_for_that_path() {
    let (_server, account, _) = with_inbox();

    let by_purpose = Inbox::of(account.store);
    // SAFETY: a live store of ours, a NUL-terminated path, and a NULL GError **
    // which the wrapper tolerates.
    let by_path = unsafe {
        camel_store_get_folder_sync(
            account.store,
            c"Accounts/Posteingang".as_ptr(),
            CAMEL_STORE_FOLDER_NONE,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };

    assert!(!by_path.is_null(), "the path opened no folder");
    assert_eq!(
        by_purpose.folder, by_path,
        "a second folder over the same mailbox"
    );

    // SAFETY: one reference, handed over by the wrapper.
    unsafe { g_object_unref(by_path.cast()) };
}

/// An account whose server assigns no roles. RFC 8621 §2 makes `role`
/// nullable, so this is a legal account rather than a broken one — but Camel
/// asked for the inbox, and there is no such thing as half a folder. Picking
/// the mailbox called "Inbox" would be the provider guessing where the user's
/// mail arrives, and guessing wrong means new mail filtered into a folder
/// nobody reads.
#[test]
fn an_account_with_no_inbox_role_has_no_inbox_to_open() {
    let server = MockServer::builder().start();
    edit(&server, |account| account.seed_mailbox("Inbox", None));
    let account = Account::open();
    account.connect(sync_against(&server));

    let opened = Inbox::of(account.store);

    assert!(opened.folder.is_null(), "a folder for a role nobody claims");
    assert!(!opened.error.is_null(), "no reason given");
    // SAFETY: the error is the one the vfunc set, checked non-NULL above.
    unsafe {
        assert_eq!((*opened.error).domain, camel_store_error_quark());
        assert_eq!((*opened.error).code, CAMEL_STORE_ERROR_NO_FOLDER as i32);
    }
}

/// The same second look `get_folder_sync` takes, for the same reason: Camel
/// asks a store for its inbox early — it is where the incoming filters run —
/// and an account whose inbox arrived after the listing this store is holding
/// would otherwise report having none until something else refreshed it.
#[test]
fn an_inbox_that_appeared_since_the_listing_is_found_by_looking_again() {
    let server = MockServer::builder().start();
    edit(&server, |account| account.seed_mailbox("Work", None));
    let account = Account::open();
    account.connect(sync_against(&server));
    assert!(
        Inbox::of(account.store).folder.is_null(),
        "an inbox before there was one"
    );

    edit(&server, |state| {
        state.create_mailbox("Inbox", Some(role::INBOX), None)
    });

    let opened = Inbox::of(account.store);
    assert!(opened.error.is_null(), "opening the inbox set an error");
    assert_eq!(opened.path(), "Inbox");
}

/// The other NULL, told apart from the one above by the error alone.
#[test]
fn a_disconnected_store_has_no_inbox_to_open() {
    let account = Account::open();

    let opened = Inbox::of(account.store);

    assert!(opened.folder.is_null(), "a disconnected store opened one");
    assert!(!opened.error.is_null(), "no reason given");
    // SAFETY: the error is the one the vfunc set, checked non-NULL above.
    unsafe {
        assert_eq!((*opened.error).domain, camel_service_error_quark());
        assert_eq!(
            (*opened.error).code,
            CAMEL_SERVICE_ERROR_NOT_CONNECTED as i32
        );
    }
}

/// The same guard as the other two vfuncs', reached the same way: the wrapper
/// asserts `CAMEL_IS_STORE`, so a NULL instance can only arrive through the
/// class pointer.
#[test]
fn a_null_store_has_no_inbox_either() {
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: referencing the class installs the vfunc; NULL is exactly the
    // instance pointer under test, and `error` is writable and NULL.
    let folder = unsafe {
        let class = g_type_class_ref(store_type()).cast::<CamelStoreClass>();
        let vfunc = (*class).get_inbox_folder_sync.expect("the vfunc");
        let folder = vfunc(ptr::null_mut(), ptr::null_mut(), &mut error);
        g_type_class_unref(class.cast());
        folder
    };

    assert!(folder.is_null());
    assert!(!error.is_null(), "no reason given");
    // SAFETY: owned by us, set above.
    unsafe { glib_sys::g_error_free(error) };
}
