// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Session discovery through a same-host redirect, as a real server that
//! serves `/.well-known/jmap` via a `307` to a separate path does (Stalwart
//! among them — see `docs/NIGHT-LOG.md`, "session-discovery redirect strips
//! auth"). The first live test against such a server failed with "no
//! primary account": `ureq`'s default redirect policy drops the
//! `Authorization` header even on a same-host hop, so the redirect target
//! was fetched anonymously and answered with an empty account list.
//!
//! Deliberately exercises the real `UreqTransport` (the crate's default),
//! not a fake in-memory one — the bug is in how that transport follows a
//! redirect, which only a real HTTP round trip through `ureq` reproduces.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::session::CAPABILITY_MAIL;

#[test]
fn primary_account_resolves_through_a_same_host_session_redirect() {
    let server = MockServer::builder()
        .basic_auth("agent", "sekret")
        .session_via_redirect()
        .start();

    let client = Client::connect(server.origin(), Credentials::basic("agent", "sekret"))
        .expect("authenticated session discovery should survive the redirect hop");

    assert!(
        client.session().primary_account(CAPABILITY_MAIL).is_some(),
        "the authenticated session should resolve a primary account, not the \
         anonymous one the redirect target answers with when auth is lost"
    );
}
