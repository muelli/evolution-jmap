// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `SearchSnippet/get` (RFC 8621 §5.1) tests.

use jmap_client::{Client, Credentials};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::{EmailQueryFilter, role};
use jmap_proto::methods::Filter;

fn seed_one(server: &MockServer, subject: &str, body: &str) -> (Id, Id) {
    let account_id = server.account_id();
    let state = server.state();
    let mut state = state.lock().unwrap();
    let account = state.account_mut(&account_id).unwrap();
    let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
    let email_id = account.seed_email(EmailSeed::new(
        inbox,
        ("Bob", "bob@example.com"),
        subject,
        body,
        "2026-08-01T10:00:00Z",
    ));
    (account_id, email_id)
}

#[test]
fn search_snippet_marks_the_matching_subject_and_body_text() {
    let server = MockServer::builder().start();
    let (account_id, email_id) = seed_one(&server, "Roadmap Discussion", "See the plan attached.");
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let filter = Filter::or([
        Filter::condition(EmailQueryFilter::default().subject("Roadmap")),
        Filter::condition(EmailQueryFilter::default().body("plan")),
    ]);
    let snippets = client
        .search_snippet_get(&account_id, [email_id.clone()], Some(filter))
        .unwrap();

    assert_eq!(snippets.len(), 1);
    assert_eq!(snippets[0].email_id, email_id);
    assert_eq!(
        snippets[0].subject.as_deref(),
        Some("<mark>Roadmap</mark> Discussion")
    );
    assert_eq!(
        snippets[0].preview.as_deref(),
        Some("See the <mark>plan</mark> attached.")
    );
}

#[test]
fn search_snippet_leaves_preview_null_when_only_the_subject_matches() {
    let server = MockServer::builder().start();
    let (account_id, email_id) = seed_one(&server, "Roadmap Discussion", "See the plan attached.");
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let filter = EmailQueryFilter::default().subject("Roadmap");
    let snippets = client
        .search_snippet_get(&account_id, [email_id], Some(filter))
        .unwrap();

    assert_eq!(snippets.len(), 1);
    assert!(snippets[0].subject.is_some());
    assert_eq!(snippets[0].preview, None);
}

#[test]
fn search_snippet_text_leaf_matches_both_subject_and_body() {
    let server = MockServer::builder().start();
    let (account_id, email_id) = seed_one(&server, "Roadmap Discussion", "the plan is on track");
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let filter = EmailQueryFilter::default().text("plan");
    let snippets = client
        .search_snippet_get(&account_id, [email_id], Some(filter))
        .unwrap();

    assert_eq!(snippets.len(), 1);
    // "plan" is in the body only; the subject has no occurrence of it.
    assert_eq!(snippets[0].subject, None);
    assert_eq!(
        snippets[0].preview.as_deref(),
        Some("the <mark>plan</mark> is on track")
    );
}

#[test]
fn search_snippet_with_no_filter_highlights_nothing() {
    let server = MockServer::builder().start();
    let (account_id, email_id) = seed_one(&server, "Roadmap Discussion", "See the plan attached.");
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let snippets = client
        .search_snippet_get(&account_id, [email_id], None::<EmailQueryFilter>)
        .unwrap();

    assert_eq!(snippets.len(), 1);
    assert_eq!(snippets[0].subject, None);
    assert_eq!(snippets[0].preview, None);
}

#[test]
fn search_snippet_escapes_html_in_the_matched_text() {
    let server = MockServer::builder().start();
    let (account_id, email_id) = seed_one(&server, "Q&A <followup>", "see body");
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let filter = EmailQueryFilter::default().subject("Q&A");
    let snippets = client
        .search_snippet_get(&account_id, [email_id], Some(filter))
        .unwrap();

    assert_eq!(
        snippets[0].subject.as_deref(),
        Some("<mark>Q&amp;A</mark> &lt;followup&gt;")
    );
}

#[test]
fn search_snippet_of_an_unknown_id_is_silently_absent() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let filter = EmailQueryFilter::default().subject("anything");
    let snippets = client
        .search_snippet_get(&account_id, [Id::from("no-such-email")], Some(filter))
        .unwrap();

    assert!(snippets.is_empty());
}
