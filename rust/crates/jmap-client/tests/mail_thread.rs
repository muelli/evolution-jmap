// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `Thread/get` (RFC 8621 §3.1) tests.

use jmap_client::{Client, Credentials};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::role;

#[test]
fn thread_get_returns_the_thread_named_by_the_emails_own_thread_id() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();

    let email_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_email(EmailSeed::new(
            inbox,
            ("Bob", "bob@example.com"),
            "Hello Alice",
            "Hi Alice, how are you?",
            "2026-08-01T10:00:00Z",
        ))
    };

    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let email = client
        .email_get(&account_id, std::slice::from_ref(&email_id), None)
        .unwrap()
        .into_iter()
        .next()
        .expect("the seeded email exists");
    let thread_id = email.thread_id.clone().expect("every Email has a threadId");

    let threads = client.thread_get(&account_id, [thread_id.clone()]).unwrap();

    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].id.as_ref(), Some(&thread_id));
    assert_eq!(threads[0].email_ids, vec![email_id]);
}

#[test]
fn thread_get_of_an_unknown_id_is_silently_absent_like_email_get() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let threads = client
        .thread_get(&account_id, [Id::from("no-such-thread")])
        .unwrap();

    assert!(threads.is_empty());
}
