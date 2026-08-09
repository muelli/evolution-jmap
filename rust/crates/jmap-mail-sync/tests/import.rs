// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Putting a message the client already holds into a mailbox, against a live
//! mock server: the upload and the `Email/import` behind it, what the imported
//! message is listed as afterwards, and the two ways the server refuses.
//!
//! Everything else this crate writes changes a message the account already has.
//! This is the one call that adds one, and it adds it as *bytes* — so what the
//! tests are mostly about is that those bytes survive the trip: the message a
//! folder lists after an import, and the source it hands back when the message
//! is opened, both have to be the message that went up.

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{Keywords, MailSync, MessageFlags, MessageSummary, SyncError};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::mail::email_import_error;

/// The RFC 5322 bytes of a message Evolution would hand over — CRLF line
/// endings, a header block, a blank line, a body.
const MESSAGE: &[u8] = b"From: Bob <bob@example.com>\r\n\
To: Alice <alice@example.com>\r\n\
Subject: Lunch?\r\n\
Message-ID: <lunch@example.com>\r\n\
Date: Thu, 15 Jan 2026 09:30:00 +0000\r\n\
\r\n\
One o'clock at the usual place.\r\n";

struct Fixture {
    server: MockServer,
    account_id: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        Self { server, account_id }
    }

    fn sync(&self) -> MailSync {
        let client = Client::connect(self.server.origin(), Credentials::none()).unwrap();
        MailSync::new(client, self.account_id.clone())
    }

    /// An empty mailbox to import into.
    fn seed_mailbox(&self, name: &str) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&self.account_id).unwrap();
        account.seed_mailbox(name, None)
    }

    /// What the mailbox holds now, as a folder refresh would find it.
    fn listing(&self, mailbox: &Id) -> Vec<MessageSummary> {
        let (_, messages) = self.sync().messages(mailbox).expect("the mailbox lists");
        messages
    }
}

/// The one row a mailbox holds, or a failure naming how many it holds instead.
fn only(messages: Vec<MessageSummary>) -> MessageSummary {
    assert_eq!(messages.len(), 1, "one imported message");
    messages.into_iter().next().expect("the row just counted")
}

#[test]
fn an_imported_message_is_a_message_of_the_mailbox_it_was_filed_into() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");

    let uid = fixture
        .sync()
        .import_message(&inbox, MESSAGE.to_vec(), &Keywords::default(), None)
        .expect("the message is imported");

    let row = only(fixture.listing(&inbox));
    // The uid the import answered with is the uid the mailbox lists it under:
    // what the caller records for the message has to be what it will next find
    // the message by.
    assert_eq!(row.uid, uid);
    assert_eq!(row.subject.as_deref(), Some("Lunch?"));
    assert_eq!(row.size as usize, MESSAGE.len());
    assert_eq!(
        row.from.first().map(|from| from.email.as_str()),
        Some("bob@example.com")
    );
    assert_eq!(row.message_id.as_deref(), Some("lunch@example.com"));
}

#[test]
fn the_bytes_that_went_up_are_the_bytes_that_come_back_down() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let sync = fixture.sync();

    let uid = sync
        .import_message(&inbox, MESSAGE.to_vec(), &Keywords::default(), None)
        .expect("the message is imported");

    // The whole point of importing rather than composing: a message that was
    // taken apart into properties and written out again would not be these
    // bytes, and a signature over them would no longer verify.
    let source = sync.message_source(&uid).expect("the message is readable");
    assert_eq!(source, MESSAGE);
}

#[test]
fn the_keywords_a_row_carries_go_up_with_the_message() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let flags = MessageFlags {
        seen: true,
        flagged: true,
        ..MessageFlags::default()
    };

    fixture
        .sync()
        .import_message(
            &inbox,
            MESSAGE.to_vec(),
            &Keywords::new(&flags, &["Work".to_owned()]),
            None,
        )
        .expect("the message is imported");

    // Read back the way a folder reads any other row, so this is the whole
    // round trip through the keyword mapping rather than a look at the request.
    let row = only(fixture.listing(&inbox));
    assert_eq!(row.flags, flags);
    assert_eq!(row.tags, vec!["Work".to_owned()]);
}

