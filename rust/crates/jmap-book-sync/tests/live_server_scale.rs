// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `BookSync` against an address book larger than any other test here
//! uses.
//!
//! The other live-server tests in `jmap-book-sync` use one or two contacts.
//! This test asks whether the cold listing time and the incremental
//! `get_changes` cost stay reasonable in a larger book. The scale test of
//! `jmap-mail-sync` answers the same question for mailboxes.
//!
//! This file covers the smallest step: 500 cards and some edits after the
//! import. It also covers bulk deletes from a large book, and a batch where a
//! tenth of the cards carry a ~50 KB PHOTO. One `ContactCard/set` call per
//! card, without batching, hits the `Http.rateLimitAuthenticated` limit of
//! Stalwart (1000 req/60s) before 500 calls. A live run showed a 429 on call
//! #498, 2.2 seconds after the start. Thus [`retrying`] below absorbs a 429
//! with a sleep.
//!
//! Larger steps (1,000 and 5,000 cards) use this same file with a higher
//! `JMAP_SCALE_TEST_CARDS`. `jmap-cal-sync` has the calendar equivalent.
//!
//! ## Running it
//!
//! Use the same environment as `live_server.rs` (see
//! `docs/manual-test-live-server.md`). Set `JMAP_LIVE_SERVER_URL`,
//! `JMAP_LIVE_SERVER_WRITE_USER` and `JMAP_LIVE_SERVER_WRITE_PASSWORD`, then
//! run:
//!
//! ```console
//! $ cargo test -p evolution-jmap-book-sync --test live_server_scale -- --ignored --nocapture
//! ```
//!
//! `JMAP_SCALE_TEST_CARDS` sets the batch size (default 500). The tests leave
//! their cards in the address book for inspection, so use a throwaway
//! write-test account for this test only.
//!
//! A test skips (it does not fail) when `JMAP_LIVE_SERVER_WRITE_USER` is unset.

use std::thread::sleep;
use std::time::{Duration, Instant};

use jmap_book_sync::{BookSync, SyncError};
use jmap_client::Error as ClientError;
use jmap_proto::contacts::AddressBook;
use jmap_proto::session::CAPABILITY_CONTACTS;
use jmap_test_support::{connect_for_write, env_count, unique_suffix};

/// Retries `attempt` on a 429 (`Http.rateLimitAuthenticated`) up to 19 times,
/// with a 5-second sleep before each retry. Thus a scale run does not fail when
/// it is faster than the rate-limit window of the server. The caller gets any
/// other error, or the last 429, unchanged.
fn retrying<T>(mut attempt: impl FnMut() -> Result<T, SyncError>) -> Result<T, SyncError> {
    for remaining in (0..20).rev() {
        match attempt() {
            Err(SyncError::Client(ClientError::Http { status: 429, .. })) if remaining > 0 => {
                sleep(Duration::from_secs(5));
            }
            other => return other,
        }
    }
    unreachable!("the loop above always returns on its last iteration (remaining == 0)")
}

fn batch_size() -> usize {
    env_count("JMAP_SCALE_TEST_CARDS", 500)
}

/// Returns the peak resident set size of this process, read from `/proc`. Linux
/// only, which is the platform of every runner for this test.
fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")
            .and_then(|rest| rest.trim().strip_suffix("kB"))
            .and_then(|value| value.trim().parse().ok())
    })
}

fn vcard(run: u128, label: &str, i: usize) -> String {
    let name = format!("agent-scale-{run}-{label}-{i}");
    format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-not-a-server-id\r\n\
         FN:{name}\r\n\
         N:{name};;;;\r\n\
         END:VCARD\r\n"
    )
}

