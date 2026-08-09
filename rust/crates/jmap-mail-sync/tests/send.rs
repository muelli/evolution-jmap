// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sending a message the composer just built, against a live mock server: the
//! `Email/import` that puts it in the account, the `EmailSubmission/set` that
//! hands it to the server's own submission machinery, and what the message
//! looks like in the account once it has gone.
//!
//! Every other write in this crate changes mail the account already holds.
//! Sending is the one that leaves the account: what the tests below are about
//! is that the bytes the composer produced are the bytes submitted, that the
//! envelope the caller names is the envelope the server is told to use — the
//! addresses the mail actually goes to are not the ones in the headers — and
//! that a sent message stops being a draft and ends up where the user expects
//! to find it.

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{MailSync, MessageSummary, Outgoing, SyncError};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::mail::{Envelope, EnvelopeAddress};

/// The RFC 5322 bytes Evolution's composer would hand a transport.
const MESSAGE: &[u8] = b"From: Alice <alice@example.com>\r\n\
To: Bob <bob@example.com>\r\n\
Subject: Lunch?\r\n\
Message-ID: <lunch@example.com>\r\n\
Date: Thu, 15 Jan 2026 09:30:00 +0000\r\n\
\r\n\
One o'clock at the usual place.\r\n";

struct Fixture {
    server: MockServer,
    account_id: Id,
}

/// What the mock recorded about an accepted submission, copied out from under
/// the server's lock.
struct Submission {
    email_id: Id,
    identity_id: Id,
    envelope: Envelope,
}

impl Fixture {
    fn start() -> Self {
        Self::started_with(MockServer::builder())
    }

    fn started_with(builder: jmap_mock::MockServerBuilder) -> Self {
        let server = builder.start();
        let account_id = server.account_id();
        Self { server, account_id }
    }

    fn sync(&self) -> MailSync {
        let client = Client::connect(self.server.origin(), Credentials::none()).unwrap();
        MailSync::new(client, self.account_id.clone())
    }

    fn seed_mailbox(&self, name: &str, role: Option<&str>) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&self.account_id).unwrap();
        account.seed_mailbox(name, role)
    }

    fn seed_identity(&self, name: &str, email: &str) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&self.account_id).unwrap();
        account.seed_identity(name, email)
    }

    /// What the mailbox holds now, as a folder refresh would find it.
    fn listing(&self, mailbox: &Id) -> Vec<MessageSummary> {
        let (_, messages) = self.sync().messages(mailbox).expect("the mailbox lists");
        messages
    }

    /// The one submission the server accepted, as it recorded it.
    fn only_submission(&self) -> Submission {
        let state = self.server.state();
        let state = state.lock().unwrap();
        let outbox = &state.account(&self.account_id).unwrap().outbox;
        assert_eq!(outbox.len(), 1, "one accepted submission");
        let recorded = outbox.first().expect("the submission just counted");
        Submission {
            email_id: recorded.email_id.clone(),
            identity_id: recorded.identity_id.clone(),
            envelope: recorded.envelope.clone(),
        }
    }

    fn outbox_is_empty(&self) -> bool {
        let state = self.server.state();
        let state = state.lock().unwrap();
        state.account(&self.account_id).unwrap().outbox.is_empty()
    }
}

/// The one row a mailbox holds, or a failure naming how many it holds instead.
fn only(messages: Vec<MessageSummary>) -> MessageSummary {
    assert_eq!(messages.len(), 1, "one message");
    messages.into_iter().next().expect("the row just counted")
}

/// The envelope a transport builds out of Camel's `from` and `recipients`.
fn envelope(from: &str, rcpt_to: &[&str]) -> Envelope {
    Envelope {
        mail_from: EnvelopeAddress::new(from),
        rcpt_to: rcpt_to
            .iter()
            .map(|address| EnvelopeAddress::new(*address))
            .collect(),
    }
}

