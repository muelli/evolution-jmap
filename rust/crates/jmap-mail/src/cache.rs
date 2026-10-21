// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The cache that keeps a fetched message, so that the next open does not
//! download it again.
//!
//! [`crate::message`] makes mail from a row with two round trips: an
//! `Email/get` for the blob id, then the blob. RFC 8621 §4.1 makes an `Email`
//! immutable, so a second open of the same row always gives the same bytes.
//! Without a cache, the two round trips of each open are waste. Also, a message
//! that the user has read becomes unreadable when the network goes away, in a
//! provider whose store is a `CamelOfflineStore`.
//!
//! Camel's own providers keep these bytes in a `CamelDataCache`: a directory,
//! one subdirectory for each kind of cached data, and one file for each entry,
//! named by a key. IMAPX keeps its message cache in one. This module is the safe
//! wrapper. It has three operations, and all of them are best-effort.
//!
//! ## Best-effort is the contract, not a shortcut
//!
//! No function here gives a failure to the caller as an error, and
//! [`MessageCache::open`] returns `None`, not a `Result`. A cache is an
//! optimisation over a fetch that works. A full disk, a directory that the user
//! made read-only, or an entry that another process removed during a read: with
//! each of these, mail must still open, only slower. An error for them would
//! make a working account into a broken one. Also, no caller can use the error.
//! The caller is a vfunc, and its error out-parameter is for the case where the
//! message cannot be produced at all.
//!
//! ## One cache per folder, one entry per message
//!
//! A JMAP email id identifies a message in the *account*. The same message in
//! two mailboxes has the same id in both, because a JMAP mailbox is more like a
//! label than a directory. Thus the key is the uid alone and the directory is
//! the account's directory. A message in five mailboxes is one file, not five.
//! The folder holds the cache object (see [`crate::folder`]), because the folder
//! knows the cache directory of the store at a well-defined moment.
//!
//! ## A uid becomes a file name
//!
//! RFC 8620 §1.2 limits an id to URL-safe characters. This layer depends on
//! that limit, because `camel_data_cache_add` joins the key onto a path. Without
//! a check, a server that answers `Email/query` with `../../../.config/autostart`
//! can choose where this provider writes. Thus `valid_key` checks each key
//! against the grammar of the RFC, and this module does not cache a key that
//! fails the check.
//!
//! ## An entry is checked against its row
//!
//! One `write_all` writes an entry, and then the stream closes. A write that
//! fails removes the entry. But if the process stops between the write and the
//! removal, a short file stays. Camel's parser reads that file as a *complete*
//! message with a truncated body, because MIME has no length.
//!
//! Thus both operations take the size that the row of the folder claims. This
//! is the `size` of RFC 8621 §4.1, which the specification defines as the
//! octets of exactly the bytes that the `blobId` references. An entry shorter
//! than that size is not a message. [`load`] removes such an entry and does not
//! serve it. [`store`] does not write one, for the same reason that it does not
//! write an empty one: an entry that the cache does not serve is a syscall that
//! only makes a miss.
//!
//! **Shorter, not different.** This check prevents truncation, and truncation
//! always gives a short file. A server whose `size` is too small by a byte has
//! a *reporting* fault. An exact comparison would make that message one that
//! the cache can never keep, so each open costs two more round trips forever.
//! A mail client that accepts an entry that is too long stays usable. A client
//! that downloads again each message from a slightly incorrect server does not.
//!
//! [`load`]: MessageCache::load
//! [`store`]: MessageCache::store
//!
//! ## The bound: an entry survives when it is not used, not forever
//!
//! A cache that only grows fills the disk. Thus the cache removes an entry that
//! nobody opened for `UNUSED_FOR`. `CamelDataCache` has two clocks. This module
//! uses `set_expire_access`, the atime of the file, and not `set_expire_age`,
//! its mtime. An `Email` is immutable, so its mtime is only the time of its
//! *download*. A bound on age alone removes a message that the user reads each
//! week at the same time as a message they read once. That spends a round trip
//! on the wrong message.
//!
//! atime is a weaker signal than it looks. `relatime`, the usual mount option,
//! updates it only once a day, and `noatime` never updates it. But both fail in
//! the safe direction here. A resolution of one day is small against a bound of
//! one month. With `noatime`, the atime stays at the time of the write, so the
//! bound becomes the age bound. An entry that stays too long costs a file. An
//! entry removed too early costs a round trip.
//!
//! **The sweep is lazy, and this is not a quota.** Camel expires a bucket when a
//! lookup goes into it, at most once an hour. Thus nothing is removed from an
//! account that nobody opens. The bound limits a cache by time, not by a number
//! of megabytes. A real quota must be our own code
//! (`camel_data_cache_foreach_remove` is the hook), and its size is a question
//! for the user. This bound needs no answer from the user.
//!
//! ## What is not here yet
//!
//! **A setting.** `UNUSED_FOR` is a constant, not a setting, because Camel has
//! no place for it. The `limit-by-age` of `CamelOfflineSettings` controls which
//! messages are *downloaded* for offline use, not how long the cache keeps a
//! downloaded message. If this module used it, the offline window of an account
//! would also be the bound of its cache, without notice. A setting of our own
//! needs a field in an account editor, which is not the work of this file.
//!
//! ## The atomic write, where EDS has it
//!
//! EDS 3.62 added `camel_data_cache_add_atomic`/`commit_atomic`/`discard_atomic`.
//! These write under a temporary name in the same directory, and rename the file
//! into place only after the full write. This makes a half-written entry
//! impossible, not only detected. [`store`] uses this path when the installed
//! EDS has it (the `camel_data_cache_atomic` marker of `eds-sys`, the same probe
//! style as each other version gap in this tree). On 3.52-3.61, which this
//! project also builds against, it uses the open-write-close sequence above.
//! The size check stays in both cases. It catches a server that reports an
//! incorrect `size`, and a rename does not.
//!
//! [`store`]: MessageCache::store

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

