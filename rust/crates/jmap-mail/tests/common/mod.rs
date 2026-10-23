// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A Camel account stood up by hand, for the tests that need a *real* store.
//!
//! [`JmapStore::detached`] is enough for anything that only reads the store's
//! own state, and most of this crate's tests use it. It is not enough for
//! anything that reaches Camel. Its parent bytes are zeroed, not constructed,
//! so a detached store is not a GObject, and passing one to a Camel function
//! is undefined behavior. Two things in the provider reach Camel: building a
//! folder, which asserts `CAMEL_IS_STORE` on its parent, and calling a store
//! vfunc through `camel_store_*`, which asserts the same. Both need what this
//! module builds.
//!
//! Camel itself constructs a store through `camel_session_add_service`. In
//! Evolution this means an `EMailSession` over a source registry on the
//! session bus. `g_initable_new`, called with the three construct properties
//! a `CamelService` needs, builds the same object without any of that.
//!
//! This module uses `g_initable_new`, not `g_object_new`, because a
//! `CamelStore` is a `GInitable`, and its `init` opens the summary database
//! that every folder of the store keeps its rows in. A store constructed the
//! shorter way looks complete but has no summary database:
//! `camel_store_get_db` returns NULL, and removing the first row of a folder
//! crashes the process inside Camel. This is a property of the harness, not
//! of the provider (Camel constructs a service no other way). It only
//! becomes visible once a folder has a summary, which is why the fix is here
//! rather than earlier.

#![allow(dead_code)]

pub mod array;
pub mod race;
pub mod signals;

