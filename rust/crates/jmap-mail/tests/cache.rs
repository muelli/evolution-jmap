// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The message cache: a message fetched once and kept on disk.
//!
//! `get_message_sync` downloads a message each time the user clicks its row.
//! RFC 8621 §4.1 makes an `Email` immutable, so a second download always
//! produces the same bytes as the first. `CamelDataCache` is where Camel's own
//! providers store those bytes, one file per message, under the account's
//! cache directory. This module wraps `CamelDataCache`.
//!
//! The tests here pin the wrapper's contract, not the vfunc's contract
//! (`tests/message.rs` covers the vfunc). They check: bytes read back exactly
//! as written; a miss reported as a miss, not a failure; two caches over one
//! directory sharing the same entries, which lets a message filed in two
//! mailboxes stay one file; and a uid that is a path, refused as a cache key.
//!
//! One entry is not a message: an entry shorter than the summary row claims.
//! A process killed mid-write leaves an entry like this, and Camel's parser
//! would otherwise read it as a complete message with a short body.
//!
//! Since this increment, the cache also has a bound: an entry nobody has
//! opened for long enough is swept. This gives a user a describable size for
//! a cache that otherwise only grows.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use jmap_mail::cache::MessageCache;

/// The message every test in this file caches. Long enough that a truncated
/// read is visible as truncated.
const SOURCE: &[u8] = b"From: bob@example.com\r\nSubject: Cached\r\n\r\nThe body, \
                        which is longer than the headers so that a truncated \
                        read is not mistaken for a complete one.\r\n";

/// Distinguishes one test's cache directory from the next, including two
/// tests running at once, which a Rust test binary does by default.
static DIRECTORIES: AtomicUsize = AtomicUsize::new(0);

/// A directory of its own, removed when the test ends.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jmap-mail-cache-{}-{}",
            std::process::id(),
            DIRECTORIES.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("a directory for the cache");
        Self { path }
    }

    fn as_str(&self) -> &str {
        self.path.to_str().expect("a UTF-8 temporary path")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a panic during teardown would not improve a test that
        // already failed.
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Every file the cache has written, wherever under its directory the cache
/// put them.
///
/// `CamelDataCache` files a key into one of sixty-four subdirectories, chosen
/// by a hash of the key. This hashing is `CamelDataCache`'s own business, not
/// this crate's. So the bound tests below read the directory tree instead of
/// constructing a path, and check only the one fact about the hashing they
/// depend on (see [`share_a_bucket`]).
fn files_under(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(directory).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// Asserts that the cache filed exactly `keys` entries, all in one bucket.
///
/// Camel sweeps a bucket when something in it is looked up, but skips the
/// entry being looked up. So an expiry is observable only through another key
/// that hashes to the same bucket. `M1` and `M2` hash to the same bucket, a
/// fact about `g_str_hash`, not a guarantee. Asserting it here means a Camel
/// that changes its hashing fails these tests with a clear message, instead
/// of letting them quietly prove nothing.
fn share_a_bucket(scratch: &Scratch, keys: usize) -> Vec<PathBuf> {
    let files = files_under(&scratch.path);
    assert_eq!(
        files.len(),
        keys,
        "not every entry reached the disk: {files:?}"
    );
    let bucket = files[0].parent();
    assert!(
        files.iter().all(|file| file.parent() == bucket),
        "the entries landed in different hash buckets, so opening one cannot \
         sweep another and this test would prove nothing: {files:?}"
    );
    files
}

#[test]
fn a_stored_message_reads_back_byte_for_byte() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    assert!(
        cache.store("M1", SOURCE, None),
        "the message was not cached"
    );
    assert_eq!(
        cache.load("M1", None).as_deref(),
        Some(SOURCE),
        "the cached message did not read back as it was written"
    );
}

/// A cache with no origin file keeps its entries when it adopts an origin. A
/// different origin later removes them, and a second cache object on the same
/// directory sees the same origin.
#[test]
fn a_different_origin_removes_the_entries() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");
    assert!(cache.store("M1", SOURCE, None));

    cache.adopt_origin("https://old.example/jmap A1");
    assert_eq!(cache.load("M1", None).as_deref(), Some(SOURCE));
    cache.adopt_origin("https://old.example/jmap A1");
    assert_eq!(cache.load("M1", None).as_deref(), Some(SOURCE));

    let other = MessageCache::open(scratch.as_str()).expect("a second cache");
    other.adopt_origin("https://new.example/jmap A1");
    assert_eq!(
        cache.load("M1", None),
        None,
        "an entry of the old origin stayed"
    );
}

