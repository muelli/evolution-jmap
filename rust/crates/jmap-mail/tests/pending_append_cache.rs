// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The pending-append byte store: the not-yet-uploaded bytes of a message
//! `crate::append` appended while offline.
//!
//! `tests/cache.rs` pins `MessageCache`'s own contract, which this store
//! deliberately does not share: `MessageCache` is best-effort and sweeps an
//! entry nobody opened for a while, which is wrong for a draft that has not
//! reached the server yet. These tests pin the two differences that matter:
//! an entry here is never swept, and a second, independent instance's
//! sweep setting never leaks into this one even when both share an account's
//! cache directory.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use jmap_mail::pending_append_cache::PendingAppendCache;

/// The bytes every test in this file stores.
const SOURCE: &[u8] = b"From: bob@example.com\r\nSubject: Draft\r\n\r\n\
                        A draft written offline, not yet uploaded.\r\n";

/// Distinguishes one test's directory from the next, including two tests
/// running at once, which a Rust test binary does by default.
static DIRECTORIES: AtomicUsize = AtomicUsize::new(0);

/// A directory of its own, removed when the test ends.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jmap-mail-pending-append-{}-{}",
            std::process::id(),
            DIRECTORIES.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("a directory for the store");
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

#[test]
fn a_stored_draft_reads_back_byte_for_byte() {
    let scratch = Scratch::new();
    let store = PendingAppendCache::open(scratch.as_str()).expect("a store in a fresh directory");

    assert!(
        store.store("jmap-local-m1", SOURCE),
        "the draft was not stored"
    );
    assert_eq!(
        store.load("jmap-local-m1").as_deref(),
        Some(SOURCE),
        "the stored draft did not read back as it was written"
    );
}

#[test]
fn a_uid_that_was_never_stored_is_a_miss() {
    let scratch = Scratch::new();
    let store = PendingAppendCache::open(scratch.as_str()).expect("a store in a fresh directory");

    assert!(
        store.load("jmap-local-m1").is_none(),
        "an empty store answered with a draft"
    );
}

#[test]
fn removing_a_draft_makes_it_a_miss_again() {
    let scratch = Scratch::new();
    let store = PendingAppendCache::open(scratch.as_str()).expect("a store in a fresh directory");

    assert!(store.store("jmap-local-m1", SOURCE));
    store.remove("jmap-local-m1");

    assert!(
        store.load("jmap-local-m1").is_none(),
        "a removed draft was still served"
    );
}

/// Removing a key that was never stored is a defined no-op, the same
/// contract `camel_data_cache_remove` itself documents: `crate::append`'s
/// replay removes the entry once the upload succeeds, without first
/// checking that it is still there.
#[test]
fn removing_an_unstored_uid_does_not_panic() {
    let scratch = Scratch::new();
    let store = PendingAppendCache::open(scratch.as_str()).expect("a store in a fresh directory");

    store.remove("jmap-local-never-stored");
}

/// A refiled draft (the user edits and re-saves before reconnecting)
/// replaces the entry rather than appending to it.
#[test]
fn storing_a_uid_twice_replaces_the_entry() {
    let scratch = Scratch::new();
    let store = PendingAppendCache::open(scratch.as_str()).expect("a store in a fresh directory");

    assert!(store.store("jmap-local-m1", b"the first, shorter draft"));
    assert!(store.store("jmap-local-m1", SOURCE));
    assert_eq!(
        store.load("jmap-local-m1").as_deref(),
        Some(SOURCE),
        "the second store did not replace the first"
    );
}

/// RFC 8620 S1.2 limits an id to URL-safe characters, and this store turns a
/// uid into a file name the same way `MessageCache` does. `synthetic_uid`
/// only ever produces such a uid, but the check is this store's own
/// defence, not a trust in the caller.
#[test]
fn a_uid_that_is_a_path_is_not_a_key() {
    let scratch = Scratch::new();
    let store = PendingAppendCache::open(scratch.as_str()).expect("a store in a fresh directory");

    for hostile in ["../escape", "..", ".", "sub/dir", "/absolute", ""] {
        assert!(
            !store.store(hostile, SOURCE),
            "{hostile:?} was accepted as a store key"
        );
        assert!(
            store.load(hostile).is_none(),
            "{hostile:?} was looked up as a store key"
        );
    }

    assert!(
        !scratch
            .path
            .parent()
            .expect("a parent")
            .join("escape")
            .exists(),
        "a key escaped the store directory"
    );
}

/// The account-wide property `MessageCache` also has: two instances over one
/// directory see the same entries, so a folder's own instance and a
/// reconnect-walk instance agree on what is pending.
#[test]
fn two_stores_over_one_directory_share_their_entries() {
    let scratch = Scratch::new();
    let first = PendingAppendCache::open(scratch.as_str()).expect("a store in a fresh directory");
    let second =
        PendingAppendCache::open(scratch.as_str()).expect("a second store over the same one");

    assert!(first.store("jmap-local-m1", SOURCE));
    assert_eq!(
        second.load("jmap-local-m1").as_deref(),
        Some(SOURCE),
        "the second store did not see the first one's entry"
    );
}

/// `MessageCache::open`'s own `an_entry_nobody_has_opened_within_the_bound_
/// is_swept` test proves its default bound sweeps an entry sharing a bucket
/// with a freshly looked-up key. This store disables expiry entirely
/// (`GFALSE`, not a bound), so the same shape of wait and shared-bucket
/// lookup must leave both entries alone: a draft is not an optimisation a
/// sweep may discard, it is the only copy of a message the user wrote.
#[test]
fn an_entry_is_never_swept_even_after_a_wait() {
    let scratch = Scratch::new();
    let writer = PendingAppendCache::open(scratch.as_str()).expect("a store in a fresh directory");

    assert!(writer.store("jmap-local-m1", SOURCE));
    assert!(writer.store("jmap-local-m2", SOURCE));

    std::thread::sleep(Duration::from_millis(1_500));

    let reader = PendingAppendCache::open(scratch.as_str()).expect("a store over the same one");
    assert_eq!(
        reader.load("jmap-local-m2").as_deref(),
        Some(SOURCE),
        "the entry being opened was swept"
    );
    assert_eq!(
        reader.load("jmap-local-m1").as_deref(),
        Some(SOURCE),
        "an entry past a `MessageCache`-style bound was swept, but this store disables expiry"
    );
}

/// A store that cannot be created is not a failure to report to the user
/// here either; `crate::append` falls back to failing the offline append
/// the way it does today, the same "no cache, no crash" contract
/// `MessageCache::open` gives.
#[test]
fn a_directory_that_cannot_be_created_is_not_a_store() {
    let scratch = Scratch::new();
    let blocked = scratch.path.join("in-the-way");
    fs::write(&blocked, b"a file where the store directory would go").expect("a file");

    assert!(
        PendingAppendCache::open(blocked.to_str().expect("a UTF-8 path")).is_none(),
        "a store was opened over a plain file"
    );
}
