// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapStore::messages` and `JmapStore::messages_since`: the listing and the
//! delta that fill a folder's summary, routed to the mailbox's own account
//! when it belongs to a shared account merged into the folder tree.

mod common;

use std::ffi::{CStr, CString};
use std::ptr;

use common::{Account, sync_against};
use eds_sys::{
    camel_folder_get_folder_summary, camel_folder_summary_get, camel_message_info_get_subject,
    camel_store_get_folder_sync,
};
use glib_sys::GError;
use gobject_sys::g_object_unref;
use jmap_mail::store::JmapStore;
use jmap_mail_sync::MessageUpdate;
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;

/// No flags: the tree the store already has, with no new request. Matches
/// `subscriptions.rs`'s `CACHED`.
const CACHED: eds_sys::CamelStoreGetFolderInfoFlags = 0;

/// ROADMAP item 179(d): `Email/query` and `Email/get` are method calls under
/// one `accountId`, so listing a
/// [`jmap_mail_sync::FolderTree::merge_shared`] mailbox must reach that
/// mailbox's own account, not the account this store otherwise addresses.
/// The caller names the owner, as the folder Camel opened does. The test
/// below of the id collision takes the same route through a real folder.
#[test]
fn messages_lists_the_shared_mailboxs_owning_account() {
    let server = MockServer::builder().start();

    let shared_account_id = Id::new("agent-shared-owner");
    let shared_inbox_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let sharer = state.add_account(shared_account_id.clone(), "Bob");
        sharer.is_personal = false;
        let inbox = sharer.seed_mailbox("Inbox", None);
        sharer.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Bob", "bob@example.com"),
            "Lunch?",
            "One o'clock.",
            "2026-01-15T09:30:00Z",
        ));
        inbox
    };

    let store = JmapStore::detached();
    store.store_connection(sync_against(&server));
    store.folders(CACHED).expect("listed");

    let (_, messages) = store
        .messages(&shared_inbox_id, Some(&shared_account_id))
        .expect("the listing to reach the shared mailbox's own account");

    assert_eq!(
        messages.len(),
        1,
        "the message lives in the shared account, not this store's own"
    );
}

/// ROADMAP item 179(d), the delta half: `Email/changes` and `Email/get` are
/// also method calls under one `accountId`, so [`JmapStore::messages_since`]
/// needs the same routing as [`JmapStore::messages`], from the same caller.
#[test]
fn messages_since_reads_the_shared_mailboxs_owning_account() {
    let server = MockServer::builder().start();

    let shared_account_id = Id::new("agent-shared-owner");
    let shared_inbox_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let sharer = state.add_account(shared_account_id.clone(), "Bob");
        sharer.is_personal = false;
        sharer.seed_mailbox("Inbox", None)
    };

    let store = JmapStore::detached();
    store.store_connection(sync_against(&server));
    store.folders(CACHED).expect("listed");

    let (since, _) = store
        .messages(&shared_inbox_id, Some(&shared_account_id))
        .expect("the first listing of the shared mailbox");

    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let sharer = state.account_mut(&shared_account_id).unwrap();
        sharer.deliver_email(EmailSeed::new(
            shared_inbox_id.clone(),
            ("Bob", "bob@example.com"),
            "Lunch?",
            "One o'clock.",
            "2026-01-15T09:30:00Z",
        ));
    }

    let update = store
        .messages_since(&shared_inbox_id, Some(&shared_account_id), &since, 0)
        .expect("the delta to reach the shared mailbox's own account");

    match update {
        MessageUpdate::Changed { present, .. } => {
            assert_eq!(present.len(), 1, "the new message must be reported");
        }
        MessageUpdate::Relisted { messages, .. } => {
            assert_eq!(messages.len(), 1, "the new message must be reported");
        }
        other => panic!("expected the new message to be reported, got {other:?}"),
    }
}

/// The id collision, reproduced through the folder Camel opens.
///
/// Mailbox ids are unique inside one account only. Two accounts therefore
/// number their mailboxes from the same first id, and a shared account's
/// mailbox id can equal an id of the store's own account. A lookup by
/// mailbox id alone ([`jmap_mail_sync::FolderTree::owning_account`]) then
/// answers with the first match in the merged tree, which is the store's own
/// folder, and the user sees their own messages in a folder that shows a
/// colleague's name.
///
/// The folder is not ambiguous: Camel opens it by path, and the path names
/// one account. The folder must therefore carry the account it was opened
/// for, and the listing must use that account.
#[test]
fn a_shared_mailbox_lists_its_own_account_when_the_mailbox_ids_collide() {
    let server = MockServer::builder().start();

    let shared_account_id = Id::new("agent-shared-owner");
    let (own_mailbox_id, own_uid, shared_mailbox_id, shared_uid) = {
        let state = server.state();
        let mut state = state.lock().unwrap();

        let own = state
            .account_mut(&server.account_id())
            .expect("the mock's own account");
        let own_mailbox_id = own.seed_mailbox("Inbox", None);
        let own_uid = own.seed_email(EmailSeed::new(
            own_mailbox_id.clone(),
            ("Alice", "alice@example.com"),
            "Own message",
            "In the user's own inbox.",
            "2026-01-15T09:30:00Z",
        ));

        let sharer = state.add_account(shared_account_id.clone(), "agent-owner@example.test");
        sharer.is_personal = false;
        let shared_mailbox_id = sharer.seed_mailbox("Inbox", None);
        let shared_uid = sharer.seed_email(EmailSeed::new(
            shared_mailbox_id.clone(),
            ("Bob", "bob@example.com"),
            "Shared message",
            "In the shared inbox.",
            "2026-01-15T10:30:00Z",
        ));

        (own_mailbox_id, own_uid, shared_mailbox_id, shared_uid)
    };
    assert_eq!(
        own_mailbox_id, shared_mailbox_id,
        "the two accounts must number their mailboxes from the same id"
    );

    let account = Account::open();
    account.connect(sync_against(&server));
    account.jmap().folders(CACHED).expect("listed");

    let path = CString::new("Shared/agent-owner@example.test/Inbox").expect("a path with no NUL");
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live store and a NUL-terminated path. The first open
    // refreshes the folder, which is the listing under test.
    let folder = unsafe {
        camel_store_get_folder_sync(account.store, path.as_ptr(), 0, ptr::null_mut(), &mut error)
    };
    assert!(!folder.is_null(), "the shared mailbox did not open");

    // The message ids collide for the same reason the mailbox ids do, so the
    // uid alone cannot tell the two messages apart. The subject can.
    assert_eq!(
        own_uid, shared_uid,
        "the two messages must have the same uid"
    );
    let uid = CString::new(shared_uid.as_str()).expect("a uid with no NUL");
    // SAFETY: the open above refreshed the folder, so its summary holds the
    // rows of the listing. Nothing else touches them.
    let subject = unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        let row = camel_folder_summary_get(summary, uid.as_ptr());
        assert!(!row.is_null(), "the shared mailbox listed no message");
        let subject = camel_message_info_get_subject(row);
        assert!(!subject.is_null(), "the row has no subject");
        let subject = CStr::from_ptr(subject).to_string_lossy().into_owned();
        g_object_unref(row.cast());
        // SAFETY: the one reference `camel_store_get_folder_sync` returned.
        g_object_unref(folder.cast());
        subject
    };

    assert_eq!(
        subject, "Shared message",
        "the shared mailbox showed the user's own mailbox, so the listing used the wrong account"
    );
}
