// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The byte store for a message [`crate::append`] could not upload while
//! offline.
//!
//! [`crate::cache::MessageCache`] keeps a fetched message so a later open
//! does not download it again, and its own docs state the contract: every
//! operation is best-effort, and an entry nobody has opened for a while is
//! swept. A not-yet-uploaded draft cannot use that contract. Losing it is
//! not "mail opens one round trip slower"; it is the only copy of a message
//! the user wrote and has not sent yet. So this is a second, separate
//! `CamelDataCache`, with expiry disabled entirely: an entry here survives
//! until [`crate::append`] removes it itself, once the server has accepted
//! the upload or refused it for good.
//!
//! `CamelDataCache` reads its expiry setting from the cache object, not from
//! the subdirectory a call names, so sharing [`MessageCache`]'s own instance
//! would put every pending append under the same sweep as a merely fetched
//! message. A second instance, over the same account cache directory but
//! its own subdirectory, keeps the two rules apart while still living in
//! the one place the account's cache already is.
//!
//! [`MessageCache`]: crate::cache::MessageCache
//!
//! The entry's key is the row's own synthetic uid
//! ([`crate::message_info::synthetic_uid`]), so the row and its bytes go
//! missing or survive a restart together.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::{Mutex, MutexGuard, PoisonError};

use eds_sys::{
    CamelDataCache, camel_data_cache_get, camel_data_cache_new, camel_data_cache_remove,
    camel_data_cache_set_expire_enabled,
};
use gio_sys::{g_io_stream_close, g_io_stream_get_input_stream};
use glib_sys::{GError, GFALSE, g_clear_error};
use jmap_backend_core::owned::Owned;
use jmap_backend_core::trampoline::log_critical;

use crate::cache::{describe, read_all, valid_key, write_entry};

/// The subdirectory of the entries, below the same account cache directory
/// [`crate::cache::MessageCache`] uses. A name of its own keeps the two
/// kinds of entry from colliding on disk even though they share a root
/// directory.
const PENDING_APPEND: &CStr = c"pending-append";

/// The not-yet-uploaded bytes of a message [`crate::append`] appended while
/// offline, kept until the upload succeeds or is given up on.
pub struct PendingAppendCache {
    /// The Camel object, behind a lock of our own. See
    /// [`crate::cache::MessageCache`]'s identical field for why: Camel
    /// documents no thread-safety guarantee for `CamelDataCache`, and the
    /// lock is held for the IO of one entry, never across a network call.
    cache: Mutex<Owned<CamelDataCache>>,
}

// SAFETY: this reference is the only one to the `CamelDataCache` that this
// process holds. It is created here, released when the `Owned` drops with
// this struct, and no accessor gives it out. Each use goes through the
// `Mutex` above. `Owned` itself is neither `Send` nor `Sync`; this pair of
// manual impls overrides that default because of the invariant above, the
// same reasoning `MessageCache` gives for its own identical impls.
unsafe impl Send for PendingAppendCache {}
unsafe impl Sync for PendingAppendCache {}

impl PendingAppendCache {
    /// Opens (and creates) the store under `directory`, or returns `None`.
    ///
    /// `directory` is the same path `crate::folder::account_cache` passes to
    /// [`crate::cache::MessageCache::open`]: one account cache directory,
    /// each kind of entry in its own `CamelDataCache` instance so each can
    /// set its own expiry.
    pub fn open(directory: &str) -> Option<Self> {
        let path = CString::new(directory).ok()?;
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a NUL-terminated path that is alive during the call, and
        // an out-parameter that is writable and NULL. The reference that
        // `camel_data_cache_new` returns is ours. `cache` releases it on
        // each path that does not return it in `Self`.
        let Some(cache) =
            (unsafe { Owned::from_raw(camel_data_cache_new(path.as_ptr(), &mut error)) })
        else {
            log_critical(&format!(
                "no pending-append store under {directory}: {}",
                // SAFETY: `error` is NULL or an owned GError from the failed
                // call.
                unsafe { describe(error) }
            ));
            // SAFETY: `error` is NULL or an owned GError that this function
            // asked for, and this function is its only holder.
            unsafe { g_clear_error(&mut error) };
            return None;
        };
        // SAFETY: as above.
        unsafe { g_clear_error(&mut error) };

        // GFALSE, not a bound: an entry here survives until `crate::append`
        // removes it itself, not after a fixed time nobody opened it. The
        // module doc above explains why `MessageCache`'s own sweep does not
        // apply to a not-yet-uploaded draft.
        // SAFETY: the cache is the live object from above, and the call
        // borrows nothing.
        unsafe { camel_data_cache_set_expire_enabled(cache.as_ptr(), GFALSE) };

        Some(Self {
            cache: Mutex::new(cache),
        })
    }

