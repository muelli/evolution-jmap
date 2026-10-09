// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `BookSync` against an address book of a size nothing else here has ever
//! reached.
//!
//! Every other live-server test in `jmap-book-sync` uses a single contact;
//! item 89 proved `jmap-mail-sync` clean at real-mailbox sizes, and item 94
//! asks the same question of contacts: cold listing time and incremental
//! `get_changes` cost once the book is no longer tiny.
//!
//! This file covers the first, smallest slice of item 94(a): 500 cards (half
//! of the item's first checkpoint) and a handful of edits afterward. One
//! `ContactCard/set` call per card, no batching, hits Stalwart's
//! `Http.rateLimitAuthenticated` (1000 req/60s) well before 500 calls even at
//! the default throwaway plan — confirmed empirically (a 429 on call #498,
//! 2.2 seconds in, far faster than the window the limit is measured over) —
//! so [`retrying`] below absorbs a 429 with a short sleep rather than this
//! file assuming it never happens the way item 89's mail equivalent could.
//! The 5,000-card step, the PHOTO-blob variant, bulk deletes and the real-EDS
//! leg (item 80's live-Stalwart book factory) are still open.
//!
//! ## Running it
//!
//! Same environment as this crate's other live-server test — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up:
//!
//! ```console
//! $ cargo test -p evolution-jmap-book-sync --test live_server_scale -- --ignored --nocapture
//! ```
//!
//! `JMAP_SCALE_TEST_CARDS` overrides the batch size (default 500); the
//! write-test account should be a throwaway created for this alone (see
//! `stw seed` in the manual-test doc) since this leaves that many cards in
//! its address book for inspection rather than cleaning up after itself.
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset, the same tolerance every write-path test in this repository gives
//! an unconfigured environment.

use std::env;
use std::thread::sleep;
use std::time::{Duration, Instant};

use jmap_book_sync::{BookSync, SyncError};
use jmap_client::{Client, Credentials, Error as ClientError};
use jmap_proto::session::CAPABILITY_CONTACTS;

/// Retries `attempt` on a 429 (`Http.rateLimitAuthenticated`) with a short
/// sleep, up to 10 times, so a scale run does not fail just because it moved
/// faster than the server's own rate-limit window. Any other error, or
/// exhausting the retries, is returned as-is for the caller to `unwrap`/panic
/// on, same as before this helper existed.
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

/// A value unique to this process invocation, so a concurrent or prior run's
/// leftover cards can never be mistaken for this run's own.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-cal-sync/tests/live_server.rs::connect_for_write` exactly.
fn connect_for_write() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user, password))
        .expect("could not fetch the session document for the write-test account");
    Some(client)
}

fn batch_size() -> usize {
    env::var("JMAP_SCALE_TEST_CARDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(500)
}

/// The resident set size this process has peaked at, read from `/proc`. Linux
/// only, which every runner this test actually runs on is.
fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")
            .and_then(|rest| rest.trim().strip_suffix("kB"))
            .and_then(|value| value.trim().parse().ok())
    })
}

fn vcard(run: u128, i: usize) -> String {
    let name = format!("agent-scale-{run}-{i}");
    format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-not-a-server-id\r\n\
         FN:{name}\r\n\
         N:{name};;;;\r\n\
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
    let address_book_id = client
        .address_books(&account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the write-test account needs a default address book")
        .id
        .expect("the server named the address book");

    let sync = BookSync::new(client, account_id, address_book_id);
    let n = batch_size();
    let run = unique_suffix();

    let import_start = Instant::now();
    let mut saved_uids = Vec::with_capacity(n);
    for i in 0..n {
        let saved = retrying(|| sync.save_contact(&vcard(run, i), None))
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
        let edited = vcard(run, 999_999).replace("agent-scale", "agent-scale-edited");
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
