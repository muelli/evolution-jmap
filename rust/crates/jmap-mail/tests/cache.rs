// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The message cache: a message fetched once, kept on disk.
//!
//! `get_message_sync` downloads a message every time the user clicks its row,
//! and RFC 8621 §4.1 makes an `Email` immutable — so the second download can
//! only ever produce the bytes the first one did. `CamelDataCache` is where
//! Camel's own providers keep those bytes, a file per message under the
//! account's cache directory, and this is the wrapper around it.
//!
//! What the tests here pin is the wrapper's contract rather than the vfunc's
//! (`tests/message.rs` covers that end): bytes read back as they were written,
//! a miss that is a miss rather than a failure, two caches over one directory
//! seeing the same entries — which is what makes a message filed in two
//! mailboxes one file — and a uid that is a *path* not being allowed to become
//! one.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use jmap_mail::cache::MessageCache;

/// The message every test here caches. Long enough that a short read would be
/// visible as one.
const SOURCE: &[u8] = b"From: bob@example.com\r\nSubject: Cached\r\n\r\nThe body, \
                        which is longer than the headers so that a truncated \
                        read is not mistaken for a complete one.\r\n";

/// Tells one test's cache directory from the next, including two running at
/// once — which is what a Rust test binary does by default.
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
        // Best effort: a test that has already failed is not made better by a
        // panic in its teardown.
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn a_stored_message_reads_back_byte_for_byte() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    assert!(cache.store("M1", SOURCE), "the message was not cached");
    assert_eq!(
        cache.load("M1").as_deref(),
        Some(SOURCE),
        "the cached message did not read back as it was written"
    );
}

#[test]
fn a_uid_that_was_never_cached_is_a_miss() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    assert!(
        cache.load("M1").is_none(),
        "an empty cache answered with a message"
    );
}

/// Re-storing is what a refetched message does, and it has to leave the entry
/// readable rather than appended to: `camel_data_cache_add` replaces the file,
/// and a wrapper that opened it for appending would double every message it
/// cached twice.
#[test]
fn storing_a_uid_twice_replaces_the_entry() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    assert!(cache.store("M1", b"the first, longer version of the message"));
    assert!(cache.store("M1", SOURCE));
    assert_eq!(
        cache.load("M1").as_deref(),
        Some(SOURCE),
        "the second store did not replace the first"
    );
}

/// A message with no bytes in it is not a message, and an entry with none is
/// what a process killed between creating one and writing it leaves behind.
/// Refused at both ends, because the alternative is `get_message_sync` serving
/// an empty message out of the cache for as long as the entry lives, in
/// preference to the download that would have replaced it.
#[test]
fn an_entry_with_nothing_in_it_is_not_a_message() {
    let scratch = Scratch::new();
    let cache = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");

    assert!(!cache.store("M1", b""), "an empty message was cached");
    assert!(
        cache.load("M1").is_none(),
        "an empty entry was served as a message"
    );
}

/// The account-wide property. A JMAP email id identifies a message in the
/// account, not in a mailbox, so the same message filed in two mailboxes is one
/// entry — which is what lets every folder of an account hold a cache of its
/// own over one directory without storing the mail twice.
#[test]
fn two_caches_over_one_directory_share_their_entries() {
    let scratch = Scratch::new();
    let first = MessageCache::open(scratch.as_str()).expect("a cache in a fresh directory");
    let second = MessageCache::open(scratch.as_str()).expect("a second cache over the same one");

    assert!(first.store("M1", SOURCE));
    assert_eq!(
        second.load("M1").as_deref(),
        Some(SOURCE),
        "the second cache did not see the first one's entry"
    );
}

/// RFC 8620 §1.2 limits an id to URL-safe characters, and a *file name* is what
/// this cache turns one into. A server that answered `Email/query` with
/// `../../../../etc/cron.d/x` would otherwise be a server that picks the path
/// its mail is written to.
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
            !cache.store(hostile, SOURCE),
            "{hostile:?} was accepted as a cache key"
        );
        assert!(
            cache.load(hostile).is_none(),
            "{hostile:?} was looked up as a cache key"
        );
    }

    // And nothing of the sort reached the filesystem: whatever the cache put
    // under its own directory, it put nothing above it.
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

/// A cache that cannot be created is not a failure to report to the user —
/// mail still opens, it is just fetched every time — so the constructor answers
/// `None` rather than an error, and every caller treats that as "no cache".
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