/// Returns a deterministic ~50 KB PHOTO payload in base64, so repeat runs are
/// byte-identical. `jmap-vcard` puts a PHOTO inline into the JSContact `media`
/// map and never uses `Blob/upload`. Thus this test measures the
/// `ContactCard/set` payload size, not a blob transfer.
fn photo_base64() -> String {
    use base64::Engine;
    let bytes: Vec<u8> = (0..50_000u32).map(|i| (i % 256) as u8).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Same shape as [`vcard`], plus an inline PHOTO in vCard 3.0 form
/// (`jmap-vcard` round-trips only 3.0). `tests/live_server_media.rs` confirms
/// this form against a real server, but with only a few bytes.
fn vcard_with_photo(run: u128, label: &str, i: usize, photo: &str) -> String {
    let name = format!("agent-scale-{run}-{label}-{i}");
    format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-not-a-server-id\r\n\
         FN:{name}\r\n\
         N:{name};;;;\r\n\
         PHOTO;ENCODING=b;TYPE=JPEG:{photo}\r\n\
         END:VCARD\r\n"
    )
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_real_sized_address_book_round_trips_through_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the scale test");
        return;
    };

    let account_id = client
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");
    let n = batch_size();
    let run = unique_suffix();
    // A dedicated book, not the account's default one. The three tests in this
    // file run concurrently under cargo's default test threading; `get_changes`
    // is account-wide before `classify` narrows it to one book, so sharing a
    // book would let one test's cards show up as another test's changes.
    let address_book_id = client
        .address_book_create(
            &account_id,
            &AddressBook::new(format!("agent-scale-main-{run}")),
        )
        .expect("AddressBook/set create failed against the real server")
        .id
        .expect("the server named the new address book");

    let sync = BookSync::new(client, account_id, address_book_id);

    let import_start = Instant::now();
    let mut saved_uids = Vec::with_capacity(n);
    for i in 0..n {
        let saved = retrying(|| sync.save_contact(&vcard(run, "main", i), None))
            .unwrap_or_else(|error| panic!("ContactCard/set create #{i} failed: {error}"));
        saved_uids.push(saved.uid);
    }
    let import_elapsed = import_start.elapsed();
    eprintln!(
        "SCALE: imported {n} cards in {:?} ({:?}/card)",
        import_elapsed,
        import_elapsed / n as u32
    );

    let listing_start = Instant::now();
    let (state_after_import, listed) = sync
        .list_existing()
        .expect("listing the book failed after the bulk import");
    let listing_elapsed = listing_start.elapsed();
    eprintln!(
        "SCALE: cold listing of {} cards took {:?}",
        listed.len(),
        listing_elapsed
    );
    for uid in &saved_uids {
        assert!(
            listed.iter().any(|contact| &contact.uid == uid),
            "every imported card should be in the cold listing"
        );
    }

    let edit_count = 20usize.min(n);
    let to_edit: Vec<String> = saved_uids.iter().take(edit_count).cloned().collect();
    let edit_start = Instant::now();
    for uid in &to_edit {
        let edited = vcard(run, "main", 999_999).replace("agent-scale", "agent-scale-edited");
        retrying(|| sync.save_contact(&edited, Some(uid.as_str())))
            .unwrap_or_else(|error| panic!("ContactCard/set update of {uid} failed: {error}"));
    }
    let edit_elapsed = edit_start.elapsed();
    eprintln!(
        "SCALE: edited {edit_count} of {n} cards in {:?} ({:?}/card)",
        edit_elapsed,
        edit_elapsed / edit_count as u32
    );

    let changes_start = Instant::now();
    let changes = sync
        .get_changes(&state_after_import)
        .expect("get_changes failed after editing a few cards in an already-large book");
    let changes_elapsed = changes_start.elapsed();
    eprintln!(
        "SCALE: get_changes after editing {edit_count} of {n} cards took {:?}, reported {} changed",
        changes_elapsed,
        changes.changed.len()
    );
    for uid in &to_edit {
        assert!(
            changes.changed.iter().any(|contact| &contact.uid == uid),
            "every edited card should be reported changed by get_changes"
        );
    }

    if let Some(kb) = peak_rss_kb() {
        eprintln!("SCALE: peak RSS so far {kb} kB");
    }
}

