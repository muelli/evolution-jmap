// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `get_inbox_folder_sync`/`get_trash_folder_sync`/`get_junk_folder_sync`
//! against a real JMAP server — the vfuncs behind
//! `camel_store_get_{inbox,trash,junk}_folder_sync`, which Camel calls by
//! purpose rather than by path (incoming filters, "Empty Trash", junk
//! handling).
//!
//! `jmap-mail-sync`'s own `FolderTree::role` is already exercised against
//! real Stalwart by several `jmap-mail-sync` live-server tests. What none of
//! them reach is `jmap-mail/src/folders.rs`'s `open_by_role`, the three
//! vfuncs built on it, and the join between a role lookup and
//! `camel_store_get_folder_sync` that opens the folder by the path the role
//! resolved to. `tests/folders.rs` proves all of that, but only against
//! `jmap-mockd`. This is that confirmation, against Stalwart's own default
//! role assignments.
//!
//! ## Running it
//!
//! Same environment as the other live-server tests — see
//! `docs/manual-test-live-server.md`:
//!
//! ```console
//! $ cargo test -p jmap-mail --features testing --test live_server_special_folders -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

mod common;

use std::env;
use std::ptr;

use common::Account;
use eds_sys::{
    CamelFolder, camel_folder_get_full_name, camel_store_get_inbox_folder_sync,
    camel_store_get_junk_folder_sync, camel_store_get_trash_folder_sync,
};
use glib_sys::GError;
use jmap_client::{Client, Credentials};
use jmap_mail::folder::JmapFolder;
use jmap_mail_sync::{FolderRole, MailSync};
use jmap_proto::Id;
use jmap_proto::session::CAPABILITY_MAIL;

/// Mirrors `jmap-mail-sync/tests/live_server_folder.rs::connect_for_write`.
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

/// One "open the folder for this purpose" call, owned the way Camel owns it.
struct Purpose {
    folder: *mut CamelFolder,
    error: *mut GError,
}

impl Purpose {
    fn asked(
        store: *mut eds_sys::CamelStore,
        wrapper: unsafe extern "C" fn(
            *mut eds_sys::CamelStore,
            *mut gio_sys::GCancellable,
            *mut *mut GError,
        ) -> *mut CamelFolder,
    ) -> Self {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: `store` is a live store of ours, and `error` is writable and
        // currently NULL.
        let folder = unsafe { wrapper(store, ptr::null_mut(), &mut error) };
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

impl Drop for Purpose {
    fn drop(&mut self) {
        // SAFETY: the call handed over one reference to the folder (if any)
        // and ownership of the error (if any).
        unsafe {
            if !self.folder.is_null() {
                gobject_sys::g_object_unref(self.folder.cast());
            }
            if !self.error.is_null() {
                glib_sys::g_error_free(self.error);
            }
        }
    }
}

/// Each of the three role-purpose vfuncs opens the mailbox Stalwart itself
/// assigned that role to, not a folder guessed by name — cross-checked
/// against a second, independent connection's own `FolderTree::role`, so the
/// assertion is the server's own role assignment rather than this store's
/// cached copy of it.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn special_folders_are_the_mailboxes_the_real_server_assigned_those_roles_to() {
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

    for role in [FolderRole::Inbox, FolderRole::Trash, FolderRole::Junk] {
        let expected = tree
            .role(role)
            .unwrap_or_else(|| panic!("the write-test account needs a mailbox with role {role:?}"));

        let opened = match role {
            FolderRole::Inbox => Purpose::asked(account.store, camel_store_get_inbox_folder_sync),
            FolderRole::Trash => Purpose::asked(account.store, camel_store_get_trash_folder_sync),
            FolderRole::Junk => Purpose::asked(account.store, camel_store_get_junk_folder_sync),
            _ => unreachable!("only the three roles above are asked for"),
        };

        assert!(
            opened.error.is_null(),
            "opening the {role:?} folder against the real server set an error"
        );
        assert_eq!(
            opened.path(),
            expected.path,
            "the {role:?} vfunc opened a different path than the real server's own role \
             assignment names"
        );
        assert_eq!(
            opened.mailbox(),
            expected.id,
            "the {role:?} vfunc opened a folder over a different mailbox than the real \
             server's own role assignment"
        );
    }
}
