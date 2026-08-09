// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where a message fetched once is kept, so the next open is not another
//! download.
//!
//! [`crate::message`] turns a row into mail with two round trips: an
//! `Email/get` for the blob id, then the blob itself. RFC 8621 §4.1 makes an
//! `Email` immutable, so the second click on the same row can only ever produce
//! the bytes the first one did — which makes the two round trips per open pure
//! waste, and makes a message the user has read unreadable the moment the
//! network goes away, in a provider whose store is a `CamelOfflineStore`.
//!
//! `CamelDataCache` is where Camel's own providers keep those bytes: a
//! directory, a subdirectory per kind of thing cached, and a file per entry
//! named by a key. IMAPX keeps its message cache in one. This is the safe
//! wrapper — three operations, all of them best-effort.
//!
//! ## Best-effort is the contract, not a shortcut
//!
//! Nothing here reports a failure to the caller as an error, and
//! [`MessageCache::open`] answers `None` rather than a `Result`. A cache is an
//! optimisation over a fetch that works: a full disk, a directory the user made
//! read-only, an entry another process removed mid-read are all conditions
//! under which mail must still open, just slower. Reporting them would turn a
//! working account into a broken one, and there is nobody to report them *to* —
//! the caller is a vfunc whose error out-parameter is reserved for the reason
//! the message could not be produced at all.
//!
//! ## One cache per folder, one entry per message
//!
//! A JMAP email id identifies a message in the *account*: the same message
//! filed in two mailboxes carries the same id in both, because a JMAP mailbox
//! is closer to a label than to a directory. So the key is the uid alone and
//! the directory is the account's, which makes a message filed in five
//! mailboxes one file rather than five — even though the object holding the
//! cache is the folder (see [`crate::folder`]), which is where the store's
//! cache directory is known at a well-defined moment.
//!
//! ## A uid is about to become a file name
//!
//! RFC 8620 §1.2 limits an id to URL-safe characters, and this is the layer
//! where believing that matters: `camel_data_cache_add` joins the key onto a
//! path. A server that answered `Email/query` with `../../../.config/autostart`
//! would otherwise be a server that chooses where this provider writes. Keys
//! are therefore checked against the RFC's own grammar and a key that fails it
//! is simply not cached — see [`valid_key`].
//!
//! ## An entry is checked against the row it belongs to
//!
//! An entry is written by one `write_all` and closed, and a write that fails
//! takes the entry back out. What that does not survive is the process dying
//! between the two: what is left is a short file that Camel's parser will read
//! as a *complete* message with a truncated body, because MIME has no length.
//!
//! So both operations take the size the folder's row claims — RFC 8621 §4.1's
//! `size`, which the spec defines as the octets of exactly the bytes the `blobId`
//! references — and an entry shorter than that is not a message. [`load`] drops
//! such an entry instead of serving it, and [`store`] declines to write one at
//! all, on the same reasoning that already refuses an empty one: an entry the
//! cache will not serve is a syscall spent on producing a miss.
//!
//! **Shorter, not different.** Truncation is the failure this closes, and a
//! short file is the whole of it. A server whose `size` is a byte out in the
//! other direction is a *reporting* fault, and an exact comparison would turn it
//! into a message that can never be cached — every open another two round trips,
//! forever. Between a mail client that tolerates an over-long entry and one that
//! re-downloads every message a slightly wrong server holds, the first is the one
//! that stays usable.
//!
//! [`load`]: MessageCache::load
//! [`store`]: MessageCache::store
//!
//! ## What is not here yet
//!
//! **A bound.** Nothing removes an entry that is merely old, so the cache grows
//! with every message the user has ever opened. `CamelDataCache` has the
//! machinery — `set_expire_age` and `set_expire_enabled`, evaluated when an entry
//! is added — and Evolution's own "empty cache" is `camel_data_cache_clear`;
//! which of the two this provider should use is a settings question rather than a
//! mechanism one, and it is its own increment.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::{Mutex, MutexGuard, PoisonError};

use eds_sys::{
    CamelDataCache, camel_data_cache_add, camel_data_cache_get, camel_data_cache_new,
    camel_data_cache_remove,
};
use gio_sys::{
    g_input_stream_read, g_io_stream_close, g_io_stream_get_input_stream,
    g_io_stream_get_output_stream, g_output_stream_write_all,
};
use glib_sys::{GError, GFALSE, g_clear_error};
use gobject_sys::g_object_unref;
use jmap_backend_core::trampoline::log_critical;

