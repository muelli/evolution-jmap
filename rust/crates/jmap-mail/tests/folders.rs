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

use std::sync::Arc;
use std::time::Duration;

use eds_sys::{
    CAMEL_SERVICE_ERROR_NOT_CONNECTED, CAMEL_STORE_FOLDER_INFO_REFRESH, camel_service_error_quark,
};
use jmap_client::{Client, Credentials};
use jmap_mail::connect::StoreError;
use jmap_mail::store::JmapStore;
use jmap_mail_sync::{FolderTree, MailSync};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::mail::role;

/// No flags at all: what Camel passes when it wants the tree it was given last
/// time.
const CACHED: eds_sys::CamelStoreGetFolderInfoFlags = 0;
const REFRESH: eds_sys::CamelStoreGetFolderInfoFlags = CAMEL_STORE_FOLDER_INFO_REFRESH;

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