    /// Stores `source` under `uid`, and tells if the write succeeded.
    ///
    /// Reuses `write_entry`, the same all-or-nothing write
    /// [`crate::cache::MessageCache::store`] uses: a failed write leaves no
    /// entry, not a short one that a later replay would upload truncated.
    pub fn store(&self, uid: &str, source: &[u8]) -> bool {
        let Some(key) = self.key(uid) else {
            return false;
        };
        let cache = self.lock();
        write_entry(&cache, PENDING_APPEND, &key, source, uid)
    }

    /// The bytes stored under `uid`, or `None` if there are none, or if they
    /// cannot be read.
    pub fn load(&self, uid: &str) -> Option<Vec<u8>> {
        let key = self.key(uid)?;
        let cache = self.lock();

        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live cache, two NUL-terminated strings that are alive
        // during the call, and a NULL error out-parameter. On a hit, the
        // returned reference is ours; `stream` releases it on each exit
        // from this scope.
        let Some(stream) = (unsafe {
            Owned::from_raw(camel_data_cache_get(
                cache.as_ptr(),
                PENDING_APPEND.as_ptr(),
                key.as_ptr(),
                &mut error,
            ))
        }) else {
            // A miss (no file) is the usual result before an upload
            // succeeds, so no log. The GError must still be cleared.
            // SAFETY: as above.
            unsafe { g_clear_error(&mut error) };
            return None;
        };
        // SAFETY: as above.
        unsafe { g_clear_error(&mut error) };

        // SAFETY: `stream` is an owned `GIOStream` from the call above. Its
        // input stream is borrowed from it, and `stream` lives longer.
        let source = unsafe { read_all(g_io_stream_get_input_stream(stream.as_ptr()), uid) };
        // SAFETY: this closes the reference that this function owns, and
        // `stream` releases it on each exit from this scope. A close of a
        // failed stream is defined.
        unsafe {
            g_io_stream_close(stream.as_ptr(), ptr::null_mut(), ptr::null_mut());
        }

        source
    }

    /// Removes the entry for `uid`, once the upload it held has either
    /// succeeded or been given up on.
    pub fn remove(&self, uid: &str) {
        let Some(key) = self.key(uid) else {
            return;
        };
        let cache = self.lock();
        // SAFETY: a live cache and a NUL-terminated key that is alive during
        // the call. Removing a key that is not present is a defined no-op.
        unsafe {
            camel_data_cache_remove(
                cache.as_ptr(),
                PENDING_APPEND.as_ptr(),
                key.as_ptr(),
                ptr::null_mut(),
            );
        }
    }

    /// A uid as the file name that it becomes, or `None` if it cannot be
    /// one. The same grammar check as [`crate::cache::MessageCache::key`],
    /// because this store joins the key onto a path the same way.
    fn key(&self, uid: &str) -> Option<CString> {
        if !valid_key(uid) {
            log_critical(&format!(
                "pending-append id {uid:?} is not a JMAP id and will not be stored"
            ));
            return None;
        }
        CString::new(uid).ok()
    }

    /// A poisoned lock means that an operation panicked during an entry. The
    /// lock guards a reference to a live Camel object, which a panic cannot
    /// damage. Thus the code continues, and the failure does not stop the
    /// account.
    fn lock(&self) -> MutexGuard<'_, Owned<CamelDataCache>> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