/// The subdirectory entries live in, below the account's cache directory.
///
/// Named rather than empty because a `CamelDataCache` is keyed by a path *and* a
/// key: the path is what tells one kind of cached thing from another under one
/// account, and a provider that put messages at the root would have nowhere to
/// put the next kind. IMAPX uses `"cur"`, after a maildir; this is what the
/// files are.
const MESSAGES: &CStr = c"messages";

/// How much is read from an entry at a time. A message is one file and is read
/// whole; the buffer only decides how many `read` calls that takes.
const CHUNK: usize = 64 * 1024;

/// The account's message cache.
pub struct MessageCache {
    /// The Camel object, behind a lock of our own.
    ///
    /// Camel documents no thread-safety guarantee for `CamelDataCache`, and
    /// Camel drives a folder from several threads at once — a refresh and two
    /// message opens are three operations that may all be in flight. The lock is
    /// held for the length of one entry's IO and never across a network fetch,
    /// which is the part of an open worth overlapping.
    cache: Mutex<*mut CamelDataCache>,
}

// SAFETY: the pointer is the only one to a `CamelDataCache` this process holds
// (it is created here and unreffed in `drop`, and no accessor hands it out), and
// every use of it goes through the `Mutex` above — so there is no way to reach
// the object from two threads at once.
unsafe impl Send for MessageCache {}
unsafe impl Sync for MessageCache {}

impl MessageCache {
    /// Opens — and creates — the cache under `directory`, or answers `None`.
    ///
    /// `camel_data_cache_new` makes the directory, so this is the call that
    /// fails when the account's cache directory cannot exist. `None` rather than
    /// an error, per the module's contract: the caller carries on without a
    /// cache. Logged as a critical, because a cache directory that cannot be
    /// made is a broken installation rather than a passing condition, and the
    /// symptom without a log line would be mail that is merely slow.
    pub fn open(directory: &str) -> Option<Self> {
        let path = CString::new(directory).ok()?;
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a NUL-terminated path alive across the call, and an
        // out-parameter that is writable and currently NULL.
        let cache = unsafe { camel_data_cache_new(path.as_ptr(), &mut error) };
        if cache.is_null() {
            log_critical(&format!(
                "no message cache under {directory}: {}",
                // SAFETY: `error` is NULL or an owned GError the failed call
                // left behind.
                unsafe { describe(error) }
            ));
            // SAFETY: `error` is either NULL or an owned GError this function
            // asked for and is the only holder of.
            unsafe { g_clear_error(&mut error) };
            return None;
        }
        // A cache that was created and an error are not both possible, but a
        // GError left behind by a call that succeeded would leak.
        // SAFETY: as above.
        unsafe { g_clear_error(&mut error) };
        Some(Self {
            cache: Mutex::new(cache),
        })
    }