#[cfg(not(camel_data_cache_atomic))]
use eds_sys::camel_data_cache_add;
use eds_sys::{
    CamelDataCache, CamelFolder, CamelFolderClass, camel_data_cache_get,
    camel_data_cache_get_filename, camel_data_cache_new, camel_data_cache_remove,
    camel_data_cache_set_expire_access, camel_data_cache_set_expire_enabled, time_t,
};
#[cfg(camel_data_cache_atomic)]
use eds_sys::{
    camel_data_cache_add_atomic, camel_data_cache_commit_atomic, camel_data_cache_discard_atomic,
};
use gio_sys::{
    g_input_stream_read, g_io_stream_close, g_io_stream_get_input_stream,
    g_io_stream_get_output_stream, g_output_stream_write_all,
};
use glib_sys::{GError, GFALSE, GTRUE, g_clear_error, g_free, gchar};
use jmap_backend_core::error::fail;
use jmap_backend_core::marshal::{dup_string, read_string};
use jmap_backend_core::owned::Owned;
use jmap_backend_core::trampoline::{guard_ptr, log_critical, log_critical_for_message};

use crate::connect::StoreError;
use crate::folder::JmapFolder;

/// The subdirectory of the entries, below the cache directory of the account.
///
/// It has a name and is not empty, because a `CamelDataCache` uses a path *and*
/// a key. The path separates the kinds of cached data of one account. A provider
/// that puts messages at the root has no place for the next kind. IMAPX uses
/// `"cur"`, as in a maildir. This name tells what the files are.
const MESSAGES: &CStr = c"messages";

/// The quantity of data that one read takes from an entry. A message is one file
/// and is read fully. The buffer only sets the number of `read` calls.
const CHUNK: usize = 64 * 1024;