/// The path that `filename` returns, read as an ordinary file, holds the
/// exact bytes `store` wrote. This is the purpose of a filename:
/// `CamelFolderClass::get_filename` can hand it to a caller instead of
/// re-serializing the message.
#[test]
fn a_stored_messages_filename_reads_back_the_same_bytes() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    assert!(cache.store("M1", SOURCE, None));
    let path = cache.filename("M1").expect("a filename for a cached entry");
    assert_eq!(
        fs::read(&path).expect("the filename names a readable file"),
        SOURCE,
        "the file at the filename was not the cached message"
    );
}

/// `camel_data_cache_get_filename` is a pure path computation, the same one
/// `store` and `load` use internally to find an entry. It answers even for a
/// uid that was never stored, exactly as `camel-imapx-folder.c`'s own
/// `get_filename` does: neither checks that the file exists. Only the file is
/// missing, not the answer.
#[test]
fn an_uncached_uid_still_gets_a_computed_filename() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    let path = cache
        .filename("M1")
        .expect("a filename computed for an uncached uid");
    assert!(
        !Path::new(&path).exists(),
        "a uid nothing stored already has a file on disk"
    );
}

/// `filename` gives the same refusal as `store` and `load` for a uid that is
/// a path in disguise. `filename` is one more way this cache turns a uid into
/// a file name, and a hostile uid gets no path from it either.
#[test]
fn a_uid_that_is_a_path_gets_no_filename() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    for hostile in ["../escape", "..", ".", "sub/dir", "/absolute", ""] {
        assert!(
            cache.filename(hostile).is_none(),
            "{hostile:?} was accepted as a cache key"
        );
    }
}

#[test]
fn a_uid_that_was_never_cached_is_a_miss() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    assert!(
        cache.load("M1", None).is_none(),
        "an empty cache answered with a message"
    );
}

/// A refetched message re-stores its entry, and the result must be readable,
/// not appended to. `camel_data_cache_add` replaces the file. A wrapper that
/// opened the file for appending would duplicate the content of every
/// message cached twice.
#[test]
fn storing_a_uid_twice_replaces_the_entry() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    assert!(cache.store("M1", b"the first, longer version of the message", None));
    assert!(cache.store("M1", SOURCE, None));
    assert_eq!(
        cache.load("M1", None).as_deref(),
        Some(SOURCE),
        "the second store did not replace the first"
    );
}

/// A message with no bytes is not a message. An entry with no bytes is what a
/// process killed between creating and writing it leaves behind. Both
/// `store` and `load` refuse this entry. Otherwise, `get_message_sync` would
/// serve an empty message from the cache for as long as the entry lives,
/// instead of the download that would have replaced it.
#[test]
fn an_entry_with_nothing_in_it_is_not_a_message() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    assert!(!cache.store("M1", b"", None), "an empty message was cached");
    assert!(
        cache.load("M1", None).is_none(),
        "an empty entry was served as a message"
    );
}

/// The account-wide property. A JMAP email id identifies a message in the
/// account, not in a mailbox. The same message filed in two mailboxes is
/// therefore one entry. This lets every folder of an account hold its own
/// cache over one directory, without storing the mail twice.
#[test]
fn two_caches_over_one_directory_share_their_entries() {
    let scratch = Scratch::new();
    let first = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");
    let second = MessageCache::open(scratch.as_str()).expect("a second cache over the same one");

    assert!(first.store("M1", SOURCE, None));
    assert_eq!(
        second.load("M1", None).as_deref(),
        Some(SOURCE),
        "the second cache did not see the first one's entry"
    );
}

