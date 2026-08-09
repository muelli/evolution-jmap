// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Listing a mailbox against a live mock server: what `Email/query` and
//! `Email/get` actually answer, and the two limits a server is allowed to
//! impose on either.

use jmap_client::{Client, Credentials};
use jmap_mail_sync::MailSync;
use jmap_mock::{EmailSeed, MockServer, MockServerBuilder};
use jmap_proto::Id;
use jmap_proto::mail::{keyword, role};

struct Fixture {
    server: MockServer,
    account_id: Id,
}

impl Fixture {
    fn start() -> Self {
        Self::started_with(MockServer::builder())
    }

    fn started_with(builder: MockServerBuilder) -> Self {
        let server = builder.start();
        let account_id = server.account_id();
        Self { server, account_id }
    }

    fn sync(&self) -> MailSync {
        let client = Client::connect(self.server.origin(), Credentials::none()).unwrap();
        MailSync::new(client, self.account_id.clone())
    }

    /// Seed a mailbox and `count` messages in it, an hour apart — and *newest
    /// first*, so that the order they were created in, which is also the order
    /// their ids sort in, is the reverse of the order a listing must produce.
    /// Seeded the other way round, every wrong answer here would look right.
    fn seed_mailbox_with(&self, name: &str, role: Option<&str>, count: u32) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&self.account_id).unwrap();
        let mailbox = account.seed_mailbox(name, role);
        for index in 0..count {
            account.seed_email(EmailSeed::new(
                mailbox.clone(),
                ("Bob", "bob@example.com"),
                &format!("Message {index}"),
                "text",
                &format!("2026-01-15T{:02}:00:00Z", 23 - index % 24),
            ));
        }
        mailbox
    }

    /// The subjects of a listing, in the order it came back in.
    fn subjects(messages: &[jmap_mail_sync::MessageSummary]) -> Vec<&str> {
        messages
            .iter()
            .map(|message| message.subject.as_deref().unwrap_or_default())
            .collect()
    }
}

#[test]
fn a_mailboxs_messages_come_back_oldest_first() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox_with("Inbox", Some(role::INBOX), 3);

    let (_, messages) = fixture.sync().messages(&inbox).unwrap();

    assert_eq!(
        Fixture::subjects(&messages),
        ["Message 2", "Message 1", "Message 0"],
        "the newest message was seeded first and belongs last"
    );
    assert!(
        messages
            .windows(2)
            .all(|pair| pair[0].received_at < pair[1].received_at),
        "the order the server sorted in has to survive the fetch"
    );
}

#[test]
fn only_the_mailbox_asked_about_is_listed() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox_with("Inbox", Some(role::INBOX), 2);
    let sent = fixture.seed_mailbox_with("Sent", Some(role::SENT), 1);

    let sync = fixture.sync();

    assert_eq!(sync.messages(&inbox).unwrap().1.len(), 2);
    assert_eq!(sync.messages(&sent).unwrap().1.len(), 1);
}

#[test]
fn an_empty_mailbox_lists_nothing() {
    let fixture = Fixture::start();
    let empty = fixture.seed_mailbox_with("Archive", Some(role::ARCHIVE), 0);

    assert!(fixture.sync().messages(&empty).unwrap().1.is_empty());
}

#[test]
fn a_mailbox_the_account_does_not_have_lists_nothing() {
    let fixture = Fixture::start();
    fixture.seed_mailbox_with("Inbox", Some(role::INBOX), 1);

    let (_, messages) = fixture
        .sync()
        .messages(&Id::new("no-such-mailbox"))
        .unwrap();

    assert!(
        messages.is_empty(),
        "a filter matching nothing is an empty answer, not a failure"
    );
}

#[test]
fn the_flags_a_listing_carries_are_the_servers_keywords() {
    let fixture = Fixture::start();
    let inbox = {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&fixture.account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_email(
            EmailSeed::new(
                inbox.clone(),
                ("Bob", "bob@example.com"),
                "Read and answered",
                "text",
                "2026-01-15T09:00:00Z",
            )
            .keyword(keyword::SEEN)
            .keyword(keyword::ANSWERED),
        );
        inbox
    };

    let (_, messages) = fixture.sync().messages(&inbox).unwrap();

    let [message] = messages.as_slice() else {
        panic!("one message, got {}", messages.len());
    };
    assert!(message.flags.seen && message.flags.answered);
    assert!(!message.flags.flagged);
    assert_eq!(message.from.len(), 1);
    assert_eq!(message.from[0].email, "bob@example.com");
    assert_eq!(message.received_at, Some(1_768_467_600));
    assert!(message.blob_id.is_some(), "the body has to be fetchable");
}

#[test]
fn more_messages_than_one_get_may_ask_about_are_fetched_in_several() {
    // Two per `Email/get`, five messages: three calls, and a client that sends
    // one would be told `requestTooLarge` rather than quietly served.
    let fixture = Fixture::started_with(MockServer::builder().objects_in_get(2));
    let inbox = fixture.seed_mailbox_with("Inbox", Some(role::INBOX), 5);

    let (_, messages) = fixture.sync().messages(&inbox).unwrap();

    assert_eq!(messages.len(), 5);
    assert_eq!(
        Fixture::subjects(&messages),
        [
            "Message 4",
            "Message 3",
            "Message 2",
            "Message 1",
            "Message 0"
        ],
        "chunking must not reorder the query's answer"
    );
}

#[test]
fn a_server_that_pages_its_query_answer_is_read_to_the_end() {
    // A server may cap a `/query` answer whether or not the client asked it to
    // (RFC 8620 §5.5); a client that stops at the first page shows the user a
    // folder missing most of its mail.
    let fixture = Fixture::started_with(MockServer::builder().query_page_size(2));
    let inbox = fixture.seed_mailbox_with("Inbox", Some(role::INBOX), 5);

    let (_, messages) = fixture.sync().messages(&inbox).unwrap();

    assert_eq!(messages.len(), 5);
    assert_eq!(
        Fixture::subjects(&messages),
        [
            "Message 4",
            "Message 3",
            "Message 2",
            "Message 1",
            "Message 0"
        ],
        "a page boundary must not reorder or duplicate anything"
    );
}