/// The time an entry survives without an open, before the next lookup near it
/// removes it. The module docs tell why this is an atime and a constant.
///
/// Thirty days: long enough to keep the message from last month, short enough
/// not to keep a year of mail. If the bound is wrong, the cost is one
/// `Email/get` and one blob download for a message that the user opens again.
/// Each open had that cost before this cache existed. Thus the failure of the
/// bound is the behaviour that the cache replaced.
const UNUSED_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The message cache of the account.
pub struct MessageCache {
    /// The Camel object, behind a lock of our own.
    ///
    /// Camel documents no thread-safety guarantee for `CamelDataCache`. Camel
    /// uses a folder from many threads at once: a refresh and two message opens
    /// can be in progress together. The lock is held for the IO of one entry
    /// and never during a network fetch, because the fetch is the part of an
    /// open that must overlap.
    cache: Mutex<Owned<CamelDataCache>>,
}

// SAFETY: this reference is the only one to the `CamelDataCache` that this
// process holds. It is created here, it is released when the `Owned` drops with
// this struct, and no accessor gives it out. Each use of it goes through the
// `Mutex` above, so two threads cannot reach the object at once. `Owned` itself
// is neither `Send` nor `Sync` (its own docs tell why). This pair of manual
// impls overrides that default because of the invariant above.
unsafe impl Send for MessageCache {}
unsafe impl Sync for MessageCache {}

impl MessageCache {
    /// Opens (and creates) the cache under `directory`, or returns `None`.
    ///
    /// `camel_data_cache_new` makes the directory, so this call fails when the
    /// cache directory of the account cannot exist. It returns `None`, not an
    /// error, as the module contract tells: the caller continues without a
    /// cache. It logs a critical, because a cache directory that cannot be made
    /// is a broken installation, not a temporary condition. Without a log line,
    /// the only symptom is slow mail.
    pub fn open(directory: &str) -> Option<Self> {
        Self::open_bounded(directory, UNUSED_FOR)
    }

    /// The same, but the caller gives the bound, not `UNUSED_FOR`.
    ///
    /// The policy is in one constant and the mechanism takes a parameter. Thus a
    /// test can see an entry go without a wait of one month and without an
    /// atime of its own. Camel reads the age from the filesystem, so a forged
    /// age tests `utimes`, not this code.
    pub fn open_bounded(directory: &str, unused_for: Duration) -> Option<Self> {
        let path = CString::new(directory).ok()?;
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a NUL-terminated path that is alive during the call, and an
        // out-parameter that is writable and NULL. The reference that
        // `camel_data_cache_new` returns is ours. `cache` releases it on each
        // path that does not return it in `Self`.
        let Some(cache) =
            (unsafe { Owned::from_raw(camel_data_cache_new(path.as_ptr(), &mut error)) })
        else {
            log_critical(&format!(
                "no message cache under {directory}: {}",
                // SAFETY: `error` is NULL or an owned GError from the failed
                // call.
                unsafe { describe(error) }
            ));
            // SAFETY: `error` is NULL or an owned GError that this function
            // asked for, and this function is its only holder.
            unsafe { g_clear_error(&mut error) };
            return None;
        };
        // A created cache and an error cannot both occur, but a GError from a
        // successful call would leak.
        // SAFETY: as above.
        unsafe { g_clear_error(&mut error) };

        // Camel reads the bound as a `time_t`, and -1 means "no bound". Thus a
        // duration too large for a `time_t` becomes the largest bound, and does
        // not wrap to the value that stops the sweep. A fresh `CamelDataCache`
        // already has expiry enabled, so only the bound (initially -1) changes.
        // SAFETY: the cache is the live object from above, and neither call
        // borrows anything.
        unsafe {
            camel_data_cache_set_expire_enabled(cache.as_ptr(), GTRUE);
            camel_data_cache_set_expire_access(
                cache.as_ptr(),
                time_t::try_from(unused_for.as_secs()).unwrap_or(time_t::MAX),
            );
        }

        Some(Self {
            cache: Mutex::new(cache),
        })
    }

