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
//! is simply not cached — see `valid_key`.
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
//! ## The bound: an entry survives being ignored, not being forgotten
//!
//! A cache that only grows is a cache that ends as a full disk, so an entry
//! nobody has opened for `UNUSED_FOR` is dropped. Of the two clocks
//! `CamelDataCache` can be given, that is `set_expire_access` — the file's atime
//! — rather than `set_expire_age`, which is its mtime, and an `Email` being
//! immutable means its mtime is the moment it was *downloaded* and nothing else.
//! A bound on age alone would therefore drop the message the user reads every
//! week on the same schedule as the one they read once, which is the wrong one
//! to spend a round trip on.
//!
//! atime is a weaker signal than it looks — `relatime`, the usual mount option,
//! only updates it once a day, and `noatime` never does — but both fail in the
//! conservative direction here: a day's resolution is nothing against a bound of
//! a month, and on `noatime` the atime stays at the moment the file was written,
//! so the bound quietly becomes the age one. An entry kept too long is a file;
//! an entry dropped too early is a round trip.
//!
//! **The sweep is lazy and this is not a quota.** Camel expires a bucket when a
//! lookup lands in it, at most once an hour, so an account nobody opens is an
//! account nothing is swept from, and a cache is only ever as small as its bound
//! makes it — not as small as a number of megabytes. A real quota would have to
//! be ours to write (`camel_data_cache_foreach_remove` is the hook), and the
//! number it enforced would be one to ask the user about; this bound needs no
//! question answered.
//!
//! ## What is not here yet
//!
//! **A knob.** `UNUSED_FOR` is a constant rather than a setting, because Camel
//! has nowhere to put it: `CamelOfflineSettings`'s `limit-by-age` is about which
//! messages get *downloaded* for offline use, not how long a downloaded one is
//! kept, and reading it as the latter would be an account's offline window
//! silently doubling as its cache's. A setting of our own is a field in an
//! account editor, which is M7's business rather than this file's.
//!
//! **Writing an entry under a temporary name and renaming it into place**, which
//! is what would make a half-written entry impossible rather than merely
//! detected (see the size check above). EDS 3.62 grew
//! `camel_data_cache_add_atomic`/`commit_atomic` for exactly this; 3.52, which
//! this builds against, has neither, so the check stays the answer here.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use eds_sys::{
    CamelDataCache, camel_data_cache_add, camel_data_cache_get, camel_data_cache_new,
    camel_data_cache_remove, camel_data_cache_set_expire_access,
    camel_data_cache_set_expire_enabled, time_t,
};
use gio_sys::{
    g_input_stream_read, g_io_stream_close, g_io_stream_get_input_stream,
    g_io_stream_get_output_stream, g_output_stream_write_all,
};
use glib_sys::{GError, GFALSE, GTRUE, g_clear_error};
use jmap_backend_core::owned::Owned;
use jmap_backend_core::trampoline::{log_critical, log_critical_for_message};

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

/// How long an entry survives without being opened, before the next lookup in
/// its neighbourhood sweeps it. See the module docs for why this is an atime and
/// why it is a constant.
///
/// Thirty days: long enough that "the message I was reading last month" is still
/// there, short enough that a year of mail is not. What it costs when it is
/// wrong is one `Email/get` and one blob download for a message the user has
/// come back to — which is exactly what every open cost before this cache
/// existed, so the failure mode of the bound is the behaviour it replaced.
const UNUSED_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The account's message cache.
pub struct MessageCache {
    /// The Camel object, behind a lock of our own.
    ///
    /// Camel documents no thread-safety guarantee for `CamelDataCache`, and
    /// Camel drives a folder from several threads at once — a refresh and two
    /// message opens are three operations that may all be in flight. The lock is
    /// held for the length of one entry's IO and never across a network fetch,
    /// which is the part of an open worth overlapping.
    cache: Mutex<Owned<CamelDataCache>>,
}