#[test]
fn a_sent_message_is_submitted_through_the_identity_it_names() {
    let fixture = Fixture::start();
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));
    let sent = fixture.seed_mailbox("Sent", Some("sent"));
    let identity = fixture.seed_identity("Alice", "alice@example.com");

    let uid = fixture
        .sync()
        .send_message(Outgoing {
            source: MESSAGE.to_vec(),
            identity: identity.clone(),
            envelope: Some(envelope("alice@example.com", &["bob@example.com"])),
            staging: drafts,
            destination: Some(sent),
        })
        .expect("the message is sent");

    // The submission the server would have handed to its MTA: this message,
    // through this identity. A send that reached the account and never reached
    // the submission machinery is a message the user believes went out.
    let submission = fixture.only_submission();
    assert_eq!(submission.email_id, uid);
    assert_eq!(submission.identity_id, identity);
}

#[test]
fn the_bytes_the_composer_produced_are_the_bytes_the_account_holds() {
    let fixture = Fixture::start();
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));
    let sent = fixture.seed_mailbox("Sent", Some("sent"));
    let identity = fixture.seed_identity("Alice", "alice@example.com");
    let sync = fixture.sync();

    let uid = sync
        .send_message(Outgoing {
            source: MESSAGE.to_vec(),
            identity,
            envelope: None,
            staging: drafts,
            destination: Some(sent),
        })
        .expect("the message is sent");

    // The reason sending imports bytes rather than composing an `Email` out of
    // properties: a message taken apart and written out again would not be
    // these bytes, and a signature over them would no longer verify — for the
    // recipient *and* for the copy the sender keeps.
    let source = sync.message_source(&uid).expect("the message is readable");
    assert_eq!(source, MESSAGE);
}

#[test]
fn the_envelope_the_caller_names_is_the_envelope_that_goes_out() {
    let fixture = Fixture::start();
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));
    let sent = fixture.seed_mailbox("Sent", Some("sent"));
    let identity = fixture.seed_identity("Alice", "alice@example.com");

    // Neither address is in the message's headers. That is the ordinary case
    // rather than a contrived one: a `Bcc` recipient is a recipient with no
    // header, a redirected message keeps the headers it arrived with, and a
    // sender's envelope address is often not the one they sign their mail as.
    // A transport that let the server derive the envelope from the headers
    // would silently drop the blind recipient and deliver to nobody else.
    fixture
        .sync()
        .send_message(Outgoing {
            source: MESSAGE.to_vec(),
            identity,
            envelope: Some(envelope(
                "alice@lists.example.com",
                &["carol@example.net", "dave@example.org"],
            )),
            staging: drafts,
            destination: Some(sent),
        })
        .expect("the message is sent");

    let submission = fixture.only_submission();
    assert_eq!(
        submission.envelope.mail_from.email,
        "alice@lists.example.com"
    );
    let rcpt_to: Vec<&str> = submission
        .envelope
        .rcpt_to
        .iter()
        .map(|address| address.email.as_str())
        .collect();
    assert_eq!(rcpt_to, ["carol@example.net", "dave@example.org"]);
}

#[test]
fn a_sent_message_leaves_the_mailbox_it_was_staged_in_for_the_one_it_is_filed_in() {
    let fixture = Fixture::start();
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));
    let sent = fixture.seed_mailbox("Sent", Some("sent"));
    let identity = fixture.seed_identity("Alice", "alice@example.com");

    let uid = fixture
        .sync()
        .send_message(Outgoing {
            source: MESSAGE.to_vec(),
            identity,
            envelope: Some(envelope("alice@example.com", &["bob@example.com"])),
            staging: drafts.clone(),
            destination: Some(sent.clone()),
        })
        .expect("the message is sent");

    // The staging mailbox is where the message lived while it was being sent,
    // and a message left behind in it is one the user finds in Drafts after
    // sending it — offered back to them to send again.
    assert!(fixture.listing(&drafts).is_empty(), "left behind in Drafts");

    let row = only(fixture.listing(&sent));
    assert_eq!(row.uid, uid);
    // And it is no longer a draft, and no longer unread: the user wrote it and
    // has read every word of it.
    assert!(!row.flags.draft, "still a draft after being sent");
    assert!(row.flags.seen, "the sender's own message came back unread");
}