    /// The bytes cached for `uid`, if any, given the size the folder's row
    /// claims for it — see [`claimed`] for what a claim is.
    ///
    /// `None` covers every reason there are none — never stored, removed since,
    /// unreadable, damaged, a key this cache will not look up — because the
    /// caller does the same thing with all of them: fetch the message.
    pub fn load(&self, uid: &str, listed: Option<u32>) -> Option<Vec<u8>> {
        let key = self.key(uid)?;
        let cache = self.lock();

        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live cache (the pointer is non-NULL by construction and
        // only `drop` invalidates it), two NUL-terminated strings alive across
        // the call, and a NULL error out-parameter.
        let stream =
            unsafe { camel_data_cache_get(*cache, MESSAGES.as_ptr(), key.as_ptr(), &mut error) };
        // A miss is the ordinary answer here — the file does not exist — so it
        // is not logged; the GError still has to go.
        // SAFETY: as above.
        unsafe { g_clear_error(&mut error) };
        if stream.is_null() {
            return None;
        }

        // SAFETY: `stream` is an owned `GIOStream` from the call above; the
        // input stream it hands over is borrowed from it and outlived by it.
        let source = unsafe { read_all(g_io_stream_get_input_stream(stream)) };
        // SAFETY: closing and dropping the reference this function owns. Closing
        // an already-failed stream is defined, and its error is not interesting:
        // whether the entry was read is what `source` says.
        unsafe {
            g_io_stream_close(stream, ptr::null_mut(), ptr::null_mut());
            g_object_unref(stream.cast());
        }

        let source = source?;
        // Neither an entry with nothing in it nor one shorter than the message
        // is a message, and both are what a process that died between `add` and
        // the write leaves behind. RFC 5322 has no zero-octet document, and
        // Camel's parser would make a message out of either rather than refuse
        // it — which the caller would then serve instead of fetching, for as
        // long as the entry survived.
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

        log_critical(&format!(
            "the cached copy of message {uid} was dropped: {damage}"
        ));
        // Dropped rather than merely refused: nothing will ever serve it, and
        // the cache has no bound of its own, so leaving it is leaving a file
        // that costs disk and produces this log line at every open. A fetch that
        // succeeds writes the entry again; one that does not leaves the cache
        // where it should be — empty of this message.
        // SAFETY: as above, and the removal's own failure is nothing this can
        // act on.
        unsafe {
            camel_data_cache_remove(*cache, MESSAGES.as_ptr(), key.as_ptr(), ptr::null_mut());
        }
        None
    }

    /// Caches `source` as the bytes of `uid`, reporting whether it landed.
    ///
    /// `listed` is the size the folder's row claims, as in [`load`].
    ///
    /// Reported rather than silent so a test can tell a refusal from a success;
    /// the caller ignores it, because there is nothing it would do differently.
    ///
    /// An entry that could not be written completely is removed rather than
    /// left: a short file is not a failed cache write, it is a message that
    /// opens with half its body missing every time it is opened from then on.
    ///
    /// [`load`]: MessageCache::load
    pub fn store(&self, uid: &str, source: &[u8], listed: Option<u32>) -> bool {
        let Some(key) = self.key(uid) else {
            return false;
        };
        if source.is_empty() {
            // The one entry [`MessageCache::load`] will not serve, so writing it
            // would only be a way to spend a syscall on a miss. A message with
            // no bytes is not a message; whatever produced one, the cache is not
            // where that gets settled.
            return false;
        }
        if let Some(listed) = claimed(listed).filter(|listed| source.len() < *listed as usize) {
            // The other entry `load` will not serve, and here the bytes are the
            // ones that just came off the network — so what disagrees is the
            // server with itself, rather than a file with a crash. Worth a line
            // either way, because the visible symptom is a message that is
            // downloaded again every single time it is opened.
            log_critical(&format!(
                "message {uid} arrived as {} octets where its row says {listed}, and is not cached",
                source.len()
            ));
            return false;
        }
        let cache = self.lock();

        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: as in `load`. `add` replaces any entry already under the key.
        let stream =
            unsafe { camel_data_cache_add(*cache, MESSAGES.as_ptr(), key.as_ptr(), &mut error) };
        if stream.is_null() {
            log_critical(&format!("message {uid} could not be cached: {}", unsafe {
                describe(error)
            }));
            // SAFETY: an owned GError or NULL.
            unsafe { g_clear_error(&mut error) };
            return false;
        }

        // SAFETY: `stream` is an owned `GIOStream`; the output stream is
        // borrowed from it, `source` is a live buffer of the length given, and
        // the two out-parameters are locals.
        let stored = unsafe {
            let mut written: usize = 0;
            let written_all = g_output_stream_write_all(
                g_io_stream_get_output_stream(stream),
                // gio-sys types the buffer as `*mut`, which the C declaration
                // does not: `g_output_stream_write_all` takes a `const void *`.
                // The cast restores what the header says.
                source.as_ptr().cast_mut().cast(),
                source.len(),
                &mut written,
                ptr::null_mut(),
                &mut error,
            ) != GFALSE;
            // The close is what flushes, so a write that reported success and a
            // close that failed is still an incomplete entry. Its error goes
            // into a local of its own: GLib logs a critical of its own for a
            // second `g_set_error` over an out-parameter that is already set,
            // and the write's reason is the one worth reporting.
            let mut on_close: *mut GError = ptr::null_mut();
            let closed = g_io_stream_close(stream, ptr::null_mut(), &mut on_close) != GFALSE;
            if error.is_null() {
                error = on_close;
            } else {
                g_clear_error(&mut on_close);
            }
            g_object_unref(stream.cast());
            written_all && closed && written == source.len()
        };

        if !stored {
            log_critical(&format!(
                "message {uid} was cached incompletely and has been dropped: {}",
                // SAFETY: `error` is NULL or an owned GError from one of the two
                // calls above.
                unsafe { describe(error) }
            ));
            // SAFETY: as in `load`, and the removal's own failure is nothing
            // this can act on.
            unsafe {
                camel_data_cache_remove(*cache, MESSAGES.as_ptr(), key.as_ptr(), ptr::null_mut());
            }
        }
        // SAFETY: an owned GError or NULL.
        unsafe { g_clear_error(&mut error) };
        stored
    }

    /// A uid as the file name it is about to become, or `None` if it is not one.
    fn key(&self, uid: &str) -> Option<CString> {
        if !valid_key(uid) {
            log_critical(&format!(
                "message id {uid:?} is not a JMAP id and will not be cached"
            ));
            return None;
        }
        // Unreachable given the check — a NUL is not URL-safe — and still not
        // an `expect`, because the cost of being wrong is a panic inside a
        // vfunc.
        CString::new(uid).ok()
    }

    /// A poisoned lock means an operation panicked mid-entry. What the lock
    /// guards is a pointer to a live Camel object, which that cannot damage, so
    /// carrying on beats taking the account down with whatever already failed.
    fn lock(&self) -> MutexGuard<'_, *mut CamelDataCache> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for MessageCache {
    fn drop(&mut self) {
        let cache = *self.cache.get_mut().unwrap_or_else(PoisonError::into_inner);
        // SAFETY: the one reference this type took in `open`, and `&mut self`
        // means nothing is using it.
        unsafe { g_object_unref(cache.cast()) };
    }
}

/// The size a summary row claims, out of the number it carries.
///
/// Zero is not a claim. It is what Camel's counter holds for a row that was
/// never given a size — `MessageSummary` leaves it there for an `Email` that
/// arrived without one, and a row loaded from a summary database written before
/// the column existed has it too — and RFC 5322 has no zero-octet message for it
/// to honestly mean. Read as a claim it would be the one claim every entry
/// satisfies, which is harmless; named here so that the reason it is harmless is
/// not the reason it is being relied on.
fn claimed(listed: Option<u32>) -> Option<u32> {
    listed.filter(|listed| *listed != 0)
}

/// Whether `key` is an id RFC 8620 §1.2 allows, and therefore a file name this
/// cache is willing to create.
///
/// The RFC's grammar exactly: one to 255 characters of `A-Za-z0-9_-`, not
/// beginning with a dash. Every part of it earns its place here — the character
/// set keeps a key inside one directory, the ban on a leading dash is the RFC's
/// own (it keeps an id out of an argument position), and rejecting the empty
/// string is what stops a key naming the directory itself. `.` and `..`, the two
/// names that would matter most, are refused by the character set.
fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 255
        && !key.starts_with('-')
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Reads an entry to its end, or `None` if it could not be read whole.
///
/// A partial read is not half a message: the bytes are a MIME document, and one
/// cut short parses into a message with a truncated body and no complaint. So a
/// failed read discards what it had rather than handing it over.
///
/// # Safety
///
/// `input` must point at a live `GInputStream`.
unsafe fn read_all(input: *mut gio_sys::GInputStream) -> Option<Vec<u8>> {
    let mut source: Vec<u8> = Vec::new();
    let mut chunk = [0u8; CHUNK];
    loop {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live stream by the contract above, a local buffer of the
        // length given, and an error out-parameter that starts NULL each pass.
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
            log_critical(&format!("a cached message could not be read: {}", unsafe {
                describe(error)
            }));
            // SAFETY: an owned GError or NULL.
            unsafe { g_clear_error(&mut error) };
            return None;
        }
        // SAFETY: as above; a successful read leaves no error, and clearing
        // NULL is a no-op.
        unsafe { g_clear_error(&mut error) };
        if read == 0 {
            return Some(source);
        }
        // A non-negative `gssize` fits a `usize` on every platform Rust
        // supports, and `read` is at most `chunk.len()`.
        source.extend_from_slice(&chunk[..read as usize]);
    }
}

/// A GError's message, for a log line. `"(no detail)"` for the NULL a Camel
/// function is entitled to leave behind, since a log line that ended in nothing
/// would read like a truncated one.
///
/// # Safety
///
/// `error` must be NULL or point at a live `GError`.
unsafe fn describe(error: *mut GError) -> String {
    // SAFETY: the contract above; the message of a live GError is a
    // NUL-terminated string owned by it.
    unsafe {
        error
            .as_ref()
            .and_then(|error| jmap_backend_core::marshal::read_string(error.message))
            .unwrap_or_else(|| "(no detail)".to_owned())
    }
}
