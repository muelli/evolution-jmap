// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A submission that needs a mid-send credential refresh must not restage
//! the message it already imported.
//!
//! `JmapTransport::send_message` retries staging (`Email/import`, via
//! `MailSync::stage_outgoing_message`) and submitting (`EmailSubmission/set`,
//! via `MailSync::submit_staged_message`) as two separate `retry_once_after`
//! attempts, not one composite attempt: staging already commits a real,
//! visible draft on success, and retrying it again just because the
//! *submission* that followed needed a fresh token would leave an orphaned
//! duplicate draft behind once the retried submission succeeds.
//!
//! This proves that shape directly against `MailSync` and the very
//! `retry_once_after` `jmap-mail`'s transport calls, rather than through
//! `JmapTransport` itself: a `JmapTransport::detached` instance has no
//! `CamelService`, so `refresh_credentials` always reports "nothing to
//! refresh" and its own internal retry never fires. See
//! `transport_stale_token.rs`'s doc comment for the same limitation.

use jmap_backend_core::retry::retry_once_after;
use jmap_client::{Client, Credentials};
use jmap_mail_sync::{MailSync, Outgoing, SyncError};
use jmap_mock::MockServer;
use jmap_proto::mail::{Envelope, EnvelopeAddress, role};

const GOOD: &str = "token-the-connection-was-built-with";
const FRESH: &str = "token-a-refresh-would-hand-back";

const MESSAGE: &[u8] = b"From: Alice <alice@example.com>\r\n\
To: Bob <bob@example.com>\r\n\
Subject: Lunch?\r\n\
\r\n\
One o'clock at the usual place.\r\n";

#[test]
fn a_submission_that_needs_a_credential_refresh_does_not_restage_the_message() {
    let server = MockServer::builder().bearer_token(GOOD).start();
    let account_id = server.account_id();
    let (drafts, sent, identity) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        (
            account.seed_mailbox("Drafts", Some(role::DRAFTS)),
            account.seed_mailbox("Sent", Some(role::SENT)),
            account.seed_identity("Alice", "alice@example.com"),
        )
    };

    let client = Client::connect(server.origin(), Credentials::bearer(GOOD)).expect("connected");
    let sync = MailSync::new(client, account_id);
    let outgoing = Outgoing {
        source: MESSAGE.to_vec(),
        identity,
        envelope: Some(Envelope {
            mail_from: EnvelopeAddress::new("alice@example.com"),
            rcpt_to: vec![EnvelopeAddress::new("bob@example.com")],
        }),
        staging: drafts.clone(),
        destination: Some(sent.clone()),
    };

    // Exactly `jmap-mail`'s transport: staging and submitting are retried as
    // two separate attempts, not one.
    let uid = retry_once_after(
        || sync.stage_outgoing_message(&outgoing),
        SyncError::is_unauthorized,
        || unreachable!("staging a fresh message must not need a refresh here"),
    )
    .expect("the message stages");

    // The token goes stale in the window between staging and submitting:
    // the window the old, composite-retrying code got wrong.
    server.expire_bearer_token_after(0);
    let result = retry_once_after(
        || sync.submit_staged_message(&uid, &outgoing),
        SyncError::is_unauthorized,
        || {
            server.set_bearer_token(FRESH);
            sync.client().set_credentials(Credentials::bearer(FRESH));
            true
        },
    );
    assert!(result.is_ok(), "the retried submission did not succeed");

    assert_eq!(
        server
            .method_calls()
            .iter()
            .filter(|call| call.as_str() == "Email/import")
            .count(),
        1,
        "the message was imported more than once"
    );
    let (_, sent_messages) = sync.messages(&sent).expect("the sent mailbox lists");
    assert_eq!(sent_messages.len(), 1, "the message did not end up in Sent");
    let (_, draft_messages) = sync.messages(&drafts).expect("the drafts mailbox lists");
    assert!(
        draft_messages.is_empty(),
        "an orphaned draft was left behind"
    );
}
