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
//! bus. `g_object_new` with the three construct properties a `CamelService`
//! needs is the same object without any of that.

#![allow(dead_code)]

use std::ffi::CString;
use std::ptr;

use eds_sys::{CamelProvider, CamelStore, camel_session_get_type};
use glib_sys::gchar;
use gobject_sys::{GObject, g_object_new, g_object_unref};
use jmap_mail::provider::register;
use jmap_mail::store::{JmapStore, store_type};
use jmap_mail_sync::MailSync;

/// A session and the store that hangs off it.
///
/// The two are kept together because a `CamelService` holds only a weak
/// reference to its session: a test that unreffed the session while the store
/// lived would leave the store pointing at nothing.
pub struct Account {
    session: *mut GObject,
    pub store: *mut CamelStore,
}

impl Account {
    /// Constructs the session and the store on it. The provider struct
    /// [`register`] leaks is what names the store's type, and a
    /// `CamelService` is constructed with all three of session, provider and
    /// uid.
    pub fn open() -> Self {
        let provider: *const CamelProvider = register();
        let dir = CString::new(std::env::temp_dir().to_string_lossy().as_ref())
            .expect("a temporary directory path with no NUL in it");

        // SAFETY: a variadic construct call. Every property named is one
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

            let store = g_object_new(
                store_type(),
                c"session".as_ptr(),
                session,
                c"provider".as_ptr(),
                provider,
                c"uid".as_ptr(),
                c"jmap-test".as_ptr(),
                ptr::null::<gchar>(),
            );
            assert!(!store.is_null(), "g_object_new returned no store");

            Self {
                session,
                store: store.cast::<CamelStore>(),
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
}

impl Drop for Account {
    fn drop(&mut self) {
        // SAFETY: one reference each, taken by `g_object_new` and never handed
        // out; the store goes first because it references the session.
        unsafe {
            g_object_unref(self.store.cast());
            g_object_unref(self.session.cast());
        }
    }
}
