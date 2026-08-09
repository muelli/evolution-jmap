// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A Camel account stood up by hand, for the tests that need a *real* store.
//!
//! [`JmapStore::detached`] is enough for anything that only reads the store's
//! own state, and it is what most of this crate's tests use. It is not enough
//! for anything that reaches Camel: the parent bytes are zeroed rather than
//! constructed, so a detached store is not a GObject and passing one to a Camel
//! function is undefined behaviour. Two things in the provider do reach Camel —
//! building a folder, which asserts `CAMEL_IS_STORE` on its parent, and calling
//! a store vfunc through `camel_store_*`, which asserts the same — and both
//! therefore need what this module builds.
//!
//! Camel itself constructs a store through `camel_session_add_service`, which
//! in Evolution means an `EMailSession` over a source registry on the session
//! bus. `g_initable_new` with the three construct properties a `CamelService`
//! needs is the same object without any of that.
//!
//! `g_initable_new` rather than `g_object_new`, because a `CamelStore` is a
//! `GInitable` and what its `init` does is open the summary database every
//! folder of the store keeps its rows in. A store constructed the shorter way
//! looks complete and has none: `camel_store_get_db` returns NULL, and the
//! first row a folder removes takes the process down inside Camel. That is a
//! property of the harness rather than of the provider — Camel constructs a
//! service no other way — but it only becomes visible once a folder has a
//! summary, which is why it is being fixed here rather than earlier.

#![allow(dead_code)]

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
use jmap_mail::provider::register;
use jmap_mail::store::{JmapStore, store_type};
use jmap_mail::transport::{JmapTransport, transport_type};
use jmap_mail_sync::MailSync;

/// A session and the store that hangs off it.
///
/// The two are kept together because a `CamelService` holds only a weak
/// reference to its session: a test that unreffed the session while the store
/// lived would leave the store pointing at nothing.
pub struct Account {
    session: *mut GObject,
    pub store: *mut CamelStore,
    directory: PathBuf,
}

/// Tells one account's directory from the next. A store's summary database is
/// a file under its session's directories, and two accounts sharing one would
/// be two tests sharing a folder's rows — including two tests running at once,
/// which is what a Rust test binary does by default.
static ACCOUNTS: AtomicUsize = AtomicUsize::new(0);

impl Account {
    /// Constructs the session and the store on it. The provider struct
    /// [`register`] leaks is what names the store's type, and a
    /// `CamelService` is constructed with all three of session, provider and
    /// uid.
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

        // SAFETY: variadic construct calls. Every property named is one
        // `CamelSession` or `CamelService` declares, and each value has the
        // type that property carries — two strings for the session's
        // directories, the session and provider for the store, and a NULL
        // terminating the list.
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

    /// Where Camel says this account may keep files — the session's cache
    /// directory with the service's uid under it.
    ///
    /// Asked of Camel rather than composed from the two, because it is the
    /// answer the provider itself builds its message cache from: a test that
    /// guessed the layout would agree with itself rather than with Camel.
    pub fn cache_dir(&self) -> String {
        // SAFETY: a live `CamelService` — a `CamelStore` is one — and the
        // string it returns is owned by the service and outlives this call.
        unsafe {
            let dir = camel_service_get_user_cache_dir(self.store.cast::<CamelService>());
            assert!(!dir.is_null(), "the store has no cache directory");
            std::ffi::CStr::from_ptr(dir).to_string_lossy().into_owned()
        }
    }
}

/// The other service of an account: a `CamelJmapTransport` on the same session.
///
/// Its own object rather than a field of [`Account`], because that is what it
/// is in Evolution — two `ESource`s, two `camel_session_add_service` calls, two
/// services that share a session and nothing else. Most tests want only the
/// store, and constructing a transport for them would be Camel machinery
/// running for no reason.
///
/// It borrows the account for the session's sake: a `CamelService` holds only a
/// weak reference to its session, so a transport outliving the account it was
/// built on would be a service pointing at nothing.
pub struct Transport<'a> {
    _account: &'a Account,
    pub service: *mut CamelService,
}

impl<'a> Transport<'a> {
    /// Constructs the transport on `account`'s session, with a uid of its own —
    /// a service is keyed by uid, and two sharing one would be one service.
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
        // out; the store goes first because it references the session.
        unsafe {
            g_object_unref(self.store.cast());
            g_object_unref(self.session.cast());
        }
        // The summary database the store just closed, and whatever else Camel
        // put beside it. Best effort: a test that has already failed is not
        // made better by a panic in its teardown.
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