    /// The cached bytes for `uid`, if any, checked against the size that the
    /// row of the folder claims. See `claimed` for what a claim is.
    ///
    /// `None` covers each reason for no bytes: never stored, removed, not
    /// readable, damaged, or a key that this cache does not look up. The caller
    /// does the same for all of them: it fetches the message.
    pub fn load(&self, uid: &str, listed: Option<u32>) -> Option<Vec<u8>> {
        let key = self.key(uid)?;
        let cache = self.lock();

        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live cache (the constructor makes the pointer non-NULL, and
        // only `Drop` makes it invalid), two NUL-terminated strings that are
        // alive during the call, and a NULL error out-parameter. On a hit, the
        // returned reference is ours. `stream` releases it on each exit from
        // this scope.
        let Some(stream) = (unsafe {
            Owned::from_raw(camel_data_cache_get(
                cache.as_ptr(),
                MESSAGES.as_ptr(),
                key.as_ptr(),
                &mut error,
            ))
        }) else {
            // A miss (no file) is the usual result, so no log. The GError must
            // still be cleared.
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
        // failed stream is defined. Its error is not important, because
        // `source` tells if the read succeeded.
        unsafe {
            g_io_stream_close(stream.as_ptr(), ptr::null_mut(), ptr::null_mut());
        }

        let source = source?;
        // An empty entry is not a message, and an entry shorter than the
        // message is not one either. A process that stops between `add` and
        // the write leaves both. RFC 5322 has no zero-octet document. Camel's
        // parser makes a message from either, and the caller then serves it
        // and does not fetch, for as long as the entry survives.
        let damage = if source.is_empty() {
            Some("it has nothing in it".to_owned())
        } else {
            claimed(listed)
                .filter(|listed| source.len() < *listed as usize)
                .map(|listed| {
                    format!(
                        "it is {} octets where the message is {listed}",
                        source.len()
                    )
                })
        };
        let Some(damage) = damage else {
            return Some(source);
        };

        log_critical_for_message(
            uid,
            &format!("the cached copy of message {uid} was dropped: {damage}"),
        );
        // Removed, not only refused: nothing serves it. If it stays, it costs
        // disk and gives this log line at each open until the bound removes it.
        // But each open resets the atime that the bound measures. A successful
        // fetch writes the entry again. A failed fetch leaves the cache without
        // this message, which is correct.
        // SAFETY: as above. This code cannot act on a failure of the removal.
        unsafe {
            camel_data_cache_remove(
                cache.as_ptr(),
                MESSAGES.as_ptr(),
                key.as_ptr(),
                ptr::null_mut(),
            );
        }
        None
    }

    /// Caches `source` as the bytes of `uid`, and tells if the write succeeded.
    ///
    /// `listed` is the size that the row of the folder claims, as in [`load`].
    ///
    /// The result lets a test tell a refusal from a success. The caller ignores
    /// it, because the caller does the same in both cases.
    ///
    /// This function removes an entry that it cannot write fully. A short file
    /// is not only a failed cache write. It is a message that opens without
    /// part of its body at each later open.
    ///
    /// [`load`]: MessageCache::load
    pub fn store(&self, uid: &str, source: &[u8], listed: Option<u32>) -> bool {
        let Some(key) = self.key(uid) else {
            return false;
        };
        if source.is_empty() {
            // [`MessageCache::load`] does not serve this entry, so a write only
            // spends a syscall on a miss. A message with no bytes is not a
            // message, and the cache is not the place to correct its cause.
            return false;
        }
        if let Some(listed) = claimed(listed).filter(|listed| source.len() < *listed as usize) {
            // `load` does not serve this entry either. Here the bytes come
            // directly from the network, so the server disagrees with itself;
            // no crash is the cause. Log it, because the visible symptom is a
            // message that downloads again at each open.
            log_critical_for_message(
                uid,
                &format!(
                    "message {uid} arrived as {} octets where its row says {listed}, and is not cached",
                    source.len()
                ),
            );
            return false;
        }
        let cache = self.lock();

        // Two implementations of one contract: write `source` under `key`, and
        // tell if the write succeeded. The 3.52-3.61 arm (see "The atomic
        // write" in the module docs) writes directly to the final path and
        // checks the result. The 3.62+ arm writes to a temporary name first.
        // Thus a failed write leaves nothing under `key`, not a short file.

        #[cfg(not(camel_data_cache_atomic))]
        // SAFETY: `add` replaces an entry that is already under the key. The
        // returned reference is ours, and `stream` releases it on each exit
        // from this scope.
        let stored = unsafe {
            let mut error: *mut GError = ptr::null_mut();
            let Some(stream) = Owned::from_raw(camel_data_cache_add(
                cache.as_ptr(),
                MESSAGES.as_ptr(),
                key.as_ptr(),
                &mut error,
            )) else {
                log_critical_for_message(
                    uid,
                    &format!("message {uid} could not be cached: {}", describe(error)),
                );
                g_clear_error(&mut error);
                return false;
            };

            // The output stream is borrowed from `stream`, `source` is a live
            // buffer of the given length, and the two out-parameters are
            // locals.
            let mut written: usize = 0;
            let written_all = g_output_stream_write_all(
                g_io_stream_get_output_stream(stream.as_ptr()),
                // gio-sys gives the buffer the type `*mut`, but the C function
                // takes a `const void *`. The cast agrees with the header.
                source.as_ptr().cast_mut().cast(),
                source.len(),
                &mut written,
                ptr::null_mut(),
                &mut error,
            ) != GFALSE;
            // The close flushes. Thus a successful write with a failed close
            // is still an incomplete entry. The close error goes into its own
            // local, because GLib logs a critical when a second `g_set_error`
            // writes to an out-parameter that is set. Report the write error
            // first, because it tells the cause.
            let mut on_close: *mut GError = ptr::null_mut();
            let closed =
                g_io_stream_close(stream.as_ptr(), ptr::null_mut(), &mut on_close) != GFALSE;
            if error.is_null() {
                error = on_close;
            } else {
                g_clear_error(&mut on_close);
            }
            let stored = written_all && closed && written == source.len();

            if !stored {
                log_critical_for_message(
                    uid,
                    &format!(
                        "message {uid} was cached incompletely and has been dropped: {}",
                        describe(error)
                    ),
                );
                // This code cannot act on a failure of the removal.
                camel_data_cache_remove(
                    cache.as_ptr(),
                    MESSAGES.as_ptr(),
                    key.as_ptr(),
                    ptr::null_mut(),
                );
            }
            g_clear_error(&mut error);
            stored
        };

        #[cfg(camel_data_cache_atomic)]
        // SAFETY: `add_atomic` returns a stream open on a temporary file in the
        // directory of the final file. This code owns the stream until it gives it to
        // `commit_atomic` or `discard_atomic` below. Each of the two takes
        // ownership of it: it closes the stream and releases the busy-bag
        // reservation of `add_atomic`, for either result of the write.
        let stored = unsafe {
            let mut error: *mut GError = ptr::null_mut();
            let Some(stream) = Owned::from_raw(camel_data_cache_add_atomic(
                cache.as_ptr(),
                MESSAGES.as_ptr(),
                key.as_ptr(),
                &mut error,
            )) else {
                log_critical_for_message(
                    uid,
                    &format!("message {uid} could not be cached: {}", describe(error)),
                );
                g_clear_error(&mut error);
                return false;
            };

            // The output stream is borrowed from `stream`, and `source` is a
            // live buffer of the given length. No close here: both calls
            // below close the stream when they take ownership of it.
            let mut written: usize = 0;
            let written_all = g_output_stream_write_all(
                g_io_stream_get_output_stream(stream.as_ptr()),
                source.as_ptr().cast_mut().cast(),
                source.len(),
                &mut written,
                ptr::null_mut(),
                &mut error,
            ) != GFALSE
                && written == source.len();

            let stored = if written_all {
                // `into_raw` gives the stream to `commit_atomic`, which
                // consumes it. A second release from the `Drop` of `stream`
                // would free it twice. `commit_atomic` opens a stream on the
                // renamed file. Only its presence is necessary, so
                // `Owned::from_raw(..).is_some()` checks it and drops it.
                Owned::from_raw(camel_data_cache_commit_atomic(
                    cache.as_ptr(),
                    stream.into_raw(),
                    &mut error,
                ))
                .is_some()
            } else {
                // `discard_atomic` takes ownership of `stream` in the same
                // way, closes it, and removes the temporary file. It gives
                // no error.
                camel_data_cache_discard_atomic(cache.as_ptr(), stream.into_raw());
                false
            };

            if !stored {
                log_critical_for_message(
                    uid,
                    &format!(
                        "message {uid} was cached incompletely and has been dropped: {}",
                        describe(error)
                    ),
                );
            }
            g_clear_error(&mut error);
            stored
        };

        stored
    }

    /// The path on disk of the entry for `uid`, also if it is not cached yet.
    ///
    /// `camel_data_cache_get_filename` only computes a path, the same path
    /// that `camel_data_cache_add`/`_get` use internally. Thus this function
    /// returns a path also for a message that was never fetched. The
    /// `get_filename` of `camel-imapx-folder.c` does the same: it returns the
    /// computed path without an existence test. This is safe for the same
    /// reason as there. Each caller of `CamelFolderClass::get_filename` treats a
    /// path that it cannot open as "no local copy", and reads the message in a
    /// different way. "View Source" in `e-mail-formatter-source.c` is this
    /// fallback. No caller reads the path as a promise that the file exists.
    ///
    /// `None` for the same reason that [`Self::key`] refuses: a uid that is not
    /// a valid JMAP id is never a file name that this cache creates.
    pub fn filename(&self, uid: &str) -> Option<String> {
        let key = self.key(uid)?;
        let cache = self.lock();

        // SAFETY: a live cache and two NUL-terminated strings that are alive
        // during the call. The returned pointer is a fresh `g_malloc`'d string
        // that this call owns.
        let raw = unsafe {
            camel_data_cache_get_filename(cache.as_ptr(), MESSAGES.as_ptr(), key.as_ptr())
        };
        // SAFETY: `raw` is NULL or the fresh allocation above, read before the
        // one free below. `read_string` copies it and keeps no borrow.
        let path = unsafe { read_string(raw) };
        // SAFETY: `raw` is NULL or a `g_malloc`'d pointer that this function
        // owns. `g_free` on NULL is a defined no-op.
        unsafe { g_free(raw.cast()) };
        path
    }

    /// A uid as the file name that it becomes, or `None` if it cannot be one.
    fn key(&self, uid: &str) -> Option<CString> {
        if !valid_key(uid) {
            log_critical_for_message(
                uid,
                &format!("message id {uid:?} is not a JMAP id and will not be cached"),
            );
            return None;
        }
        // The check makes this unreachable, because a NUL is not URL-safe. It
        // is still not an `expect`, because an error here is a panic in a vfunc.
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

/// Installs the vfunc on a class whose first member is a `CamelFolderClass`.
///
/// # Safety
///
/// `class` must point to an initialised class struct that starts with a
/// `CamelFolderClass`. Each descendant of `CamelFolder` has one.
pub unsafe fn install_vfuncs(class: *mut CamelFolderClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.get_filename = Some(get_filename);
}

/// Returns the path on disk where `uid` is cached, or would be cached. "Save
/// Message As", "View Source" and similar actions read the raw message bytes
/// from this path, and do not ask Camel to serialize the message again.
///
/// On success, a path, also if no file is there yet. NULL with the error set
/// when the folder has no cache, or `uid` is not a valid key. See
/// [`MessageCache::filename`] for why the usual case needs no check.
unsafe extern "C" fn get_filename(
    folder: *mut CamelFolder,
    uid: *const gchar,
    error: *mut *mut GError,
) -> *mut gchar {
    // SAFETY: the Camel contract for the vfunc: a valid instance of ours, a
    // NUL-terminated uid that is alive during the call, and an out-parameter
    // that is NULL, or writable and NULL.
    unsafe {
        guard_ptr("get_filename", error, || {
            let Some(uid) = read_string(uid) else {
                return fail(error, &StoreError::NoLocalFile, StoreError::to_gerror);
            };
            let Some(this) = JmapFolder::borrow(folder) else {
                return fail(error, &StoreError::NoLocalFile, StoreError::to_gerror);
            };
            let Some(cache) = this.cache() else {
                return fail(error, &StoreError::NoLocalFile, StoreError::to_gerror);
            };
            match cache.filename(&uid) {
                Some(path) => dup_string(&path),
                None => fail(error, &StoreError::NoLocalFile, StoreError::to_gerror),
            }
        })
    }
}

/// The size that a summary row claims, from the number that it holds.
///
/// Zero is not a claim. Camel's counter holds zero for a row that never got a
/// size. `MessageSummary` leaves zero for an `Email` that arrived without a
/// size. A row from a summary database that is older than the column also has
/// zero. RFC 5322 has no zero-octet message, so zero cannot be a true size. As
/// a claim, zero is satisfied by each entry, which is harmless. This function
/// makes that rule explicit, so the code does not depend on an accident.
fn claimed(listed: Option<u32>) -> Option<u32> {
    listed.filter(|listed| *listed != 0)
}

/// Whether `key` is an id that RFC 8620 §1.2 allows, and thus a file name that
/// this cache can create.
///
/// This is exactly the grammar of the RFC: one to 255 characters of
/// `A-Za-z0-9_-`, without a dash at the start. Each part of it is necessary.
/// The character set keeps a key in one directory. The RFC itself bans the
/// leading dash, so that an id is not read as a command-line option. The ban on
/// the empty string stops a key that names the directory itself. The character
/// set refuses `.` and `..`, the two most dangerous names.
fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 255
        && !key.starts_with('-')
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Reads an entry to its end, or returns `None` if it cannot read all of it.
///
/// A partial read is not half a message. The bytes are a MIME document, and a
/// truncated one parses into a message with a truncated body and no error.
/// Thus a failed read discards its bytes and does not return them.
///
/// `uid` names the entry in the log line of a failed read. It is the
/// identifier of the caller, because the stream has no identifier.
///
/// # Safety
///
/// `input` must point to a live `GInputStream`.
unsafe fn read_all(input: *mut gio_sys::GInputStream, uid: &str) -> Option<Vec<u8>> {
    let mut source: Vec<u8> = Vec::new();
    let mut chunk = [0u8; CHUNK];
    loop {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live stream by the contract above, a local buffer of the
        // given length, and an error out-parameter that is NULL at each pass.
        let read = unsafe {
            g_input_stream_read(
                input,
                chunk.as_mut_ptr().cast(),
                chunk.len(),
                ptr::null_mut(),
                &mut error,
            )
        };
        if read < 0 {
            log_critical_for_message(
                uid,
                &format!(
                    "the cached copy of message {uid} could not be read: {}",
                    unsafe { describe(error) }
                ),
            );
            // SAFETY: an owned GError or NULL.
            unsafe { g_clear_error(&mut error) };
            return None;
        }
        // SAFETY: as above. A successful read sets no error, and a clear of
        // NULL is a no-op.
        unsafe { g_clear_error(&mut error) };
        if read == 0 {
            return Some(source);
        }
        // A non-negative `gssize` fits a `usize` on each platform that Rust
        // supports, and `read` is at most `chunk.len()`.
        source.extend_from_slice(&chunk[..read as usize]);
    }
}

/// The message of a GError, for a log line. `"(no detail)"` for a NULL, which a
/// Camel function can leave. A log line that ends with nothing looks truncated.
///
/// # Safety
///
/// `error` must be NULL or point to a live `GError`.
unsafe fn describe(error: *mut GError) -> String {
    // SAFETY: the contract above. The message of a live GError is a
    // NUL-terminated string that the GError owns.
    unsafe {
        error
            .as_ref()
            .and_then(|error| jmap_backend_core::marshal::read_string(error.message))
            .unwrap_or_else(|| "(no detail)".to_owned())
    }
}
