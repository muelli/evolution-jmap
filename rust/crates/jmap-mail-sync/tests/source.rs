// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The bytes of one summary row, against a mock server: the second `Email/get`
//! request, the download result, and the three failure cases.

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

    /// Seed one message in one mailbox, and return its id.
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

    /// Change the message in the store, to make the server answer differently.
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

    // The source is the message with all headers, not a copy of the summary
    // row.
    assert!(source.contains("Subject: Lunch?"), "{source}");
    assert!(source.contains("From: Bob <bob@example.com>"), "{source}");
    assert!(
        source.contains("One o'clock at the usual place."),
        "{source}"
    );
    // A header block, an empty line, and a body, as a MIME parser needs.
    let (headers, body) = source.split_once("\r\n\r\n").expect("a header/body split");
    assert!(!headers.contains("One o'clock"), "{source}");
    assert_eq!(body, "One o'clock at the usual place.\r\n");
}

#[test]
fn a_message_the_server_no_longer_has_is_reported_as_gone() {
    let fixture = Fixture::start();
    fixture.seed_message("Lunch?", "body");

    // A valid id that the account does not have, as after a different client
    // deletes the message of a summary uid.
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
    // RFC 8621 §4.1 makes `blobId` a server-set property of each Email. Without
    // it, the server breaks the protocol, and a retry cannot read the message.
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
    // The message is still listed with a blob id, but the blob is gone. This is
    // different from the two cases above: the row is correct and a retry can
    // work, so the result must be a transport failure.
    fixture.with_email(&uid, |email| email.blob_id = Some(Id::new("B404")));

    let error = fixture
        .sync()
        .message_source(&uid)
        .expect_err("a blob the server will not serve has no bytes");

    match error {
        SyncError::NoSuchMessage(_) => panic!("a missing blob is not a missing message"),
        SyncError::NoSuchFolder(_) => panic!("a missing blob is not a missing folder"),
        SyncError::NoIdentity(_) | SyncError::NoOutgoingFolder => {
            panic!("a missing blob has nothing to do with sending")
        }
        SyncError::Client(_) => {}
    }
}

#[test]
fn the_source_is_asked_for_by_the_blob_the_server_names_now() {
    let fixture = Fixture::start();
    let uid = fixture.seed_message("Lunch?", "body");

    // Put the blob of a second message on the first. A folder summary has no
    // blob id field, so the fetch must read the blob id from the server. This
    // test proves that it does.
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

/// A regression: a body larger than the old 10 MiB default limit of `ureq`
/// arrives complete. One photo attachment can be this large.
#[test]
fn a_message_larger_than_ten_mebibytes_is_readable() {
    let fixture = Fixture::start();
    // An irregular length, so that a truncated read cannot have the correct
    // length by accident.
    let body = "x".repeat(11 * 1024 * 1024 + 7);
    let uid = fixture.seed_message("Holiday photos", &body);

    let source = fixture
        .sync()
        .message_source(&uid)
        .expect("a large message is still a message");
    let source = String::from_utf8(source).expect("the mock serves a text message");

    assert!(source.contains(&body), "the body did not arrive whole");
}

/// The size of the row limits the download. If the row gives much less than
/// the blob size, the server contradicts itself. The client refuses the answer
/// at the ceiling, and does not buffer it first.
///
/// This test completes the test above. Without it, a very large constant
/// limit passes both.
#[test]
fn a_row_that_understates_its_size_bounds_the_download_to_what_it_said() {
    let fixture = Fixture::start();
    let uid = fixture.seed_message("Holiday photos", &"x".repeat(512 * 1024));
    // Much less than the message, and less than the `download_ceiling` margin.
    // Thus the test checks the row size, not the margin.
    fixture.with_email(&uid, |email| email.size = Some(1));

    let error = fixture
        .sync()
        .message_source(&uid)
        .expect_err("a blob far past the size its row states is not read");

    match error {
        SyncError::Client(jmap_client::Error::ResponseTooLarge { limit }) => {
            assert_eq!(limit, jmap_mail_sync::download_ceiling(Some(1)));
        }
        other => panic!("expected the download to be bounded by the row, got {other:?}"),
    }
}

/// A row without a size still gets a ceiling: the limit of this repository,
/// not of a dependency, and more than the old 10 MiB.
#[test]
fn a_row_with_no_size_still_reads_a_large_message() {
    let fixture = Fixture::start();
    let body = "x".repeat(11 * 1024 * 1024 + 7);
    let uid = fixture.seed_message("Holiday photos", &body);
    // RFC 8621 §4.1.1 makes `size` server-set, so this server is wrong. The
    // user must still get the message.
    fixture.with_email(&uid, |email| email.size = None);

    let source = fixture
        .sync()
        .message_source(&uid)
        .expect("a message whose size the server withheld is still a message");
    assert!(
        String::from_utf8(source)
            .expect("the mock serves a text message")
            .contains(&body),
        "the body did not arrive whole"
    );
}
