// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync` against a mailbox of a size nothing else here has ever reached.
//!
//! Every other live-server test in this crate uses a handful of messages; a
//! real account has tens of thousands, and the failure modes differ:
//! `Email/query` position paging, `maxObjectsInGet`/`maxCallsInRequest`
//! chunking, the time a cold listing takes, and whether `messages_since`'s
//! incremental path stays cheap once a mailbox is no longer small. This is
//! the first batch, scoped to a few hundred messages; later batches can
//! raise the size once this one has a clean baseline.
//!
//! ## Running it
//!
//! Same environment as this crate's other live-server tests — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_scale -- --ignored --nocapture
//! ```
//!
//! `JMAP_SCALE_TEST_MESSAGES` overrides the batch size (default 500); the
//! write-test account should be a throwaway created for this alone (see
//! `stw seed` in the manual-test doc) since this leaves that many messages in
//! its Inbox for inspection rather than cleaning up after itself — deleting
//! the whole throwaway account afterwards is cheaper than destroying each
//! message one at a time over JMAP.
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset, the same tolerance every write-path test in this repository gives
//! an unconfigured environment.

use std::env;
use std::time::Instant;

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{Keywords, MailSync, MessageUpdate};
use jmap_proto::session::CAPABILITY_MAIL;

/// A value unique to this process invocation, so a concurrent or prior run's
/// leftover messages can never be mistaken for this run's own.
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
    env::var("JMAP_SCALE_TEST_MESSAGES")
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

fn message(subject: &str) -> Vec<u8> {
    format!(
        "From: agent-scale@example.invalid\r\n\
         To: agent-scale@example.invalid\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         One message of a real-sized batch.\r\n"
    )
    .into_bytes()
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn importing_a_real_sized_batch_round_trips_through_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the scale test");
        return;
    };

    let core = client.session().core_capability();
    eprintln!(
        "SCALE: server limits maxObjectsInGet={:?} maxCallsInRequest={:?} maxObjectsInSet={:?}",
        core.as_ref().map(|core| core.max_objects_in_get),
        core.as_ref().map(|core| core.max_calls_in_request),
        core.as_ref().map(|core| core.max_objects_in_set),
    );
    assert!(
        core.is_some(),
        "the real server should advertise the core capability"
    );

    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let inbox_id = client
        .mailbox_get(&account_id)
        .unwrap()
        .list
        .into_iter()
        .find(|mailbox| mailbox.role.as_deref() == Some(jmap_proto::mail::role::INBOX))
        .expect("the write-test account needs an Inbox")
        .id
        .expect("the server named the Inbox");

    let sync = MailSync::new(client, account_id);
    let n = batch_size();
    let run = unique_suffix();

    let import_start = Instant::now();
    let mut imported = Vec::with_capacity(n);
    for i in 0..n {
        let subject = format!("agent-scale-{run}-{i}");
        let uid = sync
            .import_message(&inbox_id, message(&subject), &Keywords::default(), None)
            .unwrap_or_else(|error| {
                panic!("Email/import #{i} failed against the real server: {error}")
            });
        imported.push(uid);
    }
    let import_elapsed = import_start.elapsed();
    eprintln!(
        "SCALE: imported {n} messages in {:?} ({:?}/message)",
        import_elapsed,
        import_elapsed / n as u32
    );

    let listing_start = Instant::now();
    let (state_after_import, listed) = sync
        .messages(&inbox_id)
        .expect("listing the Inbox failed after the bulk import");
    let listing_elapsed = listing_start.elapsed();
    eprintln!(
        "SCALE: cold listing of {} messages took {:?}",
        listed.len(),
        listing_elapsed
    );
    for uid in &imported {
        assert!(
            listed.iter().any(|summary| &summary.uid == uid),
            "every imported message should be in the cold listing"
        );
    }

    let extra = 50usize;
    let mut extra_imported = Vec::with_capacity(extra);
    for i in 0..extra {
        let subject = format!("agent-scale-{run}-extra-{i}");
        let uid = sync
            .import_message(&inbox_id, message(&subject), &Keywords::default(), None)
            .unwrap_or_else(|error| {
                panic!("Email/import of extra message #{i} failed against the real server: {error}")
            });
        extra_imported.push(uid);
    }

    let since_start = Instant::now();
    let update = sync
        .messages_since(&inbox_id, &state_after_import, listed.len())
        .expect("messages_since failed after adding to an already-large mailbox");
    let since_elapsed = since_start.elapsed();

    let present = match update {
        MessageUpdate::Changed { present, .. } => present,
        other => panic!("expected a Changed update after adding {extra} messages, got {other:?}"),
    };
    eprintln!(
        "SCALE: messages_since after adding {extra} to a {n}-message mailbox took {:?}, reported {} present",
        since_elapsed,
        present.len()
    );
    for uid in &extra_imported {
        assert!(
            present.iter().any(|summary| &summary.uid == uid),
            "every newly added message should be reported present by messages_since"
        );
    }

    if let Some(kb) = peak_rss_kb() {
        eprintln!("SCALE: peak RSS so far {kb} kB");
    }
}
