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
    CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY, CAMEL_SERVICE_ERROR_NOT_CONNECTED,
    CAMEL_STORE_ERROR_NO_FOLDER, CAMEL_STORE_FOLDER_INFO_RECURSIVE,
    CAMEL_STORE_FOLDER_INFO_REFRESH, CAMEL_STORE_FOLDER_INFO_SUBSCRIBED,
    CAMEL_STORE_FOLDER_INFO_SUBSCRIPTION_LIST, CAMEL_STORE_FOLDER_NONE, CamelFolder,
    CamelFolderInfo, CamelStore, CamelStoreClass, camel_folder_get_flags,
    camel_folder_get_full_name, camel_folder_info_free, camel_offline_store_get_type,
    camel_service_error_quark, camel_store_can_refresh_folder, camel_store_error_quark,
    camel_store_get_folder_info_sync, camel_store_get_folder_sync,
    camel_store_get_inbox_folder_sync, camel_store_get_junk_folder_sync,
    camel_store_get_trash_folder_sync,
};
use glib_sys::{GError, GFALSE};
use gobject_sys::{g_object_unref, g_type_class_peek, g_type_class_ref, g_type_class_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::connect::StoreError;
use jmap_mail::folder::JmapFolder;
use jmap_mail::folder_info::FolderInfoChain;
use jmap_mail::folders::Request;
use jmap_mail::store::{JmapStore, store_type};
use jmap_mail::subscribe;
use jmap_mail_sync::{FolderInfo, FolderTree, MailSync};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::{Mailbox, role};

/// No flags at all: what Camel passes when it wants the tree it was given last
/// time.
const CACHED: eds_sys::CamelStoreGetFolderInfoFlags = 0;
const REFRESH: eds_sys::CamelStoreGetFolderInfoFlags = CAMEL_STORE_FOLDER_INFO_REFRESH;
/// Every real caller in Camel and Evolution sets this one; the two that do not
/// are `camel_store_get_folder_info_sync`'s own virtual-folder paths.
const RECURSIVE: eds_sys::CamelStoreGetFolderInfoFlags = CAMEL_STORE_FOLDER_INFO_RECURSIVE;
/// What Evolution's folder tree adds for a store that is `CamelSubscribable`:
/// only the folders the user ticked.
const SUBSCRIBED: eds_sys::CamelStoreGetFolderInfoFlags = CAMEL_STORE_FOLDER_INFO_SUBSCRIBED;
/// And what its subscription editor asks with: every folder there is to tick.
const SUBSCRIPTION_LIST: eds_sys::CamelStoreGetFolderInfoFlags =
    CAMEL_STORE_FOLDER_INFO_SUBSCRIPTION_LIST;

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

// ---------------------------------------------------------------------------
// the ticks the user set, as a filter on that part

/// The shape [`hand_built`] has, with only the named mailboxes ticked.
///
/// Every mailbox says which it is rather than leaving the property out: RFC
/// 8621 §2 gives `isSubscribed` no default, and `jmap-mail-sync` reads a
/// missing one as a tick, so an omission here would be the opposite of what a
/// test about unsubscribed folders wants.
fn ticked(subscribed: &[&str]) -> FolderTree {
    let mailbox = |id: &str, name: &str, parent: Option<&str>| Mailbox {
        id: Some(Id::new(id)),
        name: name.to_owned(),
        parent_id: parent.map(Id::new),
        is_subscribed: Some(subscribed.contains(&name)),
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

/// Every path one request asks for, parents before children.
///
/// [`requested`] reads the roots alone, which is all the `top` and the depth
/// can change; a filter changes what hangs below them too, so these tests need
/// the whole of what was asked for.
fn requested_paths(
    tree: &FolderTree,
    top: Option<&str>,
    flags: eds_sys::CamelStoreGetFolderInfoFlags,
) -> Vec<String> {
    let request = Request::new(tree, top, flags);
    let mut paths = Vec::new();
    let mut pending: Vec<&FolderInfo> = request.roots.iter().rev().collect();
    while let Some(folder) = pending.pop() {
        paths.push(folder.path.clone());
        pending.extend(folder.children.iter().rev());
    }
    paths
}

/// The baseline the rest of this section is measured against: the ticks are a
/// property of the folders either way, and without the flag they change
/// nothing about which of them the call asks for.
#[test]
fn the_ticks_change_nothing_when_the_flag_is_not_set() {
    let tree = ticked(&["Personal"]);

    assert_eq!(
        requested_paths(&tree, None, RECURSIVE),
        ["Personal", "Work", "Work/Invoices", "Work/Invoices/Paid"]
    );
}

/// And with it, the folders the user unticked are gone. This is the flag
/// Evolution's folder tree passes for a store that is `CamelSubscribable`, so
/// it is the whole reason the tick in the subscription editor changes what the
/// user sees.
#[test]
fn the_subscribed_flag_leaves_out_the_folders_the_user_unticked() {
    let tree = ticked(&["Personal", "Work"]);

    assert_eq!(
        requested_paths(&tree, None, RECURSIVE | SUBSCRIBED),
        ["Personal", "Work"]
    );
}

/// An unticked folder with a ticked one below it stays, because dropping it
/// would drop the ticked folder with it: `CamelFolderInfo` hangs a child off
/// its parent, so there is no answer in which `Work/Invoices` is present and
/// `Work` is not.
#[test]
fn an_unticked_folder_stays_when_something_below_it_is_ticked() {
    let tree = ticked(&["Invoices"]);

    assert_eq!(
        requested_paths(&tree, None, RECURSIVE | SUBSCRIBED),
        ["Work", "Work/Invoices"]
    );
}

/// Every level of it, not just the one immediately above.
#[test]
fn every_level_above_a_ticked_folder_is_kept() {
    let tree = ticked(&["Paid"]);

    assert_eq!(
        requested_paths(&tree, None, RECURSIVE | SUBSCRIBED),
        ["Work", "Work/Invoices", "Work/Invoices/Paid"]
    );
}

/// And a folder kept only for what is below it is still described as unticked
/// — it is in the answer because of its children, and saying otherwise would
/// put a tick in the subscription editor the user never set.
#[test]
fn a_folder_kept_only_for_its_children_is_not_called_subscribed() {
    let tree = ticked(&["Invoices"]);

    let request = Request::new(&tree, None, RECURSIVE | SUBSCRIBED);

    let work = request.roots.first().expect("Work is in the answer");
    assert_eq!(work.path, "Work");
    assert!(!work.subscribed, "a folder the user never ticked");
}

/// The other direction of the same question, and the one place the filter and
/// the depth cut visibly differ. `from_forest` deliberately leaves
/// `CAMEL_FOLDER_CHILDREN` on a folder whose children the *depth* left out —
/// they exist, and the expander is how the caller asks for them. Children the
/// *ticks* left out are not part of this view at all, so the folder has none.
#[test]
fn a_folder_whose_children_are_all_unticked_has_none_in_the_answer() {
    let tree = ticked(&["Work"]);

    let request = Request::new(&tree, None, RECURSIVE | SUBSCRIBED);

    let work = request.roots.first().expect("Work is in the answer");
    assert_eq!(work.path, "Work");
    assert!(work.children.is_empty(), "an unticked child came along");
}

/// An account the user has unticked entirely asks for nothing — which Camel
/// reads as a NULL chain with no error, the same as a `top` that names no
/// folder.
#[test]
fn an_account_with_nothing_ticked_asks_for_nothing() {
    let tree = ticked(&[]);

    assert!(requested_paths(&tree, None, RECURSIVE | SUBSCRIBED).is_empty());
}

/// The filter is applied to the subtree `top` names rather than instead of it:
/// a `top` whose subtree holds nothing ticked asks for nothing, even though
/// the account has ticked folders elsewhere.
#[test]
fn a_top_with_nothing_ticked_below_it_asks_for_nothing() {
    let tree = ticked(&["Personal"]);

    assert!(requested_paths(&tree, Some("Work"), RECURSIVE | SUBSCRIBED).is_empty());
}

/// And a `top` that does is filtered like any other root, ancestors and all.
#[test]
fn a_top_is_filtered_like_any_other_root() {
    let tree = ticked(&["Work", "Paid"]);

    assert_eq!(
        requested_paths(&tree, Some("Work"), RECURSIVE | SUBSCRIBED),
        ["Work", "Work/Invoices", "Work/Invoices/Paid"]
    );
}

/// The depth is still the depth. `Work` is in the answer only for the ticked
/// folder below it, and the caller asked for one level: the depth the request
/// carries is the one an ordinary parent would have got, and cutting to it
/// stays `from_forest`'s job rather than becoming the filter's.
#[test]
fn the_filter_does_not_change_the_depth_the_call_asks_for() {
    let tree = ticked(&["Invoices"]);

    assert_eq!(
        requested(&tree, None, SUBSCRIBED),
        (vec!["Work".to_owned()], Some(0))
    );
}

/// `SUBSCRIPTION_LIST` is the subscription editor's own question — "which
/// folders are there to tick" — so it is answered with all of them. For JMAP
/// that is the listing this store already has: `Mailbox/get` returns every
/// mailbox of the account with its `isSubscribed`, so there is no second,
/// wider request to make the way an IMAP store makes `LIST` beside `LSUB`.
#[test]
fn the_subscription_list_flag_asks_for_every_folder() {
    let tree = ticked(&["Personal"]);

    assert_eq!(
        requested_paths(&tree, None, RECURSIVE | SUBSCRIPTION_LIST),
        ["Personal", "Work", "Work/Invoices", "Work/Invoices/Paid"]
    );
}

/// And it outranks `SUBSCRIBED` if a caller sets both, because a subscription
/// editor showing only the folders that are already ticked is one the user
/// cannot tick anything new in.
#[test]
fn the_subscription_list_flag_outranks_the_subscribed_one() {
    let tree = ticked(&["Personal"]);

    assert_eq!(
        requested_paths(&tree, None, RECURSIVE | SUBSCRIPTION_LIST | SUBSCRIBED),
        requested_paths(&tree, None, RECURSIVE | SUBSCRIPTION_LIST)
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

/// The subscription flag reaching the answer, against a real account rather
/// than a hand-built tree: the ticks come off through the same
/// `Mailbox/set` the subscription editor writes, and the folder that keeps
/// `Work` in the answer is the ticked one below it.
#[test]
fn the_vfunc_leaves_out_the_folders_the_user_unticked() {
    let (_server, store) = nested();
    subscribe::set_subscribed(&store, "Work", false).expect("unticked");
    subscribe::set_subscribed(&store, "Work/Invoices/Paid", false).expect("unticked");

    let answered = Answered::of(&store, None, RECURSIVE | SUBSCRIBED);

    assert!(
        answered.error.is_null(),
        "a successful listing set an error"
    );
    assert_eq!(answered.paths(), ["Inbox", "Work", "Work/Invoices"]);
}

/// And the editor's own question, which has to reach the vfunc as the *whole*
/// account or the user has no way to tick a folder back on.
#[test]
fn the_vfunc_answers_the_subscription_list_with_every_folder() {
    let (_server, store) = nested();
    subscribe::set_subscribed(&store, "Work/Invoices", false).expect("unticked");

    let answered = Answered::of(&store, None, RECURSIVE | SUBSCRIPTION_LIST);

    assert_eq!(
        answered.paths(),
        ["Inbox", "Work", "Work/Invoices", "Work/Invoices/Paid"]
    );
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
// and the three folders Camel asks for by purpose rather than by name

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

/// One "open the folder for this purpose" call and both of its answers, owned
/// the way Camel owns them.
///
/// The three wrappers have one signature, so one type covers all of them; which
/// of the three was asked is the constructor.
struct Purpose {
    folder: *mut CamelFolder,
    error: *mut GError,
}

impl Purpose {
    /// Through the public wrapper, which is how Evolution asks — and which
    /// returns NULL without even reaching the store if the class left the vfunc
    /// unset, so calling it is also what pins the override.
    fn asked(
        store: *mut CamelStore,
        wrapper: unsafe extern "C" fn(
            *mut CamelStore,
            *mut gio_sys::GCancellable,
            *mut *mut GError,
        ) -> *mut CamelFolder,
    ) -> Self {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: `store` is a live store of ours or NULL — which every one of
        // these wrappers type-checks — and `error` is writable and currently
        // NULL.
        let folder = unsafe { wrapper(store, ptr::null_mut(), &mut error) };
        Self { folder, error }
    }

    fn inbox(store: *mut CamelStore) -> Self {
        Self::asked(store, camel_store_get_inbox_folder_sync)
    }

    fn trash(store: *mut CamelStore) -> Self {
        Self::asked(store, camel_store_get_trash_folder_sync)
    }

    fn junk(store: *mut CamelStore) -> Self {
        Self::asked(store, camel_store_get_junk_folder_sync)
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

    /// What Camel believes about the folder it handed back.
    fn flags(&self) -> eds_sys::CamelFolderFlags {
        assert!(!self.folder.is_null(), "no folder to ask");
        // SAFETY: a live folder of ours.
        unsafe { camel_folder_get_flags(self.folder) }
    }
}

impl Drop for Purpose {
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

    let opened = Purpose::inbox(account.store);

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

    let by_purpose = Purpose::inbox(account.store);
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

    let opened = Purpose::inbox(account.store);

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
        Purpose::inbox(account.store).folder.is_null(),
        "an inbox before there was one"
    );

    edit(&server, |state| {
        state.create_mailbox("Inbox", Some(role::INBOX), None)
    });

    let opened = Purpose::inbox(account.store);
    assert!(opened.error.is_null(), "opening the inbox set an error");
    assert_eq!(opened.path(), "Inbox");
}

/// The other NULL, told apart from the one above by the error alone.
#[test]
fn a_disconnected_store_has_no_inbox_to_open() {
    let account = Account::open();

    let opened = Purpose::inbox(account.store);

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

// ---------------------------------------------------------------------------
// and the other two: where deleted mail and spam are delivered

/// An account whose trash and junk are named and placed the way a real server
/// is free to name and place them: in the user's own language, nested under
/// another mailbox — and with decoys called `Trash` and `Junk` at the top level
/// claiming no role at all.
///
/// The decoys are what a provider that matched on names would open, and what
/// makes this account tell the two apart. RFC 8621 §2 gives a mailbox a `role`;
/// its name is a label for the user.
fn with_trash_and_junk() -> (MockServer, Account, Id, Id) {
    let server = MockServer::builder().start();
    let (trash, junk) = edit(&server, |account| {
        account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_mailbox("Trash", None);
        account.seed_mailbox("Junk", None);
        let system = account.seed_mailbox("System", None);
        (
            account.seed_child_mailbox("Papierkorb", Some(role::TRASH), &system),
            account.seed_child_mailbox("Werbung", Some(role::JUNK), &system),
        )
    });
    let account = Account::open();
    account.connect(sync_against(&server));
    (server, account, trash, junk)
}

/// "The folder in @store into which trash is delivered", which for a JMAP
/// account is the mailbox holding the `trash` role and nothing else. Camel's own
/// answer — the one this override replaces — is a virtual folder over the
/// `DELETED` flag, and that flag is local to this client: no JMAP keyword
/// carries it, so a message another client moved to trash is not in it and a
/// message this one deleted is in no folder any other client can see.
#[test]
fn the_trash_is_the_mailbox_holding_the_trash_role() {
    let (_server, account, trash, _) = with_trash_and_junk();

    let opened = Purpose::trash(account.store);

    assert!(opened.error.is_null(), "opening the trash set an error");
    assert_eq!(opened.path(), "System/Papierkorb");
    assert_eq!(opened.mailbox(), trash);
    // And nothing about the folder says "trash" yet: Camel's wrapper does not
    // mark what a store hands it, so `CAMEL_FOLDER_IS_TRASH` stays off until
    // something sets it. `crate::folder`'s `flags` is where that decision will
    // have to be taken, and it is not this increment's — see the note there.
    assert_eq!(opened.flags(), CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY);
}

/// And the same for junk. `$junk` *is* a JMAP keyword, so Camel's virtual folder
/// over the `JUNK` flag would not be empty here — it would be a second, and
/// differently populated, spam folder sitting next to the account's own: the
/// server files spam into the mailbox, and marking a message read on a phone
/// does not move it out of a search.
#[test]
fn the_junk_is_the_mailbox_holding_the_junk_role() {
    let (_server, account, _, junk) = with_trash_and_junk();

    let opened = Purpose::junk(account.store);

    assert!(opened.error.is_null(), "opening the junk set an error");
    assert_eq!(opened.path(), "System/Werbung");
    assert_eq!(opened.mailbox(), junk);
    assert_eq!(opened.flags(), CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY);
}

/// The same folder Camel already has open for that path, for the reason the
/// inbox has to be: Evolution reaches the trash both ways — by purpose when the
/// user empties it, by path when they click it — and two `CamelFolder`s over one
/// mailbox would be two summaries and two sets of flags.
#[test]
fn the_trash_is_the_folder_camel_already_has_open_for_that_path() {
    let (_server, account, _, _) = with_trash_and_junk();

    let by_purpose = Purpose::trash(account.store);
    // SAFETY: a live store of ours, a NUL-terminated path, and a NULL GError **
    // which the wrapper tolerates.
    let by_path = unsafe {
        camel_store_get_folder_sync(
            account.store,
            c"System/Papierkorb".as_ptr(),
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

/// An account whose server assigns neither role — legal, like the role-less
/// inbox, and answered the same way. Camel documents NULL as meaning "no such
/// folder exists" as well as "it went wrong", but the error is set regardless:
/// the account plainly has mailboxes called `Trash` and `Junk`, and a silent
/// NULL would leave nothing anywhere saying why neither of them is the trash.
#[test]
fn an_account_with_no_trash_or_junk_role_has_neither_to_open() {
    let server = MockServer::builder().start();
    edit(&server, |account| {
        account.seed_mailbox("Trash", None);
        account.seed_mailbox("Junk", None)
    });
    let account = Account::open();
    account.connect(sync_against(&server));

    for opened in [Purpose::trash(account.store), Purpose::junk(account.store)] {
        assert!(opened.folder.is_null(), "a folder for a role nobody claims");
        assert!(!opened.error.is_null(), "no reason given");
        // SAFETY: the error is the one the vfunc set, checked non-NULL above.
        unsafe {
            assert_eq!((*opened.error).domain, camel_store_error_quark());
            assert_eq!((*opened.error).code, CAMEL_STORE_ERROR_NO_FOLDER as i32);
        }
    }
}

/// The second look the other two openers take, for the same reason: a mailbox
/// the user made in webmail after this store listed the account is one the
/// server would file deleted mail into, and reporting no trash until something
/// else refreshed would send the next delete nowhere.
#[test]
fn a_trash_that_appeared_since_the_listing_is_found_by_looking_again() {
    let server = MockServer::builder().start();
    edit(&server, |account| account.seed_mailbox("Inbox", None));
    let account = Account::open();
    account.connect(sync_against(&server));
    assert!(
        Purpose::trash(account.store).folder.is_null(),
        "a trash before there was one"
    );

    edit(&server, |state| {
        state.create_mailbox("Bin", Some(role::TRASH), None)
    });

    let opened = Purpose::trash(account.store);
    assert!(opened.error.is_null(), "opening the trash set an error");
    assert_eq!(opened.path(), "Bin");
}

/// The other NULL, told apart from the one above by the error alone.
#[test]
fn a_disconnected_store_has_no_trash_or_junk_to_open() {
    let account = Account::open();

    for opened in [Purpose::trash(account.store), Purpose::junk(account.store)] {
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
}

/// And the same guard as every other vfunc here, reached through the class
/// because the wrappers assert `CAMEL_IS_STORE`.
#[test]
fn a_null_store_has_no_trash_or_junk_either() {
    // SAFETY: referencing the class installs the vfuncs; NULL is exactly the
    // instance pointer under test, and each `error` is writable and NULL.
    unsafe {
        let class = g_type_class_ref(store_type()).cast::<CamelStoreClass>();
        let vfuncs = [
            (*class).get_trash_folder_sync.expect("the trash vfunc"),
            (*class).get_junk_folder_sync.expect("the junk vfunc"),
        ];
        for vfunc in vfuncs {
            let mut error: *mut GError = ptr::null_mut();
            let folder = vfunc(ptr::null_mut(), ptr::null_mut(), &mut error);
            assert!(folder.is_null());
            assert!(!error.is_null(), "no reason given");
            glib_sys::g_error_free(error);
        }
        g_type_class_unref(class.cast());
    }
}

/// The other half of the same decision, and the visible one: Camel's listing
/// *wrapper* adds `.#evolution/Trash` and `.#evolution/Junk` to whatever a
/// store answers with, for as long as the store's flags claim it wants them.
/// A JMAP account that kept them would show the user two trash folders — the
/// server's, where their phone puts deleted mail, and a search over a flag only
/// this client ever sets — and two junk folders, disagreeing about what spam is.
///
/// Through the wrapper rather than the vfunc, because the virtual folders are
/// the wrapper's doing: the vfunc never sees them.
#[test]
fn the_listing_offers_no_virtual_trash_or_junk_beside_the_accounts_own() {
    let (_server, account, _, _) = with_trash_and_junk();

    let mut paths = Vec::new();
    // SAFETY: a live store of ours, a NULL `top` — the whole account — and an
    // error out-parameter the wrapper tolerates being NULL. The forest is
    // walked and then freed with the function Camel's contract names.
    unsafe {
        let head = camel_store_get_folder_info_sync(
            account.store,
            ptr::null(),
            RECURSIVE,
            ptr::null_mut(),
            ptr::null_mut(),
        );
        collect(head, &mut paths);
        camel_folder_info_free(head);
    }

    assert_eq!(
        paths,
        [
            "Inbox",
            "Junk",
            "System",
            "System/Papierkorb",
            "System/Werbung",
            "Trash"
        ],
        "the listing gained a folder the account does not have"
    );
}

// ---------------------------------------------------------------------------
// which folders Send / Receive checks for new mail

/// A listing with all four cases in it: the inbox, a folder the user ticked,
/// one they unticked, and one kept in the answer only because something below
/// it is ticked.
///
/// Every mailbox says whether it is subscribed rather than leaving the property
/// out, for the reason [`ticked`] gives.
fn checkable() -> FolderTree {
    let mailbox =
        |id: &str, name: &str, parent: Option<&str>, role: Option<&str>, ticked| Mailbox {
            id: Some(Id::new(id)),
            name: name.to_owned(),
            parent_id: parent.map(Id::new),
            role: role.map(str::to_owned),
            is_subscribed: Some(ticked),
            ..Mailbox::default()
        };
    FolderTree::from_mailboxes(&[
        mailbox("M1", "Inbox", None, Some(role::INBOX), true),
        mailbox("M2", "Lists", None, None, true),
        mailbox("M3", "Old", None, None, false),
        mailbox("M4", "Work", None, None, false),
        mailbox("M5", "Invoices", Some("M4"), None, true),
    ])
    .expect("a well-formed mailbox list")
}

/// The folders of a listing that Evolution would check for new mail, by path.
///
/// This is `get_folders` out of Evolution's `mail-send-recv.c`, which walks the
/// forest `get_folder_info_sync` answered with and asks
/// `camel_store_can_refresh_folder` about each info in it. Going through the
/// wrapper rather than the class pointer because that is the call Evolution
/// makes, and it is the wrapper that refuses a store whose slot is empty.
fn checked(tree: &FolderTree, store: *mut CamelStore) -> Vec<String> {
    let chain = FolderInfoChain::from_tree(tree);
    let mut checked = Vec::new();
    // SAFETY: the chain is the forest just built and still owned here, so every
    // info in it is live and its `full_name` is NUL-terminated; `store` is a
    // real `CamelStore` of ours, which is what the wrapper type-checks; the
    // error out-parameter is NULL, which the wrapper tolerates.
    unsafe {
        walk(chain.as_ptr(), &mut |info| {
            if camel_store_can_refresh_folder(store, info, ptr::null_mut()) != GFALSE {
                checked.push(
                    std::ffi::CStr::from_ptr((*info).full_name)
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        })
    };
    checked
}

/// Depth-first over a sibling chain and its children, parents first — the order
/// Evolution's `get_folders` walks one in.
///
/// # Safety
///
/// `head` is NULL or the head of a live `CamelFolderInfo` sibling chain.
unsafe fn walk(head: *mut CamelFolderInfo, visit: &mut impl FnMut(*mut CamelFolderInfo)) {
    let mut info = head;
    while !info.is_null() {
        visit(info);
        unsafe {
            walk((*info).child, visit);
            info = (*info).next;
        }
    }
}

/// What the vfunc is *for*: Evolution asks it once per folder to build the list
/// Send / Receive refreshes, and a folder that answers no is one whose new mail
/// the user never hears about until they click it.
///
/// The rule is the inbox plus the folders the user ticked. Two of the four
/// cases are the interesting ones. `Old` is unticked and stays out, which is
/// the whole point of a tick. `Work` is unticked too and is only in the listing
/// at all because `Work/Invoices` under it is ticked — see `ticked` in
/// `crate::folders` — so it is a folder the user does not see and must not be
/// checked, while the ticked folder below it must.
#[test]
fn send_receive_checks_the_inbox_and_the_folders_the_user_ticked() {
    let account = Account::open();

    assert_eq!(
        checked(&checkable(), account.store),
        ["Inbox", "Lists", "Work/Invoices"]
    );
}

/// The one folder that is checked whether or not it is ticked. Mail arrives in
/// the inbox by definition, an account whose inbox is never checked is one that
/// never reports new mail at all, and this is the rule `CamelStore`'s own
/// default implements — the override widens it rather than replacing it.
#[test]
fn the_inbox_is_checked_even_when_the_user_unticked_it() {
    let account = Account::open();
    let tree = FolderTree::from_mailboxes(&[Mailbox {
        id: Some(Id::new("M1")),
        name: "Inbox".to_owned(),
        role: Some(role::INBOX.to_owned()),
        is_subscribed: Some(false),
        ..Mailbox::default()
    }])
    .expect("a well-formed mailbox list");

    assert_eq!(checked(&tree, account.store), ["Inbox"]);
}

/// And why the override has to exist at all: left to `CamelStore`'s inherited
/// answer, a ticked folder that is not the inbox is not checked for new mail.
///
/// Called on the parent class directly, so a Camel that widened its own default
/// fails here with a sentence rather than leaving this provider carrying an
/// override nobody needs any more.
#[test]
fn the_inherited_answer_checks_nothing_but_the_inbox() {
    let account = Account::open();
    let chain = FolderInfoChain::from_tree(&checkable());
    let mut checked = Vec::new();

    // SAFETY: the store type is registered and its class initialised, so the
    // parent's class is live; every info in the chain is owned here; the error
    // out-parameter is NULL, which the default tolerates.
    unsafe {
        let class = g_type_class_ref(store_type());
        let parent = g_type_class_peek(camel_offline_store_get_type()).cast::<CamelStoreClass>();
        assert!(!parent.is_null(), "the parent class is not initialised");
        let inherited = (*parent)
            .can_refresh_folder
            .expect("CamelStore stopped answering the refresh question");
        walk(chain.as_ptr(), &mut |info| {
            if inherited(account.store, info, ptr::null_mut()) != GFALSE {
                checked.push(
                    std::ffi::CStr::from_ptr((*info).full_name)
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        });
        g_type_class_unref(class);
    }

    assert_eq!(
        checked,
        ["Inbox"],
        "CamelStore's default has changed; the override may no longer be needed"
    );
}

/// Opening a folder that has server-side messages answers a non-empty summary
/// on the very first open, without requiring a second open or manual refresh.
///
/// Operator-observed 2026-08-27 on a freshly added account: the first view of
/// the Inbox listed "no messages" until cycling away and back.
#[test]
fn opening_a_folder_with_messages_answers_non_empty_summary_on_first_open() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Alice", "alice@example.com"),
            "Welcome",
            "Welcome to JMAP!",
            "2026-01-01T09:00:00Z",
        ));
        account.seed_email(EmailSeed::new(
            inbox,
            ("Bob", "bob@example.com"),
            "Hello",
            "Hello world!",
            "2026-01-02T10:00:00Z",
        ));
    }

    let account = Account::open();
    account.connect(sync_against(&server));

    let path = CString::new("Inbox").expect("a path with no NUL");
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live store of ours, a NUL-terminated path, and an error out-parameter.
    let folder = unsafe {
        camel_store_get_folder_sync(
            account.store,
            path.as_ptr(),
            CAMEL_STORE_FOLDER_NONE,
            ptr::null_mut(),
            &mut error,
        )
    };
    assert!(!folder.is_null(), "opening folder returned NULL");
    assert!(error.is_null(), "opening folder set error");

    // On the FIRST open (no second open, no manual refresh), summary must contain the 2 messages.
    unsafe {
        assert_eq!(
            eds_sys::camel_folder_get_message_count(folder),
            2,
            "opening a folder with server-side messages must answer a non-empty summary on the first open"
        );
        let array = eds_sys::compat::folder_dup_uids(folder);
        assert!(!array.is_null());
        assert_eq!((*array).len, 2);
        eds_sys::compat::folder_free_uids(folder, array);
        g_object_unref(folder.cast());
    }
}