// SAFETY: the reference is the only one to a `CamelDataCache` this process
// holds (it is created here and released when the `Owned` drops with this
// struct, and no accessor hands it out), and every use of it goes through the
// `Mutex` above — so there is no way to reach the object from two threads at
// once. `Owned` itself is neither `Send` nor `Sync` (its own docs say why),
// but that is a default this manual pair of impls is explicitly overriding on
// the strength of the invariant just stated.
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
        Self::open_bounded(directory, UNUSED_FOR)
    }

    /// The same, with the bound given rather than taken from `UNUSED_FOR`.
    ///
    /// The policy lives in one constant and the mechanism takes a parameter, so
    /// that a test can watch an entry go without waiting a month or writing an
    /// atime of its own — the age Camel reads is the filesystem's, and forging
    /// one would be testing `utimes` rather than this.
    pub fn open_bounded(directory: &str, unused_for: Duration) -> Option<Self> {
        let path = CString::new(directory).ok()?;
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a NUL-terminated path alive across the call, and an
        // out-parameter that is writable and currently NULL. The reference
        // `camel_data_cache_new` hands back is ours, and `cache` releases it on
        // whatever path does not return it in `Self`.
        let Some(cache) =
            (unsafe { Owned::from_raw(camel_data_cache_new(path.as_ptr(), &mut error)) })
        else {
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
        };
        // A cache that was created and an error are not both possible, but a
        // GError left behind by a call that succeeded would leak.
        // SAFETY: as above.
        unsafe { g_clear_error(&mut error) };

        // Camel reads the bound as a `time_t` and spells "no bound" as -1, so a
        // duration too large for one is clamped to the largest bound rather than
        // wrapped into the value that turns the sweep off. Both calls are
        // saying what is already the default — expiry is enabled on a fresh
        // `CamelDataCache` — except for the bound itself, which starts at -1.
        // SAFETY: the cache is the live object just created, and neither call
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

    /// The bytes cached for `uid`, if any, given the size the folder's row
    /// claims for it — see `claimed` for what a claim is.
    ///
    /// `None` covers every reason there are none — never stored, removed since,
    /// unreadable, damaged, a key this cache will not look up — because the
    /// caller does the same thing with all of them: fetch the message.
    pub fn load(&self, uid: &str, listed: Option<u32>) -> Option<Vec<u8>> {
        let key = self.key(uid)?;
        let cache = self.lock();

        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live cache (the pointer is non-NULL by construction and
        // only `Drop` invalidates it), two NUL-terminated strings alive across
        // the call, and a NULL error out-parameter. The reference the call
        // hands back on a hit is ours; `stream` releases it wherever this
        // scope ends.
        let Some(stream) = (unsafe {
            Owned::from_raw(camel_data_cache_get(
                cache.as_ptr(),
                MESSAGES.as_ptr(),
                key.as_ptr(),
                &mut error,
            ))
        }) else {
            // A miss is the ordinary answer here — the file does not exist — so
            // it is not logged; the GError still has to go.
            // SAFETY: as above.
            unsafe { g_clear_error(&mut error) };
            return None;
        };
        // SAFETY: as above.
        unsafe { g_clear_error(&mut error) };

        // SAFETY: `stream` is an owned `GIOStream` from the call above; the
        // input stream it hands over is borrowed from it and outlived by it.
        let source = unsafe { read_all(g_io_stream_get_input_stream(stream.as_ptr())) };
        // SAFETY: closing the reference this function owns; `stream` itself
        // releases it wherever this scope ends. Closing an already-failed
        // stream is defined, and its error is not interesting: whether the
        // entry was read is what `source` says.
        unsafe {
            g_io_stream_close(stream.as_ptr(), ptr::null_mut(), ptr::null_mut());
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

        log_critical_for_message(
            uid,
            &format!("the cached copy of message {uid} was dropped: {damage}"),
        );
        // Dropped rather than merely refused: nothing will ever serve it, so
        // leaving it is leaving a file that costs disk and produces this log
        // line at every open until the bound gets round to it — and the bound is
        // measured in the opens that would each produce one. A fetch that
        // succeeds writes the entry again; one that does not leaves the cache
        // where it should be — empty of this message.
        // SAFETY: as above, and the removal's own failure is nothing this can
        // act on.
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

        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: as in `load`. `add` replaces any entry already under the
        // key. The reference the call hands back is ours; `stream` releases
        // it wherever this scope ends.
        let Some(stream) = (unsafe {
            Owned::from_raw(camel_data_cache_add(
                cache.as_ptr(),
                MESSAGES.as_ptr(),
                key.as_ptr(),
                &mut error,
            ))
        }) else {
            log_critical_for_message(
                uid,
                &format!("message {uid} could not be cached: {}", unsafe {
                    describe(error)
                }),
            );
            // SAFETY: an owned GError or NULL.
            unsafe { g_clear_error(&mut error) };
            return false;
        };

        // SAFETY: `stream` is an owned `GIOStream`; the output stream is
        // borrowed from it, `source` is a live buffer of the length given, and
        // the two out-parameters are locals.
        let stored = unsafe {
            let mut written: usize = 0;
            let written_all = g_output_stream_write_all(
                g_io_stream_get_output_stream(stream.as_ptr()),
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
            let closed =
                g_io_stream_close(stream.as_ptr(), ptr::null_mut(), &mut on_close) != GFALSE;
            if error.is_null() {
                error = on_close;
            } else {
                g_clear_error(&mut on_close);
            }
            written_all && closed && written == source.len()
        };

        if !stored {
            log_critical_for_message(
                uid,
                &format!(
                    "message {uid} was cached incompletely and has been dropped: {}",
                    // SAFETY: `error` is NULL or an owned GError from one of the
                    // two calls above.
                    unsafe { describe(error) }
                ),
            );
            // SAFETY: as in `load`, and the removal's own failure is nothing
            // this can act on.
            unsafe {
                camel_data_cache_remove(
                    cache.as_ptr(),
                    MESSAGES.as_ptr(),
                    key.as_ptr(),
                    ptr::null_mut(),
                );
            }
        }
        // SAFETY: an owned GError or NULL.
        unsafe { g_clear_error(&mut error) };
        stored
    }

    /// A uid as the file name it is about to become, or `None` if it is not one.
    fn key(&self, uid: &str) -> Option<CString> {
        if !valid_key(uid) {
            log_critical_for_message(
                uid,
                &format!("message id {uid:?} is not a JMAP id and will not be cached"),
            );
            return None;
        }
        // Unreachable given the check — a NUL is not URL-safe — and still not
        // an `expect`, because the cost of being wrong is a panic inside a
        // vfunc.
        CString::new(uid).ok()
    }

    /// A poisoned lock means an operation panicked mid-entry. What the lock
    /// guards is a reference to a live Camel object, which that cannot damage,
    /// so carrying on beats taking the account down with whatever already
    /// failed.
    fn lock(&self) -> MutexGuard<'_, Owned<CamelDataCache>> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
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
