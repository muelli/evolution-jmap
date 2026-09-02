// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `Thread/changes` (RFC 8620 §5.2, RFC 8621 §3): this mock never merges
//! replies into an existing thread, so a Thread's whole lifecycle mirrors its
//! one Email's — created and destroyed alongside it, never updated.

use std::collections::BTreeSet;

use jmap_client::{Client, Credentials};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::mail::role;

#[test]
fn thread_changes_reports_a_delivered_email_as_created() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();

    let (since, inbox) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        (account.threads.state(), inbox)
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let email_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .deliver_email(EmailSeed::new(
                inbox,
                ("Bob", "bob@example.com"),
                "Hello Alice",
                "Hi Alice, how are you?",
                "2026-08-01T10:00:00Z",
            ))
    };
    let thread_id = client
        .email_get(&account_id, std::slice::from_ref(&email_id), None)
        .unwrap()
        .into_iter()
        .next()
        .expect("the delivered email exists")
        .thread_id
        .expect("every Email has a threadId");

    let changes = client.changes(&account_id, "Thread", &since).unwrap();
    assert_eq!(changes.created, vec![thread_id]);
    assert!(changes.updated.is_empty());
    assert!(changes.destroyed.is_empty());
}

#[test]
fn thread_changes_reports_a_destroyed_emails_thread_as_destroyed() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();

    let (since, email_id, thread_id) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        let email_id = account.seed_email(EmailSeed::new(
            inbox,
            ("Bob", "bob@example.com"),
            "Hello Alice",
            "Hi Alice, how are you?",
            "2026-08-01T10:00:00Z",
        ));
        let thread_id = account
            .emails
            .get(&email_id)
            .unwrap()
            .thread_id
            .clone()
            .unwrap();
        (account.threads.state(), email_id, thread_id)
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let destroyed = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .destroy_email(&email_id)
    };
    assert!(destroyed);

    let changes = client.changes(&account_id, "Thread", &since).unwrap();
    assert!(changes.created.is_empty());
    assert!(changes.updated.is_empty());
    assert_eq!(changes.destroyed, vec![thread_id]);
}

#[test]
fn thread_changes_never_reports_an_update_since_threads_here_never_merge() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();

    let (since, email_id, thread_id) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        let since = account.threads.state();
        let email_id = account.deliver_email(EmailSeed::new(
            inbox,
            ("Bob", "bob@example.com"),
            "Hello Alice",
            "Hi Alice, how are you?",
            "2026-08-01T10:00:00Z",
        ));
        let thread_id = account
            .emails
            .get(&email_id)
            .unwrap()
            .thread_id
            .clone()
            .unwrap();
        (since, email_id, thread_id)
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    client
        .email_update(
            &account_id,
            &email_id,
            serde_json::json!({"keywords/$seen": true}),
        )
        .unwrap();

    let changes = client.all_changes(&account_id, "Thread", &since).unwrap();
    assert_eq!(changes.created, BTreeSet::from([thread_id]));
    assert!(
        changes.updated.is_empty(),
        "an Email keyword edit does not touch its Thread's emailIds"
    );
    assert!(changes.destroyed.is_empty());
}
