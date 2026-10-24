// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `initial_setup_sync` against a real JMAP server: the vfunc behind
//! `camel_store_initial_setup_sync`. Evolution's account wizard calls this
//! vfunc right after a store first connects.
//!
//! `tests/folders.rs` proves the vfunc's logic against `jmap-mockd`: a
//! mailbox claiming `FolderRole::Sent` or `FolderRole::Drafts` gets its Camel
//! path saved under the matching `CAMEL_STORE_SETUP_*_FOLDER` key. This test
//! confirms the same behavior against Stalwart's own default role
//! assignments, the way `tests/live_server_special_folders.rs` does for the
//! Inbox/Trash/Junk vfuncs. A freshly seeded Stalwart account already has
//! Sent and Drafts mailboxes with those roles (confirmed directly over
//! `Mailbox/get` before writing this test). So the write-test account needs
//! no setup beyond what every other live-server test here already assumes.
//!
//! ## Running it
//!
//! Same environment as the other live-server tests, see
//! `docs/manual-test-live-server.md`:
//!
//! ```console
//! $ cargo test -p jmap-mail --features testing --test live_server_initial_setup -- --ignored
//! ```
//!
//! This test is skipped, not failed, when
//! `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are unset.

mod common;

use std::ffi::CStr;
use std::ptr;

use common::Account;
use eds_sys::{
    CAMEL_STORE_SETUP_DRAFTS_FOLDER, CAMEL_STORE_SETUP_SENT_FOLDER, GHashTable,
    camel_store_initial_setup_sync,
};
use glib_sys::{GError, GTRUE, g_hash_table_destroy, g_hash_table_lookup};
use jmap_mail_sync::{FolderRole, MailSync};
use jmap_proto::session::CAPABILITY_MAIL;
use jmap_test_support::connect_for_write;

/// One `initial_setup_sync` call and its answer, owned the way Camel owns it.
/// Mirrors `jmap-mail/tests/folders.rs::Setup`.
struct Setup {
    save: *mut GHashTable,
    error: *mut GError,
    ok: glib_sys::gboolean,
}

impl Setup {
    /// Calls the public wrapper, the same way Evolution's account wizard
    /// asks. The wrapper creates the hash table that the vfunc inserts into.
    fn run(store: *mut eds_sys::CamelStore) -> Self {
        let mut save: *mut GHashTable = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: `store` is a live store of ours. Both out-parameters are
        // writable and currently NULL.
        let ok = unsafe {
            camel_store_initial_setup_sync(store, &mut save, ptr::null_mut(), &mut error)
        };
        Self { save, error, ok }
    }

    fn get(&self, key: &CStr) -> Option<String> {
        if self.save.is_null() {
            return None;
        }
        // SAFETY: `save` is a live table for the length of this call. A
        // non-NULL answer is a NUL-terminated string that the table owns.
        unsafe {
            let value = g_hash_table_lookup(self.save, key.as_ptr().cast());
            (!value.is_null()).then(|| CStr::from_ptr(value.cast()).to_string_lossy().into_owned())
        }
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        // SAFETY: if `save` is not NULL, it is the table whose ownership the
        // wrapper transferred. If `error` is not NULL, it is ours to free.
        unsafe {
            if !self.save.is_null() {
                g_hash_table_destroy(self.save);
            }
            if !self.error.is_null() {
                glib_sys::g_error_free(self.error);
            }
        }
    }
}

/// `initial_setup_sync` reports the real server's own Sent and Drafts
/// mailboxes by their Camel path. A second, independent connection's own
/// `FolderTree::role` cross-checks this, so the assertion tests the server's
/// own role assignment, not this store's cached copy of it.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn initial_setup_reports_the_real_servers_sent_and_drafts_paths() {
    let Some(store_client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let Some(check_client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };

    let store_account_id = store_client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let account = Account::open();
    account.connect(MailSync::new(store_client, store_account_id));

    let check_account_id = check_client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let check_sync = MailSync::new(check_client, check_account_id);
    let (_, tree) = check_sync
        .folder_tree()
        .expect("listing the write-test account's folder tree failed");

    let expected_sent = tree
        .role(FolderRole::Sent)
        .unwrap_or_else(|| panic!("the write-test account needs a mailbox with role Sent"));
    let expected_drafts = tree
        .role(FolderRole::Drafts)
        .unwrap_or_else(|| panic!("the write-test account needs a mailbox with role Drafts"));

    let setup = Setup::run(account.store);

    assert_eq!(
        setup.ok, GTRUE,
        "initial setup failed against the real server"
    );
    assert!(setup.error.is_null(), "initial setup set an error");
    assert_eq!(
        setup.get(CAMEL_STORE_SETUP_SENT_FOLDER),
        Some(expected_sent.path.clone()),
        "the Sent path did not match the real server's own role assignment"
    );
    assert_eq!(
        setup.get(CAMEL_STORE_SETUP_DRAFTS_FOLDER),
        Some(expected_drafts.path.clone()),
        "the Drafts path did not match the real server's own role assignment"
    );
}