#[test]
fn a_message_with_nowhere_to_be_filed_stays_where_it_was_staged_and_is_no_longer_a_draft() {
    let fixture = Fixture::start();
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));
    let identity = fixture.seed_identity("Alice", "alice@example.com");

    let uid = fixture
        .sync()
        .send_message(Outgoing {
            source: MESSAGE.to_vec(),
            identity,
            envelope: Some(envelope("alice@example.com", &["bob@example.com"])),
            staging: drafts.clone(),
            destination: None,
        })
        .expect("the message is sent");

    // An account whose caller named no mailbox to file the sent copy in — one
    // with no Sent role, or one where Evolution saves the copy itself. The
    // message stays where it was staged, because RFC 8621 §4.6 leaves no
    // message in no mailbox at all, but it stops claiming to be a draft: it has
    // been sent.
    let row = only(fixture.listing(&drafts));
    assert_eq!(row.uid, uid);
    assert!(!row.flags.draft, "still a draft after being sent");
    assert!(row.flags.seen, "the sender's own message came back unread");
}

#[test]
fn an_identity_the_account_does_not_have_sends_nothing() {
    let fixture = Fixture::start();
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));
    let sent = fixture.seed_mailbox("Sent", Some("sent"));

    let error = fixture
        .sync()
        .send_message(Outgoing {
            source: MESSAGE.to_vec(),
            identity: Id::new("I404"),
            envelope: Some(envelope("alice@example.com", &["bob@example.com"])),
            staging: drafts.clone(),
            destination: Some(sent.clone()),
        })
        .expect_err("a message submitted through no identity is not sent");

    // The server's own refusal, kept whole — the caller above turns it into a
    // sentence for the user, and an account whose identity has gone is not a
    // broken account.
    match error {
        SyncError::Client(jmap_client::Error::Set(_)) => {}
        other => panic!("expected the server's refusal, got {other:?}"),
    }
    assert!(fixture.outbox_is_empty(), "a refused submission went out");

    // And the draft is left behind, in the staging mailbox and still a draft.
    // That is the honest outcome of a refusal after the import: the message
    // exists in the account, the user's work is not lost, and it is where they
    // would look for an unsent message.
    let row = only(fixture.listing(&drafts));
    assert!(row.flags.draft, "the unsent message is not marked a draft");
    assert!(fixture.listing(&sent).is_empty(), "filed as though it went");
}

#[test]
fn a_message_over_the_accounts_upload_limit_is_not_sent() {
    let fixture = Fixture::started_with(MockServer::builder().size_upload(1024));
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));
    let identity = fixture.seed_identity("Alice", "alice@example.com");

    let error = fixture
        .sync()
        .send_message(Outgoing {
            source: vec![b'x'; 4096],
            identity,
            envelope: Some(envelope("alice@example.com", &["bob@example.com"])),
            staging: drafts.clone(),
            destination: None,
        })
        .expect_err("a message over the account's limit was sent");

    // Refused before the upload, with both numbers, exactly as an append is:
    // sending is the same import underneath, and the limit is knowable from the
    // session document without spending the user's uplink on finding out.
    match error {
        SyncError::Client(jmap_client::Error::TooLarge { size, limit }) => {
            assert_eq!(size, 4096);
            assert_eq!(limit, 1024);
        }
        other => panic!("expected the upload limit, got {other:?}"),
    }
    assert!(fixture.outbox_is_empty(), "a refused submission went out");
    assert!(fixture.listing(&drafts).is_empty());
}
