// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The send of a message from the composer, against a mock server: the
//! `Email/import` into the account, the `EmailSubmission/set` to the server
//! submission system, and the message in the account after the send.
//!
//! Sending is the only write that goes out of the account. The tests check
//! these rules:
//!
//! - The server submits the same bytes that the composer made.
//! - The server uses the envelope from the caller. The real recipients are not
//!   always the header addresses.
//! - A sent message is no longer a draft, and it goes to the expected mailbox.

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{MailSync, MessageSummary, Outgoing, SyncError};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::mail::{Envelope, EnvelopeAddress};

/// The RFC 5322 bytes that the Evolution composer gives to a transport.
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

/// The mock record of an accepted submission, copied out of the server lock.
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

    /// The current contents of the mailbox, as a folder refresh finds them.
    fn listing(&self, mailbox: &Id) -> Vec<MessageSummary> {
        let (_, messages) = self.sync().messages(mailbox).expect("the mailbox lists");
        messages
    }

    /// The single submission that the server accepted, as recorded.
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

/// The single row of a mailbox. Fails with the count if it is not one row.
fn only(messages: Vec<MessageSummary>) -> MessageSummary {
    assert_eq!(messages.len(), 1, "one message");
    messages.into_iter().next().expect("the row just counted")
}

/// The envelope that a transport makes from the Camel `from` and `recipients`.
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

    // The submission for the server MTA: this message, through this identity.
    // If the message gets to the account but not to submission, the user
    // thinks that it went out.
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

    // This is why a send imports bytes and does not compose an `Email` from
    // properties. A rebuilt message has different bytes, and its signature
    // fails for the recipient *and* for the copy of the sender.
    let source = sync.message_source(&uid).expect("the message is readable");
    assert_eq!(source, MESSAGE);
}

#[test]
fn the_envelope_the_caller_names_is_the_envelope_that_goes_out() {
    let fixture = Fixture::start();
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));
    let sent = fixture.seed_mailbox("Sent", Some("sent"));
    let identity = fixture.seed_identity("Alice", "alice@example.com");

    // The headers do not contain these addresses. This is a usual case: a `Bcc`
    // recipient has no header, a redirected message keeps its old headers, and
    // the envelope sender is often not the `From` address. An envelope from
    // the headers sends to Bob, not to the real recipients.
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

    // The message is in the staging mailbox during the send. If it stays there,
    // the user finds it in Drafts after the send, as if not yet sent.
    assert!(fixture.listing(&drafts).is_empty(), "left behind in Drafts");

    let row = only(fixture.listing(&sent));
    assert_eq!(row.uid, uid);
    // It is not a draft and not unread, because the user wrote it.
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

    // The caller gave no mailbox for the sent copy: the account has no Sent
    // role, or Evolution saves the copy itself. The message stays in the
    // staging mailbox, because RFC 8621 §4.6 requires at least one mailbox.
    // But it is no longer a draft, because it was sent.
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

    // Keep the server refusal unchanged. The caller makes a message for the
    // user from it. A missing identity does not mean a broken account.
    match error {
        SyncError::Client(jmap_client::Error::Set(_)) => {}
        other => panic!("expected the server's refusal, got {other:?}"),
    }
    assert!(fixture.outbox_is_empty(), "a refused submission went out");

    // The draft stays in the staging mailbox, and it is still a draft. This is
    // correct for a refusal after the import: the work of the user is not
    // lost, and it is where the user looks for an unsent message.
    let row = only(fixture.listing(&drafts));
    assert!(row.flags.draft, "the unsent message is not marked a draft");
    // But it is read: the user wrote it, and an unread draft adds a bold line
    // and a count to Drafts. Here only the import sets `$seen`, because the
    // client does not apply the later patch.
    assert!(row.flags.seen, "the refused message came back unread");
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

    // Refused before the upload, with both numbers, as for an append. A send
    // uses the same import, and the session document gives the limit, so no
    // upload is necessary to find it.
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