use std::ffi::CString;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use eds_sys::{
    CamelProvider, CamelService, CamelStore, camel_service_get_user_cache_dir,
    camel_session_get_type,
};
use gio_sys::g_initable_new;
use glib_sys::{GError, gchar};
use gobject_sys::{GObject, g_object_new, g_object_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::provider::register;
use jmap_mail::store::{JmapStore, store_type};
use jmap_mail::transport::{JmapTransport, transport_type};
use jmap_mail_sync::MailSync;
use jmap_mock::MockServer;

/// A session and the store that hangs off it.
///
/// The two are kept together because a `CamelService` holds only a weak
/// reference to its session. A test that unreffed the session while the
/// store lived would leave the store pointing at nothing.
pub struct Account {
    session: *mut GObject,
    pub store: *mut CamelStore,
    directory: PathBuf,
}

/// Tells one account's directory from the next. A store's summary database
/// is a file under its session's directories. Two accounts sharing one
/// directory would be two tests sharing a folder's rows, including two
/// tests that run at once, which is what a Rust test binary does by default.
static ACCOUNTS: AtomicUsize = AtomicUsize::new(0);

impl Account {
    /// Constructs the session and the store on it. The provider struct that
    /// [`register`] leaks names the store's type. A `CamelService` is
    /// constructed with all three of session, provider, and uid.
    pub fn open() -> Self {
        let provider: *const CamelProvider = register();
        let directory = std::env::temp_dir().join(format!(
            "jmap-mail-test-{}-{}",
            std::process::id(),
            ACCOUNTS.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).expect("a directory for the account");
        let dir = CString::new(directory.to_string_lossy().as_ref())
            .expect("a temporary directory path with no NUL in it");

        // SAFETY: variadic construct calls. Every named property is one that
        // `CamelSession` or `CamelService` declares, and each value has the
        // type that property carries: two strings for the session's
        // directories, the session and provider for the store, and a NULL
        // that terminates the list.
        unsafe {
            let session = g_object_new(
                camel_session_get_type(),
                c"user-data-dir".as_ptr(),
                dir.as_ptr(),
                c"user-cache-dir".as_ptr(),
                dir.as_ptr(),
                ptr::null::<gchar>(),
            );
            assert!(!session.is_null(), "g_object_new returned no session");

            let mut error: *mut GError = ptr::null_mut();
            let store = g_initable_new(
                store_type(),
                ptr::null_mut(),
                ptr::addr_of_mut!(error),
                c"session".as_ptr(),
                session,
                c"provider".as_ptr(),
                provider,
                c"uid".as_ptr(),
                c"jmap-test".as_ptr(),
                ptr::null::<gchar>(),
            );
            assert!(
                !store.is_null() && error.is_null(),
                "the store would not initialise"
            );

            Self {
                session,
                store: store.cast::<CamelStore>(),
                directory,
            }
        }
    }

    /// The store as its Rust self, for the state no Camel accessor reaches.
    pub fn jmap(&self) -> &JmapStore {
        // SAFETY: `self.store` is an instance of `JmapStore`, constructed
        // above, and it lives as long as this `Account`.
        unsafe { JmapStore::borrow(self.store) }.expect("a store of ours")
    }

    /// Installs a live connection, the way `connect_sync` would.
    pub fn connect(&self, sync: MailSync) {
        self.jmap().store_connection(sync);
    }

    /// Where Camel says this account may keep files: the session's cache
    /// directory, with the service's uid under it.
    ///
    /// This asks Camel rather than composing the two parts, because it is
    /// the answer the provider itself builds its message cache from. A test
    /// that guessed the layout would agree with itself, not with Camel.
    pub fn cache_dir(&self) -> String {
        // SAFETY: a live `CamelService` (a `CamelStore` is one). The string
        // it returns is owned by the service and outlives this call.
        unsafe {
            let dir = camel_service_get_user_cache_dir(self.store.cast::<CamelService>());
            assert!(!dir.is_null(), "the store has no cache directory");
            std::ffi::CStr::from_ptr(dir).to_string_lossy().into_owned()
        }
    }
}

/// The other service of an account: a `CamelJmapTransport` on the same session.
///
/// This is its own object, not a field of [`Account`], because that is what
/// it is in Evolution: two `ESource`s, two `camel_session_add_service` calls,
/// two services that share a session and nothing else. Most tests want only
/// the store, and constructing a transport for them would run Camel
/// machinery for no reason.
///
/// It borrows the account for the session's sake. A `CamelService` holds
/// only a weak reference to its session, so a transport that outlives the
/// account it was built on would be a service pointing at nothing.
pub struct Transport<'a> {
    _account: &'a Account,
    pub service: *mut CamelService,
}

impl<'a> Transport<'a> {
    /// Constructs the transport on `account`'s session, with a uid of its own.
    /// A service is keyed by uid, and two services sharing one uid would be
    /// one service.
    pub fn open(account: &'a Account) -> Self {
        let provider: *const CamelProvider = register();

        // SAFETY: a variadic construct call. Every property named is one
        // `CamelService` declares, each value has the type that property
        // carries, and the list is NULL-terminated. `g_initable_new` rather
        // than `g_object_new` for the reason the module docs give.
        let service = unsafe {
            let mut error: *mut GError = ptr::null_mut();
            let service = g_initable_new(
                transport_type(),
                ptr::null_mut(),
                ptr::addr_of_mut!(error),
                c"session".as_ptr(),
                account.session,
                c"provider".as_ptr(),
                provider,
                c"uid".as_ptr(),
                c"jmap-test-transport".as_ptr(),
                ptr::null::<gchar>(),
            );
            assert!(
                !service.is_null() && error.is_null(),
                "the transport would not initialise"
            );
            service.cast::<CamelService>()
        };

        Self {
            _account: account,
            service,
        }
    }

    /// Installs a live connection, the way `authenticate_sync` would.
    pub fn connect(&self, sync: MailSync) {
        self.jmap().install_connection(sync);
    }

    /// The transport as its Rust self, for the state no Camel accessor reaches.
    pub fn jmap(&self) -> &JmapTransport {
        // SAFETY: `self.service` is an instance of `JmapTransport`, constructed
        // above, and it lives as long as this `Transport`.
        unsafe { JmapTransport::borrow(self.service.cast()) }.expect("a transport of ours")
    }
}

impl Drop for Transport<'_> {
    fn drop(&mut self) {
        // SAFETY: the one reference, taken at construction and never handed out.
        unsafe { g_object_unref(self.service.cast()) };
    }
}

impl Drop for Account {
    fn drop(&mut self) {
        // SAFETY: one reference each, taken at construction and never handed
        // out. The store is freed first, because it references the session.
        unsafe {
            g_object_unref(self.store.cast());
            g_object_unref(self.session.cast());
        }
        // Removes the summary database the store just closed, and whatever
        // else Camel put beside it. Best effort: a test that already failed
        // is not improved by a panic during its teardown.
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// A connection to the mock server's one account, with no credentials.
pub fn sync_against(server: &MockServer) -> MailSync {
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    MailSync::new(client, server.account_id())
}
