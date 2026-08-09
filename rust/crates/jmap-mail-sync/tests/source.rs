// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The bytes behind one summary row, against a live mock server: what
//! `Email/get` has to be asked a second time for, what the download answers
//! with, and the three ways the question can have no answer.

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{MailSync, SyncError};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;

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

    /// One message in one mailbox, and its id.
    fn seed_message(&self, subject: &str, body: &str) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&self.account_id).unwrap();
        let mailbox = account.seed_mailbox("Inbox", Some("inbox"));
        account.seed_email(EmailSeed::new(
            mailbox,
            ("Bob", "bob@example.com"),
            subject,
            body,
            "2026-01-15T09:30:00Z",
        ))
    }

    /// Reach into the account and change the message the way a server would
    /// have answered differently.
    fn with_email(&self, uid: &Id, edit: impl FnOnce(&mut jmap_proto::mail::Email)) {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&self.account_id).unwrap();
        edit(account.emails.get_mut(uid).unwrap());
    }
}

#[test]
fn the_source_of_a_message_is_the_rfc_5322_bytes_the_server_holds() {
    let fixture = Fixture::start();
    let uid = fixture.seed_message("Lunch?", "One o'clock at the usual place.");

    let source = fixture.sync().message_source(&uid).unwrap();
    let source = String::from_utf8(source).expect("the mock serves a text message");

    // The headers a message list never shows and a reader does: the source is
    // the message, not a rendering of the summary row.
    assert!(source.contains("Subject: Lunch?"), "{source}");
    assert!(source.contains("From: Bob <bob@example.com>"), "{source}");
    assert!(
        source.contains("One o'clock at the usual place."),
        "{source}"
    );
    // A header block, an empty line, a body — the shape a MIME parser needs.
    let (headers, body) = source.split_once("\r\n\r\n").expect("a header/body split");
    assert!(!headers.contains("One o'clock"), "{source}");
    assert_eq!(body, "One o'clock at the usual place.\r\n");
}

#[test]
fn a_message_the_server_no_longer_has_is_reported_as_gone() {
    let fixture = Fixture::start();
    fixture.seed_message("Lunch?", "body");

    // An id of the right shape that the account never held: what a uid left in
    // a folder summary becomes after another client deleted the message.
    let error = fixture
        .sync()
        .message_source(&Id::new("E404"))
        .expect_err("a message that is not there has no source");

    match error {
        SyncError::NoSuchMessage(uid) => assert_eq!(uid.as_str(), "E404"),
        other => panic!("expected the message to be reported as gone, got {other:?}"),
    }
}

#[test]
fn a_message_the_server_names_no_blob_for_cannot_be_read() {
    let fixture = Fixture::start();
    let uid = fixture.seed_message("Lunch?", "body");
    // RFC 8621 §4.1 makes `blobId` a server-set property of every Email, so a
    // server that answers without one has broken the protocol rather than
    // deleted anything — and the message stays unreadable however often we ask.
    fixture.with_email(&uid, |email| email.blob_id = None);

    let error = fixture
        .sync()
        .message_source(&uid)
        .expect_err("a message with no blob has no source");

    match error {
        SyncError::Client(jmap_client::Error::Protocol(message)) => {
            assert!(message.contains("blobId"), "{message}");
        }
        other => panic!("expected a protocol failure, got {other:?}"),
    }
}

#[test]
fn a_blob_the_server_will_not_serve_is_the_downloads_own_failure() {
    let fixture = Fixture::start();
    let uid = fixture.seed_message("Lunch?", "body");
    // The message is still listed and still names a blob; the blob is gone.
    // Distinct from both cases above: the row is fine and retrying is not
    // hopeless, so this must arrive as the transport failure it is.
    fixture.with_email(&uid, |email| email.blob_id = Some(Id::new("B404")));

    let error = fixture
        .sync()
        .message_source(&uid)
        .expect_err("a blob the server will not serve has no bytes");

    match error {
        SyncError::NoSuchMessage(_) => panic!("a missing blob is not a missing message"),
        SyncError::NoSuchFolder(_) => panic!("a missing blob is not a missing folder"),
        SyncError::NoIdentity(_) => panic!("a missing blob has nothing to do with sending"),
        SyncError::Client(_) => {}
    }
}

#[test]
fn the_source_is_asked_for_by_the_blob_the_server_names_now() {
    let fixture = Fixture::start();
    let uid = fixture.seed_message("Lunch?", "body");

    // A second message's blob, put on the first: nothing in a folder summary
    // records a blob id — Camel has no field for one — so the fetch must read
    // it back off the server rather than remember it, and this is what tells
    // the two apart.
    let other = fixture.seed_message("Dinner?", "elsewhere entirely");
    let blob = {
        let state = fixture.server.state();
        let state = state.lock().unwrap();
        let account = state.account(&fixture.account_id).unwrap();
        account.emails.get(&other).unwrap().blob_id.clone().unwrap()
    };
    fixture.with_email(&uid, |email| email.blob_id = Some(blob));

    let source = String::from_utf8(fixture.sync().message_source(&uid).unwrap()).unwrap();
    assert!(source.contains("Subject: Dinner?"), "{source}");
}
