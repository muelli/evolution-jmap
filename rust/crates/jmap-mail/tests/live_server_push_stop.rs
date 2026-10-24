// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `PushRefresh::stop` against a real Stalwart EventSource connection that is
//! genuinely blocked mid-read, not a synthetic one.
//!
//! [`jmap_client::eventsource`]'s own module doc explains the mechanism. The
//! background thread that reads a `StateChange` line by line checks its
//! [`CancelFlag`](jmap_client::transport::CancelFlag) only between reads.
//! A cancellation while it is parked inside a blocking read on an idle stream
//! would never be noticed on its own. `EventSourceSubscription::stop` also
//! shuts the raw socket down, which is what actually interrupts that read.
//! `dropping_the_subscription_does_not_block_on_a_live_connection`
//! (`jmap-client/src/eventsource.rs`) proves the shutdown call works against
//! a hand-rolled local TCP/TLS server built for the test. `push.rs` in this
//! crate proves `PushRefresh::stop` tears down the bookkeeping against
//! `jmap-mockd`, but never while a read was actually parked waiting on
//! something. Neither test has measured `stop()` against a real server's own
//! socket, where buffering, proxies, or TLS termination could behave
//! differently than either stand-in. This file does.
//!
//! ## Running it
//!
//! Same environment as the other live-server tests — see
//! `docs/manual-test-live-server.md`:
//!
//! ```console
//! $ cargo test -p jmap-mail --features testing -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

use std::time::{Duration, Instant};

use jmap_backend_core::push::PushRefresh;
use jmap_client::eventsource::expand_url;
use jmap_proto::session::CAPABILITY_MAIL;
use jmap_test_support::connect_for_write;

/// No ping requested (`pingSeconds` of `0` below): nothing at all should
/// arrive on this stream for the length of the test. That is the point.
/// The pump thread's background reader must be genuinely parked inside a
/// blocking read on the real socket, with no event or ping due to wake it,
/// when `stop` is asked to return.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn stop_returns_promptly_while_blocked_reading_a_real_idle_stream() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let url = expand_url(
        client.session().event_source_url.trim(),
        &["Email"],
        false,
        0,
    );
    let headers = client
        .authorization_header()
        .map(|value| vec![("Authorization".to_owned(), value)])
        .unwrap_or_default();

    let mut push = PushRefresh::start(url, headers, account_id, vec!["Email".to_owned()], |_| {});

    // Give the pump thread time to finish the real TCP/TLS handshake and
    // reach its blocking read on Stalwart's own response. This avoids
    // racing `stop` against a connection attempt still in flight.
    std::thread::sleep(Duration::from_millis(500));

    let start = Instant::now();
    push.stop();
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "PushRefresh::stop took {elapsed:?} against a real, idle Stalwart \
         EventSource connection; the socket-shutdown interrupt that the local \
         TCP/TLS test server accepts did not work the same way here"
    );
}
