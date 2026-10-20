// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the layout from the session document of a running mock server.
//!
//! The unit tests in `src/layout.rs` use hand-written session documents.

use jmap_client::{Client, Credentials};
use jmap_collection_sync::CollectionLayout;
use jmap_mock::MockServer;
use jmap_proto::session::{CAPABILITY_CONTACTS, CAPABILITY_SUBMISSION};

fn layout_of(server: &MockServer) -> CollectionLayout {
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    CollectionLayout::from_session(client.session())
}

#[test]
fn the_mock_account_offers_mail_contacts_and_calendars() {
    let server = MockServer::builder().start();
    let layout = layout_of(&server);

    let mail = layout.mail.as_ref().expect("the mock offers mail");
    assert_eq!(mail.account.id, server.account_id());
    assert!(
        mail.can_send,
        "the mock offers submission in the same account"
    );
    assert!(!mail.account.read_only);
    assert_eq!(
        layout.contacts.as_ref().unwrap().id,
        server.account_id(),
        "one account serves all three"
    );
    assert_eq!(layout.calendars.as_ref().unwrap().id, server.account_id());
    assert!(!layout.is_empty());
}

#[test]
fn a_server_without_submission_is_a_mail_account_that_cannot_send() {
    let server = MockServer::builder()
        .without_capability(CAPABILITY_SUBMISSION)
        .start();
    let layout = layout_of(&server);

    let mail = layout.mail.expect("mail survives losing submission");
    assert_eq!(mail.account.id, server.account_id());
    assert!(!mail.can_send);
}

#[test]
fn a_server_without_contacts_warrants_no_address_book() {
    let server = MockServer::builder()
        .without_capability(CAPABILITY_CONTACTS)
        .start();
    let layout = layout_of(&server);

    assert_eq!(layout.contacts, None);
    assert!(layout.mail.is_some(), "the other services are untouched");
    assert!(layout.calendars.is_some());
}
