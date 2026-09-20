// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `get_quota_info_sync` through Camel's own wrapper, against a real JMAP
//! server.
//!
//! `jmap-mail-sync/tests/live_server_quota.rs` already proves
//! `MailSync::quotas` — the one `Quota/get` call this vfunc reads — against
//! real Stalwart, and `jmap-mail/src/quota.rs` has a unit test pinning
//! `applies_to_mail`'s classification to that real shape. Neither drives the
//! vfunc itself: `camel_folder_get_quota_info_sync` on a real `CamelFolder`,
//! reached through `parent_store` and Camel's own class dispatch, has only
//! ever run against `jmap-mockd` (`tests/quota.rs`). This is that
//! confirmation, the same shape as `live_server_append.rs` and
//! `live_server_transfer.rs`.
//!
//! Like `jmap-mail-sync`'s own live-server quota test, this tolerates either
//! an empty quota list (`NoQuota`/`G_IO_ERROR_NOT_SUPPORTED`) or a populated
//! one, rather than assuming the write-test account has a quota configured.
//!
//! ## Running it
//!
//! Same environment as the other live-server tests — see
//! `docs/manual-test-live-server.md`:
//!
//! ```console
//! $ cargo test -p jmap-mail --features testing -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

mod common;

use std::env;
use std::ptr;

use common::Account;
use eds_sys::{
    CamelFolderQuotaInfo, camel_folder_get_quota_info_sync, camel_folder_quota_info_free,
};
use gio_sys::{G_IO_ERROR_NOT_SUPPORTED, g_io_error_quark};
use glib_sys::GError;
use gobject_sys::g_object_unref;
use jmap_client::{Client, Credentials};
use jmap_mail::folder::new_folder;
use jmap_mail_sync::MailSync;
use jmap_proto::session::CAPABILITY_MAIL;

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-mail-sync/tests/live_server_quota.rs::connect_for_write`.
fn connect_for_write() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user, password))
        .expect("could not fetch the session document for the write-test account");
    Some(client)
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn get_quota_info_sync_reads_the_real_servers_own_answer() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    let sync = MailSync::new(client, account_id);
    let account = Account::open();
    account.connect(sync);

    let name = format!("agent-mailquota-{}", unique_suffix());
    let created = account
        .jmap()
        .create_folder(None, &name)
        .expect("create_folder failed against the real server");

    // SAFETY: `account.store` is a live `CamelStore`, and `created` is a
    // `FolderInfo` a real discovery over that same store's connection just
    // returned.
    let folder = unsafe { new_folder(account.store, &created) };
    assert!(!folder.is_null(), "the folder would not construct");

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live folder, and an out-parameter that is writable and
    // currently NULL.
    let info: *mut CamelFolderQuotaInfo =
        unsafe { camel_folder_get_quota_info_sync(folder, ptr::null_mut(), &mut error) };

    if info.is_null() {
        // SAFETY: a live GError, set on the NULL branch above.
        unsafe {
            assert!(!error.is_null(), "a NULL answer with no error set");
            assert_eq!(
                (*error).domain,
                g_io_error_quark(),
                "an account with nothing scoped to Mail should fail as NOT_SUPPORTED"
            );
            assert_eq!((*error).code, G_IO_ERROR_NOT_SUPPORTED);
            glib_sys::g_error_free(error);
        }
        eprintln!("real server reported no quota scoped to Mail for the write-test account");
    } else {
        assert!(error.is_null(), "a successful answer set an error too");
        // SAFETY: a live `CamelFolderQuotaInfo` chain Camel handed back.
        unsafe {
            let mut node = info;
            let mut count = 0;
            while !node.is_null() {
                assert!(
                    (*node).total >= (*node).used,
                    "used ({}) exceeds total ({}) in the real server's own quota",
                    (*node).used,
                    (*node).total
                );
                count += 1;
                node = (*node).next;
            }
            eprintln!("real server returned a chain of {count} Mail quota node(s)");
            camel_folder_quota_info_free(info);
        }
    }

    // SAFETY: the one reference `new_folder` returned.
    unsafe { g_object_unref(folder.cast()) };
    account
        .jmap()
        .delete_folder(&created.id)
        .expect("delete_folder failed against the real server");
}
