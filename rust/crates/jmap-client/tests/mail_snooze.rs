// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Snoozing mail against the mock's Cyrus-shaped extension: the capability
//! gate, the move-and-record round trip, and the two refusals (no extension;
//! snoozed but not in the snoozed mailbox).

use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::mail::{Email, EmailImport, SnoozeDetails, role};
use jmap_proto::methods::{SetRequest, SetResponse};
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_CYRUS_MAIL, CAPABILITY_MAIL};
use jmap_proto::{Id, UtcDate};

/// An inbox with one imported message and a connected client:
/// `(client, inbox id, email id)`.
fn seeded(server: &MockServer) -> (Client, Id, Id) {
    let account_id = server.account_id();
    let inbox = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_mailbox("Inbox", Some(role::INBOX))
    };

    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let message = b"From: alice@example.com\r\nTo: bob@example.com\r\nSubject: Ping\r\n\r\nHi\r\n";
    let upload = client
        .upload_blob(&account_id, "message/rfc822", message.to_vec())
        .unwrap();
    let imported = client
        .email_import(
            &account_id,
            &EmailImport::new(upload.blob_id, inbox.clone()),
        )
        .unwrap();
    let email_id = imported.id.expect("server assigned an email id");

    (client, inbox, email_id)
}

/// The extension is a session-document fact a client can gate on before
/// offering snooze anywhere.
#[test]
fn the_session_names_the_snooze_extension() {
    let with = MockServer::builder().snooze_extension().start();
    let client = Client::connect(with.origin(), Credentials::none()).unwrap();
    let account = client.session().accounts.get(&with.account_id()).unwrap();
    assert!(account.has_capability(CAPABILITY_CYRUS_MAIL));

    let without = MockServer::builder().start();
    let client = Client::connect(without.origin(), Credentials::none()).unwrap();
    let account = client
        .session()
        .accounts
        .get(&without.account_id())
        .unwrap();
    assert!(!account.has_capability(CAPABILITY_CYRUS_MAIL));
}

/// The full round trip: the snoozed-role mailbox is created once and reused,
/// the message moves into it, and the wake time comes back on `Email/get`.
#[test]
fn snoozing_moves_the_message_and_records_the_wake_time() {
    let server = MockServer::builder().snooze_extension().start();
    let account_id = server.account_id();
    let (client, inbox, email_id) = seeded(&server);

    let snoozed_mailbox = client.snoozed_mailbox(&account_id).unwrap();
    let snoozed_id = snoozed_mailbox.id.expect("server assigned a mailbox id");
    assert_eq!(
        client.snoozed_mailbox(&account_id).unwrap().id.as_ref(),
        Some(&snoozed_id),
        "a second ask must reuse the mailbox, not create a sibling"
    );

    let details = SnoozeDetails::new(UtcDate::new("2026-01-02T08:00:00Z"))
        .with_move_to_mailbox_id(inbox.clone());
    client
        .snooze_email(&account_id, &email_id, &snoozed_id, &details)
        .unwrap();

    let fetched = client
        .email_get(&account_id, std::slice::from_ref(&email_id), None)
        .unwrap();
    let email = &fetched[0];
    assert_eq!(
        email.snoozed.as_ref().map(|snoozed| snoozed.until.as_str()),
        Some("2026-01-02T08:00:00Z")
    );
    let mailboxes = email.mailbox_ids.as_ref().unwrap();
    assert_eq!(mailboxes.get(&snoozed_id), Some(&true));
    assert!(!mailboxes.contains_key(&inbox), "snoozing leaves the inbox");
}

/// Without the extension the property does not exist: the same call is
/// refused, which is what gates the UI on servers like Stalwart.
#[test]
fn snoozing_without_the_extension_is_refused() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let (client, _inbox, email_id) = seeded(&server);

    // The role alone is an ordinary folder (RFC 9979 §8.1) — creating it
    // works on any server; only `snoozed` itself is gated.
    let snoozed_mailbox = client.snoozed_mailbox(&account_id).unwrap();
    let snoozed_id = snoozed_mailbox.id.expect("server assigned a mailbox id");

    let details = SnoozeDetails::new(UtcDate::new("2026-01-02T08:00:00Z"));
    match client.snooze_email(&account_id, &email_id, &snoozed_id, &details) {
        Err(Error::Set(set_error)) => assert_eq!(set_error.error_type, "invalidProperties"),
        other => panic!("expected SetError, got {other:?}"),
    }
}

/// Setting `snoozed` without moving the message into the snoozed mailbox is
/// refused: the extension couples the two (raw request, since
/// `snooze_email` always moves).
#[test]
fn a_snooze_outside_the_snoozed_mailbox_is_refused() {
    let server = MockServer::builder().snooze_extension().start();
    let account_id = server.account_id();
    let (client, _inbox, email_id) = seeded(&server);

    let request = SetRequest::<Email>::new(account_id.clone()).update(
        email_id.clone(),
        serde_json::json!({"snoozed": {"until": "2026-01-02T08:00:00Z"}}),
    );
    let arguments = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_CYRUS_MAIL],
            "Email/set",
            &request,
        )
        .unwrap();
    let response: SetResponse<Email> = serde_json::from_value(arguments).unwrap();
    let refusal = response
        .not_updated
        .as_ref()
        .and_then(|map| map.get(&email_id))
        .expect("the update must be refused");
    assert_eq!(refusal.error_type, "invalidProperties");
}
