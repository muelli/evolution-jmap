// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `ShareNotification/get` delivery (RFC 9670 §4): Track E Phase C step 2's
//! remaining slice. A `shareWith` grant, widen, narrow, or revoke on
//! `AddressBook/set` or `Mailbox/set` produces a notification the recipient
//! (never the owner) can read back.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use serde_json::json;

#[test]
fn address_book_share_grant_and_revoke_deliver_share_notifications() {
    let bob = Id::new("P-bob");
    let server = MockServer::builder()
        .bearer_token("alice-token")
        .bearer_token_as("bob-token", bob.clone())
        .start();
    let account_id = server.account_id();
    let work = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_address_book("Work", false)
    };

    let alice = Client::connect(server.origin(), Credentials::bearer("alice-token")).unwrap();
    let bob_client = Client::connect(server.origin(), Credentials::bearer("bob-token")).unwrap();

    assert!(
        bob_client
            .share_notifications(&account_id)
            .unwrap()
            .is_empty(),
        "no grant yet, no notification"
    );

    alice
        .address_book_update(
            &account_id,
            &work,
            json!({"shareWith": {bob.as_str(): {"mayRead": true}}}),
        )
        .unwrap();

    let notifications = bob_client.share_notifications(&account_id).unwrap();
    assert_eq!(
        notifications.len(),
        1,
        "the grant produced one notification"
    );
    let granted = &notifications[0];
    assert_eq!(granted.object_type, "AddressBook");
    assert_eq!(granted.object_id, work);
    assert_eq!(granted.object_account_id, account_id);
    assert!(
        granted.old_rights.is_none(),
        "nothing was granted before this"
    );
    assert_eq!(
        granted.new_rights.as_ref().and_then(|v| v.get("mayRead")),
        Some(&json!(true))
    );

    assert!(
        alice.share_notifications(&account_id).unwrap().is_empty(),
        "the owner is never their own recipient"
    );

    alice
        .address_book_update(
            &account_id,
            &work,
            json!({format!("shareWith/{}", bob.as_str()): null}),
        )
        .unwrap();

    let notifications = bob_client.share_notifications(&account_id).unwrap();
    assert_eq!(
        notifications.len(),
        2,
        "the revoke produced a second notification"
    );
    let revoked = &notifications[1];
    assert_eq!(
        revoked.old_rights.as_ref().and_then(|v| v.get("mayRead")),
        Some(&json!(true))
    );
    assert!(
        revoked.new_rights.is_none(),
        "nothing is granted after a revoke"
    );
}

#[test]
fn mailbox_share_grant_delivers_a_share_notification() {
    let bob = Id::new("P-bob");
    let server = MockServer::builder()
        .bearer_token("alice-token")
        .bearer_token_as("bob-token", bob.clone())
        .start();
    let account_id = server.account_id();
    let inbox = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_mailbox("Work", None)
    };

    let alice = Client::connect(server.origin(), Credentials::bearer("alice-token")).unwrap();
    let bob_client = Client::connect(server.origin(), Credentials::bearer("bob-token")).unwrap();

    alice
        .mailbox_update(
            &account_id,
            &inbox,
            json!({"shareWith": {bob.as_str(): {"mayReadItems": true}}}),
        )
        .unwrap();

    let notifications = bob_client.share_notifications(&account_id).unwrap();
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].object_type, "Mailbox");
    assert_eq!(notifications[0].object_id, inbox);
    assert_eq!(
        notifications[0]
            .new_rights
            .as_ref()
            .and_then(|v| v.get("mayReadItems")),
        Some(&json!(true))
    );
}
