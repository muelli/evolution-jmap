// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `ClientBuilder::rebase_urls_to_origin` — real-server readiness for a
//! deployment whose session document names a scheme/host the client cannot
//! route to, even though the session document itself came from a reachable
//! address.
//!
//! This is exactly what a live Stalwart 0.16 test deployment does: its
//! session always advertises `apiUrl`/`downloadUrl`/`uploadUrl`/
//! `eventSourceUrl` as `https://<configured hostname>/…`, unconditionally,
//! regardless of which listener actually answered the connection. No
//! Stalwart setting closes that gap, so a client that blindly
//! trusts the advertised `apiUrl` cannot make a single method call against
//! that deployment even though session discovery itself succeeded.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use serde_json::json;

/// Nothing listens here: loopback, so a connection attempt fails instantly
/// (no DNS lookup, no timeout wait) rather than hanging until the client's
/// connect timeout — port 1 is privileged and unassigned on Linux.
const UNREACHABLE_ORIGIN: &str = "http://127.0.0.1:1";

#[test]
fn without_the_option_method_calls_target_the_advertised_origin_and_fail() {
    let server = MockServer::builder()
        .basic_auth("agent", "sekret")
        .advertise_origin(UNREACHABLE_ORIGIN)
        .start();

    let client = Client::connect(server.origin(), Credentials::basic("agent", "sekret"))
        .expect("session discovery itself reaches the real, reachable origin");

    assert!(
        client.echo(json!({"ok": true})).is_err(),
        "the session's apiUrl points at an unreachable origin, so the call \
         made without rebasing should fail rather than silently succeed \
         against a different address"
    );
}

#[test]
fn rebase_urls_to_origin_reaches_the_server_through_the_origin_actually_connected_to() {
    let server = MockServer::builder()
        .basic_auth("agent", "sekret")
        .advertise_origin(UNREACHABLE_ORIGIN)
        .start();

    let client = Client::builder()
        .rebase_urls_to_origin(true)
        .connect(server.origin(), Credentials::basic("agent", "sekret"))
        .expect("connect through the real origin, session names a different one");

    let sent = json!({"night-shift": "rebase apiUrl to the reachable origin"});
    assert_eq!(
        client.echo(sent.clone()).unwrap(),
        sent,
        "Core/echo should round-trip through the origin the client actually \
         connected to, not the unreachable one the session document names"
    );
}