/// RFC 8620 §1.2 limits an id to URL-safe characters, and this cache turns an
/// id into a file name. Without this check, a server that answered
/// `Email/query` with `../../../../etc/cron.d/x` could pick the path its mail
/// is written to.
#[test]
fn a_uid_that_is_a_path_is_not_a_key() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    for hostile in [
        "../escape",
        "..",
        ".",
        "sub/dir",
        "/absolute",
        "with space",
        "-leading-dash",
        "",
    ] {
        assert!(
            !cache.store(hostile, SOURCE, None),
            "{hostile:?} was accepted as a cache key"
        );
        assert!(
            cache.load(hostile, None).is_none(),
            "{hostile:?} was looked up as a cache key"
        );
    }

    // Nothing of this kind reached the filesystem: the cache put nothing
    // above its own directory.
    assert!(
        !scratch
            .path
            .parent()
            .expect("a parent")
            .join("escape")
            .exists(),
        "a key escaped the cache directory"
    );
}

/// RFC 8620 §1.2 defines a JMAP `Id` as 1 to 255 characters from
/// `[A-Za-z0-9_-]`. So `_` and `-` inside a uid are not hostile, unlike a
/// leading dash or a path separator. An ordinary id using `_` or `-` must
/// still be cached.
#[test]
fn a_uid_with_underscore_or_dash_is_a_key() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    for uid in ["M_1", "M-1", "under_score", "em-dash-id"] {
        assert!(cache.store(uid, SOURCE, None), "{uid:?} was not cached");
        assert_eq!(
            cache.load(uid, None).as_deref(),
            Some(SOURCE),
            "{uid:?} did not read back"
        );
    }
}

/// The check the rest of this block is about. A summary row carries the
/// octet count RFC 8621 §4.1 defines for the message. An entry that agrees
/// with this count is the ordinary case, and it is served without remark.
/// This test pins that the check costs nothing when nothing is wrong, the
/// failure mode a too-strict comparison would produce.
#[test]
fn an_entry_the_row_agrees_with_is_a_message() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");
    let size = Some(SOURCE.len() as u32);

    assert!(
        cache.store("M1", SOURCE, size),
        "the message was not cached"
    );
    assert_eq!(
        cache.load("M1", size).as_deref(),
        Some(SOURCE),
        "an entry the row agrees with was not served"
    );
}

/// What a process killed mid-write leaves behind: a file holding only the
/// beginning of a message. MIME has no overall length field, so Camel's
/// parser reads this file as a complete message with a short body, without
/// error. This message opens wrong every time, instead of the download that
/// would open it correctly.
#[test]
fn an_entry_shorter_than_the_row_claims_is_not_a_message() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");
    let truncated = &SOURCE[..SOURCE.len() - 30];

    // Written without a size claim: whatever killed the process did not
    // consult the row.
    assert!(cache.store("M1", truncated, None));

    assert!(
        cache.load("M1", Some(SOURCE.len() as u32)).is_none(),
        "a truncated entry was served as the message"
    );
}

/// The refused entry is removed, not merely refused. Nothing will ever serve
/// it. An entry left in place would keep producing the same critical error at
/// every open, until the bound below eventually swept the file that each
/// open keeps checking.
#[test]
fn an_entry_that_was_refused_is_dropped() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    assert!(cache.store("M1", &SOURCE[..SOURCE.len() - 30], None));
    assert!(cache.load("M1", Some(SOURCE.len() as u32)).is_none());

    assert!(
        cache.load("M1", None).is_none(),
        "the refused entry is still on disk"
    );
}

/// The other direction is a server that under-reports the size, not a file
/// that is short. These are not the same fault. An exact comparison would
/// make every message from such a server impossible to cache, forcing two
/// round trips per open forever, to catch a condition that truncation cannot
/// produce.
#[test]
fn an_entry_longer_than_the_row_claims_is_still_a_message() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");
    let understated = Some(SOURCE.len() as u32 - 30);

    assert!(cache.store("M1", SOURCE, understated));
    assert_eq!(
        cache.load("M1", understated).as_deref(),
        Some(SOURCE),
        "an entry longer than its row claims was thrown away"
    );
}

/// Zero is what Camel's counter holds for a row that was never given a size:
/// an `Email` that arrived with no size, or a row read from a summary
/// database written before the size column existed. Zero means no size
/// claim, not a claim that the message is empty.
#[test]
fn a_row_with_no_size_claims_nothing() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    assert!(cache.store("M1", SOURCE, Some(0)));
    assert_eq!(
        cache.load("M1", Some(0)).as_deref(),
        Some(SOURCE),
        "a row carrying no size was read as one claiming an empty message"
    );
}

