// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `PushRefresh::stop` against a real Stalwart EventSource connection that is
//! genuinely blocked mid-read, not a synthetic one.
//!
//! [`jmap_client::eventsource`]'s own module doc explains the mechanism: the
//! background thread that reads a `StateChange` line by line only checks its
//! [`CancelFlag`](jmap_client::transport::CancelFlag) between reads, so a
//! cancellation while it is parked inside a blocking read on an idle stream
//! would never be noticed on its own — `EventSourceSubscription::stop` also
//! shuts the raw socket down, which is what actually interrupts that read.
//! `dropping_the_subscription_does_not_block_on_a_live_connection`
//! (`jmap-client/src/eventsource.rs`) proves the shutdown call works against
//! a hand-rolled local TCP/TLS server built for the test; `push.rs` in this
//! crate proves `PushRefresh::stop` tears down the bookkeeping against
//! `jmap-mockd`, but never while a read was actually parked waiting on
//! something. Neither has ever measured `stop()` against a real server's own
//! socket, where buffering, proxies or TLS termination could behave
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

use std::env;
use std::time::{Duration, Instant};

use jmap_backend_core::push::PushRefresh;
use jmap_client::eventsource::expand_url;
use jmap_client::{Client, Credentials};
use jmap_proto::session::CAPABILITY_MAIL;

/// Mirrors `live_server_push.rs::connect_for_write` exactly.
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

/// No ping requested (`pingSeconds` of `0` below): nothing at all should
/// arrive on this stream for the length of the test, which is the point —
/// the pump thread's background reader has to be genuinely parked inside a
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
    // reach its blocking read on Stalwart's own response, rather than racing
    // `stop` against a connection attempt still in flight.
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
