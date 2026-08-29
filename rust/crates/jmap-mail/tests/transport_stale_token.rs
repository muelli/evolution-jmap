// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A pooled transport connection whose bearer token went stale —
//! `docs/ROADMAP.md` item 23, the transport half.
//!
//! `JmapTransport::send_message`'s three network calls (identity lookup,
//! outgoing-mailbox lookup, the send itself) each now run through
//! `retry_once_after`: a 401 asks [`JmapTransport::refresh_credentials`] for a
//! fresh access token and, if one came back, retries once. Mirrors
//! `jmap-mail/tests/stale_token.rs` (the store's own version) exactly, one
//! layer over: a [`JmapTransport::detached`] instance has no `CamelService`,
//! so this proves the same two things that one does — a stale token with
//! nothing to refresh from is reported once, and a connection whose
//! credentials were replaced serves the very next call — not the GObject half
//! of the refresh, which needs a real `CamelService`/`CamelSession` no
//! headless test here can build.

use jmap_client::{Client, Credentials};
use jmap_mail::transport::JmapTransport;
use jmap_mail_sync::MailSync;
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::mail::{Envelope, EnvelopeAddress, role};

const GOOD: &str = "token-the-connection-was-built-with";
const FRESH: &str = "token-a-refresh-would-hand-back";
const SENDER: &str = "alice@example.com";

const MESSAGE: &[u8] = b"From: Alice <alice@example.com>\r\n\
To: Bob <bob@example.com>\r\n\
Subject: Lunch?\r\n\
\r\n\
One o'clock at the usual place.\r\n";

/// A mock with one identity and one Drafts mailbox to stage into — enough for
/// `send_message`'s two lookups to succeed once authenticated.
struct Fixture {
    server: MockServer,
    account_id: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().bearer_token(GOOD).start();
        let account_id = server.account_id();
        {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            account.seed_identity("Alice", SENDER);
            account.seed_mailbox("Drafts", Some(role::DRAFTS));
        }
        Self { server, account_id }
    }

    fn client(&self, token: &str) -> Client {
        Client::connect(self.server.origin(), Credentials::bearer(token)).expect("connected")
    }

    fn detached(&self, token: &str) -> Box<JmapTransport> {
        let transport = JmapTransport::detached();
        transport.install_connection(MailSync::new(self.client(token), self.account_id.clone()));
        transport
    }

    fn envelope(&self) -> Envelope {
        Envelope {
            mail_from: EnvelopeAddress::new(SENDER),
            rcpt_to: vec![EnvelopeAddress::new("bob@example.com")],
        }
    }

    fn send(&self, transport: &JmapTransport) -> Result<(), jmap_mail::connect::StoreError> {
        transport
            .send_message(MESSAGE.to_vec(), self.envelope())
            .map(|_| ())
    }
}

#[test]
fn a_stale_bearer_token_is_reported_once_when_there_is_nothing_to_refresh_from() {
    let fixture = Fixture::start();
    let transport = fixture.detached(GOOD);

    // The connection is good to begin with, so the failure below is the
    // rotation and nothing else.
    assert!(fixture.send(&transport).is_ok(), "a live token was refused");

    // The server now accepts a different token; the pooled connection still
    // carries the old one, which is the hourly bug exactly.
    fixture.server.set_bearer_token(FRESH);
    let before = fixture.server.unauthorized_responses();
    let result = fixture.send(&transport);
    assert!(result.is_err(), "a stale token was accepted");

    // A detached instance has no `CamelService`, so `refresh_credentials`
    // reports "nothing to refresh" and the retry never runs. One refused
    // request, not two.
    assert_eq!(
        fixture.server.unauthorized_responses() - before,
        1,
        "a 401 with no refreshable credentials must not be retried"
    );
}

#[test]
fn fresh_credentials_installed_on_the_live_connection_fix_the_very_next_call() {
    let fixture = Fixture::start();
    let transport = fixture.detached(GOOD);

    fixture.server.set_bearer_token(FRESH);
    assert!(fixture.send(&transport).is_err());

    let installed = transport.inspect_connection(|sync| {
        sync.client().set_credentials(Credentials::bearer(FRESH));
    });
    assert!(installed.is_some(), "there was no connection to refresh");

    assert!(
        fixture.send(&transport).is_ok(),
        "the connection did not pick up the fresh credentials"
    );
}

#[test]
fn a_failure_that_is_not_a_401_is_still_reported_unchanged() {
    let fixture = Fixture::start();
    let transport = fixture.detached(GOOD);
    assert!(transport.drop_connection());

    let result = fixture.send(&transport);
    assert!(matches!(
        result,
        Err(jmap_mail::connect::StoreError::Disconnected)
    ));
}
