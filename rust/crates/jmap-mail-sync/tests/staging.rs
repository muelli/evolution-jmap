// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where an outgoing message waits and where it is filed, against a live mock
//! server.
//!
//! [`Outgoing`] carries two mailbox ids — the one the message is imported into
//! while it is being sent, and the one the server files it into once the
//! submission is accepted — and a `CamelTransport` is handed neither. Camel's
//! `send_to_sync` gets a message and two address lists and nothing about
//! folders at all, so the two mailboxes have to be found in the account, which
//! is what the method under test here does.
//!
//! They are found by *role*, never by name: RFC 8621 §2 puts a `role` on a
//! mailbox for exactly this, and an account whose drafts live in `Entwürfe` is
//! an ordinary account rather than one that cannot send.

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{MailSync, SyncError};
use jmap_mock::MockServer;
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

    fn seed_mailbox(&self, name: &str, role: Option<&str>) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&self.account_id).unwrap();
        account.seed_mailbox(name, role)
    }
}

#[test]
fn a_message_waits_in_drafts_and_is_filed_in_sent() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox", Some("inbox"));
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));
    let sent = fixture.seed_mailbox("Sent", Some("sent"));

    let mailboxes = fixture
        .sync()
        .outgoing_mailboxes()
        .expect("the account can send");

    // The ordinary account, and the whole point of staging somewhere other than
    // the destination: a message in Sent is a message that has been sent, and
    // one that is still being submitted has not.
    assert_eq!(mailboxes.staging, drafts);
    assert_eq!(mailboxes.destination, Some(sent));
}

#[test]
fn the_mailboxes_are_the_ones_the_account_gave_the_role_to() {
    let fixture = Fixture::start();
    // A German account, plus two decoys with the English names and no role at
    // all. Matching on the name would put the user's outgoing mail in a folder
    // they happen to have called Drafts — and leave the folder their server
    // files sent mail in out of it.
    fixture.seed_mailbox("Drafts", None);
    fixture.seed_mailbox("Sent", None);
    let drafts = fixture.seed_mailbox("Entwürfe", Some("drafts"));
    let sent = fixture.seed_mailbox("Gesendet", Some("sent"));

    let mailboxes = fixture
        .sync()
        .outgoing_mailboxes()
        .expect("the account can send");

    assert_eq!(mailboxes.staging, drafts);
    assert_eq!(mailboxes.destination, Some(sent));
}

#[test]
fn an_account_with_no_sent_mailbox_leaves_the_message_where_it_waited() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox", Some("inbox"));
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));

    let mailboxes = fixture
        .sync()
        .outgoing_mailboxes()
        .expect("the account can send");

    // No destination rather than a guess: RFC 8621 §4.6 leaves no message in no
    // mailbox, so the copy stays in Drafts — and it stops being a draft, which
    // is [`Outgoing::accepted_patch`]'s half of this.
    assert_eq!(mailboxes.staging, drafts);
    assert_eq!(mailboxes.destination, None);
}

#[test]
fn an_account_with_no_drafts_waits_in_the_mailbox_it_will_be_filed_in() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox", Some("inbox"));
    let sent = fixture.seed_mailbox("Sent", Some("sent"));

    let mailboxes = fixture
        .sync()
        .outgoing_mailboxes()
        .expect("the account can send");

    // Sent is where the message is going anyway, so an account with no Drafts
    // stages it there — and names no destination, because a message cannot be
    // filed out of a mailbox into the same one.
    assert_eq!(mailboxes.staging, sent);
    assert_eq!(mailboxes.destination, None);
}

#[test]
fn an_account_with_nowhere_to_put_an_outgoing_message_cannot_send() {
    let fixture = Fixture::start();
    // A mailbox, so this is not the empty-account case: the account has folders
    // and none of them is for mail the user writes.
    fixture.seed_mailbox("Inbox", Some("inbox"));
    fixture.seed_mailbox("Archive", Some("archive"));

    let error = fixture
        .sync()
        .outgoing_mailboxes()
        .expect_err("an account with no drafts and no sent mailbox can send");

    // Not the Inbox, and not the first folder that happens to be there. The
    // Inbox is where the server *delivers*, and importing the user's own
    // outgoing mail into it would manufacture arrivals they then have to sort
    // out — for a message that may not even go out.
    match error {
        SyncError::NoOutgoingFolder => {}
        other => panic!("expected the account to have nowhere to send from, got {other:?}"),
    }
}

#[test]
fn the_mailboxes_are_read_from_the_account_at_every_send() {
    let fixture = Fixture::start();
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));
    let sync = fixture.sync();

    let before = sync.outgoing_mailboxes().expect("the account can send");
    assert_eq!(before.destination, None);

    // Another client — or the user, in another window — gives the account a
    // Sent mailbox. Nothing caches the tree, for [`MailSync::identity_for`]'s
    // reason: a listing held across a session goes wrong quietly, by filing
    // sent mail into a mailbox that is gone or by not finding one just made.
    let sent = fixture.seed_mailbox("Sent", Some("sent"));

    let after = sync.outgoing_mailboxes().expect("the account can send");
    assert_eq!(after.staging, drafts);
    assert_eq!(after.destination, Some(sent));
}
