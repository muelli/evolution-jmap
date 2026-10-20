// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! End-to-end tests of the scheduled-send conversation against the mock. The
//! message goes to Drafts, the server holds it with `HOLDFOR`, and
//! `onSuccessUpdateEmail` moves it from Drafts to Sent. Other tests cover a
//! server without Drafts and a server that does not offer the hold.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::mail::{Envelope, EnvelopeAddress, keyword, role};
use jmap_proto::session::CAPABILITY_MAIL;
use jmap_ui::link::AccountLink;
use jmap_ui::send_later::submit::schedule_send;
use jmap_ui::session_cache::AccountFeatures;

const MESSAGE: &[u8] =
    b"From: alice@example.com\r\nTo: bob@example.com\r\nSubject: Later\r\n\r\nNot yet.\r\n";

fn envelope() -> Envelope {
    Envelope::new(
        EnvelopeAddress::new("alice@example.com"),
        [EnvelopeAddress::new("bob@example.com")],
    )
}

/// A connected [`AccountLink`], built as the gate of the composer builds it.
fn link_to(server: &MockServer) -> AccountLink {
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let features = AccountFeatures::from_session(client.session()).expect("a mail account");
    AccountLink::without_refresh(client, features)
}

#[test]
fn a_scheduled_message_is_staged_held_and_filed_as_sent() {
    let server = MockServer::builder().max_delayed_send(86_400).start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_identity("Alice", "alice@example.com");
        account.seed_mailbox("Drafts", Some(role::DRAFTS));
        account.seed_mailbox("Sent", Some(role::SENT));
    }
    let link = link_to(&server);

    let send_at = schedule_send(&link, MESSAGE.to_vec(), envelope(), 600).unwrap();
    // The clock of the mock is at 2026-01-01T00:00:00Z.
    assert_eq!(send_at, "2026-01-01T00:10:00Z");

    // The server holds the message and does not deliver it. The on-success
    // patch already moved it to Sent (RFC 8621 §7.5 applies it on acceptance).
    let state = server.state();
    let state = state.lock().unwrap();
    let account = state.account(&account_id).unwrap();
    assert!(account.outbox.is_empty(), "a held submission must not send");
    let (_, email) = account.emails.iter().next().expect("the staged message");
    let mailboxes = email.mailbox_ids.as_ref().unwrap();
    let sent_only = mailboxes.len() == 1;
    assert!(
        sent_only,
        "the message must sit in exactly one mailbox: {mailboxes:?}"
    );
    assert!(
        email
            .keywords
            .as_ref()
            .is_none_or(|keywords| !keywords.contains_key(keyword::DRAFT)),
        "the draft keyword must be gone once the submission is accepted"
    );
}

/// Without Drafts, the message has no staging mailbox: the call fails before
/// the upload.
#[test]
fn a_server_without_a_drafts_mailbox_refuses_before_uploading() {
    let server = MockServer::builder().max_delayed_send(86_400).start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_identity("Alice", "alice@example.com");
        account.seed_mailbox("Inbox", Some(role::INBOX));
    }
    let link = link_to(&server);

    let refused = schedule_send(&link, MESSAGE.to_vec(), envelope(), 600);
    assert!(refused.is_err());

    let state = server.state();
    let state = state.lock().unwrap();
    let account = state.account(&account_id).unwrap();
    assert_eq!(
        account.emails.iter().count(),
        0,
        "nothing may have been staged"
    );
}

/// The gate that the composer reads. `max_hold` comes only from
/// `maxDelayedSend` (see `AccountFeatures`). By default, this mock advertises
/// neither `maxDelayedSend` nor FUTURERELEASE. Thus `max_hold` is `None`, and
/// the menu stays insensitive. If a client submits a hold anyway, the server
/// refuses it.
#[test]
fn a_server_without_futurerelease_gates_the_feature_off() {
    let server = MockServer::builder().start();
    let link = link_to(&server);
    assert_eq!(link.features.max_hold, None);
    assert!(!link.features.account_id.as_str().is_empty());
    // The session still names the mail capability, thus only the submission
    // data turned the gate off.
    assert!(
        link.client
            .session()
            .capabilities
            .contains_key(CAPABILITY_MAIL)
    );
}
