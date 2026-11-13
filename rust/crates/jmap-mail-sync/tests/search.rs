// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Server-side search, the call behind a Camel folder's `search_by_expression`.
//!
//! `tests/live_server_search.rs` makes the same checks against a real server.
//! This file makes them against the mock, so that `cargo test` covers them too.

use jmap_client::{Client, Credentials};
use jmap_mail_sync::MailSync;
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::{EmailQueryFilter, role};
use jmap_proto::methods::Filter;

fn seed(server: &MockServer, mailbox: &Id, subject: &str) -> Id {
    let state = server.state();
    let mut state = state.lock().unwrap();
    state
        .account_mut(&server.account_id())
        .unwrap()
        .seed_email(EmailSeed::new(
            mailbox.clone(),
            ("Bob", "bob@example.com"),
            subject,
            "text",
            "2026-01-15T09:00:00Z",
        ))
}

fn subject_is(text: &str) -> Filter<EmailQueryFilter> {
    Filter::condition(EmailQueryFilter::default().subject(text))
}

/// A search finds the matching messages of the one mailbox it names: not
/// the ones that do not match, and not a match filed in another mailbox.
#[test]
fn a_search_finds_the_matches_in_its_own_mailbox_only() {
    let server = MockServer::builder().start();
    let (inbox, archive) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&server.account_id()).unwrap();
        (
            account.seed_mailbox("Inbox", Some(role::INBOX)),
            account.seed_mailbox("Archive", Some(role::ARCHIVE)),
        )
    };
    let wanted = seed(&server, &inbox, "Invoice 42");
    seed(&server, &inbox, "Lunch?");
    let elsewhere = seed(&server, &archive, "Invoice 41");

    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let sync = MailSync::new(client, server.account_id());

    assert_eq!(
        sync.search(&inbox, subject_is("Invoice")).unwrap(),
        vec![wanted]
    );
    assert_eq!(
        sync.search(&archive, subject_is("Invoice")).unwrap(),
        vec![elsewhere]
    );
    assert_eq!(
        sync.search(&inbox, subject_is("Receipt")).unwrap(),
        Vec::<Id>::new()
    );
}

/// ROADMAP item 179(d): `Email/query` is a method call under one
/// `accountId`, so searching a shared mailbox must carry that account, not
/// the sync's own one. `search_as` is the entry point a caller that
/// resolved the owning account (`FolderTree::owning_account`) uses instead
/// of `search`.
#[test]
fn search_as_searches_the_named_account_not_the_syncs_own() {
    let server = MockServer::builder().start();

    let shared = Id::new("agent-shared-owner");
    let (shared_inbox, wanted) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let sharer = state.add_account(shared.clone(), "Bob");
        sharer.is_personal = false;
        let inbox = sharer.seed_mailbox("Inbox", None);
        let wanted = sharer.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Bob", "bob@example.com"),
            "Invoice 42",
            "text",
            "2026-01-15T09:00:00Z",
        ));
        sharer.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Bob", "bob@example.com"),
            "Lunch?",
            "text",
            "2026-01-15T09:00:00Z",
        ));
        (inbox, wanted)
    };

    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let sync = MailSync::new(client, server.account_id());

    assert_eq!(
        sync.search_as(&shared, &shared_inbox, subject_is("Invoice"))
            .expect("the search to reach the named shared account"),
        vec![wanted],
        "the match lives in the shared account, not this sync's own"
    );
}
