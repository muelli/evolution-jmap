// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `initial_setup_sync` against a real JMAP server: the vfunc behind
//! `camel_store_initial_setup_sync`, which Evolution's account wizard calls
//! right after a store first connects.
//!
//! `tests/folders.rs` proves the vfunc's logic against `jmap-mockd`: a
//! mailbox claiming `FolderRole::Sent` or `FolderRole::Drafts` gets its Camel
//! path saved under the matching `CAMEL_STORE_SETUP_*_FOLDER` key. This is
//! the same confirmation against Stalwart's own default role assignments,
//! the way `tests/live_server_special_folders.rs` does for the Inbox/Trash/
//! Junk vfuncs: a freshly seeded Stalwart account already has Sent and
//! Drafts mailboxes with those roles (confirmed directly over `Mailbox/get`
//! before writing this test), so the write-test account needs no setup
//! beyond what every other live-server test here already assumes.
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
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

mod common;

use std::env;
use std::ffi::CStr;
use std::ptr;

use common::Account;
use eds_sys::{
    CAMEL_STORE_SETUP_DRAFTS_FOLDER, CAMEL_STORE_SETUP_SENT_FOLDER, GHashTable,
    camel_store_initial_setup_sync,
};
use glib_sys::{GError, GTRUE, g_hash_table_destroy, g_hash_table_lookup};
use jmap_client::{Client, Credentials};
use jmap_mail_sync::{FolderRole, MailSync};
use jmap_proto::session::CAPABILITY_MAIL;

/// Mirrors `tests/live_server_special_folders.rs::connect_for_write`.
fn connect_for_write() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    Some(
        Client::builder()
            .rebase_urls_to_origin(rebase)
            .connect(&origin, Credentials::basic(user, password))
            .expect("could not fetch the session document for the write-test account"),
    )
}

/// One `initial_setup_sync` call and its answer, owned the way Camel owns it.
/// Mirrors `jmap-mail/tests/folders.rs::Setup`.
struct Setup {
    save: *mut GHashTable,
    error: *mut GError,
    ok: glib_sys::gboolean,
}

impl Setup {
    /// Through the public wrapper, which is how Evolution's account wizard
    /// asks, and which creates the hash table the vfunc inserts into.
    fn run(store: *mut eds_sys::CamelStore) -> Self {
        let mut save: *mut GHashTable = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: `store` is a live store of ours, and both out-parameters
        // are writable and currently NULL.
        let ok = unsafe {
            camel_store_initial_setup_sync(store, &mut save, ptr::null_mut(), &mut error)
        };
        Self { save, error, ok }
    }

    fn get(&self, key: &CStr) -> Option<String> {
        if self.save.is_null() {
            return None;
        }
        // SAFETY: `save` is a live table for the length of this call, and a
        // non-NULL answer is a NUL-terminated string it owns.
        unsafe {
            let value = g_hash_table_lookup(self.save, key.as_ptr().cast());
            (!value.is_null()).then(|| CStr::from_ptr(value.cast()).to_string_lossy().into_owned())
        }
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        // SAFETY: `save`, if not NULL, is the table the wrapper transferred
        // ownership of; `error`, if not NULL, is ours to free.
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
/// mailboxes by their Camel path, cross-checked against a second,
/// independent connection's own `FolderTree::role`, so the assertion is the
/// server's own role assignment rather than this store's cached copy of it.
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