#[test]
fn a_message_imported_with_no_keywords_arrives_carrying_none() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");

    fixture
        .sync()
        .import_message(&inbox, MESSAGE.to_vec(), &Keywords::default(), None)
        .expect("the message is imported");

    // Unread in particular: an append that quietly marked the message read
    // would hide it from the user in the folder it was appended to.
    let row = only(fixture.listing(&inbox));
    assert_eq!(row.flags, MessageFlags::default());
    assert!(row.tags.is_empty(), "{:?}", row.tags);
}

#[test]
fn the_moment_the_message_was_received_is_the_moment_it_keeps() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    // What Camel hands over: seconds since the epoch, here 2026-01-15T09:30:00Z.
    let received_at = 1_768_469_400;

    fixture
        .sync()
        .import_message(
            &inbox,
            MESSAGE.to_vec(),
            &Keywords::default(),
            Some(received_at),
        )
        .expect("the message is imported");

    // Out through the date written into the request and back in through the one
    // read out of the listing: a message that sorted to the wrong end of the
    // folder would be the visible half of an error in either direction.
    let row = only(fixture.listing(&inbox));
    assert_eq!(row.received_at, Some(received_at));
}

#[test]
fn a_message_imported_without_a_moment_is_dated_by_the_server() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");

    fixture
        .sync()
        .import_message(&inbox, MESSAGE.to_vec(), &Keywords::default(), None)
        .expect("the message is imported");

    // RFC 8621 §4.8 leaves `receivedAt` to the server when it is not given, and
    // a row with no date at all is one Evolution sorts to the epoch — so the
    // absence has to reach the server as an absence rather than as a zero.
    let row = only(fixture.listing(&inbox));
    let received_at = row.received_at.expect("the server dated the message");
    assert!(received_at > 0, "{received_at}");
}

#[test]
fn a_date_no_utc_date_can_name_is_left_to_the_server() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");

    // Camel keeps a date as a signed 64-bit count of seconds and this one names
    // no year a `UTCDate` can spell. It is not a reason to refuse the message:
    // what the user asked for is that the message be appended, and the date is
    // the part of it nothing can honestly carry.
    let uid = fixture
        .sync()
        .import_message(
            &inbox,
            MESSAGE.to_vec(),
            &Keywords::default(),
            Some(i64::MAX),
        )
        .expect("an unwritable date does not stop the import");

    let row = only(fixture.listing(&inbox));
    assert_eq!(row.uid, uid);
    assert!(row.received_at.is_some(), "the server dated the message");
}

#[test]
fn bytes_that_are_not_a_message_are_refused_as_such() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");

    let error = fixture
        .sync()
        .import_message(
            &inbox,
            b"\x00\x01 not a message at all".to_vec(),
            &Keywords::default(),
            None,
        )
        .expect_err("bytes that are not a message are not imported");

    // The server's own refusal, kept whole: `invalidEmail` is a sentence for the
    // user about the message, not a broken account and not a missing folder.
    match error {
        SyncError::Client(jmap_client::Error::Set(set_error)) => {
            assert_eq!(set_error.error_type, email_import_error::INVALID_EMAIL);
        }
        other => panic!("expected the server's refusal, got {other:?}"),
    }
    assert!(fixture.listing(&inbox).is_empty());
}

#[test]
fn a_mailbox_the_account_does_not_have_takes_no_message() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");

    let error = fixture
        .sync()
        .import_message(
            &Id::new("M404"),
            MESSAGE.to_vec(),
            &Keywords::default(),
            None,
        )
        .expect_err("a mailbox that is not there holds no message");

    match error {
        SyncError::Client(jmap_client::Error::Set(_)) => {}
        other => panic!("expected the server's refusal, got {other:?}"),
    }
    // And the message is nowhere else either: a refused import must not leave
    // the account holding a message no mailbox shows.
    assert!(fixture.listing(&inbox).is_empty());
}
