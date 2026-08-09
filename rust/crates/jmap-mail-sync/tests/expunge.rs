// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Making a message leave one mailbox for good — the write behind Camel's
//! `expunge_sync`.
//!
//! Every other write in this crate has one request in it. This one has two, and
//! the reason is the gap between what Camel's vfunc means and what JMAP has to
//! say it with. `camel_folder_expunge_sync` asks a *folder* to get rid of the
//! messages the user marked deleted *in it*; in IMAP that is unambiguous,
//! because a message is in exactly one mailbox and removing it from the mailbox
//! is removing it. In JMAP RFC 8621 §4.6 makes `mailboxIds` a set, so the same
//! message may be filed in the inbox and in a folder the user put it in
//! themselves, and the two possible writes say very different things:
//!
//! - `Email/set` **destroy** removes the message from the account. Right for a
//!   message this mailbox is the last home of, and data loss for one the user
//!   also filed somewhere else — emptying the trash would take the copy in
//!   "Work" with it.
//! - `Email/set` **update** with `mailboxIds/<this>: null` removes it from
//!   this mailbox only. Right for the second case, and refused by any server
//!   that keeps RFC 8621 §4.6's invariant for the first, because it would leave
//!   the message in no mailbox at all.
//!
//! Neither is right on its own, and nothing on the Camel side knows which case
//! a message is in: a folder summary row records the mailbox it was listed
//! from and has never been told about any other. So the mailboxes are read
//! first and the write chosen from the answer, which is what these tests are
//! about — one read, then whichever of the two writes the read implies.

use std::collections::BTreeMap;

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{Filing, MailSync, SyncError};
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

    fn seed_mailbox(&self, name: &str) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&self.account_id)
            .unwrap()
            .seed_mailbox(name, None)
    }

    fn seed_message(&self, mailbox: &Id) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&self.account_id).unwrap();
        account.seed_email(EmailSeed::new(
            mailbox.clone(),
            ("Bob", "bob@example.com"),
            "Lunch?",
            "One o'clock.",
            "2026-01-15T09:30:00Z",
        ))
    }

    /// Whether the account still holds the message at all.
    fn holds(&self, uid: &Id) -> bool {
        let state = self.server.state();
        let state = state.lock().unwrap();
        state
            .account(&self.account_id)
            .unwrap()
            .emails
            .get(uid)
            .is_some()
    }

    /// Which mailboxes the server has the message in now.
    fn mailboxes_on_server(&self, uid: &Id) -> BTreeMap<Id, bool> {
        let state = self.server.state();
        let state = state.lock().unwrap();
        let account = state.account(&self.account_id).unwrap();
        account
            .emails
            .get(uid)
            .expect("the seeded message")
            .mailbox_ids
            .clone()
            .unwrap_or_default()
    }
}

#[test]
fn a_message_this_mailbox_is_the_last_home_of_is_destroyed() {
    // The ordinary case, and the one that makes "empty the trash" mean
    // something: the mailbox being expunged is the only one naming the
    // message, so taking it out of that mailbox and destroying it are the same
    // act — and only the second is a request RFC 8621 §4.6 allows.
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let uid = fixture.seed_message(&inbox);

    fixture
        .sync()
        .expunge_message(&uid, &inbox)
        .expect("the message to be expunged");

    assert!(
        !fixture.holds(&uid),
        "the account should no longer hold a message its last mailbox expunged"
    );
}

#[test]
fn a_message_filed_elsewhere_too_only_leaves_this_mailbox() {
    // The case that makes the read worth its round trip. The user copied the
    // message into a folder of their own and then deleted it out of the inbox;
    // destroying it would take their copy with it.
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let archive = fixture.seed_mailbox("Archive");
    let uid = fixture.seed_message(&inbox);
    let sync = fixture.sync();
    sync.file_message(&uid, &Filing::copied_into(archive.clone()))
        .expect("the copy to be filed");

    sync.expunge_message(&uid, &inbox)
        .expect("the message to be expunged");

    assert!(
        fixture.holds(&uid),
        "a message the user also filed elsewhere must survive an expunge here"
    );
    assert_eq!(
        fixture.mailboxes_on_server(&uid).keys().collect::<Vec<_>>(),
        vec![&archive],
        "the expunged mailbox should be the only member the write removed"
    );
}

#[test]
fn a_message_that_is_not_in_this_mailbox_is_left_exactly_as_it_is() {
    // A summary row is a claim about the last listing, so the mailbox the row
    // was listed from may not be one the message is in any more — another
    // client can have moved it while Evolution held the folder open. Expunging
    // it here is then a statement about a mailbox the message left, and the
    // safe reading of that is that there is nothing to do: the message is
    // already gone from here, and destroying it because of where it *was*
    // would delete mail on the strength of a stale row.
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let archive = fixture.seed_mailbox("Archive");
    let uid = fixture.seed_message(&archive);

    fixture
        .sync()
        .expunge_message(&uid, &inbox)
        .expect("an expunge of a mailbox the message is not in to be no work");

    assert!(fixture.holds(&uid), "the message must survive untouched");
    assert_eq!(
        fixture.mailboxes_on_server(&uid).keys().collect::<Vec<_>>(),
        vec![&archive],
        "nothing about where the message is filed should have changed"
    );
}

#[test]
fn a_message_another_client_destroyed_is_reported_as_gone_rather_than_failed() {
    // The judgement every write in this crate makes about the same situation:
    // a uid is a claim about the last listing, and another client having
    // destroyed the message means the expunge is moot rather than broken.
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");

    let error = fixture
        .sync()
        .expunge_message(&Id::new("nosuchmessage"), &inbox)
        .expect_err("a message the account does not have");

    match error {
        SyncError::NoSuchMessage(uid) => assert_eq!(uid.as_str(), "nosuchmessage"),
        other => panic!("expected NoSuchMessage, got {other:?}"),
    }
}

#[test]
fn expunging_the_same_message_twice_reports_the_second_as_gone() {
    // What the first test leaves behind, asked again — the shape a retry after
    // a half-failed expunge takes, and the reason the caller may treat
    // NoSuchMessage as a row that leaves rather than as an error to show.
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let uid = fixture.seed_message(&inbox);
    let sync = fixture.sync();
    sync.expunge_message(&uid, &inbox).expect("the first");

    let error = sync
        .expunge_message(&uid, &inbox)
        .expect_err("the message is not there to expunge twice");

    match error {
        SyncError::NoSuchMessage(gone) => assert_eq!(gone, uid),
        other => panic!("expected NoSuchMessage, got {other:?}"),
    }
}