/// Deletes cards from an address book that is already large; the import and
/// edit test does not do this. Unlike `messages_since` in `jmap-mail-sync`,
/// `BookSync::get_changes` has no `catch_up_limit` or relist. `classify` calls
/// `ContactCard/get` only for the created and updated ids, so a delta of
/// deletes costs only the `/changes` call. This test checks that assumption
/// against a real server, not only against the mock of this crate.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn deleting_from_a_real_sized_address_book_is_reported_correctly() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the scale test");
        return;
    };

    let account_id = client
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");
    let n = batch_size();
    let run = unique_suffix();
    // A dedicated book, not the account's default one; see the same note in
    // `a_real_sized_address_book_round_trips_through_the_real_server`.
    let address_book_id = client
        .address_book_create(
            &account_id,
            &AddressBook::new(format!("agent-scale-del-{run}")),
        )
        .expect("AddressBook/set create failed against the real server")
        .id
        .expect("the server named the new address book");

    let sync = BookSync::new(client, account_id, address_book_id);

    let import_start = Instant::now();
    let mut saved_uids = Vec::with_capacity(n);
    for i in 0..n {
        let saved = retrying(|| sync.save_contact(&vcard(run, "del", i), None))
            .unwrap_or_else(|error| panic!("ContactCard/set create #{i} failed: {error}"));
        saved_uids.push(saved.uid);
    }
    let import_elapsed = import_start.elapsed();
    eprintln!(
        "SCALE: imported {n} cards in {:?} ({:?}/card), ahead of the deletion case",
        import_elapsed,
        import_elapsed / n as u32
    );

    let (state_after_import, _listed) = sync
        .list_existing()
        .expect("listing the book failed after the bulk import");

    // Cap at the batch size, so a small manual run does not delete more cards than
    // exist.
    let delete_count = n.min(300);
    let to_delete: Vec<String> = saved_uids.into_iter().take(delete_count).collect();

    let delete_start = Instant::now();
    for uid in &to_delete {
        retrying(|| sync.remove_contact(uid))
            .unwrap_or_else(|error| panic!("ContactCard/set destroy of {uid} failed: {error}"));
    }
    let delete_elapsed = delete_start.elapsed();
    eprintln!(
        "SCALE: deleted {delete_count} of {n} cards in {:?} ({:?}/card)",
        delete_elapsed,
        delete_elapsed / delete_count as u32
    );

    let changes_start = Instant::now();
    let changes = sync
        .get_changes(&state_after_import)
        .expect("get_changes failed after deleting from an already-large book");
    let changes_elapsed = changes_start.elapsed();
    eprintln!(
        "SCALE: get_changes after deleting {delete_count} from a {n}-card book took {:?}, \
         reported {} removed",
        changes_elapsed,
        changes.removed.len()
    );
    assert!(
        changes.changed.is_empty(),
        "nothing but deletions happened, so changed should be empty, got {} rows",
        changes.changed.len()
    );
    for uid in &to_delete {
        assert!(
            changes.removed.contains(uid),
            "every deleted card should be reported removed by get_changes"
        );
    }

    if let Some(kb) = peak_rss_kb() {
        eprintln!("SCALE: peak RSS so far {kb} kB");
    }
}

/// A tenth of the cards carry a ~50 KB PHOTO, and the rest are plain. Checks
/// that a batch with binary payloads costs more but works, and that every PHOTO
/// round-trips byte-identical.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_real_sized_address_book_with_photos_round_trips_through_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the scale test");
        return;
    };

    let account_id = client
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");
    let n = batch_size();
    let run = unique_suffix();
    let photo = photo_base64();
    // A dedicated book, not the account's default one; see the same note in
    // `a_real_sized_address_book_round_trips_through_the_real_server`.
    let address_book_id = client
        .address_book_create(
            &account_id,
            &AddressBook::new(format!("agent-scale-photo-{run}")),
        )
        .expect("AddressBook/set create failed against the real server")
        .id
        .expect("the server named the new address book");

    let sync = BookSync::new(client, account_id, address_book_id);

    let import_start = Instant::now();
    let mut saved_uids = Vec::with_capacity(n);
    let mut photo_uids = Vec::new();
    for i in 0..n {
        let card = if i % 10 == 0 {
            vcard_with_photo(run, "photo", i, &photo)
        } else {
            vcard(run, "photo", i)
        };
        let saved = retrying(|| sync.save_contact(&card, None))
            .unwrap_or_else(|error| panic!("ContactCard/set create #{i} failed: {error}"));
        if i % 10 == 0 {
            photo_uids.push(saved.uid.clone());
        }
        saved_uids.push(saved.uid);
    }
    let import_elapsed = import_start.elapsed();
    eprintln!(
        "SCALE: imported {n} cards ({} with a ~50 KB PHOTO) in {:?} ({:?}/card)",
        photo_uids.len(),
        import_elapsed,
        import_elapsed / n as u32
    );

    let listing_start = Instant::now();
    let (_state_after_import, listed) = sync
        .list_existing()
        .expect("listing the book failed after the bulk import");
    let listing_elapsed = listing_start.elapsed();
    eprintln!(
        "SCALE: cold listing of {} cards ({} with PHOTO) took {:?}",
        listed.len(),
        photo_uids.len(),
        listing_elapsed
    );
    for uid in &saved_uids {
        assert!(
            listed.iter().any(|contact| &contact.uid == uid),
            "every imported card should be in the cold listing"
        );
    }

    for uid in &photo_uids {
        let contact = listed
            .iter()
            .find(|contact| &contact.uid == uid)
            .unwrap_or_else(|| panic!("photo card {uid} missing from the cold listing"));
        // RFC 2426 §2.6 folds a physical line longer than 75 octets onto
        // continuation lines that start with "\r\n " (`fold_overlong_lines` in
        // `jmap-vcard`). Thus a ~50 KB PHOTO comes back folded with the same
        // octets. Unfold before the comparison.
        let unfolded = contact.vcard.replace("\r\n ", "");
        assert!(
            unfolded.contains(&photo),
            "card {uid}'s PHOTO payload should round-trip byte-identical"
        );
    }

    if let Some(kb) = peak_rss_kb() {
        eprintln!("SCALE: peak RSS so far {kb} kB");
    }
}