/// The same rule applies when writing. Bytes the cache would refuse to serve
/// are not worth storing: here, the server disagrees with itself, not a file
/// damaged by a crash. Writing the entry anyway would leave one that is read
/// once, rejected, and removed at every open.
#[test]
fn bytes_shorter_than_the_row_claims_are_not_cached() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    assert!(
        !cache.store(
            "M1",
            &SOURCE[..SOURCE.len() - 30],
            Some(SOURCE.len() as u32)
        ),
        "bytes shorter than the row claims were cached"
    );
    assert!(
        cache.load("M1", None).is_none(),
        "they reached the disk anyway"
    );
}

/// The bound. Previously, nothing removed an entry just for being old, so a
/// cache grew by every message the account ever opened, and stopped growing
/// only when the disk was full. `CamelDataCache` sweeps lazily, only when a
/// lookup touches the bucket an entry lives in. So this test drives a lookup
/// of the other key in the bucket, which is what opening a second message
/// does.
#[test]
fn an_entry_nobody_has_opened_within_the_bound_is_swept() {
    let scratch = Scratch::new();
    let writer = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    assert!(writer.store("M1", SOURCE, None));
    assert!(writer.store("M2", SOURCE, None));
    share_a_bucket(&scratch, 2);

    // Long enough that an entry written just now is past a bound of zero.
    // The comparison uses whole seconds, so crossing one second is enough.
    std::thread::sleep(Duration::from_millis(1_500));

    // Uses a bound of zero rather than a backdated file: Camel reads the
    // file's age from the filesystem, and this crate must not write that age
    // directly.
    let bounded = MessageCache::open_bounded(scratch.as_str(), Duration::ZERO)
        .expect("a cache over the same directory");
    assert_eq!(
        bounded.load("M2", None).as_deref(),
        Some(SOURCE),
        "the entry being opened was swept along with the rest"
    );
    assert!(
        bounded.load("M1", None).is_none(),
        "an entry past the bound is still on disk"
    );
}

/// The bound limits entries, it does not clear the cache. The default bound
/// is long enough that an entry written a moment ago is still served from
/// disk on the next open. This test would fail if the sweep above were
/// `camel_data_cache_clear`, which is Evolution's manual "empty cache"
/// command, not a policy to apply just because a user opened two messages.
#[test]
fn an_entry_inside_the_bound_is_left_alone() {
    let scratch = Scratch::new();
    let writer = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    assert!(writer.store("M1", SOURCE, None));
    assert!(writer.store("M2", SOURCE, None));
    share_a_bucket(&scratch, 2);

    let reader = MessageCache::open(scratch.as_str()).expect("a cache over the same directory");
    assert!(reader.load("M2", None).is_some());
    assert_eq!(
        reader.load("M1", None).as_deref(),
        Some(SOURCE),
        "an entry well inside the bound was swept"
    );
}

/// A cache that cannot be created is not a failure to report to the user.
/// Mail still opens; it is just fetched every time. So the constructor
/// returns `None` instead of an error, and every caller treats `None` as "no
/// cache".
#[test]
fn a_directory_that_cannot_be_created_is_not_a_cache() {
    let scratch = Scratch::new();
    let blocked = scratch.path.join("in-the-way");
    fs::write(&blocked, b"a file where the cache directory would go").expect("a file");

    assert!(
        MessageCache::open(blocked.to_str().expect("a UTF-8 path")).is_none(),
        "a cache was opened over a plain file"
    );
}

/// On EDS 3.62+, `store` writes under a temporary name, then renames it into
/// place (`camel_data_cache_add_atomic` and `commit_atomic`, see
/// `src/cache.rs`'s module docs). EDS 3.52-3.61 instead open a stream
/// directly on the final path. A round trip cannot detect one specific bug
/// in the atomic path: a bug that renames the file into place but also
/// leaves the temporary file behind (instead of `commit_atomic` consuming
/// it) would still read back correctly. This test's directory walk catches
/// that bug, because it leaves a `.`-suffixed file behind.
#[cfg(camel_data_cache_atomic)]
#[test]
fn storing_leaves_no_temporary_file_behind() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    assert!(cache.store("M1", SOURCE, None));

    let files = files_under(&scratch.path);
    assert_eq!(
        files.len(),
        1,
        "a temporary file was left behind alongside the committed entry: {files:?}"
    );
}
