// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A pooled mail connection whose bearer token went stale —
//! `docs/ROADMAP.md` item 23.
//!
//! Every store operation now runs through `retry_once_after`: on a 401 it
//! asks [`JmapStore::refresh_credentials`] for a fresh access token and, if
//! one came back, retries once instead of handing the caller a `GError` that
//! puts a consent window in front of the operator every hour.
//!
//! The refresh itself needs a real `CamelService` registered on a real
//! `CamelSession` — evolution-source-registry's `EMailSession`, which neither
//! this VM nor CI has — so `refresh_credentials`'s own GObject half is not
//! exercised here, only what a `JmapStore::detached()` instance *can* prove:
//! that a stale token is reported once (there is nothing to refresh from with
//! no `CamelService`), and that a connection whose credentials were replaced
//! — which is all a successful refresh does — serves the very next call.
//!
//! The server genuinely rotates the token it accepts
//! (`MockServer::set_bearer_token`); the client is not sabotaged into sending
//! something the server never took.

use jmap_client::{Client, Credentials};
use jmap_mail::store::JmapStore;
use jmap_mail_sync::MailSync;
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::mail::role;

const GOOD: &str = "token-the-connection-was-built-with";
const FRESH: &str = "token-a-refresh-would-hand-back";

/// A mock that accepts exactly one bearer token at a time, with one mailbox.
struct Fixture {
    server: MockServer,
    account_id: Id,
    inbox: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().bearer_token(GOOD).start();
        let account_id = server.account_id();
        let inbox = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            state
                .account_mut(&account_id)
                .unwrap()
                .seed_mailbox("Inbox", Some(role::INBOX))
        };
        Self {
            server,
            account_id,
            inbox,
        }
    }

    fn client(&self, token: &str) -> Client {
        Client::connect(self.server.origin(), Credentials::bearer(token)).expect("connected")
    }

    fn detached(&self, token: &str) -> Box<JmapStore> {
        let store = JmapStore::detached();
        store.store_connection(MailSync::new(self.client(token), self.account_id.clone()));
        store
    }
}

#[test]
fn a_stale_bearer_token_is_reported_once_when_there_is_nothing_to_refresh_from() {
    let fixture = Fixture::start();
    let store = fixture.detached(GOOD);

    // The connection is good to begin with, so the failure below is the
    // rotation and nothing else.
    assert!(
        store.messages(&fixture.inbox).is_ok(),
        "a live token was refused"
    );

    // The server now accepts a different token; the pooled connection still
    // carries the old one, which is the hourly bug exactly.
    fixture.server.set_bearer_token(FRESH);
    let before = fixture.server.unauthorized_responses();
    let result = store.messages(&fixture.inbox);
    assert!(result.is_err(), "a stale token was accepted");

    // A detached instance has no `CamelService`, so `refresh_credentials`
    // reports "nothing to refresh" and the retry never runs — the same path a
    // Basic-password account takes, where a re-fetch would only reproduce the
    // wrong secret. One refused request, not two.
    assert_eq!(
        fixture.server.unauthorized_responses() - before,
        1,
        "a 401 with no refreshable credentials must not be retried"
    );
}

#[test]
fn fresh_credentials_installed_on_the_live_connection_fix_the_very_next_call() {
    // What a successful refresh does, minus the `CamelService` this test
    // cannot have: it replaces the credentials *on* the pooled connection
    // rather than the connection itself, which is what lets the retry hold
    // its read guard across both attempts. This pins that a call made
    // afterwards sees the new token.
    let fixture = Fixture::start();
    let store = fixture.detached(GOOD);

    fixture.server.set_bearer_token(FRESH);
    assert!(store.messages(&fixture.inbox).is_err());

    let installed = store.inspect_connection(|sync| {
        sync.client().set_credentials(Credentials::bearer(FRESH));
    });
    assert!(installed.is_some(), "there was no connection to refresh");

    assert!(
        store.messages(&fixture.inbox).is_ok(),
        "the connection did not pick up the fresh credentials"
    );
}

#[test]
fn a_failure_that_is_not_a_401_is_still_reported_unchanged() {
    // A disconnected store must still be reported as disconnected: nothing
    // about the retry may turn an unrelated failure into a credentials one.
    let fixture = Fixture::start();
    let store = fixture.detached(GOOD);
    assert!(store.drop_connection());

    let result = store.messages(&fixture.inbox);
    assert!(matches!(
        result,
        Err(jmap_mail::connect::StoreError::Disconnected)
    ));
}
